// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use log::{error, info};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufReader, Read};
use thiserror::Error;

/// Static service configuration options.
#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    enable_dynamic_configuration: bool,
    persisted_repos_dir: Option<String>,
}

impl Config {
    pub fn enable_dynamic_configuration(&self) -> bool {
        self.enable_dynamic_configuration
    }

    pub fn persisted_repos_dir(&self) -> Option<&str> {
        self.persisted_repos_dir.as_deref()
    }

    pub fn load_from_config_data_or_default() -> Self {
        let enable_dynamic_configuration = match File::open("/config/data/config.json") {
            Ok(f) => Self::load_enable_dynamic_config(BufReader::new(f)).unwrap_or_else(|e| {
                error!("unable to load config, disabling dynamic config: {:#}", anyhow!(e));
                false
            }),
            Err(e) => {
                info!("no config found, disabling dynamic config: {:#}", anyhow!(e));
                false
            }
        };

        let persisted_repos_dir = match File::open("/config/data/persisted_repos_dir.json") {
            Ok(f) => Self::load_persisted_repos_config(BufReader::new(f)).unwrap_or_else(|e| {
                error!("unable to load config, disabling persisted repos: {:#}", anyhow!(e));
                None
            }),
            Err(e) => {
                info!("no config found, disabling persisted repos: {:#}", anyhow!(e));
                None
            }
        };

        Self { enable_dynamic_configuration, persisted_repos_dir }
    }

    fn load_enable_dynamic_config(r: impl Read) -> Result<bool, ConfigLoadError> {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ParseConfig {
            enable_dynamic_configuration: bool,
        }

        let parse_config = serde_json::from_reader::<_, ParseConfig>(r)?;

        Ok(parse_config.enable_dynamic_configuration)
    }

    fn load_persisted_repos_config(r: impl Read) -> Result<Option<String>, ConfigLoadError> {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ParseConfig {
            persisted_repos_dir: String,
        }

        let parse_config = serde_json::from_reader::<_, ParseConfig>(r)?;

        Ok((!parse_config.persisted_repos_dir.is_empty())
            .then_some(parse_config.persisted_repos_dir))
    }
}

#[derive(Debug, Error)]
enum ConfigLoadError {
    #[error("parse error")]
    Parse(#[from] serde_json::Error),
}

#[cfg(test)]
#[allow(clippy::bool_assert_comparison)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde_json::json;

    fn verify_load_dyn(input: serde_json::Value, expected: bool) {
        assert_eq!(
            Config::load_enable_dynamic_config(input.to_string().as_bytes())
                .expect("json value to be valid"),
            expected
        );
    }

    fn verify_load_repo(input: serde_json::Value, expected: Option<String>) {
        assert_eq!(
            Config::load_persisted_repos_config(input.to_string().as_bytes())
                .expect("json value to be valid"),
            expected
        );
    }

    #[test]
    fn test_load_valid_configs() {
        for val in [true, false].iter() {
            verify_load_dyn(
                json!({
                    "enable_dynamic_configuration": *val,
                }),
                *val,
            );
        }

        verify_load_repo(
            json!({
                "persisted_repos_dir": "boo",
            }),
            Some("boo".into()),
        );

        verify_load_repo(
            json!({
                "persisted_repos_dir": "",
            }),
            None,
        );
    }

    #[test]
    fn test_load_errors_on_unknown_field() {
        assert_matches!(
            Config::load_enable_dynamic_config(
                json!({
                    "enable_dynamic_configuration": false,
                    "unknown_field": 3
                })
                .to_string()
                .as_bytes()
            ),
            Err(ConfigLoadError::Parse(_))
        );
        assert_matches!(
            Config::load_persisted_repos_config(
                json!({
                    "persisted_repos_dir": "boo".to_string(),
                    "unknown_field": 3
                })
                .to_string()
                .as_bytes()
            ),
            Err(ConfigLoadError::Parse(_))
        );
    }
}
