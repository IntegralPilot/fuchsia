// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! # Diagnostics log types serde
//!
//! Serializers and deserializers for types used in various places for logging.
//!
//! Intended as utilities for used with serde as:
//!
//! ```
//! use diagnostics_log_types_serde::{optinoal_severity, Severity};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SomeType {
//!     #[serde(default, with = "optional_severity")]
//!     severity: Option<Severity>
//! }
//! ```

pub use diagnostics_log_types::Severity;
use serde::de::Deserializer;
use serde::{Deserialize, Serializer};
use std::str::FromStr;

pub mod severity {
    use super::*;

    pub fn serialize<S>(severity: &Severity, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(severity.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Severity, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Severity::from_str(&s).map_err(serde::de::Error::custom)
    }
}

pub mod optional_severity {
    use super::*;

    pub fn serialize<S>(severity: &Option<Severity>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(s) = severity.as_ref() {
            return super::severity::serialize(s, serializer);
        }
        serializer.serialize_none()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Severity>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            None => Ok(None),
            Some(s) => {
                let severity = Severity::from_str(&s).map_err(serde::de::Error::custom)?;
                Ok(Some(severity))
            }
        }
    }
}
