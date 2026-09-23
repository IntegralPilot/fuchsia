// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::act::{Action, ActionResults, Gauge, Severity};
use crate::metrics::fetch::FileDataFetcher;
use crate::metrics::metric_value::MetricValue;

mod crashes;
mod helpers;
mod memory;
mod routing;
mod sandbox_errors;

pub trait Plugin {
    /// Returns a unique name for the plugin.
    ///
    /// This is the value selected and listed on the command line.
    fn name(&self) -> &'static str;

    /// Returns a human-readable name for the plugin.
    ///
    /// This will be displayed as the label for results from this plugin.
    fn display_name(&self) -> &'static str;

    /// Run the plugin on the given inputs to produce results.
    fn run(&self, inputs: &FileDataFetcher<'_>) -> ActionResults {
        let mut results = ActionResults::new();
        results.sort_gauges = false;
        let structured_results = self.run_structured(inputs);
        for action in structured_results {
            match action {
                Action::Alert(alert) => match alert.severity {
                    Severity::Info => results.infos.push(alert.print),
                    Severity::Warning => results.warnings.push(alert.print),
                    Severity::Error => results.errors.push(alert.print),
                },
                Action::Gauge(Gauge { tag: Some(tag), value, .. }) => {
                    if let Some(MetricValue::String(raw_value)) = value.cached_value.into_inner() {
                        results.gauges.push(format!("{tag}: {raw_value}"));
                    }
                }
                _ => (),
            }
        }
        results
    }

    /// Run the plugin on the given inputs to produce results keyed by name.
    fn run_structured(&self, inputs: &FileDataFetcher<'_>) -> Vec<Action>;
}

/// Retrieve the list of all plugins registered with this library.
pub fn register_plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(crashes::CrashesPlugin {}),
        Box::new(sandbox_errors::SandboxErrorsPlugin {}),
        Box::new(routing::RoutingErrorsPlugin {}),
        Box::new(memory::MemoryPlugin {}),
    ]
}
