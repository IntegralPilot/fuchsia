// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::act::ActionResults;
use std::fmt;

pub struct ActionResultFormatter<'a> {
    action_results: &'a ActionResults,
}

impl fmt::Display for ActionResultFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_text(f)
    }
}

impl ActionResultFormatter<'_> {
    pub fn new(action_results: &ActionResults) -> ActionResultFormatter<'_> {
        ActionResultFormatter { action_results }
    }

    fn write_text(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Self::write_section(f, "Errors", &self.action_results.errors, true)?;
        Self::write_section(
            f,
            "Featured Values",
            &self.action_results.gauges,
            self.action_results.sort_gauges,
        )?;
        Self::write_section(f, "Warnings", &self.action_results.warnings, true)?;
        self.write_plugins(f)?;
        if self.action_results.verbose {
            Self::write_section(
                f,
                "Featured Values (Not Computable)",
                &self.action_results.broken_gauges,
                true,
            )?;
            Self::write_section(f, "Info", &self.action_results.infos, true)?;
        }
        Ok(())
    }

    fn write_section(
        f: &mut fmt::Formatter<'_>,
        label: &str,
        lines: &Vec<String>,
        sort: bool,
    ) -> fmt::Result {
        if lines.is_empty() {
            return Ok(());
        }

        Self::write_underlined(f, label)?;
        let mut sorted_lines;
        let output_lines = if sort {
            sorted_lines = lines.clone();
            sorted_lines.sort();
            &sorted_lines
        } else {
            lines
        };
        for line in output_lines {
            writeln!(f, "{line}")?;
        }
        writeln!(f)
    }

    fn write_underlined(f: &mut fmt::Formatter<'_>, content: &str) -> fmt::Result {
        writeln!(f, "{content}")?;
        writeln!(f, "{}", "-".repeat(content.len()))
    }

    fn write_plugins(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, result) in &self.action_results.sub_results {
            if !self.action_results.verbose
                && result.gauges.is_empty()
                && result.errors.is_empty()
                && result.warnings.is_empty()
            {
                writeln!(f, "Plugin '{name}' - nothing to show\n")?;
                continue;
            }
            writeln!(f, "Plugin '{name}'")?;
            // TODO(https://fxbug.dev/42076794): use self.verbose flag correctly in plugins.
            ActionResultFormatter::new(result).write_text(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    fn action_result_formatter_to_warnings_when_no_actions_triggered() {
        let action_results = ActionResults::new();
        let formatter = ActionResultFormatter::new(&action_results);

        assert_eq!(String::from(""), formatter.to_string());
    }

    #[fuchsia::test]
    fn action_result_formatter_to_text_when_actions_triggered() {
        let warnings = String::from(
            "\
Warnings
--------
w1
w2

",
        );

        let mut action_results = ActionResults::new();
        action_results.warnings.push(String::from("w1"));
        action_results.warnings.push(String::from("w2"));

        let formatter = ActionResultFormatter::new(&action_results);

        assert_eq!(warnings, formatter.to_string());
    }

    #[fuchsia::test]
    fn action_result_formatter_to_text_with_gauges() {
        let warnings = String::from(
            "\
Featured Values
---------------
g1

Warnings
--------
w1
w2

",
        );

        let mut action_results = ActionResults::new();
        action_results.warnings.push(String::from("w1"));
        action_results.warnings.push(String::from("w2"));
        action_results.gauges.push(String::from("g1"));

        let formatter = ActionResultFormatter::new(&action_results);

        assert_eq!(warnings, formatter.to_string());
    }

    #[fuchsia::test]
    fn action_result_formatter_sorts_output() {
        let warnings = String::from(
            "\
Featured Values
---------------
g1
g2

Warnings
--------
w1
w2

Plugin 'Crashes' - nothing to show

Plugin 'Warning'
Warnings
--------
w1
w2

Plugin 'Gauges'
Featured Values
---------------
g2
g1

",
        );

        {
            let mut action_results = ActionResults::new();
            action_results.warnings.push(String::from("w1"));
            action_results.warnings.push(String::from("w2"));
            action_results.gauges.push(String::from("g1"));
            action_results.gauges.push(String::from("g2"));
            let mut warnings_plugin = ActionResults::new();
            warnings_plugin.warnings.push(String::from("w1"));
            warnings_plugin.warnings.push(String::from("w2"));
            let mut gauges_plugin = ActionResults::new();
            gauges_plugin.gauges.push(String::from("g2"));
            gauges_plugin.gauges.push(String::from("g1"));
            gauges_plugin.sort_gauges = false;
            action_results
                .sub_results
                .push(("Crashes".to_string(), Box::new(ActionResults::new())));
            action_results.sub_results.push(("Warning".to_string(), Box::new(warnings_plugin)));
            action_results.sub_results.push(("Gauges".to_string(), Box::new(gauges_plugin)));

            let formatter = ActionResultFormatter::new(&action_results);

            assert_eq!(warnings, formatter.to_string());
        }

        // Same as before, but reversed to test sorting.
        {
            let mut action_results = ActionResults::new();
            action_results.warnings.push(String::from("w2"));
            action_results.warnings.push(String::from("w1"));
            action_results.gauges.push(String::from("g2"));
            action_results.gauges.push(String::from("g1"));
            let mut warnings_plugin = ActionResults::new();
            warnings_plugin.warnings.push(String::from("w2"));
            warnings_plugin.warnings.push(String::from("w1"));
            let mut gauges_plugin = ActionResults::new();
            gauges_plugin.gauges.push(String::from("g2"));
            gauges_plugin.gauges.push(String::from("g1"));
            gauges_plugin.sort_gauges = false;
            action_results
                .sub_results
                .push(("Crashes".to_string(), Box::new(ActionResults::new())));
            action_results.sub_results.push(("Warning".to_string(), Box::new(warnings_plugin)));
            action_results.sub_results.push(("Gauges".to_string(), Box::new(gauges_plugin)));

            let formatter = ActionResultFormatter::new(&action_results);

            assert_eq!(warnings, formatter.to_string());
        }
    }

    #[fuchsia::test]
    fn action_result_verbose_works() {
        let readable_warnings = String::from(
            "\
Featured Values
---------------
g1: 42

Warnings
--------
w1

",
        );
        let verbose_warnings = format!(
            "{}{}",
            readable_warnings,
            "\
Featured Values (Not Computable)
--------------------------------
g2: N/A

Info
----
i1

"
        );
        let mut action_results = ActionResults::new();
        action_results.warnings.push(String::from("w1"));
        action_results.gauges.push(String::from("g1: 42"));
        action_results.infos.push(String::from("i1"));
        action_results.broken_gauges.push(String::from("g2: N/A"));
        let mut verbose_action_results = action_results.clone();
        verbose_action_results.verbose = true;
        assert_eq!(readable_warnings, ActionResultFormatter::new(&action_results).to_string());
        assert_eq!(
            verbose_warnings,
            ActionResultFormatter::new(&verbose_action_results).to_string()
        );
    }
}
