// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Functions for interacting with nested json objects in a recursive way.

use serde_json::map::Entry;
use serde_json::{Map, Value};

use crate::ConfigError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NestedError {
    #[error("Configuration literal found when expecting a map.")]
    LiteralExpectedMap,

    #[error("Current key not found trying to recursively remove")]
    CurrentKeyNotFound,

    #[error("Config error: {0}")]
    Config(#[from] crate::ConfigError),

    #[error("Mapping error: {0}")]
    Mapping(#[from] crate::mapping::MappingError),
}

/// A trait that adds a recursive mapping function to a nested json value tree.
///
/// Note that implementations of this should be on value types and not
/// references. This is because most of the time most values won't change, so
/// it makes sense to copy the whole thing up front and only have to build a new
/// item if necessary.
///
/// This also makes it so you can efficiently chain [`RecursiveMap::recursive_map`]
/// calls together, since they will continue to use the same Value items until one
/// is overwritten.
pub(crate) trait RecursiveMap {
    /// The type of output that the filtering will return. That type must implement
    /// this trait as well.
    type Output: RecursiveMap;

    /// Filters values recursively through the function provided.
    fn recursive_map<T: Fn(Value) -> Option<Value>>(self, mapper: &T) -> Self::Output;

    /// Filters values recursively through the function provided. Can report an error.
    fn try_recursive_map<T: Fn(Value) -> Result<Option<Value>, crate::api::ConfigError>>(
        self,
        mapper: &T,
    ) -> Result<Self::Output, crate::api::ConfigError>;
}

impl RecursiveMap for Value {
    type Output = Option<Value>;

    fn recursive_map<T: Fn(Value) -> Option<Value>>(self, mapper: &T) -> Option<Value> {
        // We can use unwrap() because try_recursive_map() only returns
        // an error if the mapper function does
        self.try_recursive_map(&|v| Ok::<Option<Value>, crate::api::ConfigError>(mapper(v)))
            .unwrap()
    }

    fn try_recursive_map<T: Fn(Value) -> Result<Option<Value>, crate::api::ConfigError>>(
        self,
        mapper: &T,
    ) -> Result<Self::Output, crate::api::ConfigError> {
        match self {
            Value::Object(map) => {
                let mut result = Map::new();
                for (key, value) in map.into_iter() {
                    let new_value = if value.is_object() || value.is_array() {
                        value.try_recursive_map(mapper)?
                    } else {
                        mapper(value)?
                    };
                    if let Some(new_value) = new_value {
                        result.insert(key, new_value);
                    }
                }
                if result.len() == 0 { Ok(None) } else { mapper(Value::Object(result)) }
            }
            Value::Array(arr) => {
                let result = Vec::from_iter(
                    arr.into_iter()
                        .map(|v| v.try_recursive_map(mapper))
                        // This is the magic line: if there were any errors, get an error result
                        .collect::<Result<Vec<Option<Value>>, crate::api::ConfigError>>()?
                        .into_iter()
                        .flatten(),
                );
                if result.len() == 0 { Ok(None) } else { mapper(Value::Array(result)) }
            }
            other => mapper(other),
        }
    }
}

impl RecursiveMap for Option<Value> {
    type Output = Option<Value>;

    fn recursive_map<T: Fn(Value) -> Option<Value>>(self, mapper: &T) -> Self::Output {
        self.and_then(|value| value.recursive_map(mapper))
    }

    fn try_recursive_map<T: Fn(Value) -> Result<Option<Value>, crate::api::ConfigError>>(
        self,
        mapper: &T,
    ) -> Result<Self::Output, crate::api::ConfigError> {
        match self {
            None => Ok(None),
            Some(v) => v.try_recursive_map(mapper),
        }
    }
}

/// Search the given nested json value for the given `key`, and then recurse through
/// the rest of the tree for the `remaining_keys`. Returns the value at that position if found.
pub(crate) fn nested_get<'a>(
    cur: Option<&'a Map<String, Value>>,
    key: &str,
    remaining_keys: &[&str],
) -> Option<&'a Value> {
    cur.and_then(|cur| {
        if remaining_keys.len() == 0 {
            cur.get(key)
        } else {
            nested_get(
                cur.get(key).and_then(Value::as_object),
                remaining_keys[0],
                &remaining_keys[1..],
            )
        }
    })
}

/// Find `key` in `cur`, then recurisively search for the `remaining_keys` through the nested
/// object for the position to insert, creating Object entries as it goes if necessary (including
/// overwriting leaf values in the way). Sets the value if not already set to the same value,
/// and returns true if it did (or false if it already existed as the same value).
pub(crate) fn nested_set(
    cur: &mut Map<String, Value>,
    key: &str,
    remaining_keys: &[&str],
    value: Value,
) -> bool {
    if remaining_keys.len() == 0 {
        // Exit early if the value hasn't changed.
        if let Some(old_value) = cur.get(key) {
            if old_value == &value {
                return false;
            }
        }
        cur.insert(key.to_string(), value);
        true
    } else {
        if let Entry::Occupied(mut occupied) = cur.entry(key) {
            let val = occupied.get_mut();
            if let Value::Object(next_map) = val {
                nested_set(next_map, remaining_keys[0], &remaining_keys[1..], value)
            } else {
                let mut next_map = Map::new();
                nested_set(&mut next_map, remaining_keys[0], &remaining_keys[1..], value);
                *val = Value::Object(next_map);
                // since we're creating the rest of the tree, we know we either succeeded and inserted or failed and crashed.
                true
            }
        } else {
            let mut next_map = Map::new();
            nested_set(&mut next_map, remaining_keys[0], &remaining_keys[1..], value);
            cur.insert(key.to_string(), Value::Object(next_map));
            // since we're creating the rest of the tree, we know we either succeeded and inserted or failed and crashed.
            true
        }
    }
}

/// Searches the nested object to find the given `key`, then recursively through the `remaining_keys`,
/// and removes it if found. If the key or any of its parent keys don't exist, it will return an error.
pub(crate) fn nested_remove(
    cur: &mut Map<String, Value>,
    key: &str,
    remaining_keys: &[&str],
) -> Result<(), NestedError> {
    if remaining_keys.len() == 0 {
        cur.remove(&key.to_string())
            .ok_or(NestedError::Config(ConfigError::KeyNotFound))
            .map(|_| ())
    } else {
        // Just ensured this would be a case.
        let next_map = cur
            .get_mut(key)
            .ok_or(NestedError::Config(ConfigError::KeyNotFound))?
            .as_object_mut()
            .ok_or(NestedError::LiteralExpectedMap)?;
        nested_remove(next_map, remaining_keys[0], &remaining_keys[1..])?;
        if next_map.len() == 0 {
            cur.remove(key).ok_or(NestedError::CurrentKeyNotFound)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn test_try_recursive_map_object_error() {
        let value = json!({"a": 1, "b": 2});
        let res = value.try_recursive_map(&|v| {
            if v == 2 { Err(crate::api::ConfigError::KeyNotFound) } else { Ok(Some(v)) }
        });
        assert!(res.is_err());
    }

    #[test]
    fn test_try_recursive_map_array_error() {
        let value = json!([1, 2, 3]);
        let res = value.try_recursive_map(&|v| {
            if v == 2 { Err(crate::api::ConfigError::KeyNotFound) } else { Ok(Some(v)) }
        });
        assert!(res.is_err());
    }

    #[test]
    fn test_try_recursive_map_nested_error() {
        let value = json!({"a": 1, "b": [2, 3]});
        let res = value.try_recursive_map(&|v| {
            if v == 2 { Err(crate::api::ConfigError::KeyNotFound) } else { Ok(Some(v)) }
        });
        assert!(res.is_err());
    }

    #[test]
    fn test_try_recursive_map_object_success() {
        let value = json!({"a": 1, "b": 2});
        let res: Result<_, crate::api::ConfigError> = value.try_recursive_map(&|v| {
            if v.is_object() {
                Ok(Some(v))
            } else {
                Ok(Some(v.as_u64().map(|x| x + 1).unwrap().into()))
            }
        });
        assert_eq!(res.unwrap(), Some(json!({"a": 2, "b": 3})));
    }

    #[test]
    fn test_try_recursive_map_array_success() {
        let value = json!([1, 2, 3]);
        let res: Result<_, crate::api::ConfigError> = value.try_recursive_map(&|v| {
            if v.is_array() {
                Ok(Some(v))
            } else {
                Ok(Some(v.as_u64().map(|x| x + 1).unwrap().into()))
            }
        });
        assert_eq!(res.unwrap(), Some(json!([2, 3, 4])));
    }

    #[test]
    fn test_try_recursive_map_nested_success() {
        let value = json!({"a": 1, "b": [2, 3]});
        let res: Result<_, crate::api::ConfigError> = value.try_recursive_map(&|v| {
            if v.is_object() || v.is_array() {
                Ok(Some(v))
            } else {
                Ok(Some(v.as_u64().map(|x| x + 1).unwrap().into()))
            }
        });
        assert_eq!(res.unwrap(), Some(json!({"a": 2, "b": [3, 4]})));
    }
}
