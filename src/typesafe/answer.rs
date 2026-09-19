//! Answers returned by the System One endpoint
//!
//! One answer comes back per question, under the id the caller chose. Every
//! answer carries a `type` matching its question. Choice and Score answers also
//! carry a `confidence` derived from their probability distribution; Noul
//! answers do not, because the probability is the answer.

use std::collections::HashMap;

use serde::Deserialize;

use super::error::TypeSafeError;

/// A full evaluation response.
#[derive(Debug, Clone, Deserialize)]
pub struct Evaluation {
    /// The versioned model that answered, e.g. `jev-1.13.0`. Worth logging when
    /// the request used a moving alias like `jev-latest`.
    pub model: String,
    /// One answer per question, keyed by the ids from the request.
    pub answers: HashMap<String, Answer>,
    pub usage: Usage,
}

/// Token usage. Only input tokens are billed.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// One typed answer.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
    Score(ScoreAnswer),
}

/// The probability that the answer to a yes/no question is yes.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct NoulAnswer {
    /// 0 (no) to 1 (yes). A value near 0.5 means the model genuinely cannot
    /// tell, not that the answer is "medium".
    pub noul: f64,
}

/// The selected option, the distribution across all options, and confidence.
#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceAnswer {
    /// The highest-probability option.
    pub choice: String,
    /// Every option mapped to its probability. Sums to 1.
    pub probabilities: HashMap<String, f64>,
    /// How concentrated the distribution is, 0 to 1.
    pub confidence: f64,
}

/// A probability-weighted position across the question's levels.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoreAnswer {
    /// The weighted answer. Can land between levels, e.g. 1.6.
    pub score: f64,
    /// Level index (as a string key) to the description from the request.
    pub legend: HashMap<String, String>,
    /// Level index (as a string key) to its probability. Sums to 1.
    pub probabilities: HashMap<String, f64>,
    /// How concentrated the distribution is, 0 to 1.
    pub confidence: f64,
}

/// One level of a Score answer, resolved against the legend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreLevel<'a> {
    pub index: usize,
    pub description: &'a str,
    pub probability: f64,
}

impl Answer {
    pub fn type_name(&self) -> &'static str {
        match self {
            Answer::Noul(_) => "noul",
            Answer::Choice(_) => "choice",
            Answer::Score(_) => "score",
        }
    }

    /// Confidence, for the two types that report it.
    pub fn confidence(&self) -> Option<f64> {
        match self {
            Answer::Noul(_) => None,
            Answer::Choice(a) => Some(a.confidence),
            Answer::Score(a) => Some(a.confidence),
        }
    }

    pub fn as_noul(&self) -> Option<&NoulAnswer> {
        match self {
            Answer::Noul(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&ChoiceAnswer> {
        match self {
            Answer::Choice(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_score(&self) -> Option<&ScoreAnswer> {
        match self {
            Answer::Score(a) => Some(a),
            _ => None,
        }
    }
}

impl ChoiceAnswer {
    /// The probability assigned to one option.
    pub fn probability(&self, option: &str) -> Option<f64> {
        self.probabilities.get(option).copied()
    }

    /// Every option, most likely first. Ties break alphabetically so the order
    /// is stable across runs.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut ranked: Vec<(&str, f64)> = self
            .probabilities
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(b.0))
        });
        ranked
    }

    /// The gap between the top two options. A small margin means the model is
    /// torn between them even when `confidence` looks acceptable.
    pub fn margin(&self) -> f64 {
        let ranked = self.ranked();
        match ranked.as_slice() {
            [top, second, ..] => top.1 - second.1,
            [only] => only.1,
            [] => 0.0,
        }
    }
}

impl ScoreAnswer {
    /// Every level in order, lowest first, resolved against the legend.
    ///
    /// Keys arrive as strings, so this parses and sorts them numerically. A
    /// `BTreeMap` would order "10" before "2".
    pub fn levels(&self) -> Vec<ScoreLevel<'_>> {
        let mut levels: Vec<ScoreLevel<'_>> = self
            .legend
            .iter()
            .filter_map(|(key, description)| {
                let index = key.parse::<usize>().ok()?;
                Some(ScoreLevel {
                    index,
                    description: description.as_str(),
                    probability: self.probabilities.get(key).copied().unwrap_or(0.0),
                })
            })
            .collect();
        levels.sort_by_key(|level| level.index);
        levels
    }

    /// The single most likely level.
    pub fn most_likely(&self) -> Option<ScoreLevel<'_>> {
        self.levels().into_iter().max_by(|a, b| {
            a.probability
                .partial_cmp(&b.probability)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.index.cmp(&a.index))
        })
    }

    /// The weighted score snapped to the nearest level index.
    pub fn nearest_index(&self) -> usize {
        self.score.round().max(0.0) as usize
    }

    /// The description of the level nearest the weighted score.
    pub fn nearest_description(&self) -> Option<&str> {
        self.legend
            .get(&self.nearest_index().to_string())
            .map(String::as_str)
    }

    /// The score rescaled to 0.0..=1.0 across the level range, which makes
    /// scores with different level counts comparable.
    pub fn normalized(&self) -> f64 {
        let top = self.legend.len().saturating_sub(1);
        if top == 0 {
            return 0.0;
        }
        (self.score / top as f64).clamp(0.0, 1.0)
    }
}

impl Evaluation {
    /// Look up an answer by question id.
    pub fn answer(&self, id: &str) -> Result<&Answer, TypeSafeError> {
        self.answers
            .get(id)
            .ok_or_else(|| TypeSafeError::MissingAnswer(id.to_string()))
    }

    /// Look up a Noul answer and return its probability.
    pub fn noul(&self, id: &str) -> Result<f64, TypeSafeError> {
        let answer = self.answer(id)?;
        answer
            .as_noul()
            .map(|a| a.noul)
            .ok_or_else(|| TypeSafeError::AnswerTypeMismatch {
                id: id.to_string(),
                expected: "noul",
                actual: answer.type_name(),
            })
    }

    /// Look up a Choice answer.
    pub fn choice(&self, id: &str) -> Result<&ChoiceAnswer, TypeSafeError> {
        let answer = self.answer(id)?;
        answer
            .as_choice()
            .ok_or_else(|| TypeSafeError::AnswerTypeMismatch {
                id: id.to_string(),
                expected: "choice",
                actual: answer.type_name(),
            })
    }

    /// Look up a Score answer.
    pub fn score(&self, id: &str) -> Result<&ScoreAnswer, TypeSafeError> {
        let answer = self.answer(id)?;
        answer
            .as_score()
            .ok_or_else(|| TypeSafeError::AnswerTypeMismatch {
                id: id.to_string(),
                expected: "score",
                actual: answer.type_name(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Evaluation {
        serde_json::from_str(
            r#"{
              "model": "jev-1.13.0",
              "answers": {
                "is_urgent": {"type": "noul", "noul": 0.92},
                "department": {
                  "type": "choice",
                  "choice": "technical",
                  "probabilities": {"billing": 0.08, "technical": 0.85, "sales": 0.07},
                  "confidence": 0.82
                },
                "frustration": {
                  "type": "score",
                  "score": 1.6,
                  "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                  "probabilities": {"0": 0.05, "1": 0.3, "2": 0.65},
                  "confidence": 0.78
                }
              },
              "usage": {"input_tokens": 312, "output_tokens": 48}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_all_three_answer_types() {
        let e = sample();
        assert_eq!(e.model, "jev-1.13.0");
        assert_eq!(e.usage.input_tokens, 312);
        assert_eq!(e.noul("is_urgent").unwrap(), 0.92);
        assert_eq!(e.choice("department").unwrap().choice, "technical");
        assert_eq!(e.score("frustration").unwrap().score, 1.6);
    }

    #[test]
    fn noul_has_no_confidence_but_others_do() {
        let e = sample();
        assert_eq!(e.answer("is_urgent").unwrap().confidence(), None);
        assert_eq!(e.answer("department").unwrap().confidence(), Some(0.82));
        assert_eq!(e.answer("frustration").unwrap().confidence(), Some(0.78));
    }

    #[test]
    fn choice_ranks_and_measures_margin() {
        let e = sample();
        let choice = e.choice("department").unwrap();
        let ranked = choice.ranked();
        assert_eq!(ranked[0], ("technical", 0.85));
        assert_eq!(ranked[1].0, "billing");
        assert!((choice.margin() - 0.77).abs() < 1e-9);
        assert_eq!(choice.probability("sales"), Some(0.07));
        assert_eq!(choice.probability("nope"), None);
    }

    #[test]
    fn score_resolves_levels_against_the_legend() {
        let e = sample();
        let score = e.score("frustration").unwrap();
        let levels = score.levels();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0].description, "Calm");
        assert_eq!(levels[2].probability, 0.65);
        assert_eq!(score.most_likely().unwrap().description, "Very angry");
        assert_eq!(score.nearest_index(), 2);
        assert_eq!(score.nearest_description(), Some("Very angry"));
        assert!((score.normalized() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn score_levels_sort_numerically_past_nine() {
        let score: ScoreAnswer = serde_json::from_str(
            r#"{
              "score": 9.0,
              "legend": {"0":"a","1":"b","2":"c","9":"j","10":"k"},
              "probabilities": {"0":0.0,"1":0.0,"2":0.0,"9":0.5,"10":0.5},
              "confidence": 0.5
            }"#,
        )
        .unwrap();
        let indices: Vec<usize> = score.levels().iter().map(|l| l.index).collect();
        assert_eq!(indices, [0, 1, 2, 9, 10]);
    }

    #[test]
    fn wrong_type_and_missing_id_are_distinct_errors() {
        let e = sample();
        assert!(matches!(
            e.noul("department"),
            Err(TypeSafeError::AnswerTypeMismatch {
                expected: "noul",
                actual: "choice",
                ..
            })
        ));
        assert!(matches!(
            e.answer("nope"),
            Err(TypeSafeError::MissingAnswer(_))
        ));
    }
}
