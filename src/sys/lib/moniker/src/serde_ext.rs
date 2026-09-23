// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::child_name::ChildName;
use crate::moniker::Moniker;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::Schema;
use schemars::{JsonSchema, schema_for};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;

impl Serialize for ChildName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&self)
    }
}

struct ChildNameVisitor;

impl<'de> Visitor<'de> for ChildNameVisitor {
    type Value = ChildName;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a child moniker of a component instance")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match ChildName::parse(value) {
            Ok(moniker) => Ok(moniker),
            Err(err) => Err(E::custom(format!("Failed to parse ChildName: {}", err))),
        }
    }
}

impl<'de> Deserialize<'de> for ChildName {
    fn deserialize<D>(deserializer: D) -> Result<ChildName, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ChildNameVisitor)
    }
}

impl Serialize for Moniker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&self)
    }
}

struct MonikerVisitor;

impl<'de> Visitor<'de> for MonikerVisitor {
    type Value = Moniker;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a moniker of a component instance")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match Moniker::parse_str(value) {
            Ok(moniker) => Ok(moniker),
            Err(err) => Err(E::custom(format!("Failed to parse Moniker: {}", err))),
        }
    }
}

impl<'de> Deserialize<'de> for Moniker {
    fn deserialize<D>(deserializer: D) -> Result<Moniker, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(MonikerVisitor)
    }
}

impl JsonSchema for Moniker {
    fn schema_name() -> String {
        "Moniker".to_owned()
    }
    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        Schema::Object(schema_for!(String).schema)
    }
}
