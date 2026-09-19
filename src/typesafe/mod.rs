//! Client for TypeSafe's System One API (Jev)
//!
//! System One models answer typed questions about a piece of state and return
//! calibrated probabilities instead of generated text. Code owns the workflow;
//! the model supplies the semantic judgment where plain code cannot.
//!
//! TypeSafe ships official SDKs for Python and JavaScript only. This module is
//! voxtype's own client, written against the documented HTTP API so that no
//! third-party crate sits between the daemon and the network.
//!
//! # The shape of a request
//!
//! One request carries one **state** and any number of **questions** about it.
//! Jev reads the state once and evaluates every question against it in parallel,
//! so a batched request is far cheaper and faster than one request per question.
//! Build one [`Request`] per state and hang every question off it, including
//! speculative ones whose answers you may discard.
//!
//! ```no_run
//! use voxtype::typesafe::{Client, Question, Request};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::from_env()?;
//!
//! let result = client
//!     .evaluate(
//!         &Request::new("Help! My payouts have been failing for 3 days.")
//!             .ask("is_urgent", Question::noul("Does this convey urgency?"))
//!             .ask(
//!                 "department",
//!                 Question::choice(
//!                     "Which team should handle this?",
//!                     [
//!                         ("billing", "Payments, invoicing, refunds"),
//!                         ("technical", "Bugs, outages, integrations"),
//!                         ("sales", "Pricing, upgrades, new accounts"),
//!                     ],
//!                 ),
//!             )
//!             .ask(
//!                 "frustration",
//!                 Question::score("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"]),
//!             ),
//!     )
//!     .await?;
//!
//! if result.noul("is_urgent")? > 0.8 {
//!     let department = result.choice("department")?;
//!     if department.confidence > 0.7 {
//!         println!("escalate to {}", department.choice);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reading the answers
//!
//! A Noul returns one probability and no confidence, because the probability is
//! the answer: 0.5 means the model genuinely cannot tell, not "somewhat".
//! Choice and Score also return `confidence`, which collapses the shape of the
//! probability distribution into one number. Thresholds belong in calling code,
//! and should scale with the cost of being wrong: a recoverable action can act
//! on a weaker signal than a destructive one.
//!
//! # Limits worth knowing
//!
//! Jev takes text only: a string, a JSON object, or an array of text values.
//! The context budget is 64k tokens for the state plus every question combined,
//! and 32k for the state plus the single longest question. This client does not
//! count tokens, so an oversized request comes back as an API error. English is
//! the strongest language; other languages work but less accurately.
//!
//! # Privacy
//!
//! The state is whatever text you pass in, which for voxtype means the user's
//! dictation leaving the machine. Nothing in this module logs the state or the
//! API key, and [`Client`] redacts the key from its `Debug` output. Any caller
//! that sends transcripts here should be opt-in and say so plainly.

mod answer;
mod client;
mod error;
mod ordered;
mod question;
mod retry;

pub use answer::{Answer, ChoiceAnswer, Evaluation, NoulAnswer, ScoreAnswer, ScoreLevel, Usage};
pub use client::{
    Client, ClientConfig, ModelCard, Request, API_KEY_ENV, BASE_URL_ENV, DEFAULT_BASE_URL,
    DEFAULT_MODEL, DEFAULT_TIMEOUT,
};
pub use error::TypeSafeError;
pub use ordered::OrderedMap;
pub use question::{ChoiceQuestion, Entry, NoulCriteria, NoulQuestion, Question, ScoreQuestion};
pub use retry::RetryPolicy;
