// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_data::{Dictionary, DictionaryEntry, DictionaryValue};
use serde::de::value::{Error, MapDeserializer, SeqDeserializer};
use serde::de::{Deserializer, Error as _, IntoDeserializer, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use std::str::FromStr;

/// Deserialize the provided `program` into a value of `T`.
///
/// Allows runners to define their program interface as a Rust struct and to use all of serde's
/// helpers for parsing.
pub fn deserialize_program<'a, T: Deserialize<'a>>(program: &'a Dictionary) -> Result<T, Error> {
    T::deserialize(DictionaryDeserializer::new(program))
}

struct DictionaryDeserializer<'de>(&'de [DictionaryEntry]);

impl<'de> DictionaryDeserializer<'de> {
    fn new(program: &'de Dictionary) -> Self {
        Self(program.entries.as_ref().map(Vec::as_slice).unwrap_or(&[]))
    }
}

impl<'de, 'a> Deserializer<'de> for DictionaryDeserializer<'de> {
    type Error = Error;

    // The FIDL Dictionary is fundamentally a self-describing map type, so all of the other
    // deserialize methods can defer to this without any type hinting.
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Ok(visitor.visit_map(MapDeserializer::new(
            self.0.iter().map(|e| (e.key.as_str(), ValueDeserializer(&e.value))),
        ))?)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
}

impl<'de> IntoDeserializer<'de> for DictionaryDeserializer<'de> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

struct ValueDeserializer<'de>(&'de Option<Box<DictionaryValue>>);

impl<'de> Deserializer<'de> for ValueDeserializer<'de> {
    type Error = Error;

    // Because a DictionaryValue is self-describing we don't need to use any type hints and all
    // other deserialize_* methods can delegate to this.
    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.0 {
            Some(value) => match &**value {
                DictionaryValue::Str(s) => visitor.visit_borrowed_str(s.as_str()),
                DictionaryValue::StrVec(v) => {
                    visitor.visit_seq(SeqDeserializer::new(v.iter().map(String::as_str)))
                }
                DictionaryValue::ObjVec(v) => visitor
                    .visit_seq(SeqDeserializer::new(v.iter().map(DictionaryDeserializer::new))),
                other => {
                    Err(serde::de::value::Error::custom(format!("unknown dict value {other:?}")))
                }
            },
            None => visitor.visit_none(),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // DictionaryValues cannot be null, the only way to express an absent value in Dictionary is
        // to omit the key entirely.
        visitor.visit_some(self)
    }
    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_any(visitor)
    }
}

impl<'de> IntoDeserializer<'de> for ValueDeserializer<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

/// A wrapper type that allows runners to specify that they want a basic field to be stored as
/// a string and deserialized with that type's `FromStr` impl.
// TODO(https://fxbug.dev/397443131) remove once basic types supported by Dictionary
#[derive(Clone, Eq, PartialEq)]
pub struct StoreAsString<T>(pub T);

impl<T> std::ops::Deref for StoreAsString<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: ToString> Serialize for StoreAsString<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for StoreAsString<T>
where
    T: FromStr,
    <T as FromStr>::Err: Display,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let s = String::deserialize(deserializer)?;
        let inner = s.parse().map_err(|e| {
            D::Error::custom(format!(
                "couldn't parse '{s}' as a {}: {e}",
                std::any::type_name::<T>()
            ))
        })?;
        Ok(Self(inner))
    }
}

impl<T> Default for StoreAsString<T>
where
    T: Default,
{
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T> Debug for StoreAsString<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::fmt::Debug;
    use std::sync::Arc;

    #[fuchsia::test]
    fn string_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_str: String,
        }
        let original = Foo { is_a_str: String::from("hello, world!") };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn optional_string_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_str: Option<String>,
        }
        let original = Foo { is_a_str: Some(String::from("hello, world!")) };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn char_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_char: char,
        }
        let original = Foo { is_a_char: 'x' };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn string_list_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_list_of_strings: Vec<String>,
        }
        let original = Foo { is_a_list_of_strings: vec![String::from("foo"), String::from("bar")] };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn object_list_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_list_of_objects: Vec<Bar>,
        }
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Bar {
            is_a_string: String,
        }
        let original = Foo {
            is_a_list_of_objects: vec![
                Bar { is_a_string: String::from("hello, world!") },
                Bar { is_a_string: String::from("another string") },
                Bar { is_a_string: String::from("yet another string") },
            ],
        };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn store_as_string_round_trips() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_an_int: StoreAsString<u32>,
        }
        let original = Foo { is_an_int: StoreAsString(5) };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[fuchsia::test]
    fn optional_store_as_string_round_trips() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            #[serde(skip_serializing_if = "Option::is_none")]
            is_an_int: Option<StoreAsString<u32>>,
        }
        let original_some = Foo { is_an_int: Some(StoreAsString(5)) };
        let roundtripped_some = round_trip_as_program(&original_some).unwrap();
        assert_eq!(original_some, roundtripped_some);

        let original_none = Foo { is_an_int: None };
        let roundtripped_none = round_trip_as_program(&original_none).unwrap();
        assert_eq!(original_none, roundtripped_none);
    }

    #[fuchsia::test]
    fn list_of_store_as_string_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_list: Vec<StoreAsString<u32>>,
        }
        let original_some = Foo { is_a_list: vec![StoreAsString(5), StoreAsString(6)] };
        let roundtripped_some = round_trip_as_program(&original_some).unwrap();
        assert_eq!(original_some, roundtripped_some);

        let original_empty = Foo { is_a_list: vec![] };
        let roundtripped_empty = round_trip_as_program(&original_empty).unwrap();
        assert_eq!(original_empty, roundtripped_empty);
    }

    #[fuchsia::test]
    fn mixed_fields_round_trip() {
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Foo {
            is_a_string: String,
            is_a_list_of_strings: Vec<String>,
            is_a_list_of_objects: Vec<Bar>,
        }
        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct Bar {
            is_a_string: String,
        }
        let original = Foo {
            is_a_string: String::from("hello, world!"),
            is_a_list_of_strings: vec![String::from("foo"), String::from("bar")],
            is_a_list_of_objects: vec![
                Bar { is_a_string: String::from("hello, world!") },
                Bar { is_a_string: String::from("another string") },
                Bar { is_a_string: String::from("yet another string") },
            ],
        };
        let roundtripped = round_trip_as_program(&original).unwrap();
        assert_eq!(original, roundtripped);
    }

    macro_rules! unsupported_type_test {
        ($t:tt) => {
            paste::paste! {
                #[test]
                fn [<$t _not_supported>]() {
                    #[derive(Debug, Deserialize, Serialize)]
                    struct Foo {
                        is_a_value: $t,
                    }
                    round_trip_as_program(&Foo { is_a_value: Default::default() })
                        .expect_err("unsupported types must not be able to roundtrip");
                }
            }
        };
    }

    // TODO(https://fxbug.dev/397443131) support bools natively
    unsupported_type_test!(bool);
    // TODO(https://fxbug.dev/397443131) support i8 natively
    unsupported_type_test!(i8);
    // TODO(https://fxbug.dev/397443131) support i16 natively
    unsupported_type_test!(i16);
    // TODO(https://fxbug.dev/397443131) support i32 natively
    unsupported_type_test!(i32);
    // TODO(https://fxbug.dev/397443131) support i64 natively
    unsupported_type_test!(i64);
    // TODO(https://fxbug.dev/397443131) support u8 natively
    unsupported_type_test!(u8);
    // TODO(https://fxbug.dev/397443131) support u16 natively
    unsupported_type_test!(u16);
    // TODO(https://fxbug.dev/397443131) support u32 natively
    unsupported_type_test!(u32);
    // TODO(https://fxbug.dev/397443131) support u64 natively
    unsupported_type_test!(u64);
    // TODO(https://fxbug.dev/397443131) support f32 natively
    unsupported_type_test!(f32);
    // TODO(https://fxbug.dev/397443131) support f64 natively
    unsupported_type_test!(f64);

    #[track_caller]
    fn round_trip_as_program<T: Debug + DeserializeOwned + Serialize>(t: &T) -> anyhow::Result<T> {
        let file_path: Arc<std::path::Path> =
            Arc::from(std::path::Path::new("/fake/path/for/roundtrip/test"));

        let mut program_json = serde_json::to_value(&t).context("serializing T as json")?;
        program_json
            .as_object_mut()
            .context("test structs must serialize as objects")?
            .insert("runner".to_string(), json!("serde_roundtrip_test"));

        let manifest_str = serde_json::to_string_pretty(&json!({
            "program": program_json,
        }))
        .context("serializing generated document as JSON")?;

        let document = cml::types::document::parse_and_hydrate(file_path, &manifest_str)
            .context("parsing generated document as CML")?;

        let compiled = cml::compile(&document, cml::CompileOptions::default())
            .context("compiling generated CML")?;

        let program = compiled
            .program
            .as_ref()
            .context("getting program block from compiled manifest")?
            .info
            .as_ref()
            .context("getting compiled info from program block")?;

        deserialize_program::<T>(&program).context("deserializing T from program block")
    }
}
