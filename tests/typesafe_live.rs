//! Live TypeSafe probe for keyword routing. Ignored by default; needs a key.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo test --test typesafe_live -- --ignored --nocapture
//! ```
//!
//! This is the harness that validated the routing design against the real
//! model. Keep it runnable: the registry criteria below are the wording that
//! scored 10/10, and changing a description is exactly the kind of edit that
//! silently costs accuracy.

use std::time::Instant;

use serde_json::{json, Value};
use voxtype::typesafe::{Client, Question, Request};

/// The command registry as Choice criteria, shaped the way a config-driven
/// registry would serialize. The transcript is the state; this is the set of
/// answers the model may give.
///
/// Two details here are load-bearing, both learned the hard way:
///
/// 1. `covers` has to say that a statement of need ("I need to buy milk") is
///    still a task. Without it the model reads the sentence as prose.
/// 2. The exclusion has to name *text composed for another reader*. An earlier
///    version excluded "prose that merely mentions a task", which pushed the
///    exact cases we want to catch into `dictation`.
fn registry() -> Vec<(&'static str, Value)> {
    vec![
        (
            "todoist",
            json!({
                "does": "Record a task, errand, or reminder on the user's todo list.",
                "covers": [
                    "a direct request: 'add buy milk to my list', 'remind me to call the dentist'",
                    "a statement of need or intention that names something the user has to do: 'I need to buy milk', 'I should email Sara about the invoice'"
                ],
                "not": ["text the user is composing for another person to read, such as an email, message, or document"]
            }),
        ),
        (
            "notify",
            json!({
                "does": "Show the spoken message back to the user as a desktop notification, right now.",
                "covers": ["'notify the build finished', 'notify call mom'"],
                "not": ["waiting for a future event before alerting", "a message addressed to another person"]
            }),
        ),
        (
            "launch",
            json!({
                "does": "Open an installed application on the user's computer right now.",
                "covers": ["'launch firefox', 'open my browser', 'start the terminal'"],
                "not": ["discussing launching a product, feature, campaign, or rocket"]
            }),
        ),
        (
            "dictation",
            json!({
                "does": "Ordinary text the user wants written down verbatim. No command is being given and nothing should be executed.",
                "covers": ["prose, notes, messages, and anything being composed for a human reader"]
            }),
        ),
    ]
}

/// Candidate spans for the command body: every word-boundary suffix. Jev
/// selects one of these, so the body comes back verbatim and cannot be
/// invented or garbled.
fn body_spans(transcript: &str) -> Vec<String> {
    let words: Vec<&str> = transcript.split_whitespace().collect();
    let mut spans: Vec<String> = (0..words.len()).map(|i| words[i..].join(" ")).collect();
    spans.truncate(12);
    spans
}

fn route_request(transcript: &str, focused_app: Option<&str>) -> Request {
    let mut state = json!({ "transcript": transcript });
    if let Some(app) = focused_app {
        state["focused_application"] = json!(app);
    }
    Request::new(state)
        .ask(
            "intent",
            Question::choice(
                "The user spoke this while dictating at their computer. Which registered command, if any, are they asking the computer to run?",
                registry(),
            ),
        )
        .ask(
            "body",
            Question::choice_options(
                "Assuming this is a command, which span is the command's payload with the instruction words removed? Pick the whole transcript if nothing should be stripped.",
                body_spans(transcript),
            ),
        )
}

const CASES: &[(&str, &str)] = &[
    ("I need to buy milk", "todoist"),
    ("remind me to call the dentist", "todoist"),
    ("add finish the report to my list", "todoist"),
    ("I should email Sara about the invoice", "todoist"),
    ("launch firefox", "launch"),
    ("open my browser", "launch"),
    ("notify call mom", "notify"),
    ("I was thinking we could launch the product in Q3", "dictation"),
    ("The meeting is at three and we should discuss the budget", "dictation"),
    (
        "Hi Sara, I need to buy milk on the way home so I will be late",
        "dictation",
    ),
];

#[tokio::test]
#[ignore]
async fn routing_accuracy_and_latency() {
    let client = Client::from_env().expect("TYPESAFE_API_KEY not set");
    let mut correct = 0usize;
    let mut latencies = Vec::new();

    println!(
        "\n{:<58} {:<10} {:<10} {:>5} {:>6}  {}",
        "transcript", "expected", "got", "conf", "ms", "body"
    );
    for (transcript, expected) in CASES {
        let started = Instant::now();
        let result = client
            .evaluate(&route_request(transcript, None))
            .await
            .expect("evaluate failed");
        latencies.push(started.elapsed().as_millis());

        let intent = result.choice("intent").expect("intent");
        let body = result.choice("body").expect("body");
        let hit = intent.choice == *expected;
        if hit {
            correct += 1;
        }
        println!(
            "{:<58} {:<10} {:<10} {:>5.2} {:>6}  {}{}",
            clip(transcript, 56),
            expected,
            intent.choice,
            intent.confidence,
            latencies.last().unwrap(),
            clip(&body.choice, 34),
            if hit { "" } else { "   <-- MISS" }
        );
    }

    latencies.sort_unstable();
    println!(
        "\n{correct}/{} correct, p50 {}ms, max {}ms",
        CASES.len(),
        latencies[latencies.len() / 2],
        latencies.last().unwrap()
    );
    assert_eq!(correct, CASES.len(), "routing regressed; check the criteria");
}

/// The focused window flips an ambiguous transcript. Voxtype already knows the
/// focused app (`src/window.rs`), so this costs nothing to include in state.
#[tokio::test]
#[ignore]
async fn focused_app_discriminates_ambiguous_transcripts() {
    let client = Client::from_env().expect("TYPESAFE_API_KEY not set");
    let transcript = "I need to buy milk";
    for app in [
        "Alacritty (terminal)",
        "Thunderbird (email composer)",
        "Todoist",
    ] {
        let result = client
            .evaluate(&route_request(transcript, Some(app)))
            .await
            .expect("evaluate failed");
        let intent = result.choice("intent").expect("intent");
        println!(
            "{:<32} -> {:<10} conf {:.2}",
            app, intent.choice, intent.confidence
        );
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max - 3).collect::<String>())
    }
}
