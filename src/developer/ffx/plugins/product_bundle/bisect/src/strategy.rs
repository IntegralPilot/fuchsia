// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::search_space::SearchSpace;
use serde::{Deserialize, Serialize};

/// The current state of the bisection search strategy.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum StrategyState {
    /// Bisection Phase: Splitting the search space in half along
    /// all active dimensions.
    Phase1Bisection,
    /// Isolation Phase: Testing one suspect dimension at its 'high' value
    /// while keeping others 'low'.
    Phase2Isolation {
        /// The index of the dimension currently being isolated.
        current_dim_idx: usize,
    },

    /// Result: Successfully found the single culprit dimension and
    /// version change.
    Resolved {
        /// The index of the culprit dimension.
        dim_idx: usize,
        /// The index of the passing version.
        low_idx: usize,
        /// The index of the failing version.
        high_idx: usize,
    },

    /// Result: Could not isolate a single root cause
    /// (complex failure or bad initial assumption).
    Unresolved,
}

/// A search strategy that maintains state across bisection steps.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchStrategy {
    /// The current state of the strategy.
    pub state: StrategyState,
}

impl SearchStrategy {
    /// Create a new SearchStrategy starting in Phase 1.
    pub fn new() -> Self {
        Self { state: StrategyState::Phase1Bisection }
    }

    /// Determines the next combination of indices to test.
    pub fn next_combination(&self, space: &SearchSpace) -> Option<Vec<usize>> {
        match self.state {
            StrategyState::Phase1Bisection => {
                if space.is_phase1_complete() {
                    return None;
                }

                let next_dimensions = space.dimensions.iter().map(|d| {
                    if d.is_active() {
                        d.mid()
                    } else {
                        d.low // Pin inactive dimensions to their known passing version
                    }
                });
                Some(next_dimensions.collect())
            }
            StrategyState::Phase2Isolation { current_dim_idx } => {
                if current_dim_idx >= space.dimensions.len() {
                    return None;
                }
                let isolated_dimensions = space.dimensions.iter().enumerate().map(|(i, d)| {
                    // Move the *current* suspect dimension to the possibly failing version
                    if i == current_dim_idx && d.high > d.low {
                        d.high
                    } else {
                        d.low // Keep all others pinned to known good
                    }
                });
                Some(isolated_dimensions.collect())
            }
            _ => None, // Resolved or Unresolved yield no further combinations
        }
    }

    /// Updates bounds based on the test result and transitions the state machine.
    pub fn apply_result(&mut self, space: &mut SearchSpace, pass: bool) {
        match self.state {
            StrategyState::Phase1Bisection => {
                // Adjust bounds for all ACTIVE dimensions
                for dim in &mut space.dimensions {
                    if dim.is_active() {
                        let mid = dim.mid();
                        if pass {
                            dim.low = mid;
                        } else {
                            dim.high = mid;
                        }
                    }
                }

                // Check if we need to transition to Phase 2
                if space.is_phase1_complete() {
                    self.state = StrategyState::Phase2Isolation { current_dim_idx: 0 };
                    self.advance_isolation_if_needed(space);
                }
            }
            StrategyState::Phase2Isolation { current_dim_idx } => {
                if !pass {
                    // We moved ONE dimension to 'high', and it failed!
                    // We found the root cause.
                    let dim = &space.dimensions[current_dim_idx];
                    self.state = StrategyState::Resolved {
                        dim_idx: current_dim_idx,
                        low_idx: dim.low,
                        high_idx: dim.high,
                    };
                } else {
                    // Moving this dimension to 'high' did not cause a failure.
                    // It's not the sole culprit. Move on to test the next dim.
                    self.state =
                        StrategyState::Phase2Isolation { current_dim_idx: current_dim_idx + 1 };
                    self.advance_isolation_if_needed(space);
                }
            }
            _ => {}
        }
    }

    /// Helper to skip dimensions that don't have a 'high' value to test.
    pub fn advance_isolation_if_needed(&mut self, space: &SearchSpace) {
        if let StrategyState::Phase2Isolation { mut current_dim_idx } = self.state {
            while current_dim_idx < space.dimensions.len()
                && space.dimensions[current_dim_idx].high == space.dimensions[current_dim_idx].low
            {
                current_dim_idx += 1;
            }

            if current_dim_idx >= space.dimensions.len() {
                self.state = StrategyState::Unresolved;
            } else {
                self.state = StrategyState::Phase2Isolation { current_dim_idx };
            }
        }
    }
}

// ---------------------------------------------------------
// Thorough Integration Tests
// ---------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::Dimension;
    use assembly_artifact_cache::{ArtifactType, MOSIdentifier, Slot};

    // A mock Controller to run tests synchronously.
    struct Controller<F> {
        pub space: SearchSpace,
        pub strategy: SearchStrategy,
        pub test_fn: F,
    }

    impl<F> Controller<F>
    where
        F: Fn(&[MOSIdentifier]) -> bool,
    {
        pub fn new(space: SearchSpace, test_fn: F) -> Self {
            let mut strategy = SearchStrategy::new();
            if space.is_phase1_complete() {
                strategy.state = StrategyState::Phase2Isolation { current_dim_idx: 0 };
                strategy.advance_isolation_if_needed(&space);
            }
            Self { space, strategy, test_fn }
        }

        pub fn run(&mut self) {
            while let Some(indices) = self.strategy.next_combination(&self.space) {
                let combination: Vec<MOSIdentifier> = indices
                    .iter()
                    .enumerate()
                    .map(|(i, &idx)| self.space.dimensions[i].get_mos_identifier(idx, Slot::A))
                    .collect();

                let pass = (self.test_fn)(&combination);
                self.strategy.apply_result(&mut self.space, pass);

                if matches!(
                    self.strategy.state,
                    StrategyState::Resolved { .. } | StrategyState::Unresolved
                ) {
                    break;
                }
            }
        }
    }

    fn get_test_dimensions() -> Vec<Dimension> {
        vec![
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
        ]
    }

    #[test]
    fn test_bisection_finds_board_culprit() {
        let space = SearchSpace::new(get_test_dimensions());

        // Culprit is Board changing from "b3" (idx 2) to "b4" (idx 3)
        let test_fn = |comb: &[MOSIdentifier]| -> bool {
            let board = &comb[2];
            board.version != "4" && board.version != "5"
        };

        let mut controller = Controller::new(space, test_fn);
        controller.run();

        if let StrategyState::Resolved { dim_idx, low_idx, high_idx } = controller.strategy.state {
            assert_eq!(dim_idx, 2, "Expected Board (idx 2) to be the culprit");
            assert_eq!(low_idx, 2, "Expected known good to be b3 (idx 2)");
            assert_eq!(high_idx, 3, "Expected known bad to be b4 (idx 3)");
        } else {
            panic!(
                "Strategy did not resolve the culprit correctly. Ended at: {:?}",
                controller.strategy.state
            );
        }
    }

    #[test]
    fn test_bisection_finds_platform_culprit() {
        let space = SearchSpace::new(get_test_dimensions());

        // Culprit is Platform changing from "1" (idx 0) to "2" (idx 1)
        let test_fn = |comb: &[MOSIdentifier]| -> bool {
            let platform = &comb[0];
            platform.version == "1" // Only 1 passes
        };

        let mut controller = Controller::new(space, test_fn);
        controller.run();

        if let StrategyState::Resolved { dim_idx, low_idx, high_idx } = controller.strategy.state {
            assert_eq!(dim_idx, 0, "Expected Platform (idx 0) to be the culprit");
            assert_eq!(low_idx, 0, "Expected known good to be 1 (idx 0)");
            assert_eq!(high_idx, 1, "Expected known bad to be 2 (idx 1)");
        } else {
            panic!("Strategy did not resolve the culprit correctly.");
        }
    }

    #[test]
    fn test_exhaustive_bisection_suite() {
        let lengths = vec![1, 2, 3, 4, 5, 6];

        for &platform_len in &lengths {
            for &product_len in &lengths {
                for &board_len in &lengths {
                    if platform_len == 1 && product_len == 1 && board_len == 1 {
                        continue;
                    }

                    if platform_len > 1 {
                        for culprit_index in 1..platform_len {
                            run_bisection_sim(
                                platform_len,
                                product_len,
                                board_len,
                                0,
                                culprit_index,
                            );
                        }
                    }
                    if product_len > 1 {
                        for culprit_index in 1..product_len {
                            run_bisection_sim(
                                platform_len,
                                product_len,
                                board_len,
                                1,
                                culprit_index,
                            );
                        }
                    }
                    if board_len > 1 {
                        for culprit_index in 1..board_len {
                            run_bisection_sim(
                                platform_len,
                                product_len,
                                board_len,
                                2,
                                culprit_index,
                            );
                        }
                    }
                }
            }
        }
    }

    fn run_bisection_sim(
        platform_len: usize,
        product_len: usize,
        board_len: usize,
        culprit_dim_idx: usize,
        culprit_val_idx: usize,
    ) {
        let make_versions =
            |len: usize| -> Vec<String> { (0..len).map(|i| i.to_string()).collect() };

        let dimensions = vec![
            Dimension::new(
                "Platform",
                ArtifactType::Platform,
                "fuchsia",
                make_versions(platform_len),
            ),
            Dimension::new("Product", ArtifactType::Product, "fuchsia", make_versions(product_len)),
            Dimension::new("Board", ArtifactType::Board, "fuchsia", make_versions(board_len)),
        ];

        let space = SearchSpace::new(dimensions);

        let test_fn = |comb: &[MOSIdentifier]| -> bool {
            let val_idx = comb[culprit_dim_idx].version.parse::<usize>().unwrap();
            val_idx < culprit_val_idx // Passing version must be earlier than culprit index
        };

        let mut controller = Controller::new(space, test_fn);
        controller.run();

        if let StrategyState::Resolved { dim_idx, low_idx, high_idx } = controller.strategy.state {
            assert_eq!(
                dim_idx, culprit_dim_idx,
                "Failed at lengths: P={}, Pr={}, B={}, culprit_dim={}, culprit_val={}. Wrong dimension resolved: {}",
                platform_len, product_len, board_len, culprit_dim_idx, culprit_val_idx, dim_idx
            );
            assert_eq!(
                low_idx,
                culprit_val_idx - 1,
                "Failed at lengths: P={}, Pr={}, B={}, culprit_dim={}, culprit_val={}. Wrong low_idx: {}",
                platform_len,
                product_len,
                board_len,
                culprit_dim_idx,
                culprit_val_idx,
                low_idx
            );
            assert_eq!(
                high_idx, culprit_val_idx,
                "Failed at lengths: P={}, Pr={}, B={}, culprit_dim={}, culprit_val={}. Wrong high_idx: {}",
                platform_len, product_len, board_len, culprit_dim_idx, culprit_val_idx, high_idx
            );
        } else {
            panic!(
                "Failed at lengths: P={}, Pr={}, B={}, culprit_dim={}, culprit_val={}. Strategy did not resolve, ended at: {:?}",
                platform_len,
                product_len,
                board_len,
                culprit_dim_idx,
                culprit_val_idx,
                controller.strategy.state
            );
        }
    }
}
