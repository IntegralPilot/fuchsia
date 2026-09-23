// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use include_str_from_working_dir::include_str_from_working_dir_env;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ICUMap(pub std::collections::HashMap<Revision, String>);

// See `rustenv` in //src/lib/assembly/config_schema:config_schema.
pub static ICU_CONFIG_INFO: std::sync::LazyLock<ICUMap> = std::sync::LazyLock::new(|| {
    serde_json::from_value(
        serde_json::from_str(include_str_from_working_dir_env!("ICU_GIT_INFO_JSON_FILE")).unwrap(),
    )
    .unwrap()
});

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Hash, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Revision {
    /// Whatever revision is currently 'default'.
    #[default]
    Default,
    /// Whatever revision is currently 'latest'.
    Latest,
    /// If none of the above work, then you can specify a git commit ID.
    ///
    /// Use:
    ///
    /// ```ignore
    /// {
    ///   "revision": { "commit_id": "f005...ba11" }
    /// }
    /// ```
    ///
    /// Where the value of `commit_id` is a fully specified git
    /// commit hash.
    CommitId(String),
}

impl std::fmt::Display for Revision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Revision::Default => write!(f, "default"),
            Revision::Latest => write!(f, "latest"),
            Revision::CommitId(id) => write!(f, "commit_id({})", id),
        }
    }
}

/// System assembly configuration for the ICU subsystem.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Clone, JsonSchema)]
#[serde(default)]
pub struct ICUConfig {
    /// The revision (corresponding to either one of the labels, or a git commit ID) of the ICU
    /// library to use in system assembly. This revision is constrained to the commit IDs available
    /// in the repos at `//third_party/icu/{default,latest}`,
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub revision: Revision,

    /// A list of packages that should receive ICU tzdata in their config directory.
    /// TODO(b/297214394): Remove this option once all components use tzdata_provider.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legacy_tzdata_packages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_from_json_tag() {
        struct TestCase {
            input: &'static str,
            expected: ICUConfig,
        }
        let tests = vec![
            TestCase {
                input: r#"{ "revision": { "commit_id": "deadbeef" } }"#,
                expected: ICUConfig {
                    revision: Revision::CommitId("deadbeef".into()),
                    legacy_tzdata_packages: vec![],
                },
            },
            TestCase {
                input: r#"{}"#,
                expected: ICUConfig { revision: Revision::Default, legacy_tzdata_packages: vec![] },
            },
            TestCase {
                input: r#"{ "legacy_tzdata_packages": [ "one", "two" ] }"#,
                expected: ICUConfig {
                    revision: Revision::Default,
                    legacy_tzdata_packages: vec!["one".to_string(), "two".to_string()],
                },
            },
        ];
        for test in tests {
            let json = serde_json::from_str(test.input).unwrap();
            let parsed: ICUConfig = serde_json::from_value(json).unwrap();
            assert_eq!(parsed, test.expected);
        }
    }
}
