// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{Error, Result};
use crate::handle::{Channel, NullableHandle};
use fidl_data_zx::{ObjType as ObjectType, Rights};

/// An empty type.
#[derive(Debug)]
pub enum Forbid {}

impl std::fmt::Display for Forbid {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unreachable!()
    }
}

/// Our internal neutral form for FIDL values.
#[derive(Debug)]
pub enum Value<OutOfLine = Forbid> {
    Null,
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Object(Vec<(String, Self)>),
    Bits(String, Box<Self>),
    Enum(String, Box<Self>),
    Union(String, String, Box<Self>),
    List(Vec<Self>),
    ServerEnd(Channel, String, Option<Rights>),
    ClientEnd(Channel, String, Option<Rights>),
    Handle(NullableHandle, ObjectType, Option<Rights>),
    OutOfLine(OutOfLine),
}

impl Value {
    /// Get this value as a u64 if it is any integer type. Useful for treating values as bit masks,
    /// as we'd like to for the Bits type.
    pub fn bits(&self) -> Option<u64> {
        match self {
            Value::U8(x) => Some(*x as u64),
            Value::U16(x) => Some(*x as u64),
            Value::U32(x) => Some(*x as u64),
            Value::U64(x) => Some(*x),
            Value::I8(x) => Some(u8::from_le_bytes(x.to_le_bytes()) as u64),
            Value::I16(x) => Some(u16::from_le_bytes(x.to_le_bytes()) as u64),
            Value::I32(x) => Some(u32::from_le_bytes(x.to_le_bytes()) as u64),
            Value::I64(x) => Some(u64::from_le_bytes(x.to_le_bytes())),
            _ => None,
        }
    }

    /// See if this value is equal to another. Unlike our PartialEq implementation, this will
    /// upcast numeric types and compare them.
    pub(crate) fn cast_equals(&self, other: &Value) -> bool {
        match self {
            Value::U8(x) => u8::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::U16(x) => u16::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::U32(x) => u32::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::U64(x) => u64::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::I8(x) => i8::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::I16(x) => i16::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::I32(x) => i32::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::I64(x) => i64::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::F32(x) => f32::try_from(other).map(|y| *x == y).unwrap_or(false),
            Value::F64(x) => f64::try_from(other).map(|y| *x == y).unwrap_or(false),
            _ => self == other,
        }
    }

    /// Cast a `Value<Forbid>` to a `Value<T>` for any `T`
    pub fn upcast<T>(self) -> Value<T> {
        match self {
            Value::Null => Value::Null,
            Value::Bool(a) => Value::Bool(a),
            Value::U8(a) => Value::U8(a),
            Value::U16(a) => Value::U16(a),
            Value::U32(a) => Value::U32(a),
            Value::U64(a) => Value::U64(a),
            Value::I8(a) => Value::I8(a),
            Value::I16(a) => Value::I16(a),
            Value::I32(a) => Value::I32(a),
            Value::I64(a) => Value::I64(a),
            Value::F32(a) => Value::F32(a),
            Value::F64(a) => Value::F64(a),
            Value::String(a) => Value::String(a),
            Value::Object(a) => {
                Value::Object(a.into_iter().map(|(a, b)| (a, b.upcast())).collect())
            }
            Value::Bits(a, b) => Value::Bits(a, Box::new(b.upcast())),
            Value::Enum(a, b) => Value::Enum(a, Box::new(b.upcast())),
            Value::Union(a, b, c) => Value::Union(a, b, Box::new(c.upcast())),
            Value::List(a) => Value::List(a.into_iter().map(Value::upcast).collect()),
            Value::ServerEnd(a, b, c) => Value::ServerEnd(a, b, c),
            Value::ClientEnd(a, b, c) => Value::ClientEnd(a, b, c),
            Value::Handle(a, b, c) => Value::Handle(a, b, c),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(l0), Self::Bool(r0)) => l0 == r0,
            (Self::U8(l0), Self::U8(r0)) => l0 == r0,
            (Self::U16(l0), Self::U16(r0)) => l0 == r0,
            (Self::U32(l0), Self::U32(r0)) => l0 == r0,
            (Self::U64(l0), Self::U64(r0)) => l0 == r0,
            (Self::I8(l0), Self::I8(r0)) => l0 == r0,
            (Self::I16(l0), Self::I16(r0)) => l0 == r0,
            (Self::I32(l0), Self::I32(r0)) => l0 == r0,
            (Self::I64(l0), Self::I64(r0)) => l0 == r0,
            (Self::F32(l0), Self::F32(r0)) => l0 == r0,
            (Self::F64(l0), Self::F64(r0)) => l0 == r0,
            (Self::String(l0), Self::String(r0)) => l0 == r0,
            (Self::Object(l0), Self::Object(r0)) => {
                let mut values = std::collections::HashMap::new();
                for (k, v) in l0 {
                    if values.insert(k, v).is_some() {
                        return false;
                    }
                }
                for (k, v) in r0 {
                    let Some(other) = values.remove(&k) else {
                        return false;
                    };

                    if v != other {
                        return false;
                    }
                }
                true
            }
            (Self::Bits(l0, l1), Self::Bits(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::Enum(l0, l1), Self::Enum(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::Union(l0, l1, l2), Self::Union(r0, r1, r2)) => l0 == r0 && l1 == r1 && l2 == r2,
            (Self::List(l0), Self::List(r0)) => l0 == r0,
            (Self::ServerEnd(l0, l1, l2), Self::ServerEnd(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::ClientEnd(l0, l1, l2), Self::ClientEnd(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::Handle(l0, l1, l2), Self::Handle(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Value {
        Value::from(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Value {
        Value::String(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Value {
        Value::Bool(v)
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Value {
        Value::U8(v)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Value {
        Value::U16(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Value {
        Value::U32(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Value {
        Value::U64(v)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Value {
        Value::I8(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Value {
        Value::I16(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Value {
        Value::I32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Value {
        Value::I64(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Value {
        Value::F32(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::F64(v)
    }
}

impl<T> From<Vec<T>> for Value
where
    Value: From<T>,
{
    fn from(v: Vec<T>) -> Value {
        Value::List(v.into_iter().map(Value::from).collect())
    }
}

impl<T> From<Option<T>> for Value
where
    Value: From<T>,
{
    fn from(v: Option<T>) -> Value {
        v.map(Value::from).unwrap_or(Value::Null)
    }
}

impl<A> FromIterator<A> for Value
where
    Value: From<A>,
{
    fn from_iter<T: IntoIterator<Item = A>>(iter: T) -> Value {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

macro_rules! try_from_int {
    ($int:ident) => {
        impl TryFrom<&mut Value> for $int {
            type Error = $crate::error::Error;

            fn try_from(value: &mut Value) -> Result<$int> {
                Self::try_from(&*value)
            }
        }

        impl TryFrom<&Value> for $int {
            type Error = $crate::error::Error;

            fn try_from(value: &Value) -> Result<$int> {
                match value {
                    Value::U8(x) => $int::try_from(*x).ok(),
                    Value::U16(x) => $int::try_from(*x).ok(),
                    Value::U32(x) => $int::try_from(*x).ok(),
                    Value::U64(x) => $int::try_from(*x).ok(),
                    Value::I8(x) => $int::try_from(*x).ok(),
                    Value::I16(x) => $int::try_from(*x).ok(),
                    Value::I32(x) => $int::try_from(*x).ok(),
                    Value::I64(x) => $int::try_from(*x).ok(),
                    _ => None,
                }
                .ok_or(Error::ValueError(format!("Cannot convert to {}", stringify!($int))))
            }
        }

        impl TryFrom<Value> for $int {
            type Error = $crate::error::Error;

            fn try_from(value: Value) -> Result<$int> {
                $int::try_from(&value)
            }
        }
    };
}

try_from_int!(u8);
try_from_int!(u16);
try_from_int!(u32);
try_from_int!(u64);
try_from_int!(i8);
try_from_int!(i16);
try_from_int!(i32);
try_from_int!(i64);

impl TryFrom<&mut Value> for bool {
    type Error = crate::error::Error;

    fn try_from(value: &mut Value) -> Result<bool> {
        bool::try_from(&*value)
    }
}

impl TryFrom<&Value> for bool {
    type Error = crate::error::Error;

    fn try_from(value: &Value) -> Result<bool> {
        match value {
            Value::Bool(x) => Ok(*x),
            Value::U8(x) => Ok(*x != 0),
            Value::U16(x) => Ok(*x != 0),
            Value::U32(x) => Ok(*x != 0),
            Value::U64(x) => Ok(*x != 0),
            Value::I8(x) => Ok(*x != 0),
            Value::I16(x) => Ok(*x != 0),
            Value::I32(x) => Ok(*x != 0),
            Value::I64(x) => Ok(*x != 0),
            _ => Err(Error::ValueError(format!("Cannot convert {} to bool", value))),
        }
    }
}

impl TryFrom<Value> for bool {
    type Error = Error;

    fn try_from(value: Value) -> Result<bool> {
        bool::try_from(&value)
    }
}

impl TryFrom<&mut Value> for f32 {
    type Error = crate::error::Error;

    fn try_from(value: &mut Value) -> Result<f32> {
        f32::try_from(&*value)
    }
}

impl TryFrom<&Value> for f32 {
    type Error = Error;

    fn try_from(value: &Value) -> Result<f32> {
        match value {
            Value::U8(x) => Ok(*x as f32),
            Value::U16(x) => Ok(*x as f32),
            Value::U32(x) => Ok(*x as f32),
            Value::U64(x) => Ok(*x as f32),
            Value::I8(x) => Ok(*x as f32),
            Value::I16(x) => Ok(*x as f32),
            Value::I32(x) => Ok(*x as f32),
            Value::I64(x) => Ok(*x as f32),
            Value::F32(x) => Ok(*x),
            Value::F64(x) => Ok(*x as f32),
            _ => Err(Error::ValueError(format!("Cannot convert {} to f32", value))),
        }
    }
}

impl TryFrom<Value> for f32 {
    type Error = Error;

    fn try_from(value: Value) -> Result<f32> {
        f32::try_from(&value)
    }
}

impl TryFrom<&mut Value> for f64 {
    type Error = crate::error::Error;

    fn try_from(value: &mut Value) -> Result<f64> {
        f64::try_from(&*value)
    }
}

impl TryFrom<&Value> for f64 {
    type Error = Error;

    fn try_from(value: &Value) -> Result<f64> {
        match value {
            Value::U8(x) => Ok(*x as f64),
            Value::U16(x) => Ok(*x as f64),
            Value::U32(x) => Ok(*x as f64),
            Value::U64(x) => Ok(*x as f64),
            Value::I8(x) => Ok(*x as f64),
            Value::I16(x) => Ok(*x as f64),
            Value::I32(x) => Ok(*x as f64),
            Value::I64(x) => Ok(*x as f64),
            Value::F32(x) => Ok(*x as f64),
            Value::F64(x) => Ok(*x),
            _ => Err(Error::ValueError(format!("Cannot convert {} to f32", value))),
        }
    }
}

impl TryFrom<Value> for f64 {
    type Error = Error;

    fn try_from(value: Value) -> Result<f64> {
        f64::try_from(&value)
    }
}

macro_rules! write_list {
    ($f:ident, $pattern:pat, $members:expr, $element:expr, $open:expr, $close:expr) => {{
        $open;
        let mut first = true;
        for $pattern in $members {
            if !first {
                write!($f, ", ")?
            } else {
                first = false;
                write!($f, " ")?;
            }

            $element;
        }

        if !first {
            write!($f, "{}", " ")?;
        }

        write!($f, "{}", $close)
    }};
}

impl<T: std::fmt::Display> std::fmt::Display for Value<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Value::*;
        match self {
            Null => write!(f, "null"),
            Bool(true) => write!(f, "true"),
            Bool(false) => write!(f, "false"),
            U8(x) => write!(f, "{}u8", x),
            U16(x) => write!(f, "{}u16", x),
            U32(x) => write!(f, "{}u32", x),
            U64(x) => write!(f, "{}u64", x),
            I8(x) => write!(f, "{}i8", x),
            I16(x) => write!(f, "{}i16", x),
            I32(x) => write!(f, "{}i32", x),
            I64(x) => write!(f, "{}i64", x),
            F32(x) => write!(f, "{}f32", x),
            F64(x) => write!(f, "{}f64", x),
            String(x) => write!(f, "\"{}\"", x),
            Object(members) => write_list!(
                f,
                (name, value),
                members,
                write!(f, "\"{}\": {}", name, value)?,
                write!(f, "{}", "{")?,
                "}"
            ),
            Bits(ty, value) => write!(f, "{}({})", ty, value),
            Enum(ty, value) => write!(f, "{}({})", ty, value),
            Union(name, field, value) => write!(f, "{}::{}({})", name, field, value),
            List(values) => {
                write_list!(f, value, values, write!(f, "{}", value)?, write!(f, "[")?, "]")
            }
            ServerEnd(_, ty, _) => write!(f, "ServerEnd<{ty}>"),
            ClientEnd(_, ty, _) => write!(f, "ClientEnd<{ty}>"),
            Handle(_, ty, _) => write!(f, "<{:?}>", ty),
            OutOfLine(t) => write!(f, "{t}"),
        }
    }
}

/// Helper macro for constructing `Value` objects. Object body is similar to a Rust struct body, and
/// field values are automatically converted to `Value` via the `From` trait.
#[macro_export]
macro_rules! vobject {
    ($name:ident $($tokens:tt)*) => {{
        vobject!((stringify!($name)) $($tokens)*)
    }};
    (($name:expr) $($tokens:tt)*) => {{
        #[allow(unused_mut)]
        let mut items = Vec::<(String, $crate::value::Value)>::new();
        vobject!(~private items | ($name) $($tokens)*);
        $crate::value::Value::Struct(items)
    }};
    () => {{ $crate::value::Value::Struct(Vec::new()) }};

    (~private $items:path | $name:ident $($tokens:tt)*) => {
        vobject!(~private $items | (stringify!($name)) $($tokens)*);
    };
    (~private $items:path | ($name:expr) : $value:expr , $($excess:tt)*) => {
        $items.push(($name.to_string(), $crate::value::Value::from($value)));
        vobject!(~private $items | $($excess)*);
    };
    (~private $items:path | ($name:expr) : $value:expr) => {
        vobject!(~private $items | ($name) : $value ,);
    };
    (~private $items:path |) => {};
}

/// Helper macro for constructing `Value::Union` values.
#[macro_export]
macro_rules! vunion {
    ($name:expr, $variant:expr, $value:expr) => {{ $crate::value::Value::Union($name.to_owned(), $variant.to_owned(), Box::new($value.into())) }};
}
