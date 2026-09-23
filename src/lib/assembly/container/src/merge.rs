// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub fn try_merge_into<T: DeserializeOwned>(base: Value, overrides: Value) -> Result<T> {
    let merged = merge(base, overrides);
    Ok(serde_json::from_value(merged)?)
}

fn merge(base: Value, value: Value) -> Value {
    merge_or_append(base, value, /*append=*/ false)
}

fn merge_or_append(base: Value, value: Value, append: bool) -> Value {
    match (base, value, append) {
        (base, Value::Null, _) => {
            // Override value is nothing, so nothing to do.
            base
        }
        (Value::Object(mut merged), Value::Object(overrides), _) => {
            // Both are maps/dicts, so do a key-wise merging.
            for (key, value) in overrides {
                let (key, append) = if let Some(real_key) = key.strip_prefix("__append_to_") {
                    (real_key.to_string(), true)
                } else {
                    (key, false)
                };

                // Remove the existing value, so that it can be merged before re-inserting.
                let existing = merged.remove(&key);
                match existing {
                    Some(existing) => {
                        // If there's already a value, recursively merge the override value.
                        merged.insert(key, merge_or_append(existing, value, append));
                    }
                    None => {
                        // If there's not an existing value, just use the override value.
                        merged.insert(key, value);
                    }
                }
            }
            Value::Object(merged)
        }
        (Value::Array(mut merged), Value::Array(mut overrides), /*append=*/ true) => {
            merged.append(&mut overrides);
            Value::Array(merged)
        }
        (_, value, _) => {
            // Either the override or the base value is a non-mergeable type,
            // so just return the override value.
            value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_both_null() {
        let base = Value::Null;
        let value = Value::Null;

        assert_eq!(Value::Null, merge(base, value))
    }

    #[test]
    fn test_null_override_is_no_op() {
        let base = json!("a string");
        assert_eq!("a string", merge(base, Value::Null));

        let base = json!(1);
        assert_eq!(1, merge(base, Value::Null));

        let base = json!(true);
        assert_eq!(true, merge(base, Value::Null));

        let base = json!(["item1", "item2"]);
        assert_eq!(json!(["item1", "item2"]), merge(base, Value::Null));
    }

    #[test]
    fn test_null_base_is_overridden() {
        let value = json!("a string");
        assert_eq!("a string", merge(Value::Null, value));
    }

    #[test]
    fn test_scalar_value_is_overriden_by_scalar() {
        assert_eq!(json!("override"), merge(json!(42), json!("override")));
        assert_eq!(json!(36), merge(json!(4), json!(36)));
        assert_eq!(json!(42), merge(json!(false), json!(42)));
        assert_eq!(json!(false), merge(json!(3), json!(false)));
        assert_eq!(json!(["item1", "item2"]), merge(json!(39), json!(["item1", "item2"])));
    }

    #[test]
    fn test_object_overrides_scalar_or_list() {
        let value = json!({
          "key": "value"
        });

        assert_eq!(value, merge(Value::Null, value.clone()));
        assert_eq!(value, merge(json!(true), value.clone()));
        assert_eq!(value, merge(json!(42), value.clone()));
        assert_eq!(value, merge(json!("a string"), value.clone()));
        assert_eq!(value, merge(json!(["item1", "item2"]), value.clone()));
    }

    #[test]
    fn test_merged_keys() {
        assert_eq!(
            json!({
              "base_key": "a string",
              "added_key": "some value"
            }),
            merge(
                json!({
                  "base_key": "a string"
                }),
                json!({
                  "added_key": "some value"
                })
            )
        );
    }

    #[test]
    fn test_merged_keys_adds_object() {
        assert_eq!(
            json!({
              "base_key": "a string",
              "added_key": {
                "inner_key": "some value"
              }
            }),
            merge(
                json!({
                  "base_key": "a string"
                }),
                json!({
                  "added_key": {
                    "inner_key": "some value"
                  }
                })
            )
        );
    }

    #[test]
    fn test_merge_overrides_values() {
        assert_eq!(
            json!({
              "key1": "key1 value",
              "key2": "key2 new value"
            }),
            merge(
                json!({
                  "key1": "key1 value",
                  "key2": "key2 value"
                }),
                json!({
                  "key2": "key2 new value"
                })
            )
        );
    }

    #[test]
    fn test_merge_overrides_nested_value() {
        assert_eq!(
            json!({
              "key1": "key1 value",
              "key2": {
                "inner1": "inner 1 value",
                "inner2": "inner 2 new value"
              }
            }),
            merge(
                json!({
                  "key1": "key1 value",
                  "key2": {
                    "inner1": "inner 1 value",
                    "inner2": "inner 2 value"
                  }
                }),
                json!({
                  "key2": { "inner2": "inner 2 new value" }
                })
            )
        );
    }

    #[test]
    fn test_merge_adds_empty_objects() {
        assert_eq!(
            json!({
              "key1": "key1 value",
              "key2": {}
            }),
            merge(
                json!({
                  "key1": "key1 value"
                }),
                json!({
                  "key2": {}
                })
            )
        );
    }

    #[test]
    fn test_merge_override_list() {
        assert_eq!(
            json!({
                "key1": [ "value2" ],
            }),
            merge(
                json!({
                    "key1": [ "value1" ],
                }),
                json!({
                    "key1": [ "value2" ],
                })
            )
        );
    }

    #[test]
    fn test_merge_append_lists() {
        assert_eq!(
            json!({
                "key1": [ "value1", "value2" ],
            }),
            merge(
                json!({
                    "key1": [ "value1" ],
                }),
                json!({
                    "__append_to_key1": [ "value2" ],
                })
            )
        );
    }
}
