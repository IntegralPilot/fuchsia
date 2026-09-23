// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_metadata as fmetadata;
use std::collections::BTreeMap;

/// Error type for deserialization failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Missing key in dictionary entry")]
    MissingKey,
    #[error("Missing value in dictionary entry")]
    MissingValue,
    #[error("Custom error: {0}")]
    Custom(String),
}

impl serde::de::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error::Custom(msg.to_string())
    }
}

/// Deserializes a `fuchsia.driver.metadata.Dictionary` into a type `T` that implements `Deserialize`.
pub fn from_dictionary<T>(dict: fmetadata::Dictionary) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    let mut entries = BTreeMap::new();
    if let Some(e) = dict.entries {
        for entry in e {
            entries.insert(entry.key, entry.value);
        }
    }
    let deserializer = DictionaryDeserializer { entries: &entries, prefix: "".to_string() };
    let result = T::deserialize(deserializer)?;
    Ok(result)
}

fn bytes_from_seq<'de, A>(mut seq: A) -> Result<Vec<u8>, A::Error>
where
    A: serde::de::SeqAccess<'de>,
{
    let mut bytes = Vec::new();
    while let Some(val) = seq.next_element::<i64>()? {
        let b = (val as u32).to_be_bytes();
        bytes.extend_from_slice(&b);
    }
    Ok(bytes)
}

/// A wrapper type for a vector of strings deserialized from devicetree properties (integer vectors).
/// Strings are assumed to be null-terminated and concatenated.
#[derive(Debug, PartialEq, Clone)]
pub struct StringVec(pub Vec<String>);

impl<'de> serde::Deserialize<'de> for StringVec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Vec<String>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sequence of integers representing concatenated strings")
            }
            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let bytes = bytes_from_seq(seq)?;
                let strings = bytes
                    .split(|&b| b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .collect();
                Ok(strings)
            }
        }
        Ok(StringVec(deserializer.deserialize_seq(Visitor)?))
    }
}

impl std::ops::Deref for StringVec {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct DictionaryDeserializer<'a> {
    entries: &'a BTreeMap<String, fmetadata::DictionaryValue>,
    prefix: String,
}

impl<'de, 'a> serde::Deserializer<'de> for DictionaryDeserializer<'a> {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        if let Some(val) = self.entries.get(&self.prefix) {
            match val {
                fmetadata::DictionaryValue::Int64(i) => visitor.visit_i64(*i),
                fmetadata::DictionaryValue::Int64Vec(v) => {
                    visitor.visit_seq(SeqDeserializer { vec: v.clone(), index: 0 })
                }
                _ => Err(Error::Custom("Unsupported value type".to_string())),
            }
        } else {
            visitor.visit_map(MapDeserializer::new(self.entries, &self.prefix))
        }
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        if let Some(val) = self.entries.get(&self.prefix) {
            if let fmetadata::DictionaryValue::Int64Vec(v) = val {
                let mut bytes = Vec::new();
                for val in v {
                    let b = (*val as u32).to_be_bytes();
                    bytes.extend_from_slice(&b);
                }
                if let Some(null_pos) = bytes.iter().position(|&b| b == 0) {
                    bytes.truncate(null_pos);
                }
                let s = String::from_utf8_lossy(&bytes).to_string();
                return visitor.visit_string(s);
            }
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        if let Some(val) = self.entries.get(&self.prefix) {
            if let fmetadata::DictionaryValue::Int64Vec(v) = val {
                if v.is_empty() {
                    return visitor.visit_bool(true);
                }
            }
        }
        self.deserialize_any(visitor)
    }

    serde::forward_to_deserialize_any! {
        i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

struct MapDeserializer<'a> {
    entries: &'a BTreeMap<String, fmetadata::DictionaryValue>,
    prefix: String,
    keys: Vec<String>,
    current_index: usize,
}

impl<'a> MapDeserializer<'a> {
    fn new(entries: &'a BTreeMap<String, fmetadata::DictionaryValue>, prefix: &str) -> Self {
        let mut keys = std::collections::HashSet::new();
        let prefix_with_dot =
            if prefix.is_empty() { "".to_string() } else { format!("{}.", prefix) };

        for k in entries.keys() {
            if k.starts_with(&prefix_with_dot) || prefix.is_empty() {
                let stripped =
                    if prefix.is_empty() { k.as_str() } else { &k[prefix_with_dot.len()..] };
                let child_key = match stripped.find('.') {
                    Some(idx) => &stripped[..idx],
                    None => stripped,
                };
                keys.insert(child_key.to_string());
            }
        }
        let mut keys_vec: Vec<String> = keys.into_iter().collect();
        keys_vec.sort();
        Self { entries, prefix: prefix.to_string(), keys: keys_vec, current_index: 0 }
    }
}

impl<'de, 'a> serde::de::MapAccess<'de> for MapDeserializer<'a> {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: serde::de::DeserializeSeed<'de>,
    {
        if self.current_index >= self.keys.len() {
            return Ok(None);
        }
        let key = &self.keys[self.current_index];
        seed.deserialize(KeyDeserializer { key: key.clone() }).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        let key = &self.keys[self.current_index];
        self.current_index += 1;

        let new_prefix =
            if self.prefix.is_empty() { key.clone() } else { format!("{}.{}", self.prefix, key) };

        seed.deserialize(DictionaryDeserializer { entries: self.entries, prefix: new_prefix })
    }
}

struct KeyDeserializer {
    key: String,
}

impl<'de> serde::Deserializer<'de> for KeyDeserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_string(self.key)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

struct SeqDeserializer {
    vec: Vec<i64>,
    index: usize,
}

impl<'de> serde::de::SeqAccess<'de> for SeqDeserializer {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        if self.index >= self.vec.len() {
            return Ok(None);
        }
        let val = self.vec[self.index];
        self.index += 1;
        seed.deserialize(I64Deserializer { val }).map(Some)
    }
}

struct I64Deserializer {
    val: i64,
}

impl<'de> serde::Deserializer<'de> for I64Deserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_i64(self.val)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct MyConfig {
        test_str: String,
        test_int: i64,
        test_vec: StringVec,
        nested: NestedConfig,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct NestedConfig {
        inner_val: String,
    }

    #[test]
    fn test_deserialization() {
        let mut entries = Vec::new();

        // "hello" -> [104, 101, 108, 108], [111, 0, 0, 0]
        let str_val1 = i64::from(u32::from_be_bytes([104, 101, 108, 108]));
        let str_val2 = i64::from(u32::from_be_bytes([111, 0, 0, 0]));

        entries.push(fmetadata::DictionaryEntry {
            key: "test_str".to_string(),
            value: fmetadata::DictionaryValue::Int64Vec(vec![str_val1, str_val2]),
        });
        entries.push(fmetadata::DictionaryEntry {
            key: "test_int".to_string(),
            value: fmetadata::DictionaryValue::Int64(42),
        });

        // "a\0b\0" -> [97, 0, 98, 0]
        let vec_val = i64::from(u32::from_be_bytes([97, 0, 98, 0]));
        entries.push(fmetadata::DictionaryEntry {
            key: "test_vec".to_string(),
            value: fmetadata::DictionaryValue::Int64Vec(vec![vec_val]),
        });

        // "secret" -> [115, 101, 99, 114], [101, 116, 0, 0]
        let sec_val1 = i64::from(u32::from_be_bytes([115, 101, 99, 114]));
        let sec_val2 = i64::from(u32::from_be_bytes([101, 116, 0, 0]));

        entries.push(fmetadata::DictionaryEntry {
            key: "nested.inner_val".to_string(),
            value: fmetadata::DictionaryValue::Int64Vec(vec![sec_val1, sec_val2]),
        });

        let dict = fmetadata::Dictionary { entries: Some(entries), ..Default::default() };

        let config: MyConfig = from_dictionary(dict).unwrap();
        assert_eq!(
            config,
            MyConfig {
                test_str: "hello".to_string(),
                test_int: 42,
                test_vec: StringVec(vec!["a".to_string(), "b".to_string()]),
                nested: NestedConfig { inner_val: "secret".to_string() },
            }
        );
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct BoolConfig {
        test_bool: bool,
    }

    #[test]
    fn test_bool_deserialization() {
        let mut entries = Vec::new();
        entries.push(fmetadata::DictionaryEntry {
            key: "test_bool".to_string(),
            value: fmetadata::DictionaryValue::Int64Vec(vec![]),
        });

        let dict = fmetadata::Dictionary { entries: Some(entries), ..Default::default() };

        let config: BoolConfig = from_dictionary(dict).unwrap();
        assert_eq!(config, BoolConfig { test_bool: true });
    }
}
