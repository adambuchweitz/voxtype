//! The three System One question types: Noul, Choice, and Score
//!
//! Every question carries `instructions` (what to judge) and, for Choice and
//! Score, `criteria` (the answers it may give). Both fields accept a string, a
//! JSON object, a JSON array, or null, so they are typed as [`Entry`]. Pass a
//! `&str` for the common case and a `serde_json::Value` when the question needs
//! labelled parts or supporting data.

use serde::Serialize;

use super::ordered::OrderedMap;

/// A field that accepts a string, object, array, or null.
///
/// TypeSafe calls this an `EntryType`. `"text".into()` covers the common case;
/// use `serde_json::json!({...})` when structure helps the model.
pub type Entry = serde_json::Value;

/// A typed question to evaluate against the request's state.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul(NoulQuestion),
    Choice(ChoiceQuestion),
    Score(ScoreQuestion),
}

/// A yes/no question. The answer is the probability that the answer is yes.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NoulQuestion {
    pub instructions: Entry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

/// Optional descriptions of what yes and no mean for a Noul.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NoulCriteria {
    /// What a value near 1 means.
    #[serde(rename = "true")]
    pub yes: Entry,
    /// What a value near 0 means.
    #[serde(rename = "false")]
    pub no: Entry,
}

/// Picks one option from a set. The answer carries the full distribution.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChoiceQuestion {
    pub instructions: Entry,
    /// Option name to rubric description. Use `Entry::Null` when an option
    /// needs no extra detail.
    pub criteria: OrderedMap<Entry>,
}

/// Rates the state against ordered levels. The answer is probability-weighted
/// and can land between levels.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ScoreQuestion {
    pub instructions: Entry,
    /// Ordered level descriptions, lowest first. At least two are required.
    pub criteria: Vec<Entry>,
}

impl Question {
    /// A yes/no question with no criteria.
    pub fn noul(instructions: impl Into<Entry>) -> Self {
        Question::Noul(NoulQuestion {
            instructions: instructions.into(),
            criteria: None,
        })
    }

    /// A yes/no question that spells out what yes and no mean.
    pub fn noul_with_criteria(
        instructions: impl Into<Entry>,
        yes: impl Into<Entry>,
        no: impl Into<Entry>,
    ) -> Self {
        Question::Noul(NoulQuestion {
            instructions: instructions.into(),
            criteria: Some(NoulCriteria {
                yes: yes.into(),
                no: no.into(),
            }),
        })
    }

    /// A choice between described options, in the order given.
    pub fn choice<K, V>(
        instructions: impl Into<Entry>,
        options: impl IntoIterator<Item = (K, V)>,
    ) -> Self
    where
        K: Into<String>,
        V: Into<Entry>,
    {
        Question::Choice(ChoiceQuestion {
            instructions: instructions.into(),
            criteria: options.into_iter().map(|(k, v)| (k, v.into())).collect(),
        })
    }

    /// A choice between bare options, with no rubric for any of them.
    pub fn choice_options<K: Into<String>>(
        instructions: impl Into<Entry>,
        options: impl IntoIterator<Item = K>,
    ) -> Self {
        Question::Choice(ChoiceQuestion {
            instructions: instructions.into(),
            criteria: options.into_iter().map(|k| (k, Entry::Null)).collect(),
        })
    }

    /// A rating against ordered levels, lowest first.
    pub fn score<L: Into<Entry>>(
        instructions: impl Into<Entry>,
        levels: impl IntoIterator<Item = L>,
    ) -> Self {
        Question::Score(ScoreQuestion {
            instructions: instructions.into(),
            criteria: levels.into_iter().map(Into::into).collect(),
        })
    }

    /// The wire name of this question's type, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Question::Noul(_) => "noul",
            Question::Choice(_) => "choice",
            Question::Score(_) => "score",
        }
    }

    /// Check what we can check locally, so an obvious mistake surfaces before
    /// a round-trip instead of as a 422.
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Question::Noul(_) => Ok(()),
            Question::Choice(q) => {
                if q.criteria.is_empty() {
                    Err("a Choice needs at least one option in criteria".into())
                } else {
                    Ok(())
                }
            }
            Question::Score(q) => {
                if q.criteria.len() < 2 {
                    Err(format!(
                        "a Score needs at least two levels in criteria, got {}",
                        q.criteria.len()
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn noul_serializes_without_criteria() {
        let q = Question::noul("Does this convey urgency?");
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type": "noul", "instructions": "Does this convey urgency?"})
        );
    }

    #[test]
    fn noul_criteria_use_true_and_false_keys() {
        let q = Question::noul_with_criteria(
            "Does this convey urgency?",
            "Explicitly time-sensitive",
            "No urgency expressed",
        );
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "type": "noul",
                "instructions": "Does this convey urgency?",
                "criteria": {
                    "true": "Explicitly time-sensitive",
                    "false": "No urgency expressed"
                }
            })
        );
    }

    #[test]
    fn choice_keeps_option_order() {
        let q = Question::choice(
            "Which team should handle this?",
            [
                ("billing", "Payments, invoicing, refunds"),
                ("technical", "Bugs, outages, integrations"),
                ("sales", "Pricing, upgrades, new accounts"),
            ],
        );
        let wire = serde_json::to_string(&q).unwrap();
        let billing = wire.find("billing").unwrap();
        let technical = wire.find("technical").unwrap();
        let sales = wire.find("sales").unwrap();
        assert!(billing < technical && technical < sales);
    }

    #[test]
    fn bare_choice_options_serialize_as_null() {
        let q = Question::choice_options("Which value?", ["Beaver", "Dam"]);
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "type": "choice",
                "instructions": "Which value?",
                "criteria": {"Beaver": null, "Dam": null}
            })
        );
    }

    #[test]
    fn score_serializes_levels_as_an_array() {
        let q = Question::score("How frustrated?", ["Calm", "Frustrated", "Very angry"]);
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({
                "type": "score",
                "instructions": "How frustrated?",
                "criteria": ["Calm", "Frustrated", "Very angry"]
            })
        );
    }

    #[test]
    fn structured_instructions_are_accepted() {
        let q = Question::noul(json!({
            "field": {"name": "invoice_number", "type": "string"},
            "extracted_value": "4471",
            "question": "Does `extracted_value` match the `field` in `source_text`?"
        }));
        let wire = serde_json::to_value(&q).unwrap();
        assert_eq!(wire["instructions"]["extracted_value"], json!("4471"));
    }

    #[test]
    fn validation_catches_degenerate_criteria() {
        assert!(Question::score("x", ["only one"]).validate().is_err());
        assert!(Question::choice_options("x", Vec::<String>::new())
            .validate()
            .is_err());
        assert!(Question::score("x", ["a", "b"]).validate().is_ok());
        assert!(Question::noul("x").validate().is_ok());
    }
}
