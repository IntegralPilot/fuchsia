// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Indented serializer tailored for human-readable display of input
//! report descriptors and reports.
//
// # Design Goals
// - **Maximize signal-to-noise ratio for human eyes**: Scope is defined purely
//   by indentation, eliminating closing delimiters (`}`, `]`) and placeholder
//   values (such as `None` or `null`) to keep terminal logs concise and easily
//   scannable.
// - **Serde compatibility**: Implements the [`serde::Serializer`] trait so any
//   type deriving [`serde::Serialize`] can be formatted seamlessly without
//   custom formatting boilerplate.
//
// # Non-Goals
// - **General-purpose interchange format**: This serializer is not designed for
//   any purpose other than human-readable display of input report descriptors
//   and reports.
// - **Machine parsing and deserialization**: Output is strictly optimized for
//   one-way human visual inspection rather than bidirectional round-tripping.

use serde::{Serialize, ser};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

const INDENT: &str = "  ";

/// A custom Serde serializer that writes values as an indented string format
/// specifically designed for readable output of input descriptors and reports.
///
/// Users should not use this type directly. Instead, call [`serialize_indented`]
/// which sets up the serializer and returns the formatted string.
struct IndentedSerializer {
    output: String,
    indent_level: usize,
}

impl IndentedSerializer {
    pub fn new(indent_level: usize) -> Self {
        IndentedSerializer { output: String::new(), indent_level }
    }

    fn write_indent(&mut self) {
        let indent = INDENT.repeat(self.indent_level);
        self.output.push_str(&indent);
    }
}

#[derive(Debug)]
pub struct IndentedSerializerError(String);

impl Display for IndentedSerializerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for IndentedSerializerError {}

impl ser::Error for IndentedSerializerError {
    fn custom<T: Display>(msg: T) -> Self {
        IndentedSerializerError(msg.to_string())
    }
}

/// Serializes `value` into an indented string format.
///
/// # Formatting Behavior & Examples
///
/// ### Built-in Types
/// Numbers, unit enums, booleans and strings are formatted inline:
/// ```text
/// 42           => "42"
/// "hello"      => "hello"
/// true         => "true"
/// Key::Control => "Control"
/// ```
///
/// ### Structs
/// The first line contains the struct name, followed by its fields on subsequent lines indented
/// by 2 spaces (`"  "`):
/// ```text
/// struct BasicStruct { a: 1, b: "two" }
/// =>
/// BasicStruct
///   a: 1
///   b: two
/// ```
///
/// Nested structs have their struct name on the field line, with inner fields further indented:
/// ```text
/// struct Outer { inner: Inner { x: 42 } }
/// =>
/// Outer
///   inner: Inner
///     x: 42
/// ```
///
/// Empty structs output only their struct name:
/// ```text
/// struct EmptyStruct {}
/// =>
/// EmptyStruct
/// ```
///
/// ### Lists / Sequences
/// The first line contains `List`. Elements are 0-indexed as `#<idx>: <value>` and indented
/// by 2 spaces relative to the sequence header:
/// ```text
/// vec![1, 2, 3]
/// =>
/// List
///   #0: 1
///   #1: 2
///   #2: 3
/// ```
///
/// Lists inside structs:
/// ```text
/// struct SeqStruct { items: vec![1, 2, 3] }
/// =>
/// SeqStruct
///   items: List
///     #0: 1
///     #1: 2
///     #2: 3
/// ```
///
/// Lists of structs:
/// ```text
/// struct Outer { items: vec![Inner { x: 1 }, Inner { x: 2 }] }
/// =>
/// Outer
///   items: List
///     #0: Inner
///       x: 1
///     #1: Inner
///       x: 2
/// ```
///
/// Nested lists:
/// ```text
/// vec![vec![1, 2], vec![3, 4]]
/// =>
/// List
///   #0: List
///     #0: 1
///     #1: 2
///   #1: List
///     #0: 3
///     #1: 4
/// ```
///
/// Empty lists output `List`:
/// ```text
/// vec![]
/// =>
/// List
/// ```
///
/// ### Option & None
/// `Option::None` serializes to an empty string and is automatically omitted when serializing
/// struct fields:
/// ```text
/// struct Outer { optional: None, required: Inner { x: 42 } }
/// =>
/// Outer
///   required: Inner
///     x: 42
/// ```
pub fn serialize_indented<T>(value: &T) -> Result<String, IndentedSerializerError>
where
    T: Serialize,
{
    let mut serializer = IndentedSerializer::new(0);
    value.serialize(&mut serializer)?;
    Ok(serializer.output)
}

impl<'a> ser::Serializer for &'a mut IndentedSerializer {
    type Ok = ();
    type Error = IndentedSerializerError;
    type SerializeSeq = SeqSerializer<'a>;
    type SerializeTuple = ser::Impossible<(), IndentedSerializerError>;
    type SerializeTupleStruct = ser::Impossible<(), IndentedSerializerError>;
    type SerializeTupleVariant = ser::Impossible<(), IndentedSerializerError>;
    type SerializeMap = ser::Impossible<(), IndentedSerializerError>;
    type SerializeStruct = Self;
    type SerializeStructVariant = ser::Impossible<(), IndentedSerializerError>;

    fn serialize_bool(self, v: bool) -> Result<(), IndentedSerializerError> {
        self.output.push_str(&v.to_string());
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> Result<(), IndentedSerializerError> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i16(self, v: i16) -> Result<(), IndentedSerializerError> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i32(self, v: i32) -> Result<(), IndentedSerializerError> {
        self.serialize_i64(i64::from(v))
    }
    fn serialize_i64(self, v: i64) -> Result<(), IndentedSerializerError> {
        self.output.push_str(&v.to_string());
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> Result<(), IndentedSerializerError> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u16(self, v: u16) -> Result<(), IndentedSerializerError> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u32(self, v: u32) -> Result<(), IndentedSerializerError> {
        self.serialize_u64(u64::from(v))
    }
    fn serialize_u64(self, v: u64) -> Result<(), IndentedSerializerError> {
        self.output.push_str(&v.to_string());
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), IndentedSerializerError> {
        self.serialize_f64(f64::from(v))
    }
    fn serialize_f64(self, v: f64) -> Result<(), IndentedSerializerError> {
        self.output.push_str(&v.to_string());
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<(), IndentedSerializerError> {
        self.output.push(v);
        Ok(())
    }
    fn serialize_str(self, v: &str) -> Result<(), IndentedSerializerError> {
        self.output.push_str(v);
        Ok(())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<(), IndentedSerializerError> {
        Err(IndentedSerializerError("bytes not supported".into()))
    }
    fn serialize_none(self) -> Result<(), IndentedSerializerError> {
        Ok(())
    }
    fn serialize_some<T>(self, value: &T) -> Result<(), IndentedSerializerError>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), IndentedSerializerError> {
        Err(IndentedSerializerError("unit not supported".into()))
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), IndentedSerializerError> {
        Err(IndentedSerializerError("unit struct not supported".into()))
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<(), IndentedSerializerError> {
        self.output.push_str(variant);
        Ok(())
    }
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), IndentedSerializerError>
    where
        T: ?Sized + Serialize,
    {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), IndentedSerializerError>
    where
        T: ?Sized + Serialize,
    {
        self.output.push_str(variant);
        self.output.push('(');
        value.serialize(&mut *self)?;
        self.output.push(')');
        Ok(())
    }
    fn serialize_seq(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeSeq, IndentedSerializerError> {
        self.output.push_str("List");
        Ok(SeqSerializer { writer: self, index: 0 })
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, IndentedSerializerError> {
        Err(IndentedSerializerError("tuple not supported".into()))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, IndentedSerializerError> {
        Err(IndentedSerializerError("tuple struct not supported".into()))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, IndentedSerializerError> {
        Err(IndentedSerializerError("tuple variant not supported".into()))
    }
    fn serialize_map(
        self,
        _len: Option<usize>,
    ) -> Result<Self::SerializeMap, IndentedSerializerError> {
        Err(IndentedSerializerError("map not supported".into()))
    }
    fn serialize_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, IndentedSerializerError> {
        self.output.push_str(name);
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, IndentedSerializerError> {
        Err(IndentedSerializerError("struct variant not supported".into()))
    }
}

pub struct SeqSerializer<'a> {
    writer: &'a mut IndentedSerializer,
    index: usize,
}

impl<'a> ser::SerializeSeq for SeqSerializer<'a> {
    type Ok = ();
    type Error = IndentedSerializerError;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), IndentedSerializerError>
    where
        T: ?Sized + Serialize,
    {
        let mut temp_writer = IndentedSerializer::new(self.writer.indent_level + 1);
        value.serialize(&mut temp_writer)?;
        let result = temp_writer.output;

        if result.is_empty() {
            return Ok(());
        }

        self.writer.output.push('\n');
        self.writer.write_indent();
        self.writer.output.push_str(INDENT);
        self.writer.output.push_str(&format!("#{}: ", self.index));
        self.writer.output.push_str(&result);

        self.index += 1;
        Ok(())
    }

    fn end(self) -> Result<(), IndentedSerializerError> {
        Ok(())
    }
}

impl<'a> ser::SerializeStruct for &'a mut IndentedSerializer {
    type Ok = ();
    type Error = IndentedSerializerError;

    fn serialize_field<T>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), IndentedSerializerError>
    where
        T: ?Sized + Serialize,
    {
        let mut temp_writer = IndentedSerializer::new(self.indent_level + 1);
        value.serialize(&mut temp_writer)?;
        let result = temp_writer.output;

        if result.is_empty() {
            return Ok(());
        }

        self.output.push('\n');
        self.write_indent();
        self.output.push_str(INDENT);
        self.output.push_str(key);
        self.output.push_str(": ");
        self.output.push_str(&result);

        Ok(())
    }

    fn end(self) -> Result<(), IndentedSerializerError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[test]
    fn serialize_indented_test_basic_types() {
        assert_eq!(serialize_indented(&42).unwrap(), "42");
        assert_eq!(serialize_indented(&"hello").unwrap(), "hello");
        assert_eq!(serialize_indented(&true).unwrap(), "true");
    }

    #[test]
    fn serialize_indented_test_lists() {
        let list = vec![1, 2, 3];
        #[rustfmt::skip]
        let expected = vec![
            "List",
            "  #0: 1",
            "  #1: 2",
            "  #2: 3",
        ].join("\n");
        assert_eq!(serialize_indented(&list).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_empty_list() {
        let list: Vec<u32> = vec![];
        assert_eq!(serialize_indented(&list).unwrap(), "List");
    }

    #[test]
    fn serialize_indented_test_structs_with_basic_data_types() {
        #[derive(Serialize)]
        struct BasicStruct {
            a: u32,
            b: String,
        }
        let s = BasicStruct { a: 1, b: "two".to_string() };
        #[rustfmt::skip]
        let expected = vec![
            "BasicStruct",
            "  a: 1",
            "  b: two",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_empty_struct() {
        #[derive(Serialize)]
        struct EmptyStruct {}
        let s = EmptyStruct {};
        assert_eq!(serialize_indented(&s).unwrap(), "EmptyStruct");
    }

    #[test]
    fn serialize_indented_test_structs_with_struct_field() {
        #[derive(Serialize)]
        struct Inner {
            x: u32,
        }
        #[derive(Serialize)]
        struct Outer {
            inner: Inner,
        }
        let s = Outer { inner: Inner { x: 42 } };
        #[rustfmt::skip]
        let expected = vec![
            "Outer",
            "  inner: Inner",
            "    x: 42",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_structs_with_option_field() {
        #[derive(Serialize)]
        struct Inner {
            x: u32,
        }
        #[derive(Serialize)]
        struct Outer {
            optional: Option<Inner>,
            required: Inner,
        }
        let s = Outer { optional: None, required: Inner { x: 42 } };
        #[rustfmt::skip]
        let expected = vec![
            "Outer",
            "  required: Inner",
            "    x: 42",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_structs_with_skip_serializing_if() {
        #[derive(Serialize)]
        struct WithSkipIf {
            #[serde(skip_serializing_if = "Option::is_none")]
            optional: Option<u32>,
            required: u32,
        }
        let s = WithSkipIf { optional: None, required: 99 };
        #[rustfmt::skip]
        let expected = vec![
            "WithSkipIf",
            "  required: 99",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_structs_with_seq_of_basic_type_values() {
        #[derive(Serialize)]
        struct SeqStruct {
            items: Vec<u32>,
        }
        let s = SeqStruct { items: vec![1, 2, 3] };
        #[rustfmt::skip]
        let expected = vec![
            "SeqStruct",
            "  items: List",
            "    #0: 1",
            "    #1: 2",
            "    #2: 3",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_unit_struct() {
        #[derive(Serialize)]
        struct Val(String);

        let unit_struct = Val("hello".to_string());
        assert_eq!(serialize_indented(&unit_struct).unwrap(), "hello");
    }

    #[test]
    fn serialize_indented_test_unit_enum() {
        #[derive(Serialize)]
        #[expect(dead_code, reason = "Test enum with unused variants")]
        enum Enum {
            Alpha,
            Beta,
            Gamma,
        }

        let unit_enum = Enum::Alpha;
        assert_eq!(serialize_indented(&unit_enum).unwrap(), "Alpha");
    }

    #[test]
    fn serialize_indented_test_structs_with_seq_of_structs() {
        #[derive(Serialize)]
        struct Inner {
            x: u32,
        }
        #[derive(Serialize)]
        struct Outer {
            items: Vec<Inner>,
        }
        let s = Outer { items: vec![Inner { x: 1 }, Inner { x: 2 }] };
        #[rustfmt::skip]
        let expected = vec![
            "Outer",
            "  items: List",
            "    #0: Inner",
            "      x: 1",
            "    #1: Inner",
            "      x: 2",
        ].join("\n");
        assert_eq!(serialize_indented(&s).unwrap(), expected);
    }

    #[test]
    fn serialize_indented_test_nested_lists() {
        let nested = vec![vec![1, 2], vec![3, 4]];
        #[rustfmt::skip]
        let expected = vec![
            "List",
            "  #0: List",
            "    #0: 1",
            "    #1: 2",
            "  #1: List",
            "    #0: 3",
            "    #1: 4",
        ].join("\n");
        assert_eq!(serialize_indented(&nested).unwrap(), expected);
    }
}
