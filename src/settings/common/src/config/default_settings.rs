// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use serde::de::DeserializeOwned;
use std::fmt::{Debug, Display};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;

use crate::config;
use crate::config::ConfigLoadInfo;
use crate::inspect::config_logger::InspectConfigLogger;

pub struct DefaultSetting<T, P>
where
    T: DeserializeOwned + Clone + Debug,
    P: AsRef<Path> + Display,
{
    default_value: Option<T>,
    config_file_path: P,
    cached_value: Option<Option<T>>,
    config_logger: Rc<Mutex<InspectConfigLogger>>,
}

impl<T, P> DefaultSetting<T, P>
where
    T: DeserializeOwned + Clone + std::fmt::Debug,
    P: AsRef<Path> + Display,
{
    pub fn new(
        default_value: Option<T>,
        config_file_path: P,
        config_logger: Rc<Mutex<InspectConfigLogger>>,
    ) -> Self {
        DefaultSetting { default_value, config_file_path, cached_value: None, config_logger }
    }

    /// Returns the value of this setting. Loads the value from storage if it hasn't been loaded
    /// before, otherwise returns a cached value.
    pub fn get_cached_value(&mut self) -> Result<Option<T>, Error> {
        if self.cached_value.is_none() {
            self.cached_value = Some(self.load_default_settings()?);
        }

        Ok(self.cached_value.as_ref().expect("cached value not present").clone())
    }

    /// Loads the value of this setting from storage.
    ///
    /// If the value isn't present, returns the default value.
    pub fn load_default_value(&mut self) -> Result<Option<T>, Error> {
        self.load_default_settings()
    }

    /// Attempts to load the settings from the given config_file_path.
    ///
    /// Returns the default value if unable to read or parse the file. The returned option will
    /// only be None if the default_value was provided as None.
    fn load_default_settings(&mut self) -> Result<Option<T>, Error> {
        let config_load_info: Option<ConfigLoadInfo>;
        let path = self.config_file_path.to_string();
        let load_result = match File::open(self.config_file_path.as_ref()) {
            Ok(file) => {
                #[allow(clippy::manual_map)]
                match serde_json::from_reader(BufReader::new(file)) {
                    Ok(config) => {
                        // Success path.
                        config_load_info = Some(ConfigLoadInfo {
                            status: config::ConfigLoadStatus::Success,
                            contents: if let Some(ref payload) = config {
                                Some(format!("{payload:?}"))
                            } else {
                                None
                            },
                        });
                        Ok(config)
                    }
                    Err(e) => {
                        // Found file, but failed to parse.
                        let err_msg = format!("unable to parse config: {e:?}");
                        config_load_info = Some(ConfigLoadInfo {
                            status: config::ConfigLoadStatus::ParseFailure(err_msg.clone()),
                            contents: None,
                        });
                        Err(format_err!("{:?}", err_msg))
                    }
                }
            }
            Err(..) => {
                // No file found.
                config_load_info = Some(ConfigLoadInfo {
                    status: config::ConfigLoadStatus::UsingDefaults(
                        "File not found, using defaults".to_string(),
                    ),
                    contents: None,
                });
                Ok(self.default_value.clone())
            }
        };
        if let Some(config_load_info) = config_load_info {
            self.write_config_load_to_inspect(path, config_load_info);
        } else {
            log::error!("Could not load config for {:?}", path);
        }

        load_result
    }

    /// Attempts to write the config load to inspect.
    fn write_config_load_to_inspect(
        &mut self,
        path: String,
        config_load_info: config::ConfigLoadInfo,
    ) {
        self.config_logger.lock().unwrap().write_config_load_to_inspect(path, config_load_info);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    use crate::clock;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{assert_data_tree, AnyProperty};
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect::component;
    use serde::Deserialize;
    use settings_test_common::helpers::move_executor_forward_and_get;

    #[derive(Clone, Debug, Deserialize)]
    struct TestConfigData {
        value: u32,
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_load_valid_config_data() {
        let mut setting = DefaultSetting::new(
            Some(TestConfigData { value: 3 }),
            "/config/data/fake_config_data.json",
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root()))),
        );

        assert_eq!(
            setting.load_default_value().expect("Failed to get default value").unwrap().value,
            10
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_load_invalid_config_data() {
        let mut setting = DefaultSetting::new(
            Some(TestConfigData { value: 3 }),
            "/config/data/fake_invalid_config_data.json",
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root()))),
        );
        assert!(setting.load_default_value().is_err());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_load_invalid_config_file_path() {
        let mut setting = DefaultSetting::new(
            Some(TestConfigData { value: 3 }),
            "nuthatch",
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root()))),
        );

        assert_eq!(
            setting.load_default_value().expect("Failed to get default value").unwrap().value,
            3
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_load_default_none() {
        let mut setting = DefaultSetting::<TestConfigData, &str>::new(
            None,
            "nuthatch",
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root()))),
        );

        assert!(setting.load_default_value().expect("Failed to get default value").is_none());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_no_inspect_write() {
        let mut setting = DefaultSetting::<TestConfigData, &str>::new(
            None,
            "nuthatch",
            Rc::new(Mutex::new(InspectConfigLogger::new(component::inspector().root()))),
        );

        assert!(setting.load_default_value().expect("Failed to get default value").is_none());
    }

    #[fuchsia::test]
    fn test_config_inspect_write() {
        let mut executor = TestExecutor::new_with_fake_time();
        clock::mock::set(zx::MonotonicInstant::from_nanos(0));

        let inspector = component::inspector();
        let mut setting = DefaultSetting::new(
            Some(TestConfigData { value: 3 }),
            "nuthatch",
            Rc::new(Mutex::new(InspectConfigLogger::new(inspector.root()))),
        );
        let load_result = move_executor_forward_and_get(
            &mut executor,
            async { setting.load_default_value() },
            "Unable to get default value",
        );

        assert_matches!(load_result, Ok(Some(TestConfigData { value: 3 })));
        assert_data_tree!(@executor executor, inspector, root: {
            config_loads: {
                "nuthatch": {
                    "count": AnyProperty,
                    "result_counts": {
                        "UsingDefaults": 1u64,
                    },
                    "timestamp": "0.000000000",
                    "value": "ConfigLoadInfo {\n    status: UsingDefaults(\n        \"File not found, using defaults\",\n    ),\n    contents: None,\n}",
                }
            }
        });
    }
}
