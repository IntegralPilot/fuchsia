// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::dimension::Dimension;
use serde::{Deserialize, Serialize};

/// Represents the multidimensional search space for bisection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSpace {
    /// The dimensions comprising the search space.
    pub dimensions: Vec<Dimension>,
}

impl SearchSpace {
    /// Create a new SearchSpace from a set of dimensions.
    pub fn new(dimensions: Vec<Dimension>) -> Self {
        Self { dimensions }
    }

    /// Phase 1 is complete when all dimensions have been narrowed down to 1 or 2 entries.
    pub fn is_phase1_complete(&self) -> bool {
        self.dimensions.iter().all(|d| !d.is_active())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_artifact_cache::ArtifactType;

    #[test]
    fn test_new_search_space() {
        let dimensions = vec![
            Dimension::new("Platform", ArtifactType::Platform, "fuchsia", vec!["1".into()]),
            Dimension::new("Product", ArtifactType::Product, "fuchsia", vec!["A".into()]),
        ];
        let space = SearchSpace::new(dimensions);
        assert_eq!(space.dimensions.len(), 2);
    }

    #[test]
    fn test_is_phase1_complete() {
        let d1 = Dimension::new(
            "Platform",
            ArtifactType::Platform,
            "fuchsia",
            vec!["1".into(), "2".into(), "3".into()],
        );
        let d2 = Dimension::new(
            "Product",
            ArtifactType::Product,
            "fuchsia",
            vec!["A".into(), "B".into(), "C".into()],
        );

        let mut space = SearchSpace::new(vec![d1.clone(), d2.clone()]);

        // Initial state: both have length 3 (high=2, low=0), so gap > 1 => active
        assert!(!space.is_phase1_complete());

        // Shrink one dimension to inactive
        space.dimensions[0].high = 1;
        assert!(!space.is_phase1_complete()); // d2 is still active

        // Shrink the other dimension to inactive
        space.dimensions[1].high = 1;
        assert!(space.is_phase1_complete()); // Both are inactive
    }
}
