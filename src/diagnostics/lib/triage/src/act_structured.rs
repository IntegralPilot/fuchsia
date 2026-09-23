// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::config::DiagnosticData;
use super::metrics::fetch::{Fetcher, FileDataFetcher};
use super::metrics::metric_value::{MetricValue, Problem};
use super::metrics::{MetricState, Metrics, ValueSource};
use super::plugins::{Plugin, register_plugins};
use crate::act::{Action, Actions, Alert, Gauge, Snapshot};
use crate::inspect_logger;
use serde::Serialize;
use std::collections::HashMap;

/// Provides the [metric_state] context to evaluate [Action]s and results of the [actions].
#[derive(Clone, Debug, Serialize)]
pub struct TriageOutput {
    actions: Actions,
    metrics: Metrics,
    plugin_results: HashMap<String, Vec<Action>>,
    triage_errors: Vec<String>,
}

impl TriageOutput {
    /// Instantiates a [TriageOutput] populated with Maps corresponding to the provided namespaces.
    pub fn new(namespaces: Vec<String>) -> TriageOutput {
        let mut actions: Actions = HashMap::new();
        let mut metrics: HashMap<String, HashMap<String, ValueSource>> = HashMap::new();

        // Avoid overhead of checking/creating new map when creating a synthetic action
        // Namespaces with no triggered actions are empty
        for namespace in namespaces {
            metrics.insert(namespace.clone(), HashMap::new());
            actions.insert(namespace, HashMap::new());
        }

        TriageOutput { actions, metrics, triage_errors: Vec::new(), plugin_results: HashMap::new() }
    }

    /// Add an error generated while creating TriageOutput
    pub fn add_error(&mut self, error: String) {
        self.triage_errors.push(error);
    }

    /// Add an Action to processing during TriageOutput
    pub fn add_action(&mut self, namespace: String, name: String, action: Action) {
        if let Some(actions_map) = self.actions.get_mut(&namespace) {
            actions_map.insert(name, action);
        }
    }

    /// Returns true if any triggered [Warning]s or [Error]s are generated while building the
    /// [TriageOutput], and false otherwise.
    pub fn has_reportable_issues(&self) -> bool {
        for actions_map in self.actions.values() {
            for action in actions_map.values() {
                if action.has_reportable_issue() {
                    return true;
                }
            }
        }
        false
    }
}

pub struct StructuredActionContext<'a> {
    actions: &'a Actions,
    metric_state: MetricState<'a>,
    triage_output: TriageOutput,
    plugins: Vec<Box<dyn Plugin>>,
}

impl<'a> StructuredActionContext<'a> {
    pub(crate) fn new(
        metrics: &'a Metrics,
        actions: &'a Actions,
        diagnostic_data: &'a [DiagnosticData],
        now: Option<i64>,
    ) -> StructuredActionContext<'a> {
        let fetcher = FileDataFetcher::new(diagnostic_data);
        let mut triage_output = TriageOutput::new(metrics.keys().cloned().collect::<Vec<String>>());
        fetcher.errors().iter().for_each(|e| {
            triage_output.add_error(format!("[DEBUG: BAD DATA] {e}"));
        });

        StructuredActionContext {
            actions,
            metric_state: MetricState::new(metrics, Fetcher::FileData(fetcher), now),
            triage_output,
            plugins: register_plugins(),
        }
    }
}

impl StructuredActionContext<'_> {
    // TODO(https://fxbug.dev/42178829): This must be refactored into `build`.
    // remove the unnecessary code blocks after refactor.
    /// Processes all actions, acting on the ones that trigger.
    pub fn process(&mut self) -> &TriageOutput {
        for (namespace, actions) in self.actions.iter() {
            for (name, action) in actions.iter() {
                match action {
                    Action::Alert(alert) => self.update_alerts(alert, namespace, name),
                    Action::Gauge(gauge) => self.update_gauges(gauge, namespace, name),
                    Action::Snapshot(snapshot) => self.update_snapshots(snapshot, namespace, name),
                };
            }
        }

        self.metric_state.evaluate_all_metrics();

        self.triage_output.metrics = self.metric_state.metrics.clone();

        if let Fetcher::FileData(file_data) = &self.metric_state.fetcher {
            for plugin in &self.plugins {
                let actions = plugin.run_structured(file_data);
                self.triage_output.plugin_results.insert(plugin.name().to_string(), actions);
            }
        }

        &self.triage_output
    }

    /// Update alerts after ensuring their trigger is evaluated.
    fn update_alerts(&mut self, action: &Alert, namespace: &str, name: &str) {
        self.metric_state.eval_action_metric(namespace, &action.trigger);
        self.triage_output.add_action(
            namespace.to_owned(),
            name.to_owned(),
            Action::Alert(action.clone()),
        );
    }

    /// Populate snapshots. Log a warning if the condition evaluates to a reportable Problem.
    fn update_snapshots(&mut self, action: &Snapshot, namespace: &str, name: &str) {
        let snapshot_trigger = self.metric_state.eval_action_metric(namespace, &action.trigger);
        self.metric_state.eval_action_metric(namespace, &action.repeat);
        self.triage_output.add_action(
            namespace.to_string(),
            name.to_string(),
            Action::Snapshot(action.clone()),
        );
        match snapshot_trigger {
            MetricValue::Bool(true) => {}
            MetricValue::Bool(false) => {}
            MetricValue::Problem(Problem::Ignore(_)) => {}
            MetricValue::Problem(_reason) => {
                inspect_logger::log_warn(
                    "Snapshot trigger not boolean",
                    namespace,
                    name,
                    &format!("{_reason:?}"),
                );
            }
            other => {
                inspect_logger::log_error(
                    "Bad config: Unexpected value type (need boolean)",
                    namespace,
                    name,
                    &format!("{other}"),
                );
            }
        };
    }

    /// Evaluate a [Gauge] and collect the evaluated action in the [TriageOutput].
    fn update_gauges(&mut self, action: &Gauge, namespace: &String, name: &String) {
        // The metric value is cached and added to output
        self.metric_state.eval_action_metric(namespace, &action.value);
        self.triage_output.add_action(
            namespace.to_string(),
            name.to_string(),
            Action::Gauge(action.clone()),
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::act::{ActionsSchema, Severity};
    use crate::make_metrics;
    use crate::metrics::{ExpressionContext, Metric};
    use std::cell::RefCell;

    macro_rules! cast {
        ($target: expr, $pat: path) => {{
            if let $pat(a) = $target {
                a
            } else {
                panic!("mismatch variant when cast to {}", stringify!($pat));
            }
        }};
    }

    macro_rules! trigger_eq {
        ($results: expr, $file: expr, $name: expr, $eval: expr) => {{
            assert_eq!(
                cast!($results.actions.get($file).unwrap().get($name).unwrap(), Action::Alert)
                    .trigger
                    .cached_value
                    .clone()
                    .into_inner()
                    .unwrap(),
                MetricValue::Bool($eval)
            );
        }};
    }

    #[fuchsia::test]
    fn actions_collected_correctly() {
        let metrics = make_metrics!({
            "file":{
                eval: {
                    "true": "0 == 0",
                    "false": "0 == 1",
                    "true_array": "[0 == 0]",
                    "false_array": "[0 == 1]"
                }
            }
        });
        let mut actions = Actions::new();
        let mut action_file = ActionsSchema::new();
        action_file.insert(
            "do_true".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("true", "file").unwrap(),
                print: "True was fired".to_string(),
                file_bug: Some("Some>Monorail>Component".to_string()),
                tag: None,
                severity: Severity::Warning,
            }),
        );
        action_file.insert(
            "do_false".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("false", "file").unwrap(),
                print: "False was fired".to_string(),
                file_bug: None,
                tag: None,
                severity: Severity::Warning,
            }),
        );
        action_file.insert(
            "do_true_array".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("true_array", "file")
                    .unwrap(),
                print: "True array was fired".to_string(),
                file_bug: None,
                tag: None,
                severity: Severity::Warning,
            }),
        );
        action_file.insert(
            "do_false_array".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("false_array", "file")
                    .unwrap(),
                print: "False array was fired".to_string(),
                file_bug: None,
                tag: None,
                severity: Severity::Warning,
            }),
        );

        action_file.insert(
            "do_operation".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("0 < 10", "file").unwrap(),
                print: "Inequality triggered".to_string(),
                file_bug: None,
                tag: None,
                severity: Severity::Warning,
            }),
        );
        actions.insert("file".to_string(), action_file);
        let no_data = Vec::new();
        let mut context = StructuredActionContext::new(&metrics, &actions, &no_data, None);
        let results = context.process();
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("do_true").unwrap(), Action::Alert)
                .file_bug
                .as_ref()
                .unwrap(),
            "Some>Monorail>Component"
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("do_true").unwrap(), Action::Alert)
                .print,
            "True was fired"
        );
        trigger_eq!(results, "file", "do_true", true);
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("do_false").unwrap(), Action::Alert)
                .print,
            "False was fired"
        );
        trigger_eq!(results, "file", "do_false", false);
        assert_eq!(
            cast!(
                results.actions.get("file").unwrap().get("do_true_array").unwrap(),
                Action::Alert
            )
            .print,
            "True array was fired"
        );
        trigger_eq!(results, "file", "do_true_array", true);
        assert_eq!(
            cast!(
                results.actions.get("file").unwrap().get("do_false_array").unwrap(),
                Action::Alert
            )
            .print,
            "False array was fired"
        );
        trigger_eq!(results, "file", "do_false_array", false);
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("do_operation").unwrap(), Action::Alert)
                .print,
            "Inequality triggered"
        );
        trigger_eq!(results, "file", "do_operation", true);
    }

    #[fuchsia::test]
    fn gauges_collected_correctly() {
        let metrics = make_metrics!({
            "file":{
                eval: {
                    "gauge_f1": "2 / 5",
                    "gauge_f2": "4 / 5",
                    "gauge_f3": "6 / 5",
                    "gauge_i4": "9 // 2",
                    "gauge_i5": "11 // 2",
                    "gauge_i6": "13 // 2",
                    "gauge_b7": "2 == 2",
                    "gauge_b8": "2 > 2",
                    "gauge_s9": "'foo'"
                }
            }
        });
        let mut actions = Actions::new();
        let mut action_file = ActionsSchema::new();
        macro_rules! insert_gauge {
            ($name:expr, $format:expr) => {
                action_file.insert(
                    $name.to_string(),
                    Action::Gauge(Gauge {
                        value: ValueSource::try_from_expression_with_namespace($name, "file")
                            .unwrap(),
                        format: $format,
                        tag: None,
                    }),
                );
            };
        }
        insert_gauge!("gauge_f1", None);
        insert_gauge!("gauge_f2", Some("percentage".to_string()));
        insert_gauge!("gauge_f3", Some("unknown".to_string()));
        insert_gauge!("gauge_i4", None);
        insert_gauge!("gauge_i5", Some("percentage".to_string()));
        insert_gauge!("gauge_i6", Some("unknown".to_string()));
        insert_gauge!("gauge_b7", None);
        insert_gauge!("gauge_b8", None);
        insert_gauge!("gauge_s9", None);
        actions.insert("file".to_string(), action_file);
        let no_data = Vec::new();
        let mut context = StructuredActionContext::new(&metrics, &actions, &no_data, None);

        let results = context.process();

        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f1").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Float(0.4)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f1").unwrap(), Action::Gauge)
                .format,
            None
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f2").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Float(0.8)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f2").unwrap(), Action::Gauge)
                .format
                .as_ref()
                .unwrap(),
            "percentage"
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f3").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Float(1.2)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_f3").unwrap(), Action::Gauge)
                .format
                .as_ref()
                .unwrap(),
            "unknown"
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i4").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Int(4)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i4").unwrap(), Action::Gauge)
                .format,
            None
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i5").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Int(5)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i5").unwrap(), Action::Gauge)
                .format
                .as_ref()
                .unwrap(),
            "percentage"
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i6").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Int(6)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_i6").unwrap(), Action::Gauge)
                .format
                .as_ref()
                .unwrap(),
            "unknown"
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_b7").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Bool(true)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_b7").unwrap(), Action::Gauge)
                .format,
            None
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_b8").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::Bool(false)
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_b8").unwrap(), Action::Gauge)
                .format,
            None
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_s9").unwrap(), Action::Gauge)
                .value
                .cached_value
                .borrow()
                .as_ref()
                .unwrap()
                .clone(),
            MetricValue::String("foo".to_string())
        );
        assert_eq!(
            cast!(results.actions.get("file").unwrap().get("gauge_s9").unwrap(), Action::Gauge)
                .format,
            None
        );
    }

    // TODO(https://fxbug.dev/42178827): Additional unit tests required.

    #[fuchsia::test]
    fn actions_cache_correctly() {
        let metrics = make_metrics!({
            "file":{
                eval: {
                    "true": "0 == 0",
                    "false": "0 == 1",
                    "five": "5"
                }
            }
        });
        let mut actions = Actions::new();
        let mut action_file = ActionsSchema::new();
        action_file.insert(
            "true_warning".to_string(),
            Action::Alert(Alert {
                trigger: ValueSource::try_from_expression_with_namespace("true", "file").unwrap(),
                print: "True was fired".to_string(),
                file_bug: None,
                tag: None,
                severity: Severity::Warning,
            }),
        );
        action_file.insert(
            "false_gauge".to_string(),
            Action::Gauge(Gauge {
                value: ValueSource::try_from_expression_with_namespace("false", "file").unwrap(),
                format: None,
                tag: None,
            }),
        );
        action_file.insert(
            "true_snapshot".to_string(),
            Action::Snapshot(Snapshot {
                trigger: ValueSource::try_from_expression_with_namespace("true", "file").unwrap(),
                repeat: ValueSource {
                    metric: Metric::Eval(
                        ExpressionContext::try_from_expression_with_namespace("five", "file")
                            .unwrap(),
                    ),
                    cached_value: RefCell::new(Some(MetricValue::Int(5))),
                },
                signature: "signature".to_string(),
            }),
        );
        action_file.insert(
            "test_snapshot".to_string(),
            Action::Snapshot(Snapshot {
                trigger: ValueSource::try_from_expression_with_namespace("true", "file").unwrap(),
                repeat: ValueSource::try_from_expression_with_namespace("five", "file").unwrap(),
                signature: "signature".to_string(),
            }),
        );
        actions.insert("file".to_string(), action_file);
        let no_data = Vec::new();
        let mut context = StructuredActionContext::new(&metrics, &actions, &no_data, None);
        context.process();

        // Ensure Alert caches correctly
        if let Action::Alert(alert) = actions.get("file").unwrap().get("true_warning").unwrap() {
            assert_eq!(*alert.trigger.cached_value.borrow(), Some(MetricValue::Bool(true)));
        } else {
            unreachable!("'true_warning' must be an Action::Alert")
        }

        // Ensure Gauge caches correctly
        if let Action::Gauge(gauge) = actions.get("file").unwrap().get("false_gauge").unwrap() {
            assert_eq!(*gauge.value.cached_value.borrow(), Some(MetricValue::Bool(false)));
        } else {
            unreachable!("'false_gauge' must be an Action::Gauge")
        }

        // Ensure Snapshot caches correctly
        if let Action::Snapshot(snapshot) =
            actions.get("file").unwrap().get("true_snapshot").unwrap()
        {
            assert_eq!(*snapshot.trigger.cached_value.borrow(), Some(MetricValue::Bool(true)));
            assert_eq!(*snapshot.repeat.cached_value.borrow(), Some(MetricValue::Int(5)));
        } else {
            unreachable!("'true_snapshot' must be an Action::Snapshot")
        }

        // Ensure value-calculation does not fail for a Snapshot with an empty cache.
        // The cached value for 'repeat' is expected to be pre-calculated during deserialization
        // however, an empty cached value should still be supported.
        if let Action::Snapshot(snapshot) =
            actions.get("file").unwrap().get("test_snapshot").unwrap()
        {
            assert_eq!(*snapshot.trigger.cached_value.borrow(), Some(MetricValue::Bool(true)));
            assert_eq!(*snapshot.repeat.cached_value.borrow(), Some(MetricValue::Int(5)));
        } else {
            unreachable!("'true_snapshot' must be an Action::Snapshot")
        }
    }
}
