// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::search_space::SearchSpace;
use crate::strategy::{SearchStrategy, StrategyState};
use anyhow::{Context, Result};
use assembly_artifact_cache::mos::MOSClient;
use assembly_artifact_cache::{MOSIdentifier, Slot};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, IsTerminal};

/// A struct that holds the serialized state of the bisection process.
#[derive(Serialize, Deserialize)]
struct Plan {
    space: SearchSpace,
    strategy: SearchStrategy,
    step_count: usize,
}

/// The Controller orchestrates the bisection state machine, saving and restoring
/// state to disk so that long-running, interruptible tasks (like flashing devices)
/// can resume where they left off.
pub struct Controller<TestFn, PrintFn> {
    /// The search space containing all dimensions and versions.
    pub space: SearchSpace,
    /// The bisection strategy state machine.
    pub strategy: SearchStrategy,
    /// The path to the JSON file where state is persisted.
    pub plan: Utf8PathBuf,
    /// The asynchronous function that performs the actual testing.
    test_fn: TestFn,
    /// The function that reports the current status of the bisection.
    print_fn: PrintFn,
    /// The slot to use when constructing MOSIdentifiers.
    slot: Slot,
    /// The number of bisection steps taken so far.
    pub step_count: usize,
    /// The MOS client used for querying artifact release info.
    mos_client: Option<MOSClient>,
}

const COLOR_GRAY: &str = "\x1b[90m";
const COLOR_RESET: &str = "\x1b[0m";
const SYMBOL_CULPRIT: &str = "\x1b[31;1m*\x1b[0m";
const SYMBOL_CURRENT: &str = "\x1b[32;5mO\x1b[0m";
const SYMBOL_GOOD: &str = "\x1b[32m✓\x1b[0m";
const SYMBOL_BAD: &str = "\x1b[31m✗\x1b[0m";
const CLEAR_SCREEN: &str = "\x1B[2J\x1B[1;1H";

impl<TestFn, TestFut, PrintFn> Controller<TestFn, PrintFn>
where
    TestFn: FnMut(Vec<MOSIdentifier>) -> TestFut,
    TestFut: std::future::Future<Output = Result<bool>>,
    PrintFn: FnMut(&str),
{
    /// Creates a new Controller. If a saved plan exists at `plan`, it will be
    /// loaded. Otherwise, a new plan will be created from the provided `space`.
    pub fn new(
        mut space: SearchSpace,
        plan: Utf8PathBuf,
        slot: Slot,
        test_fn: TestFn,
        print_fn: PrintFn,
        mos_client: Option<MOSClient>,
    ) -> Result<Self> {
        let mut strategy = SearchStrategy::new();
        let mut step_count = 0;

        if plan.is_file() {
            // Load existing state
            let file = File::open(&plan)
                .with_context(|| format!("Failed to open bisection plan at {}", plan))?;
            let reader = BufReader::new(file);
            let saved_plan: Plan = serde_json::from_reader(reader)
                .with_context(|| format!("Failed to parse bisection plan at {}", plan))?;

            space = saved_plan.space;
            strategy = saved_plan.strategy;
            step_count = saved_plan.step_count;
        } else {
            // Initialize new state
            if space.is_phase1_complete() {
                strategy.state = StrategyState::Phase2Isolation { current_dim_idx: 0 };
                strategy.advance_isolation_if_needed(&space);
            }
        }

        let controller =
            Self { space, strategy, plan, test_fn, print_fn, slot, step_count, mos_client };

        // Ensure the initial state (or freshly loaded state) is saved.
        controller.save_plan().context("Failed to save initial bisection plan")?;

        Ok(controller)
    }

    /// Runs the bisection loop, yielding to the async `test_fn` for each combination
    /// and persisting state after each step. Returns the final StrategyState.
    pub async fn run(&mut self) -> Result<StrategyState> {
        (self.print_fn)("Starting Multi-dimensional bisection...\n");

        while let Some(indices) = self.strategy.next_combination(&self.space) {
            self.step_count += 1;

            self.print_status();

            let combination: Vec<MOSIdentifier> = indices
                .iter()
                .enumerate()
                .map(|(i, &idx)| self.space.dimensions[i].get_mos_identifier(idx, self.slot))
                .collect();

            // Yield to the caller-provided async testing function
            let pass = (self.test_fn)(combination)
                .await
                .context("The provided testing function encountered an error during bisection")?;

            (self.print_fn)(&format!("Result: {}\n", if pass { "PASS" } else { "FAIL" }));

            // Update the state machine based on the result
            self.strategy.apply_result(&mut self.space, pass);
            self.save_plan().context("Failed to save bisection plan after step")?;

            // Check if we reached a terminal state
            if matches!(
                self.strategy.state,
                StrategyState::Resolved { .. } | StrategyState::Unresolved
            ) {
                self.print_status();
                break;
            }
        }

        let mut summary =
            String::from("\n=============================================\nBisection Complete!\n");
        match self.strategy.state {
            StrategyState::Resolved { dim_idx, low_idx, high_idx } => {
                let dim = &self.space.dimensions[dim_idx];
                let bad_artifact = dim.get_mos_identifier(high_idx, self.slot);

                summary.push_str("Root cause isolated:\n\n");
                summary.push_str(&format!("  Dimension:     {}\n", dim.name));
                summary.push_str(&format!("  Artifact Type: {}\n", dim.artifact_type));
                summary.push_str(&format!("  Repository:    {}\n", dim.repository));
                summary.push_str(&format!("  Good Version:  {}\n", dim.versions[low_idx]));
                summary.push_str(&format!("  Bad Version:   {}\n", dim.versions[high_idx]));

                if let Some(client) = &self.mos_client {
                    if let Ok(info) = client.get_artifact_release_info(&bad_artifact).await {
                        if let Some(cipd) = info.cipd {
                            summary.push_str(&format!(
                                "  CIPD URL:      https://chrome-infra-packages.appspot.com/p/{}/+/{}\n",
                                cipd.path, cipd.tag
                            ));
                        }
                    }
                }
            }
            StrategyState::Unresolved => {
                summary.push_str("Could not isolate a single root cause.\n");
                summary.push_str(
                    "The failure may involve multiple dimensions or bad initial boundaries.\n",
                );
            }
            _ => {}
        }
        summary.push_str("=============================================\n");
        (self.print_fn)(&summary);

        Ok(self.strategy.state.clone())
    }

    /// Formats and prints the current status of the bisection state machine.
    pub fn print_status(&mut self) {
        let mut message = String::new();

        if std::io::stdout().is_terminal() {
            message.push_str(CLEAR_SCREEN);
        }

        // 1. Determine the overall status and current phase of the strategy
        let phase_str = match &self.strategy.state {
            StrategyState::Phase1Bisection => "Phase 1 (Multi-Axis Bisection)",
            StrategyState::Phase2Isolation { .. } => "Phase 2 (Isolating Root Cause)",
            StrategyState::Resolved { .. } => "Finished (Resolved)",
            StrategyState::Unresolved => "Finished (Unresolved)",
        };
        message.push_str(&format!("Step {}: {}\n", self.step_count, phase_str));
        message.push_str("Bisection Search Space:\n");

        // 2. Compute display dimensions so that text columns align nicely
        let names: Vec<String> = self
            .space
            .dimensions
            .iter()
            .map(|d| format!("{}/{}", d.artifact_type, d.name))
            .collect();
        let max_name_len = names.iter().map(|n| n.len()).max().unwrap_or(0);
        let max_artifacts_len =
            self.space.dimensions.iter().map(|d| d.versions.len()).max().unwrap_or(0);

        // 3. Pre-fetch the exact combination we are testing on this step
        // so we can highlight it
        let current_combination = self.strategy.next_combination(&self.space);

        // 4. Pre-fetch culprit information if we have finished successfully
        let culprit_info =
            if let StrategyState::Resolved { dim_idx, high_idx, .. } = self.strategy.state {
                Some((dim_idx, high_idx))
            } else {
                None
            };

        // 5. Render each dimension line by line
        for (dimension_idx, dim) in self.space.dimensions.iter().enumerate() {
            if dim.versions.is_empty() {
                continue;
            }
            let name = &names[dimension_idx];
            let padded_name = format!("{:<width$}", name, width = max_name_len);

            // A dimension is considered "pinned" if we have narrowed it down
            // to 1 remaining version naturally, or if we are actively
            // isolating a DIFFERENT dimension in Phase 2.
            let is_pinned = match self.strategy.state {
                StrategyState::Phase2Isolation { current_dim_idx } => {
                    dimension_idx != current_dim_idx
                }
                _ => dim.high == dim.low,
            };

            // A dimension is considered "cleared" if it has been isolated
            // in Phase 2 and proved not to be the single root cause.
            let is_cleared = match self.strategy.state {
                StrategyState::Phase2Isolation { current_dim_idx } => {
                    dimension_idx < current_dim_idx
                }
                StrategyState::Unresolved => true,
                _ => false,
            };

            // Dim the entire line with gray text if this dimension is pinned
            let line_color = if is_pinned { COLOR_GRAY } else { "" };
            let line_reset = if is_pinned { COLOR_RESET } else { "" };

            let mut visual = String::from("[");

            // Build the visual representation of the array (e.g. `[✓✓O?✗]`)
            for version_idx in 0..dim.versions.len() {
                let is_culprit = culprit_info
                    .map_or(false, |(c_dim, c_idx)| c_dim == dimension_idx && c_idx == version_idx);
                let is_current = current_combination
                    .as_ref()
                    .map_or(false, |comb| comb[dimension_idx] == version_idx);

                if is_culprit {
                    // Mark the final root cause culprit version
                    visual.push_str(SYMBOL_CULPRIT);
                    visual.push_str(line_color);
                } else if is_current && culprit_info.is_none() {
                    // Highlight the version currently being tested on this
                    // step in bright green, blinking
                    visual.push_str(&format!("{}{}", SYMBOL_CURRENT, line_color));
                } else if version_idx <= dim.low {
                    // Version is older than or equal to the last known good, so it is assumed good
                    visual.push_str(SYMBOL_GOOD);
                    visual.push_str(line_color);
                } else if version_idx >= dim.high {
                    if is_cleared && version_idx == dim.high {
                        // The 'high' version was isolated and proved innocent
                        visual.push_str(SYMBOL_GOOD);
                    } else {
                        // Version is newer than or equal to the first known bad, so it is assumed bad
                        visual.push_str(SYMBOL_BAD);
                    }
                    visual.push_str(line_color);
                } else {
                    // Version is within the active search window, meaning we don't know yet
                    visual.push_str("?");
                }
            }
            visual.push(']');

            // Pad the end of the brackets so the status text lines up
            // for all dimensions
            let padding_len = max_artifacts_len.saturating_sub(dim.versions.len());
            visual.push_str(&" ".repeat(padding_len));

            // Status string appended to the right side of the visual string
            let status_text = if is_pinned {
                "(Pinned)".to_string()
            } else {
                format!("({} remaining)", dim.high.saturating_sub(dim.low) + 1)
            };

            message.push_str(&format!(
                "{line_color}  {}: {} {}{line_reset}\n",
                padded_name,
                visual,
                status_text,
                line_color = line_color,
                line_reset = line_reset
            ));
        }

        (self.print_fn)(&message);
    }

    /// Serializes the current search space and strategy state to the plan file.
    fn save_plan(&self) -> Result<()> {
        let saved_plan = Plan {
            space: self.space.clone(),
            strategy: self.strategy.clone(),
            step_count: self.step_count,
        };

        let json_string = serde_json::to_string_pretty(&saved_plan)
            .context("Failed to serialize bisection plan to JSON")?;

        if let Some(parent) = self.plan.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directories for {}", self.plan)
            })?;
        }

        fs::write(&self.plan, json_string)
            .with_context(|| format!("Failed to write bisection plan to {}", self.plan))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Dimension;
    use assembly_artifact_cache::ArtifactType;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[test]
    fn test_print_status_formatting() {
        let dir = tempdir().unwrap();
        let plan = Utf8PathBuf::from_path_buf(dir.path().join("plan.json")).unwrap();

        let dimensions = vec![
            Dimension::new(
                "Platform",
                ArtifactType::Platform,
                "fuchsia",
                vec!["1".into(), "2".into(), "3".into(), "4".into()],
            ),
            Dimension::new(
                "Product",
                ArtifactType::Product,
                "fuchsia",
                vec!["1".into(), "2".into(), "3".into()],
            ),
        ];

        let space = SearchSpace::new(dimensions);

        let output = Arc::new(Mutex::new(String::new()));
        let output_clone = output.clone();

        // Custom print function that captures the output
        let print_fn = move |s: &str| {
            let mut out = output_clone.lock().unwrap();
            out.clear(); // Clear previous output
            out.push_str(s);
        };

        let test_fn = |_| async move { Ok(true) };
        let mut controller = Controller::new(space, plan, Slot::A, test_fn, print_fn, None).unwrap();

        // 1. Test Initial Phase 1 State
        controller.print_status();
        let out = output.lock().unwrap().clone();
        assert!(out.contains("Step 0: Phase 1 (Multi-Axis Bisection)"));
        assert!(out.contains("Platform"));
        assert!(out.contains("Product"));
        // Current combination should be highlighted with 'O'
        assert!(out.contains(SYMBOL_CURRENT));
        assert!(out.contains("?")); // Unresolved versions

        // 2. Test Phase 2 Isolation State
        controller.strategy.state = StrategyState::Phase2Isolation { current_dim_idx: 1 };
        controller.print_status();
        let out = output.lock().unwrap().clone();
        assert!(out.contains("Phase 2 (Isolating Root Cause)"));
        // Dimension 0 (Platform) should be pinned (gray)
        assert!(out.contains("(Pinned)"));
        assert!(out.contains(COLOR_GRAY));

        // 3. Test Resolved State
        controller.strategy.state = StrategyState::Resolved { dim_idx: 1, low_idx: 1, high_idx: 2 };
        controller.print_status();
        let out = output.lock().unwrap().clone();
        assert!(out.contains("Finished (Resolved)"));
        // The culprit version should be marked with a red star
        assert!(out.contains(SYMBOL_CULPRIT));
    }

    #[test]
    fn test_controller_run_and_save_restore() {
        futures_lite::future::block_on(async move {
            let dir = tempdir().unwrap();
            let plan = Utf8PathBuf::from_path_buf(dir.path().join("plan.json")).unwrap();

            let dimensions = vec![
                Dimension::new(
                    "Platform",
                    ArtifactType::Platform,
                    "fuchsia",
                    vec!["1".into(), "2".into(), "3".into(), "4".into()],
                ),
                Dimension::new(
                    "Product",
                    ArtifactType::Product,
                    "fuchsia",
                    vec!["1".into(), "2".into(), "3".into()],
                ),
                Dimension::new(
                    "Board",
                    ArtifactType::Board,
                    "fuchsia",
                    vec!["1".into(), "2".into(), "3".into(), "4".into(), "5".into()],
                ),
            ];

            let space = SearchSpace::new(dimensions.clone());

            // First run: only do one step, then "crash" (drop the controller)
            {
                let test_fn = |comb: Vec<MOSIdentifier>| async move {
                    let board = &comb[2];
                    Ok(board.version != "4" && board.version != "5")
                };

                let mut controller =
                    Controller::new(space.clone(), plan.clone(), Slot::A, test_fn, |_| {}, None).unwrap();

                // Manually simulate exactly one step of the run loop
                let indices = controller.strategy.next_combination(&controller.space).unwrap();
                let combination: Vec<MOSIdentifier> = indices
                    .iter()
                    .enumerate()
                    .map(|(i, &idx)| {
                        controller.space.dimensions[i].get_mos_identifier(idx, Slot::A)
                    })
                    .collect();

                // The midpoint for Board(5) is idx 2 ("3"), which passes.
                let pass = combination[2].version != "4" && combination[2].version != "5";

                controller.strategy.apply_result(&mut controller.space, pass);
                controller.save_plan().unwrap();
            }

            // Second run: restore from disk and verify state was preserved
            {
                let test_fn = |comb: Vec<MOSIdentifier>| async move {
                    // Board changing from b3 -> b4 is the culprit
                    let board = &comb[2];
                    Ok(board.version != "4" && board.version != "5")
                };

                let mut controller =
                    Controller::new(space, plan.clone(), Slot::A, test_fn, |_| {}, None).unwrap();

                // The platform should have been narrowed to the UPPER half
                // (low=1) because the first test PASSED
                assert_eq!(controller.space.dimensions[0].low, 1);
                assert_eq!(controller.space.dimensions[0].high, 3);

                let result = controller.run().await.unwrap();

                // Ensure it eventually finds the correct culprit (Board 3 -> 4)
                if let StrategyState::Resolved { dim_idx, low_idx, high_idx } = result {
                    assert_eq!(dim_idx, 2);
                    assert_eq!(low_idx, 2);
                    assert_eq!(high_idx, 3);
                } else {
                    panic!("Did not resolve correctly, ended at {:?}", result);
                }
            }
        });
    }
}
