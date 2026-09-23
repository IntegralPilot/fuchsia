// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use argh::FromArgs;
use fidl_fuchsia_hardware_power as fidl_power;
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Transition {
    target_level: u8,
    latency_us: u32,
}

impl From<Transition> for fidl_power::Transition {
    fn from(value: Transition) -> Self {
        fidl_power::Transition {
            target_level: Some(value.target_level),
            latency_us: Some(value.latency_us),
            ..Default::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PowerLevel {
    level: u8,
    name: String,
    transitions: std::vec::Vec<Transition>,
}

impl From<PowerLevel> for fidl_power::PowerLevel {
    fn from(value: PowerLevel) -> Self {
        fidl_power::PowerLevel {
            level: Some(value.level),
            name: Some(value.name),
            transitions: Some(value.transitions.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PowerElement {
    name: String,
    levels: std::vec::Vec<PowerLevel>,
}

impl From<PowerElement> for fidl_power::PowerElement {
    fn from(value: PowerElement) -> Self {
        fidl_power::PowerElement {
            name: Some(value.name),
            levels: Some(value.levels.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

#[derive(Deserialize, Debug)]
enum SagElement {
    ExecutionState,
    ApplicationActivity,
}

impl From<SagElement> for fidl_power::SagElement {
    fn from(value: SagElement) -> Self {
        match value {
            SagElement::ExecutionState => fidl_power::SagElement::ExecutionState,
            SagElement::ApplicationActivity => fidl_power::SagElement::ApplicationActivity,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
enum ParentElement {
    #[serde(alias = "sag")]
    Sag(SagElement),
    #[serde(alias = "instance_name")]
    InstanceName(String),
}

impl From<ParentElement> for fidl_power::ParentElement {
    fn from(value: ParentElement) -> Self {
        match value {
            ParentElement::InstanceName(s) => fidl_power::ParentElement::InstanceName(s),
            ParentElement::Sag(s) => fidl_power::ParentElement::Sag(s.into()),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct LevelTuple {
    child_level: u8,
    parent_level: u8,
}

impl From<LevelTuple> for fidl_power::LevelTuple {
    fn from(value: LevelTuple) -> Self {
        fidl_power::LevelTuple {
            child_level: Some(value.child_level),
            parent_level: Some(value.parent_level),
            ..Default::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PowerDependency {
    child: String,
    parent: ParentElement,
    level_deps: Vec<LevelTuple>,
}

impl From<PowerDependency> for fidl_power::PowerDependency {
    fn from(value: PowerDependency) -> Self {
        fidl_power::PowerDependency {
            child: Some(value.child),
            parent: Some(value.parent.into()),
            level_deps: Some(value.level_deps.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PowerConfig {
    element: PowerElement,
    dependencies: Vec<PowerDependency>,
}

impl From<PowerConfig> for fidl_power::PowerElementConfiguration {
    fn from(value: PowerConfig) -> Self {
        fidl_power::PowerElementConfiguration {
            element: Some(value.element.into()),
            dependencies: Some(value.dependencies.into_iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

#[derive(FromArgs)]
/// Tool for compiling power configuration to FIDL.
pub struct Command {
    /// JSON5 file containing the power configuration data.
    #[argh(option)]
    values: PathBuf,

    /// path to write the persisted FIDL to.
    #[argh(option)]
    output: PathBuf,
}

fn main() -> Result<(), Error> {
    let command = argh::from_env::<Command>();
    // load & parse the json file containing value defs
    let values_raw = fs::read_to_string(command.values).context("reading values JSON")?;
    let config: Vec<PowerConfig> =
        serde_json5::from_str(&values_raw).context("parsing values JSON").unwrap();

    let fidl_configs: Vec<fidl_power::PowerElementConfiguration> = config
        .into_iter()
        .map(|json_config| {
            let fidl_config = json_config.into();
            fidl_config
        })
        .collect();

    let encoded_output =
        fidl::persist(&fidl_power::ComponentPowerConfiguration { power_elements: fidl_configs })
            .context("encoding value file")?;

    // write result to value file output
    if let Some(parent) = command.output.parent() {
        // attempt to create all parent directories, ignore failures bc they might already exist
        std::fs::create_dir_all(parent).ok();
    }
    let mut out_file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(command.output)
        .context("opening output file")?;
    out_file.write(&encoded_output).context("writing value file to output")?;

    Ok(())
}
