// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// A scrutiny golden that contains an entry per line.
/// Each entry may be prefixed with a '?' to signify that it is optional.
#[derive(Default)]
pub struct Golden {
    /// A map of the entries to whether they are required.
    entries: BTreeMap<String, bool>,
}

impl Golden {
    /// Write the golden to a file.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let entries: Vec<String> = self
            .entries
            .iter()
            .map(|(entry, required)| {
                let prefix = if *required { "" } else { "?" };
                format!("{}{}\n", prefix, entry)
            })
            .collect();
        std::fs::write(path.as_ref(), entries.join(""))
            .with_context(|| format!("Writing golden: {}", path.as_ref().display()))
    }

    /// Add many optional entries.
    pub fn add_many_optional(&mut self, entries: impl IntoIterator<Item = impl AsRef<str>>) {
        self.add_many(/*required=*/ false, entries)
    }

    /// Add many required entries.
    #[allow(unused)]
    pub fn add_many_required(&mut self, entries: impl IntoIterator<Item = impl AsRef<str>>) {
        self.add_many(/*required=*/ true, entries)
    }

    /// Helper function to adding many entries that may be required or not.
    pub fn add_many(&mut self, required: bool, entries: impl IntoIterator<Item = impl AsRef<str>>) {
        for entry in entries.into_iter() {
            let entry = if entry.as_ref().starts_with("lib/libstd-") {
                "lib/libstd-*".to_string()
            } else {
                entry.as_ref().to_string()
            };

            self.entries
                .entry(entry)
                // required=true always clobbers required=false.
                .and_modify(|previously_required| {
                    *previously_required = *previously_required || required;
                })
                .or_insert(required);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_many_optional() {
        let mut golden = Golden::default();
        golden.add_many_optional(vec!["foo", "bar"]);
        assert_eq!(golden.entries.get("foo"), Some(&false));
        assert_eq!(golden.entries.get("bar"), Some(&false));
    }

    #[test]
    fn test_add_many_required() {
        let mut golden = Golden::default();
        golden.add_many_required(vec!["foo", "bar"]);
        assert_eq!(golden.entries.get("foo"), Some(&true));
        assert_eq!(golden.entries.get("bar"), Some(&true));
    }

    #[test]
    fn test_requirements_updates() {
        let mut golden = Golden::default();

        // Optional then Required -> Required
        golden.add_many_optional(vec!["opt_req"]);
        assert_eq!(golden.entries.get("opt_req"), Some(&false));
        golden.add_many_required(vec!["opt_req"]);
        assert_eq!(golden.entries.get("opt_req"), Some(&true));

        // Required then Optional -> Required (preserves required)
        golden.add_many_required(vec!["req_opt"]);
        assert_eq!(golden.entries.get("req_opt"), Some(&true));
        golden.add_many_optional(vec!["req_opt"]);
        assert_eq!(golden.entries.get("req_opt"), Some(&true));
    }

    #[test]
    fn test_libstd_substitution() {
        let mut golden = Golden::default();
        golden.add_many_required(vec!["lib/libstd-12345678.so"]);
        assert_eq!(golden.entries.get("lib/libstd-*"), Some(&true));
        assert!(golden.entries.get("lib/libstd-12345678.so").is_none());
    }

    #[test]
    fn test_write_format() {
        let mut golden = Golden::default();
        golden.add_many_optional(vec!["optional"]);
        golden.add_many_required(vec!["required"]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_golden_write.txt");
        golden.write(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // BTreeMap sorts by key. "optional" < "required".
        // optional is optional -> "?optional"
        // required is required -> "required"
        let expected = "?optional\nrequired\n";
        assert_eq!(content, expected);
    }
}
