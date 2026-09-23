// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::act::{Action, Actions};
use super::config::ParseResult;
use super::metrics::MetricState;
use super::metrics::fetch::{Fetcher, KeyValueFetcher, TextFetcher, TrialDataFetcher};
use super::metrics::metric_value::MetricValue;
use anyhow::{Error, bail, format_err};
use serde::Deserialize;
use serde_json as json;
use std::collections::HashMap;

#[derive(Clone, Deserialize, Debug)]
pub struct Trial {
    yes: Option<Vec<String>>,
    no: Option<Vec<String>>,
    now: Option<String>,
    values: Option<HashMap<String, json::Value>>,
    klog: Option<String>,
    syslog: Option<String>,
    bootlog: Option<String>,
    annotations: Option<json::map::Map<String, json::Value>>,
}

// Outer String is namespace, inner String is trial name
pub type Trials = HashMap<String, TrialsSchema>;
pub type TrialsSchema = HashMap<String, Trial>;

pub fn validate(parse_result: &ParseResult) -> Result<(), Error> {
    let ParseResult { metrics, actions, tests } = parse_result;
    let mut failed = false;
    for (namespace, trial_map) in tests {
        for (trial_name, trial) in trial_map {
            if trial.yes.is_none() && trial.no.is_none() {
                bail!("Trial {} in {} needs a yes: or no: entry", trial_name, namespace);
            }
            let klog_fetcher;
            let syslog_fetcher;
            let bootlog_fetcher;
            let annotations_fetcher;
            let mut fetcher = match &trial.values {
                Some(values) => Fetcher::TrialData(TrialDataFetcher::new(values)),
                None => Fetcher::TrialData(TrialDataFetcher::new_empty()),
            };
            if let Fetcher::TrialData(fetcher) = &mut fetcher {
                if let Some(klog) = &trial.klog {
                    klog_fetcher = TextFetcher::from(&**klog);
                    fetcher.set_klog(&klog_fetcher);
                }
                if let Some(syslog) = &trial.syslog {
                    syslog_fetcher = TextFetcher::from(&**syslog);
                    fetcher.set_syslog(&syslog_fetcher);
                }
                if let Some(bootlog) = &trial.bootlog {
                    bootlog_fetcher = TextFetcher::from(&**bootlog);
                    fetcher.set_bootlog(&bootlog_fetcher);
                }
                if let Some(annotations) = &trial.annotations {
                    annotations_fetcher = KeyValueFetcher::try_from(annotations)?;
                    fetcher.set_annotations(&annotations_fetcher);
                }
            }
            let now = if let Some(now) = &trial.now {
                match MetricState::evaluate_math(now) {
                    MetricValue::Int(time) => Some(time),
                    oops => bail!(
                        "Trial {} in {}: 'now: {}' was not integer, it was {:?}",
                        trial_name,
                        namespace,
                        now,
                        oops
                    ),
                }
            } else {
                None
            };
            if let Some(action_names) = &trial.yes {
                for action_name in action_names.iter() {
                    let original_metrics = &(metrics.clone());
                    let original_actions = &(actions.clone());
                    let state = MetricState::new(original_metrics, fetcher.clone(), now);
                    failed = check_failure(
                        namespace,
                        trial_name,
                        action_name,
                        original_actions,
                        &state,
                        true,
                    ) || failed;
                }
            }
            if let Some(action_names) = &trial.no {
                for action_name in action_names.iter() {
                    let original_metrics = &(metrics.clone());
                    let original_actions = &(actions.clone());
                    let state = MetricState::new(original_metrics, fetcher.clone(), now);
                    failed = check_failure(
                        namespace,
                        trial_name,
                        action_name,
                        original_actions,
                        &state,
                        false,
                    ) || failed;
                }
            }
        }
    }
    if failed { Err(format_err!("Config validation test failed")) } else { Ok(()) }
}

// Returns true iff the trial did NOT get the expected result.
fn check_failure(
    namespace: &String,
    trial_name: &String,
    action_name: &String,
    actions: &Actions,
    metric_state: &MetricState<'_>,
    expected: bool,
) -> bool {
    match actions.get(namespace) {
        None => {
            println!("Namespace {action_name} not found in trial {trial_name}");
            true
        }
        Some(action_map) => match action_map.get(action_name) {
            None => {
                println!("Action {action_name} not found in trial {trial_name}");
                true
            }
            Some(action) => {
                let trigger = match action {
                    Action::Alert(properties) => &properties.trigger,
                    Action::Snapshot(properties) => &properties.trigger,
                    _ => {
                        println!("Action {action:?} cannot be tested");
                        return true;
                    }
                };
                match metric_state.eval_action_metric(namespace, trigger) {
                    MetricValue::Bool(actual) if actual == expected => false,
                    other => {
                        println!(
                            "Test {trial_name} failed: trigger '{trigger}' of action \
                            {action_name} returned {other:?}, expected {expected}",
                        );
                        true
                    }
                }
            }
        },
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::act::{Alert, Severity};
    use crate::make_metrics;
    use crate::metrics::ValueSource;

    // Correct operation of the klog, syslog, and bootlog fields of TrialDataFetcher are tested
    // in the integration test via log_tests.triage.

    macro_rules! build_map {($($tuple:expr),*) => ({
        let mut map = HashMap::new();
        $(
            let (key, value) = $tuple;
            map.insert(key.to_string(), value);
         )*
            map
    })}

    macro_rules! create_parse_result {
        (metrics: $metrics:expr, actions: $actions:expr, tests: $tests:expr) => {
            ParseResult {
                metrics: $metrics.clone(),
                actions: $actions.clone(),
                tests: $tests.clone(),
            }
        };
    }

    #[fuchsia::test]
    fn validate_works() -> Result<(), Error> {
        let metrics = make_metrics!({
            "foo":{
                eval: {
                    "true": "1 == 1",
                    "false": "1 == 0"
                }
            }
        });

        let actions = build_map!((
            "foo",
            build_map!(
                (
                    "fires",
                    Action::Alert(Alert {
                        trigger: ValueSource::try_from_expression_with_namespace("true", "foo")?,
                        print: "good".to_string(),
                        tag: None,
                        file_bug: None,
                        severity: Severity::Warning,
                    })
                ),
                (
                    "no_fire",
                    Action::Alert(Alert {
                        trigger: ValueSource::try_from_expression_with_namespace("false", "foo")?,
                        print: "what?!?".to_string(),
                        tag: None,
                        file_bug: None,
                        severity: Severity::Warning,
                    })
                )
            )
        ));
        let good_trial = Trial {
            yes: Some(vec!["fires".to_string()]),
            no: Some(vec!["no_fire".to_string()]),
            values: Some(HashMap::new()),
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: None,
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("good", good_trial))))
            ))
            .is_ok()
        );
        // Make sure it objects if a trial that should fire doesn't.
        // Also, make sure it signals failure if there's both a good and a bad trial.
        let bad_trial = Trial {
            yes: Some(vec!["fires".to_string(), "no_fire".to_string()]),
            no: None, // Test that None doesn't crash
            values: None,
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: None,
        };
        let good_trial = Trial {
            yes: Some(vec!["fires".to_string()]),
            no: Some(vec!["no_fire".to_string()]),
            values: Some(HashMap::new()),
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: None,
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("good", good_trial), ("bad", bad_trial))))
            ))
            .is_err()
        );
        // Make sure it objects if a trial fires when it shouldn't.
        let bad_trial = Trial {
            yes: Some(vec![]), // Test that empty vec works right
            no: Some(vec!["fires".to_string(), "no_fire".to_string()]),
            values: None,
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: None,
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("bad", bad_trial))))
            ))
            .is_err()
        );
        Ok(())
    }

    #[fuchsia::test]
    fn validate_time() -> Result<(), Error> {
        let metrics = HashMap::new();
        let actions = build_map!((
            "foo",
            build_map!(
                (
                    "time_quarter",
                    Action::Alert(Alert {
                        trigger: ValueSource::try_from_expression_with_namespace(
                            "Now()==250000000",
                            "foo"
                        )?,
                        print: "time_billion".to_string(),
                        tag: None,
                        file_bug: None,
                        severity: Severity::Warning,
                    })
                ),
                (
                    "time_missing",
                    Action::Alert(Alert {
                        trigger: ValueSource::try_from_expression_with_namespace(
                            "Missing(Now())",
                            "foo"
                        )?,
                        print: "time_missing".to_string(),
                        tag: None,
                        file_bug: None,
                        severity: Severity::Warning,
                    })
                )
            )
        ));
        let time_trial = Trial {
            yes: Some(vec!["time_quarter".to_string()]),
            no: Some(vec!["time_missing".to_string()]),
            values: Some(HashMap::new()),
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: Some("Seconds(0.25)".to_string()),
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("good", time_trial))))
            ))
            .is_ok()
        );
        let missing_trial = Trial {
            yes: Some(vec!["time_missing".to_string()]),
            no: Some(vec![]),
            values: Some(HashMap::new()),
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: None,
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("good", missing_trial))))
            ))
            .is_ok()
        );
        let bad_trial = Trial {
            yes: Some(vec!["time_missing".to_string()]),
            no: Some(vec![]),
            values: Some(HashMap::new()),
            klog: None,
            syslog: None,
            bootlog: None,
            annotations: None,
            now: Some("this won't parse".to_string()),
        };
        assert!(
            validate(&create_parse_result!(
                metrics: metrics,
                actions: actions,
                tests: build_map!(("foo", build_map!(("good", bad_trial))))
            ))
            .is_err()
        );
        Ok(())
    }
}
