// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The type if intl configuration to be used.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Type {
    /// Intl services are not used at all. Some very basic configurations such
    /// as bringup don't need internationalization support.
    None,

    /// The default intl services bundle is used.
    #[default]
    Default,

    /// The special small footprint intl services bundle is used.
    ///
    /// Some small footprint deployments need to be extra conscious of the
    /// storage space used.
    Small,

    /// Some products seem to have been misconfigured to use the small intl
    /// service, but full size timezone service. This is likely wrong, but we
    /// must be able to express that too.
    SmallWithTimezone,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::None => write!(f, "none"),
            Type::Default => write!(f, "default"),
            Type::Small => write!(f, "small"),
            Type::SmallWithTimezone => write!(f, "small_with_timezone"),
        }
    }
}

/// Platform configuration options for the input area.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct IntlConfig {
    /// The intl configuration type in use.  If unspecified, the Type::Default
    /// will be used on Standard systems, and Type::None on Utility and Bootstrp
    /// systems.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_type: Option<Type>,

    /// Should assembly include the zoneinfo files, in addition to the
    /// "regular" ICU time zone data.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub include_zoneinfo_files: bool,
}
