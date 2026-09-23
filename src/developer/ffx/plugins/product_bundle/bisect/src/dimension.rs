// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assembly_artifact_cache::{ArtifactType, MOSIdentifier, Slot};
use serde::{Deserialize, Serialize};

/// A single dimension (e.g., Platform, Product, Board) in the search space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    /// The name of this dimension.
    pub name: String,
    /// The artifact type of this dimension.
    pub artifact_type: ArtifactType,
    /// The repository of this dimension.
    pub repository: String,
    /// The list of known versions for this dimension.
    pub versions: Vec<String>,
    /// The index of the oldest known-good version.
    pub low: usize,
    /// The index of the newest known-bad version.
    pub high: usize,
}

impl Dimension {
    /// Create a new Dimension from a list of versions.
    pub fn new(name: &str, artifact_type: ArtifactType, repo: &str, versions: Vec<String>) -> Self {
        let high = if versions.is_empty() { 0 } else { versions.len() - 1 };
        Self {
            name: name.to_string(),
            artifact_type,
            repository: repo.to_string(),
            versions,
            low: 0,
            high,
        }
    }

    /// Reconstructs a MOSIdentifier for the version at the given index.
    pub fn get_mos_identifier(&self, index: usize, slot: Slot) -> MOSIdentifier {
        MOSIdentifier {
            name: self.name.clone(),
            artifact_type: self.artifact_type.clone(),
            version: self.versions[index].clone(),
            repository: self.repository.clone(),
            cipd: None,
            slot,
        }
    }

    /// A dimension is "active" in Phase 1 if the gap between high and low
    /// is strictly greater than 1.
    /// If high == low + 1, it means we have 2 versions left: one passing,
    /// one failing.
    pub fn is_active(&self) -> bool {
        self.high > self.low + 1
    }

    /// Calculate the safe midpoint for bisection.
    pub fn mid(&self) -> usize {
        self.low + (self.high - self.low) / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly_artifact_cache::{ArtifactType, Slot};

    #[test]
    fn test_new_empty_versions() {
        let dim = Dimension::new("Test", ArtifactType::Platform, "repo", vec![]);
        assert_eq!(dim.name, "Test");
        assert_eq!(dim.artifact_type, ArtifactType::Platform);
        assert_eq!(dim.repository, "repo");
        assert_eq!(dim.low, 0);
        assert_eq!(dim.high, 0);
        assert!(dim.versions.is_empty());
    }

    #[test]
    fn test_new_with_versions() {
        let versions = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        let dim = Dimension::new("Test", ArtifactType::Product, "repo", versions);
        assert_eq!(dim.low, 0);
        assert_eq!(dim.high, 2);
        assert_eq!(dim.versions.len(), 3);
    }

    #[test]
    fn test_get_mos_identifier() {
        let versions = vec!["1.0".to_string(), "2.0".to_string()];
        let dim = Dimension::new("Core", ArtifactType::Board, "fuchsia", versions);

        let mos_id = dim.get_mos_identifier(1, Slot::A);
        assert_eq!(mos_id.name, "Core");
        assert_eq!(mos_id.artifact_type, ArtifactType::Board);
        assert_eq!(mos_id.version, "2.0");
        assert_eq!(mos_id.repository, "fuchsia");
        assert_eq!(mos_id.slot, Slot::A);
        assert!(mos_id.cipd.is_none());
    }

    #[test]
    fn test_is_active() {
        let mut dim = Dimension::new(
            "Test",
            ArtifactType::Platform,
            "repo",
            vec!["1".to_string(), "2".to_string(), "3".to_string(), "4".to_string()],
        );

        // Initial state: 0 to 3 -> gap is 3 (>1), so active
        assert!(dim.is_active());

        // Narrow to 0 to 2 -> gap is 2 (>1), so active
        dim.high = 2;
        assert!(dim.is_active());

        // Narrow to 0 to 1 -> gap is 1 (not >1), so inactive
        dim.high = 1;
        assert!(!dim.is_active());

        // Narrow to 0 to 0 -> gap is 0 (not >1), so inactive
        dim.high = 0;
        assert!(!dim.is_active());
    }

    #[test]
    fn test_mid() {
        let mut dim = Dimension::new("Test", ArtifactType::Platform, "repo", vec![]);

        dim.low = 0;
        dim.high = 4;
        assert_eq!(dim.mid(), 2);

        dim.low = 0;
        dim.high = 3;
        assert_eq!(dim.mid(), 1);

        dim.low = 2;
        dim.high = 5;
        assert_eq!(dim.mid(), 3);

        dim.low = 2;
        dim.high = 3;
        assert_eq!(dim.mid(), 2);

        dim.low = 2;
        dim.high = 2;
        assert_eq!(dim.mid(), 2);
    }
}
