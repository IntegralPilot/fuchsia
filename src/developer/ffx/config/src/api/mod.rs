// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde_json::Value;
use thiserror::Error;

pub mod query;
pub mod value;

pub type ConfigResult = Result<ConfigValue, ConfigError>;
pub use crate::ConfigSource;
pub use query::ConfigQuery;
pub use value::ConfigValue;

use crate::ConfigLevel;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Name of configuration is required to write to a value")]
    NameRequired,

    #[error("Level of configuration is required to write to a value")]
    LevelRequired,

    #[error("Cannot override defaults")]
    CannotOverrideDefaults,

    #[error("cannot add a value to a subtree")]
    CannotAddToSubtree,
}

#[derive(Debug, Error)]
#[error("Configuration error")]
pub enum ConfigError {
    #[error("Analytics error: {0}")]
    Analytics(#[from] analytics::AnalyticsError),
    #[error("Config key not found")]
    KeyNotFound,
    #[error("Can't remove empty key")]
    EmptyKey,
    #[error("Cannot access unconfigured {level} level configuration")]
    UnconfiguredLevel { level: ConfigLevel },
    #[error("Bad value: {value}: {reason}")]
    BadValue { value: Value, reason: String },
    #[error("Failed to acquire config read lock")]
    ReadLockFailed,
    #[error("Failed to acquire config write lock")]
    WriteLockFailed,
    #[error("Invalid query: {0}")]
    InvalidQuery(String),
    #[error("Validation error: {0}")]
    ValidationError(#[from] ValidationError),

    #[error("Additive mode can only be used with an array or Value return type.")]
    AdditiveModeInvalid,

    #[error("No single source available for aggregated query")]
    NoSingleSource,

    #[error("Conversion to {to} not possible for value: {value}")]
    ConversionFailed { to: &'static str, value: Value },

    #[error("No value set. Could not convert to {0}")]
    NoValueSet(&'static str),
    #[error("Nested error: {0}")]
    Nested(#[source] Box<crate::nested::NestedError>),
    #[error("Storage error: {0}")]
    Storage(#[source] Box<crate::storage::StorageError>),
    #[error("Environment error: {0}")]
    Environment(#[source] Box<crate::environment::EnvironmentError>),
}

impl From<crate::nested::NestedError> for ConfigError {
    fn from(e: crate::nested::NestedError) -> Self {
        Self::Nested(Box::new(e))
    }
}

impl From<crate::environment::EnvironmentError> for ConfigError {
    fn from(e: crate::environment::EnvironmentError) -> Self {
        Self::Environment(Box::new(e))
    }
}

impl From<crate::mapping::MappingError> for ConfigError {
    fn from(e: crate::mapping::MappingError) -> Self {
        Self::Nested(Box::new(crate::nested::NestedError::Mapping(e)))
    }
}

impl From<crate::storage::StorageError> for ConfigError {
    fn from(e: crate::storage::StorageError) -> Self {
        Self::Storage(Box::new(e))
    }
}

impl From<ConfigError> for std::convert::Infallible {
    fn from(value: ConfigError) -> Self {
        panic!("cannot convert value into `Infallible`: {value:#}")
    }
}
