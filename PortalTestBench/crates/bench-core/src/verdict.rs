//! Deciding pass or fail.
//!
//! [`evaluate`] is a **pure function** of the criteria and the measurements. No clock, no
//! transport, no hardware — which is what makes the pass/fail decision testable from a fixture
//! vector, and that property is most of what makes a green suite mean anything here.
//!
//! A `Fail` is a *result*, not an error: the run completed and a criterion did not hold. That
//! distinction runs all the way out to `ptb`'s exit codes, where 1 is a failing test and 3 is a
//! bench that could not carry the test out. Conflating them makes "did it pass" unanswerable
//! from a script.

use serde::{Deserialize, Serialize};

/// A value a step recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MeasureValue {
    Bool(bool),
    Number(f64),
    Text(String),
    /// The step ran but the value was not available — a flag the firmware never reported, a
    /// log line that never arrived. Explicitly *not* zero or false.
    Missing,
}

impl MeasureValue {
    /// The numeric view, where one exists. `true` is 1 so a boolean can be counted.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            MeasureValue::Number(v) => Some(*v),
            MeasureValue::Bool(true) => Some(1.0),
            MeasureValue::Bool(false) => Some(0.0),
            MeasureValue::Text(_) | MeasureValue::Missing => None,
        }
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, MeasureValue::Missing)
    }
}

impl std::fmt::Display for MeasureValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeasureValue::Bool(v) => write!(f, "{v}"),
            MeasureValue::Number(v) => write!(f, "{v}"),
            MeasureValue::Text(v) => write!(f, "{v}"),
            MeasureValue::Missing => write!(f, "not measured"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: MeasureValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub at_ms: u64,
    pub step_index: usize,
}

/// What a measurement must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Predicate {
    IsTrue,
    IsFalse,
    AtMost(f64),
    AtLeast(f64),
    Between { lo: f64, hi: f64 },
    /// Present and not [`MeasureValue::Missing`]. Useful when the fact of a reading is the test.
    IsPresent,
}

impl std::fmt::Display for Predicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Predicate::IsTrue => write!(f, "must be true"),
            Predicate::IsFalse => write!(f, "must be false"),
            Predicate::AtMost(v) => write!(f, "must be at most {v}"),
            Predicate::AtLeast(v) => write!(f, "must be at least {v}"),
            Predicate::Between { lo, hi } => write!(f, "must be between {lo} and {hi}"),
            Predicate::IsPresent => write!(f, "must have been measured"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub measurement: String,
    pub must: Predicate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "verdict", rename_all = "lowercase")]
pub enum Verdict {
    /// Every criterion held.
    Pass,
    /// The run completed and a criterion did not. A real answer.
    Fail { criterion: String, got: String, expected: String },
    /// Stopped before it could decide.
    Aborted { reason: String },
    /// The bench could not carry the test out at all.
    Error { detail: String, advice: Option<String> },
}

impl Verdict {
    /// The exit code `ptb` returns. The code *is* the answer.
    pub fn exit_code(&self) -> u8 {
        match self {
            Verdict::Pass => 0,
            Verdict::Fail { .. } => 1,
            Verdict::Aborted { .. } => 2,
            Verdict::Error { .. } => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail { .. } => "fail",
            Verdict::Aborted { .. } => "aborted",
            Verdict::Error { .. } => "error",
        }
    }

    /// One line saying why, for the verdict band and the report.
    pub fn reason(&self) -> String {
        match self {
            Verdict::Pass => String::new(),
            Verdict::Fail { criterion, got, expected } => {
                format!("{criterion} {expected}, got {got}")
            }
            Verdict::Aborted { reason } => reason.clone(),
            Verdict::Error { detail, .. } => detail.clone(),
        }
    }
}

/// Decide the verdict. Pure.
///
/// Criteria are checked **in order** and the first failure wins, so the reported reason is the
/// earliest thing that went wrong rather than whichever happened to be last in the list.
pub fn evaluate(criteria: &[Criterion], measurements: &[Measurement]) -> Verdict {
    for criterion in criteria {
        let Some(measurement) =
            measurements.iter().rev().find(|m| m.name == criterion.measurement)
        else {
            // A criterion whose measurement never arrived cannot pass. Silently ignoring it
            // would turn a run that lost its evidence into a clean pass.
            return Verdict::Fail {
                criterion: criterion.measurement.clone(),
                got: "never recorded".into(),
                expected: criterion.must.to_string(),
            };
        };

        if !holds(&criterion.must, &measurement.value) {
            return Verdict::Fail {
                criterion: criterion.measurement.clone(),
                got: measurement.value.to_string(),
                expected: criterion.must.to_string(),
            };
        }
    }
    Verdict::Pass
}

fn holds(predicate: &Predicate, value: &MeasureValue) -> bool {
    // A missing value satisfies nothing except an explicit check that it is absent -- and there
    // is no such predicate, deliberately. "Not measured" must never read as a pass.
    if value.is_missing() {
        return false;
    }
    match predicate {
        Predicate::IsPresent => true,
        Predicate::IsTrue => matches!(value, MeasureValue::Bool(true)),
        Predicate::IsFalse => matches!(value, MeasureValue::Bool(false)),
        Predicate::AtMost(limit) => value.as_number().is_some_and(|v| v <= *limit),
        Predicate::AtLeast(limit) => value.as_number().is_some_and(|v| v >= *limit),
        Predicate::Between { lo, hi } => value.as_number().is_some_and(|v| v >= *lo && v <= *hi),
    }
}

/// A one-line summary of the measurements, for the status line and `/last/measurements`.
pub fn summarise(measurements: &[Measurement]) -> String {
    measurements
        .iter()
        .map(|m| match &m.unit {
            Some(unit) => format!("{} {} {unit}", m.name, m.value),
            None => format!("{} {}", m.name, m.value),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(name: &str, value: MeasureValue) -> Measurement {
        Measurement { name: name.into(), value, unit: None, at_ms: 0, step_index: 0 }
    }

    #[test]
    fn all_criteria_holding_is_a_pass() {
        let criteria = vec![
            Criterion { measurement: "home_ok".into(), must: Predicate::IsTrue },
            Criterion { measurement: "backlash".into(), must: Predicate::AtMost(900.0) },
        ];
        let measurements = vec![
            measured("home_ok", MeasureValue::Bool(true)),
            measured("backlash", MeasureValue::Number(612.0)),
        ];
        assert_eq!(evaluate(&criteria, &measurements), Verdict::Pass);
    }

    #[test]
    fn a_failing_criterion_names_itself_and_what_it_wanted() {
        let criteria = vec![Criterion { measurement: "backlash".into(), must: Predicate::AtMost(900.0) }];
        let measurements = vec![measured("backlash", MeasureValue::Number(1240.0))];
        let verdict = evaluate(&criteria, &measurements);
        assert_eq!(
            verdict,
            Verdict::Fail {
                criterion: "backlash".into(),
                got: "1240".into(),
                expected: "must be at most 900".into(),
            }
        );
        assert_eq!(verdict.exit_code(), 1);
        assert!(verdict.reason().contains("backlash"));
    }

    /// The single most dangerous way for this to be wrong: a run that lost its evidence must
    /// not come back clean.
    #[test]
    fn a_criterion_whose_measurement_never_arrived_fails() {
        let criteria = vec![Criterion { measurement: "home_ok".into(), must: Predicate::IsTrue }];
        let verdict = evaluate(&criteria, &[]);
        assert!(matches!(&verdict, Verdict::Fail { got, .. } if got == "never recorded"));
    }

    /// Likewise a measurement that was attempted and came back empty.
    #[test]
    fn a_missing_value_satisfies_nothing() {
        let measurements = vec![measured("home_ok", MeasureValue::Missing)];
        for predicate in [
            Predicate::IsTrue,
            Predicate::IsFalse,
            Predicate::IsPresent,
            Predicate::AtMost(1e9),
            Predicate::AtLeast(-1e9),
            Predicate::Between { lo: -1e9, hi: 1e9 },
        ] {
            let criteria = vec![Criterion { measurement: "home_ok".into(), must: predicate }];
            assert!(
                matches!(evaluate(&criteria, &measurements), Verdict::Fail { .. }),
                "{predicate:?} accepted a missing value"
            );
        }
    }

    /// The reported reason should be the earliest thing that went wrong, not the last one in
    /// the list -- that is what someone reads first when deciding what to look at.
    #[test]
    fn the_first_failing_criterion_is_the_one_reported() {
        let criteria = vec![
            Criterion { measurement: "home_ok".into(), must: Predicate::IsTrue },
            Criterion { measurement: "backlash".into(), must: Predicate::AtMost(900.0) },
        ];
        let measurements = vec![
            measured("home_ok", MeasureValue::Bool(false)),
            measured("backlash", MeasureValue::Number(5000.0)),
        ];
        assert!(matches!(evaluate(&criteria, &measurements), Verdict::Fail { criterion, .. } if criterion == "home_ok"));
    }

    /// A soak records the same name once per cycle; the criterion should judge the latest.
    #[test]
    fn a_repeated_measurement_is_judged_on_its_most_recent_value() {
        let criteria = vec![Criterion { measurement: "backlash".into(), must: Predicate::AtMost(900.0) }];
        let measurements = vec![
            measured("backlash", MeasureValue::Number(500.0)),
            measured("backlash", MeasureValue::Number(1500.0)),
        ];
        assert!(matches!(evaluate(&criteria, &measurements), Verdict::Fail { .. }));
    }

    #[test]
    fn exit_codes_distinguish_a_failing_test_from_a_broken_bench() {
        assert_eq!(Verdict::Pass.exit_code(), 0);
        assert_eq!(
            Verdict::Fail { criterion: "x".into(), got: "1".into(), expected: "0".into() }.exit_code(),
            1
        );
        assert_eq!(Verdict::Aborted { reason: "operator".into() }.exit_code(), 2);
        assert_eq!(Verdict::Error { detail: "link lost".into(), advice: None }.exit_code(), 3);
    }

    #[test]
    fn summaries_carry_units_when_a_measurement_has_one() {
        let measurements = vec![
            Measurement {
                name: "backlash".into(),
                value: MeasureValue::Number(612.0),
                unit: Some("usteps".into()),
                at_ms: 0,
                step_index: 0,
            },
            measured("home_ok", MeasureValue::Bool(true)),
        ];
        assert_eq!(summarise(&measurements), "backlash 612 usteps · home_ok true");
    }
}
