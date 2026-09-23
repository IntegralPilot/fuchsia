// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ConfigSource;
use crate::api::ConfigError;
use crate::api::query::{ConfigQuery, SelectMode};
use crate::mapping::{filter, flatten};
use crate::nested::RecursiveMap;

use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ConfigValue {
    pub(crate) value: Option<Value>,
    pub(crate) source: Option<ConfigSource>,
}

impl ConfigValue {
    pub fn new(value: Option<Value>, source: Option<ConfigSource>) -> Self {
        Self { value, source }
    }

    pub fn value(&self) -> Option<&Value> {
        self.value.as_ref()
    }

    pub fn source(&self) -> Option<&ConfigSource> {
        self.source.as_ref()
    }

    pub fn into_parts(self) -> (Option<Value>, Option<ConfigSource>) {
        (self.value, self.source)
    }
}

// See RecursiveMap for why the value version is the main implementation.
impl RecursiveMap for ConfigValue {
    type Output = ConfigValue;

    fn recursive_map<T: Fn(Value) -> Option<Value>>(self, mapper: &T) -> ConfigValue {
        ConfigValue { value: self.value.recursive_map(mapper), source: self.source }
    }

    fn try_recursive_map<T: Fn(Value) -> Result<Option<Value>, crate::api::ConfigError>>(
        self,
        mapper: &T,
    ) -> Result<Self::Output, crate::api::ConfigError> {
        Ok(ConfigValue { value: self.value.try_recursive_map(mapper)?, source: self.source })
    }
}

pub trait ValueStrategy {
    fn handle_arrays(value: Value) -> Option<Value> {
        flatten(value)
    }

    fn validate_query(query: &ConfigQuery<'_>) -> Result<(), ConfigError> {
        match query.select {
            SelectMode::First => Ok(()),
            SelectMode::All => Err(ConfigError::AdditiveModeInvalid),
        }
    }
}

impl From<ConfigValue> for Option<Value> {
    fn from(value: ConfigValue) -> Self {
        value.value
    }
}

impl From<Option<Value>> for ConfigValue {
    fn from(value: Option<Value>) -> Self {
        ConfigValue { value, source: None }
    }
}

impl From<Value> for ConfigValue {
    fn from(value: Value) -> Self {
        ConfigValue { value: Some(value), source: None }
    }
}

impl ValueStrategy for Value {
    fn handle_arrays(value: Value) -> Option<Value> {
        Some(value)
    }

    fn validate_query(_query: &ConfigQuery<'_>) -> Result<(), ConfigError> {
        Ok(())
    }
}

pub trait TryConvert: Sized {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError>;
}

impl<T> TryConvert for T
where
    T: From<ConfigValue>,
{
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(value.into())
    }
}

impl TryConvert for Value {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        value.value.ok_or(ConfigError::NoValueSet("Value"))
    }
}

impl ValueStrategy for Option<Value> {
    fn handle_arrays(value: Value) -> Option<Value> {
        Some(value)
    }

    fn validate_query(_query: &ConfigQuery<'_>) -> Result<(), ConfigError> {
        Ok(())
    }
}

impl ValueStrategy for String {}

impl TryConvert for String {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("String"))?;
        let conversion = inner.as_str().map(|s| s.to_string());
        conversion.ok_or(ConfigError::ConversionFailed { to: "String", value: inner })
    }
}

impl ValueStrategy for Option<String> {}

// The reason for these specific `TryConvert for Option<T>` implementations is because
// making an overall `impl<T: TryConvert> TryConvert for Option<T>` is because there's
// a conflicting definition for `impl<T> TryConvert for T where T: From<ConfigValue>`
// and it is unclear where this implementation is used or why it's necessary. This could
// likely be fixed in a follow-up change, but it seems like this might be more time to
// sink into fixing than it's worth.
impl TryConvert for Option<String> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(String::try_convert(value).ok())
    }
}

impl ValueStrategy for usize {}

impl TryConvert for usize {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("usize"))?;
        let conversion = inner
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None });
        conversion.ok_or(ConfigError::ConversionFailed { to: "usize", value: inner })
    }
}

impl ValueStrategy for u64 {}

impl TryConvert for u64 {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("u64"))?;
        let conversion = inner
            .as_u64()
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None });
        conversion.ok_or(ConfigError::ConversionFailed { to: "u64", value: inner })
    }
}

impl ValueStrategy for Option<u64> {}

impl TryConvert for Option<u64> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(u64::try_convert(value).ok())
    }
}

impl ValueStrategy for u16 {}

impl TryConvert for u16 {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("u16"))?;
        let conversion = inner
            .as_u64()
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None })
            .and_then(|v| u16::try_from(v).ok());
        conversion.ok_or(ConfigError::ConversionFailed { to: "u16", value: inner })
    }
}

impl ValueStrategy for i64 {}

impl TryConvert for i64 {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("i64"))?;
        let conversion = inner
            .as_i64()
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None });
        conversion.ok_or(ConfigError::ConversionFailed { to: "i64", value: inner })
    }
}

impl ValueStrategy for bool {}

impl TryConvert for bool {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("bool"))?;
        let conversion = inner
            .as_bool()
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None });
        conversion.ok_or(ConfigError::ConversionFailed { to: "bool", value: inner })
    }
}

impl ValueStrategy for Option<bool> {}

impl TryConvert for Option<bool> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(bool::try_convert(value).ok())
    }
}

impl ValueStrategy for PathBuf {}

impl TryConvert for PathBuf {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("PathBuf"))?;
        let conversion = inner.as_str().map(|s| PathBuf::from(s.to_string()));
        conversion.ok_or(ConfigError::ConversionFailed { to: "PathBuf", value: inner })
    }
}

impl ValueStrategy for Option<PathBuf> {}

impl TryConvert for Option<PathBuf> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(PathBuf::try_convert(value).ok())
    }
}

impl<T> ValueStrategy for Vec<T> {
    fn handle_arrays(value: Value) -> Option<Value> {
        filter(value)
    }

    fn validate_query(_query: &ConfigQuery<'_>) -> Result<(), ConfigError> {
        Ok(())
    }
}

impl<T: TryConvert> TryConvert for Vec<T> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        value
            .value
            .and_then(|val| match val.as_array() {
                Some(v) => {
                    let result: Vec<T> = v
                        .iter()
                        .filter_map(|i| T::try_convert(ConfigValue::from(Some(i.clone()))).ok())
                        .collect();
                    if !result.is_empty() { Some(result) } else { None }
                }
                None => T::try_convert(ConfigValue::from(Some(val))).map(|x| vec![x]).ok(),
            })
            .ok_or(ConfigError::KeyNotFound)
    }
}

impl ValueStrategy for f64 {}

impl TryConvert for f64 {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        let inner = value.value.ok_or(ConfigError::NoValueSet("f64"))?;
        let conversion = inner
            .as_f64()
            .or_else(|| if let Value::String(ref s) = inner { s.parse().ok() } else { None });
        conversion.ok_or(ConfigError::ConversionFailed { to: "f64", value: inner })
    }
}

impl ValueStrategy for Option<f64> {}

impl TryConvert for Option<f64> {
    fn try_convert(value: ConfigValue) -> Result<Self, ConfigError> {
        Ok(f64::try_convert(value).ok())
    }
}

/// Merges [`Map`] b into [`Map`] a.
pub fn merge_map(a: &mut Map<String, Value>, b: &Map<String, Value>) {
    for (k, v) in b.iter() {
        self::merge(a.entry(k.clone()).or_insert(Value::Null), v);
    }
}

/// Merge's `Value` b into `Value` a.
pub fn merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (&mut Value::Object(ref mut a), &Value::Object(ref b)) => merge_map(a, b),
        (a, b) => *a = b.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_merge() {
        let mut proto = Value::Null;
        let a = json!({
            "list": [ "first", "second" ],
            "string": "This is a string",
            "object" : {
                "foo" : "foo-prime",
                "bar" : "bar-prime"
            }
        });
        let b = json!({
            "list": [ "third" ],
            "title": "This is a title",
            "otherObject" : {
                "yourHonor" : "I object!"
            }
        });
        merge(&mut proto, &a);
        assert_eq!(proto["list"].as_array().unwrap()[0].as_str().unwrap(), "first");
        assert_eq!(proto["list"].as_array().unwrap()[1].as_str().unwrap(), "second");
        assert_eq!(proto["string"].as_str().unwrap(), "This is a string");
        assert_eq!(proto["object"]["foo"].as_str().unwrap(), "foo-prime");
        assert_eq!(proto["object"]["bar"].as_str().unwrap(), "bar-prime");
        merge(&mut proto, &b);
        assert_eq!(proto["list"].as_array().unwrap()[0].as_str().unwrap(), "third");
        assert_eq!(proto["title"].as_str().unwrap(), "This is a title");
        assert_eq!(proto["string"].as_str().unwrap(), "This is a string");
        assert_eq!(proto["object"]["foo"].as_str().unwrap(), "foo-prime");
        assert_eq!(proto["object"]["bar"].as_str().unwrap(), "bar-prime");
        assert_eq!(proto["otherObject"]["yourHonor"].as_str().unwrap(), "I object!");
    }

    #[fuchsia::test]
    fn test_config_error() {
        let env = crate::test_env().build().unwrap();
        let context = &env.context;

        let err = context.get::<Vec<String>, &str>("Some-key-that-does-not-exist").unwrap_err();
        match err {
            ConfigError::KeyNotFound => (),
            _ => {
                panic!("Expected KeyNotFound, got {err:?}")
            }
        }
    }
}
