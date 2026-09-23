// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use argh::FromArgs;
use cm_rust::FidlIntoNative;
use fidl::unpersist;
use fidl_fuchsia_component_decl as fdecl;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// dump configuration values for a component in a human-readable format
#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand, name = "dump-values")]
pub struct DumpValues {
    /// path to the compiled manifest
    #[argh(option)]
    cm: PathBuf,

    /// path to the configuration value file
    #[argh(option)]
    cvf: PathBuf,

    /// path to the JSON output, print to stdout if not provided
    #[argh(option)]
    output: Option<PathBuf>,
}

impl DumpValues {
    pub fn dump(self) -> Result<(), Error> {
        let DumpValues { cm, cvf, output } = self;
        let decl = crate::common::load_manifest(&cm).context("loading compiled manifest")?;
        let config_decl = decl.config.context("component must declare a config schema")?;

        let cvf_bytes =
            std::fs::read(&cvf).with_context(|| format!("reading {}", cvf.display()))?;
        let values: fdecl::ConfigValuesData =
            unpersist(&cvf_bytes).context("decoding value file")?;
        let values = values.fidl_into_native();

        let resolved = config_encoder::ConfigFields::resolve(&config_decl, values, None)
            .context("resolving config values")?;

        let fields_map = resolved
            .fields
            .into_iter()
            .map(|f| (f.key, json_value_from_config_value(f.value)))
            .collect::<BTreeMap<_, _>>();

        let for_humans = serde_json::to_string_pretty(&fields_map)
            .context("serializing config values as json")?;

        if let Some(output) = output {
            std::fs::write(&output, for_humans)
                .with_context(|| format!("writing to {}", output.display()))?;
        } else {
            println!("{}", for_humans);
        }

        Ok(())
    }
}

fn json_value_from_config_value(v: cm_rust::ConfigValue) -> serde_json::Value {
    match v {
        cm_rust::ConfigValue::Single(s) => match s {
            cm_rust::ConfigSingleValue::Bool(b) => serde_json::Value::Bool(b),
            cm_rust::ConfigSingleValue::Uint8(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Uint16(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Uint32(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Uint64(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Int8(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Int16(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Int32(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::Int64(n) => serde_json::Value::Number(n.into()),
            cm_rust::ConfigSingleValue::String(s) => serde_json::Value::String(s.to_string()),
        },
        cm_rust::ConfigValue::Vector(v) => match v {
            cm_rust::ConfigVectorValue::BoolVector(bv) => serde_json::Value::Array(
                IntoIterator::into_iter(bv).map(|b| serde_json::Value::Bool(b)).collect(),
            ),
            cm_rust::ConfigVectorValue::Uint8Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Uint16Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Uint32Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Uint64Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Int8Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Int16Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Int32Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::Int64Vector(nv) => serde_json::Value::Array(
                IntoIterator::into_iter(nv).map(|n| serde_json::Value::Number(n.into())).collect(),
            ),
            cm_rust::ConfigVectorValue::StringVector(sv) => serde_json::Value::Array(
                IntoIterator::into_iter(sv)
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            ),
        },
    }
}
