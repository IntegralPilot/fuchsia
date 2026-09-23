// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude::*;
use anyhow::Error;
use serde::Deserialize;
use std::{io, path};

/// Allows us to directly use [`log::Level`] in our config struct.
mod serde_with_level {
    use serde::de::{Deserializer, Error, Visitor};

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<log::Level, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Helper;

        impl<'de> Visitor<'de> for Helper {
            type Value = log::Level;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(formatter, "a string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                value.parse::<Self::Value>().map_err(Error::custom)
            }
        }

        deserializer.deserialize_str(Helper)
    }
}

/// Arguments decoded from the command line invocation of the driver.
///
/// All values are optional. If an argument is provided, the value takes
/// precedence over value in config file. The config file can be passed
/// through the command-line - a default config file is used if none is
/// provided.
#[derive(argh::FromArgs, PartialEq, Debug)]
pub(crate) struct DriverArgs {
    #[argh(
        option,
        long = "config-path",
        description = "path to json config file which defines configuration options.",
        default = "\"/config/data/device_config.json\".to_string()"
    )]
    pub config_path: String,

    #[argh(option, long = "serviceprefix", description = "service namespace prefix")]
    pub service_prefix: Option<String>,

    #[argh(
        option,
        long = "max-auto-restarts",
        description = "maximum number of automatic restarts"
    )]
    pub max_auto_restarts: Option<u32>,

    #[argh(option, long = "name", description = "name of network interface")]
    pub name: Option<String>,

    #[argh(option, long = "backbone-name", description = "name of backbone network interface")]
    pub backbone_name: Option<String>,

    #[argh(option, long = "log-level", description = "log level")]
    pub log_level: Option<log::Level>,

    #[argh(
        option,
        long = "ot-radio-path",
        description = "path of ot-radio protocol device to connect to. If not specified, the driver will use services to discover the radio."
    )]
    pub ot_radio_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct Config {
    #[serde(default = "Config::default_service_prefix")]
    pub service_prefix: String,

    #[serde(default = "Config::default_max_auto_restarts")]
    pub max_auto_restarts: u32,

    #[serde(default = "Config::default_name")]
    pub name: String,

    #[serde(default = "Config::default_backbone_name")]
    pub backbone_name: Option<String>,

    #[serde(default = "Config::default_log_level", with = "serde_with_level")]
    pub log_level: log::Level,

    #[serde(default = "Config::default_ot_radio_path")]
    pub ot_radio_path: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        // Default values of config if not contained in the config file
        Config {
            service_prefix: Self::default_service_prefix(),
            max_auto_restarts: Self::default_max_auto_restarts(),
            name: Self::default_name(),
            backbone_name: Self::default_backbone_name(),
            log_level: Self::default_log_level(),
            ot_radio_path: Self::default_ot_radio_path(),
        }
    }
}

impl Config {
    fn default_service_prefix() -> String {
        "/svc".to_string()
    }

    fn default_max_auto_restarts() -> u32 {
        20
    }

    fn default_log_level() -> log::Level {
        log::Level::Info
    }

    fn default_name() -> String {
        "lowpan0".to_string()
    }

    fn default_backbone_name() -> Option<String> {
        None
    }

    fn default_ot_radio_path() -> Option<String> {
        None
    }

    pub fn try_new() -> Result<Config, Error> {
        use std::path::Path;

        let args: DriverArgs = argh::from_env();

        let config_path = args.config_path.clone();

        let mut config = if Path::new(&config_path).exists() {
            info!("Config file {} exists, will load config data from it", config_path);
            Config::load(config_path)?
        } else {
            info!("Config file {} doesn't exist, will use default values", config_path);
            Config::default()
        };

        debug!("Config values before cmd-line-override {:?} ", config);

        config.cmd_line_override(args);

        debug!("Final config values are: {:?} ", config);

        Ok(config)
    }

    /// Loads config variables with the deserialized contents of config file
    fn load<P: AsRef<path::Path>>(path: P) -> Result<Self, Error> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)
            .with_context(|| format!("lowpan could not open the config file {}", path.display()))?;
        let config = serde_json::from_reader(io::BufReader::new(file)).with_context(|| {
            format!("lowpan could not deserialize the config file {}", path.display())
        })?;
        Ok(config)
    }

    /// Override the config value with command-line option if it is passed
    fn cmd_line_override(&mut self, args: DriverArgs) {
        if let Some(tmp) = args.service_prefix {
            info!("cmdline overriding service_prefix from {:?} to {:?}", self.service_prefix, tmp);
            self.service_prefix = tmp;
        }

        if let Some(tmp) = args.max_auto_restarts {
            info!(
                "cmdline overriding max_auto_restarts from {} to {}",
                self.max_auto_restarts, tmp
            );
            self.max_auto_restarts = tmp;
        }

        if let Some(tmp) = args.name {
            info!("cmdline overriding name from {:?} to {:?}", self.name, tmp);
            self.name = tmp;
        }

        if let Some(tmp) = args.backbone_name {
            info!("cmdline overriding backbone_name from {:?} to {:?}", self.backbone_name, tmp);
            self.backbone_name = Some(tmp);
        }

        if let Some(x) = args.log_level {
            info!("cmdline log level set to {:?}", x);
            self.log_level = x;
        }

        if let Some(tmp) = args.ot_radio_path {
            info!("cmdline overriding ot_radio_path from {:?} to {:?}", self.ot_radio_path, tmp);
            self.ot_radio_path = Some(tmp);
        }
    }
}
