//! Async client for TypeSafe's System One endpoint
//!
//! One endpoint does the work: `POST /v1/systemone` takes a state plus a map of
//! typed questions and returns one answer per question. Jev reads the state once
//! and evaluates every question against it in parallel, so batching questions
//! into a single request is both cheaper and faster than issuing one request per
//! question. Build one [`Request`] per state and hang every question off it.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::answer::Evaluation;
use super::error::TypeSafeError;
use super::ordered::OrderedMap;
use super::question::{Entry, Question};
use super::retry::{parse_retry_after, RetryPolicy};

/// Environment variable holding the API key.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Environment variable overriding the API base URL.
pub const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Default model alias. Moves when TypeSafe ships a new stable release; pin a
/// versioned id such as `jev-1.13.0` if confidence thresholds are tuned to one.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Default per-attempt request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// One evaluation request: a state, a model, and the questions to ask about it.
#[derive(Debug, Clone)]
pub struct Request {
    state: Entry,
    model: Option<String>,
    questions: OrderedMap<Question>,
}

impl Request {
    /// Start a request for one state. Pass a `&str` for plain text, or a
    /// `serde_json::Value` when the state has named parts.
    pub fn new(state: impl Into<Entry>) -> Self {
        Self {
            state: state.into(),
            model: None,
            questions: OrderedMap::new(),
        }
    }

    /// Add a question under an id of your choosing. The id is not sent to the
    /// model; it is how you find the answer in the response.
    pub fn ask(mut self, id: impl Into<String>, question: Question) -> Self {
        self.questions.insert(id, question);
        self
    }

    /// Add several questions at once.
    pub fn ask_all<K: Into<String>>(
        mut self,
        questions: impl IntoIterator<Item = (K, Question)>,
    ) -> Self {
        for (id, question) in questions {
            self.questions.insert(id, question);
        }
        self
    }

    /// Override the client's default model for this request.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn question_ids(&self) -> impl Iterator<Item = &str> {
        self.questions.keys()
    }

    pub fn len(&self) -> usize {
        self.questions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }

    /// Catch locally what would otherwise come back as a 422.
    fn validate(&self) -> Result<(), TypeSafeError> {
        if self.questions.is_empty() {
            return Err(TypeSafeError::EmptyRequest);
        }
        for (id, question) in self.questions.iter() {
            question
                .validate()
                .map_err(|reason| TypeSafeError::InvalidQuestion {
                    id: id.to_string(),
                    reason,
                })?;
        }
        Ok(())
    }
}

/// The exact JSON body sent to the endpoint.
#[derive(Serialize)]
struct WireRequest<'a> {
    state: &'a Entry,
    model: &'a str,
    questions: &'a OrderedMap<Question>,
}

/// Metadata for a model the account can use.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelCard {
    pub name: String,
    pub description: String,
    pub release_date: String,
}

/// The live endpoint returns `{"models": [...]}`, confirmed against the API.
/// The bare-array and `data` variants stay as cheap insurance, since the wire
/// envelope is not in the published docs and could change.
#[derive(Deserialize)]
#[serde(untagged)]
enum ModelList {
    Bare(Vec<ModelCard>),
    Wrapped { data: Vec<ModelCard> },
    Named { models: Vec<ModelCard> },
}

impl ModelList {
    fn into_vec(self) -> Vec<ModelCard> {
        match self {
            ModelList::Bare(v)
            | ModelList::Wrapped { data: v }
            | ModelList::Named { models: v } => v,
        }
    }
}

/// How to reach TypeSafe.
#[derive(Clone)]
pub struct ClientConfig {
    /// API key. Falls back to `TYPESAFE_API_KEY` when `None`.
    pub api_key: Option<String>,
    /// Base URL. Falls back to `TYPESAFE_BASE_URL`, then [`DEFAULT_BASE_URL`].
    pub base_url: Option<String>,
    /// Model used when a request does not name one.
    pub default_model: String,
    /// Per-attempt timeout. Retries each get their own.
    pub timeout: Duration,
    pub retry: RetryPolicy,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: None,
            default_model: DEFAULT_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry: RetryPolicy::default(),
        }
    }
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .finish()
    }
}

/// A TypeSafe API client. Cheap to clone; the underlying connection pool is
/// shared, so build one and keep it.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    default_model: String,
    timeout: Duration,
    retry: RetryPolicy,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("timeout", &self.timeout)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

/// The outcome of one HTTP attempt.
enum Attempt<T> {
    Done(T),
    Retry(TypeSafeError, Option<Duration>),
    Fail(TypeSafeError),
}

impl Client {
    /// Build a client, reading the key and base URL from the environment when
    /// the config leaves them unset.
    pub fn new(config: ClientConfig) -> Result<Self, TypeSafeError> {
        let api_key = config
            .api_key
            .or_else(|| std::env::var(API_KEY_ENV).ok())
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .ok_or(TypeSafeError::MissingApiKey)?;

        let base_url = config
            .base_url
            .or_else(|| std::env::var(BASE_URL_ENV).ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let base_url = base_url.trim_end_matches('/').to_string();

        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(TypeSafeError::InvalidBaseUrl(base_url));
        }
        if base_url.starts_with("http://") {
            tracing::warn!(
                "TypeSafe base URL uses HTTP without TLS. The API key and request state \
                 will be transmitted unencrypted."
            );
        }

        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| TypeSafeError::ClientBuild(e.to_string()))?;

        Ok(Self {
            http,
            api_key,
            base_url,
            default_model: config.default_model,
            timeout: config.timeout,
            retry: config.retry,
        })
    }

    /// Build a client entirely from the environment.
    pub fn from_env() -> Result<Self, TypeSafeError> {
        Self::new(ClientConfig::default())
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    /// Evaluate a state against its questions.
    pub async fn evaluate(&self, request: &Request) -> Result<Evaluation, TypeSafeError> {
        request.validate()?;

        let model = request.model.as_deref().unwrap_or(&self.default_model);
        let wire = WireRequest {
            state: &request.state,
            model,
            questions: &request.questions,
        };
        let body = serde_json::to_vec(&wire).map_err(|e| TypeSafeError::Encode(e.to_string()))?;
        let url = format!("{}/v1/systemone", self.base_url);

        // Question ids and body size only. The state is user dictation and
        // never belongs in a log line.
        tracing::debug!(
            model = %model,
            questions = request.questions.len(),
            body_bytes = body.len(),
            "Evaluating TypeSafe request"
        );

        self.with_retry(|| async {
            let response = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await;
            self.handle::<Evaluation>(response).await
        })
        .await
    }

    /// Convenience wrapper for the common shape: one state, a few questions.
    pub async fn ask<K: Into<String>>(
        &self,
        state: impl Into<Entry>,
        questions: impl IntoIterator<Item = (K, Question)>,
    ) -> Result<Evaluation, TypeSafeError> {
        self.evaluate(&Request::new(state).ask_all(questions)).await
    }

    /// List the models this account can name in the `model` field.
    pub async fn models(&self) -> Result<Vec<ModelCard>, TypeSafeError> {
        let url = format!("{}/v1/models", self.base_url);
        let list = self
            .with_retry(|| async {
                let response = self.http.get(&url).bearer_auth(&self.api_key).send().await;
                self.handle::<ModelList>(response).await
            })
            .await?;
        Ok(list.into_vec())
    }

    async fn with_retry<T, F, Fut>(&self, mut attempt: F) -> Result<T, TypeSafeError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Attempt<T>>,
    {
        let mut retries = 0u32;
        loop {
            match attempt().await {
                Attempt::Done(value) => return Ok(value),
                Attempt::Fail(err) => return Err(err),
                Attempt::Retry(err, retry_after) => {
                    if retries >= self.retry.max_retries {
                        return Err(err);
                    }
                    retries += 1;
                    let delay = self.retry.delay_for(retries, retry_after);
                    tracing::warn!(
                        attempt = retries,
                        max = self.retry.max_retries,
                        delay_ms = delay.as_millis() as u64,
                        error = %err,
                        "Retrying TypeSafe request"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Turn one reqwest outcome into an [`Attempt`].
    async fn handle<T: serde::de::DeserializeOwned>(
        &self,
        response: Result<reqwest::Response, reqwest::Error>,
    ) -> Attempt<T> {
        let response = match response {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                let err = TypeSafeError::Timeout(self.timeout);
                return if self.retry.retry_on_timeout {
                    Attempt::Retry(err, None)
                } else {
                    Attempt::Fail(err)
                };
            }
            Err(e) => {
                let err = TypeSafeError::Connection(e.to_string());
                return if self.retry.retry_on_connection_error {
                    Attempt::Retry(err, None)
                } else {
                    Attempt::Fail(err)
                };
            }
        };

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => return Attempt::Fail(TypeSafeError::Decode(e.to_string())),
        };

        if (200..300).contains(&status) {
            return match serde_json::from_str::<T>(&body) {
                Ok(value) => Attempt::Done(value),
                Err(e) => Attempt::Fail(TypeSafeError::Decode(format!(
                    "{e} (body: {})",
                    truncate(&body, 200)
                ))),
            };
        }

        let err = TypeSafeError::from_status(status, retry_after, extract_detail(&body));
        if self.retry.retries_status(status) {
            Attempt::Retry(err, retry_after)
        } else {
            Attempt::Fail(err)
        }
    }
}

/// Pull a human-readable message out of an error body, whatever shape it takes.
fn extract_detail(body: &str) -> Option<String> {
    if body.trim().is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        // `detail.message` first: that is the shape the live API returns, e.g.
        // {"detail": {"error_type": "authentication_error", "message": "..."}}.
        for path in [
            &["detail", "message"][..],
            &["error", "message"],
            &["message"],
            &["detail"],
            &["error"],
        ] {
            let mut cursor = &value;
            let mut found = true;
            for key in path {
                match cursor.get(key) {
                    Some(next) => cursor = next,
                    None => {
                        found = false;
                        break;
                    }
                }
            }
            if found {
                if let Some(text) = cursor.as_str() {
                    return Some(text.to_string());
                }
                if found && !cursor.is_null() {
                    return Some(cursor.to_string());
                }
            }
        }
    }
    Some(truncate(body, 300))
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max).collect();
    format!("{cut}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_to_the_documented_body() {
        let request = Request::new("Help! My payouts have been failing for 3 days.")
            .model("jev-latest")
            .ask("is_urgent", Question::noul("Does this convey urgency?"));

        let wire = WireRequest {
            state: &request.state,
            model: request.model.as_deref().unwrap(),
            questions: &request.questions,
        };

        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            json!({
                "state": "Help! My payouts have been failing for 3 days.",
                "model": "jev-latest",
                "questions": {
                    "is_urgent": {
                        "type": "noul",
                        "instructions": "Does this convey urgency?"
                    }
                }
            })
        );
    }

    #[test]
    fn object_state_is_passed_through_unchanged() {
        let request = Request::new(json!({"ticket": {"subject": "Duplicate charge"}})).ask(
            "refund",
            Question::noul("Did the customer ask for a refund?"),
        );
        assert_eq!(
            request.state,
            json!({"ticket": {"subject": "Duplicate charge"}})
        );
    }

    #[test]
    fn questions_keep_their_order_and_ids() {
        let request = Request::new("x")
            .ask("first", Question::noul("a"))
            .ask("second", Question::noul("b"))
            .ask("third", Question::noul("c"));
        assert_eq!(
            request.question_ids().collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
    }

    #[test]
    fn empty_and_malformed_requests_fail_before_sending() {
        assert!(matches!(
            Request::new("x").validate(),
            Err(TypeSafeError::EmptyRequest)
        ));
        assert!(matches!(
            Request::new("x")
                .ask("bad", Question::score("rate it", ["only one level"]))
                .validate(),
            Err(TypeSafeError::InvalidQuestion { .. })
        ));
    }

    #[test]
    fn missing_key_is_reported_clearly() {
        let config = ClientConfig {
            api_key: Some("   ".to_string()),
            ..Default::default()
        };
        // An all-whitespace key is treated as absent, but the environment may
        // still supply one on a developer machine.
        match Client::new(config) {
            Err(TypeSafeError::MissingApiKey) => {}
            Ok(_) => assert!(std::env::var(API_KEY_ENV).is_ok()),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn non_http_base_url_is_rejected() {
        let config = ClientConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some("api.typesafe.ai".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            Client::new(config),
            Err(TypeSafeError::InvalidBaseUrl(_))
        ));
    }

    #[test]
    fn trailing_slash_is_trimmed_from_base_url() {
        let config = ClientConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some("https://api.typesafe.ai/".to_string()),
            ..Default::default()
        };
        let client = Client::new(config).unwrap();
        assert_eq!(client.base_url(), "https://api.typesafe.ai");
    }

    #[test]
    fn debug_output_never_contains_the_key() {
        let config = ClientConfig {
            api_key: Some("sk-secret-value".to_string()),
            ..Default::default()
        };
        let client = Client::new(config.clone()).unwrap();
        assert!(!format!("{client:?}").contains("sk-secret-value"));
        assert!(!format!("{config:?}").contains("sk-secret-value"));
    }

    #[test]
    fn model_list_accepts_bare_and_wrapped_envelopes() {
        let card =
            r#"{"name":"jev-latest","description":"Latest stable","release_date":"2026-09-01"}"#;
        for body in [
            format!("[{card}]"),
            format!(r#"{{"data":[{card}]}}"#),
            format!(r#"{{"models":[{card}]}}"#),
        ] {
            let list: ModelList = serde_json::from_str(&body).unwrap();
            let cards = list.into_vec();
            assert_eq!(cards.len(), 1);
            assert_eq!(cards[0].name, "jev-latest");
        }
    }

    #[test]
    fn error_details_are_pulled_from_common_shapes() {
        // The shape the live API actually returns.
        assert_eq!(
            extract_detail(
                r#"{"detail":{"error_type":"authentication_error","message":"Must supply an API key!"}}"#
            )
            .as_deref(),
            Some("Must supply an API key!")
        );
        assert_eq!(
            extract_detail(r#"{"error":{"message":"bad question"}}"#).as_deref(),
            Some("bad question")
        );
        assert_eq!(
            extract_detail(r#"{"detail":"missing field"}"#).as_deref(),
            Some("missing field")
        );
        assert_eq!(
            extract_detail(r#"{"message":"nope"}"#).as_deref(),
            Some("nope")
        );
        assert_eq!(
            extract_detail("plain text failure").as_deref(),
            Some("plain text failure")
        );
        assert_eq!(extract_detail("   "), None);
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("héllo wörld", 5), "héllo...");
    }
}
