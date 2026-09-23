// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::Utf8Path;
use serde::Deserialize;

/// A parsed GN label.
#[derive(Debug, PartialEq, Clone)]
pub struct GNLabel {
    /// The path to the target.
    pub path: String,
    /// The name of the target.
    pub target_name: String,
    /// The toolchain of the target.
    pub toolchain: Option<String>,
}

impl GNLabel {
    /// Returns the label, without the toolchain.
    pub fn without_toolchain(&self) -> String {
        if self.target_name == Utf8Path::new(&self.path).file_name().unwrap_or("") {
            self.path.clone()
        } else {
            format!("{}:{}", self.path, self.target_name)
        }
    }
}

impl<'de> Deserialize<'de> for GNLabel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let (path_and_target, toolchain) = if let Some(toolchain_start) = s.rfind('(') {
            if !s.ends_with(')') {
                return Err(serde::de::Error::custom(format!(
                    "GN label with toolchain does not end with ')' and instead ends with '{}'",
                    s.chars().last().unwrap_or_default()
                )));
            }
            let (path_and_target, toolchain) = s.split_at(toolchain_start);
            let toolchain = toolchain.strip_prefix('(').unwrap().strip_suffix(')').unwrap();
            (path_and_target, Some(toolchain.to_string()))
        } else {
            (s.as_str(), None)
        };

        let (path, target_name) = match path_and_target.rsplit_once(':') {
            Some((path, target_name)) => (path, target_name),
            None => {
                let path = path_and_target;
                let target_name = Utf8Path::new(path).file_name().ok_or_else(|| {
                    serde::de::Error::custom(format!("GN label path has no filename: {}", path))
                })?;
                (path, target_name)
            }
        };

        if target_name.is_empty() {
            return Err(serde::de::Error::custom(format!(
                "GN label must have a non-empty target name: {}",
                s
            )));
        }

        Ok(GNLabel { path: path.to_string(), target_name: target_name.to_string(), toolchain })
    }
}

#[cfg(test)]
mod tests {
    use super::GNLabel;
    use serde_json::json;

    #[test]
    fn test_gn_label_deserialize() {
        let label: GNLabel = serde_json::from_value(json!(
            "//build/images/fuchsia:fuchsia(//build/toolchain:default)"
        ))
        .unwrap();
        assert_eq!(
            label,
            GNLabel {
                path: "//build/images/fuchsia".into(),
                target_name: "fuchsia".into(),
                toolchain: Some("//build/toolchain:default".into()),
            }
        );
    }

    #[test]
    fn test_gn_label_deserialize_no_toolchain() {
        let label: GNLabel =
            serde_json::from_value(json!("//build/images/fuchsia:fuchsia")).unwrap();
        assert_eq!(
            label,
            GNLabel {
                path: "//build/images/fuchsia".into(),
                target_name: "fuchsia".into(),
                toolchain: None,
            }
        );
    }

    #[test]
    fn test_gn_label_deserialize_no_target_name() {
        let label: GNLabel = serde_json::from_value(json!("//build/images/fuchsia")).unwrap();
        assert_eq!(
            label,
            GNLabel {
                path: "//build/images/fuchsia".into(),
                target_name: "fuchsia".into(),
                toolchain: None,
            }
        );
    }

    #[test]
    fn test_gn_label_without_toolchain() {
        let label = GNLabel {
            path: "//build/images/fuchsia".into(),
            target_name: "fuchsia".into(),
            toolchain: None,
        };
        assert_eq!(label.without_toolchain(), "//build/images/fuchsia");

        let label = GNLabel {
            path: "//build/images/fuchsia".into(),
            target_name: "another".into(),
            toolchain: None,
        };
        assert_eq!(label.without_toolchain(), "//build/images/fuchsia:another");
    }

    #[test]
    fn test_gn_label_deserialize_empty_target_name() {
        let label: Result<GNLabel, _> = serde_json::from_value(json!("//build/images/fuchsia:"));
        assert!(label.is_err());
    }

    #[test]
    fn test_gn_label_deserialize_no_filename() {
        let label: Result<GNLabel, _> = serde_json::from_value(json!("//"));
        assert!(label.is_err());
    }
}
