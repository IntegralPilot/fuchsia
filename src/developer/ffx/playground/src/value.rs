// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_codec_fdomain::{AsPlatform as _, ObjectType, Value as FidlValue, library as lib};
use futures::future::BoxFuture;
use num::BigInt;
use num::rational::BigRational;
use num::traits::ToPrimitive;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

use crate::error::Result;

mod in_use_handle;
mod iterator;

pub use in_use_handle::InUseHandle;
pub use iterator::{RangeCursor, ReplayableIterator, ReplayableIteratorCursor};

/// Errors occurring during various value conversions.
#[derive(Error, Debug, Clone)]
pub enum ValueError {
    #[error("Type conversion requires {0} to have a member {1}")]
    MemberMissing(String, String),
    #[error("Struct coerced to union must have exactly one member")]
    StructToUnionWrongNumberOfMembers,
    #[error(
        "Object used in FIDL must be used as a struct, table, or union (Cannot convert {0} to {1})"
    )]
    BadObjectConversion(String, String),
    #[error("Value of type {0} is encoded as bits but isn't bits")]
    BadBitsType(String),
    #[error("Value of type {0} is encoded as an enum but isn't an enum")]
    BadEnumType(String),
    #[error("Value of type {0} is encoded as a union but isn't a union")]
    BadUnionType(String),
    #[error("Union of type {0} has no member {1}")]
    BadUnionMember(String, String),
    #[error("List too short to encode as struct '{0}'")]
    ListStructTooShort(String),
    #[error("Cannot convert list to table because cannot encode a value at ordinal {0}")]
    TableFromListOrdinalMissing(u64),
    #[error("Cannot encode list as type {0:?}")]
    TypeFromList(lib::Type),
    #[error("Overflow converting to {0:?}")]
    ConversionOverflow(lib::Type),
    #[error("Cannot convert type to server for {0}")]
    ServerConversionFailed(String),
    #[error("Cannot convert type to client for {0}")]
    ClientConversionFailed(String),
    #[error("Cannot convert type to socket")]
    SocketConversionFailed,
    #[error("Cannot convert number to {0:?}")]
    NumberConversionFailed(lib::Type),
    #[error("Cannot convert invocable to {0:?}")]
    InvocableConversionFailed(lib::Type),
    #[error("Cannot convert iterator to {0:?}")]
    IteratorConversionFailed(lib::Type),
    #[error("Could not resolve FIDL identifier '{0}': {1}")]
    FIDLLookupFailed(String, Arc<fidl_codec_fdomain::Error>),
    #[error("Cannot convert to FIDL type")]
    MiscConversionFailed,
    #[error("Handle is not a channel")]
    NotChannel,
    #[error("Handle is a {got_ty} for {got_proto} (need {expect_ty} for {expect_proto})")]
    EndpointMismatch {
        got_ty: in_use_handle::EndpointType,
        got_proto: String,
        expect_ty: in_use_handle::EndpointType,
        expect_proto: String,
    },
    #[error("Handle is a {}, (need {})", .0, .0.opposite())]
    EndpointTypeMismatch(in_use_handle::EndpointType),
    #[error("Handle is a {0} for {1}, (need {2})")]
    EndpointProtocolMismatch(in_use_handle::EndpointType, String, String),
    #[error("Could not determine protocol for handle")]
    UndeterminedProtocol,
    #[error("Handle is not a {0}")]
    NotAnEndpoint(in_use_handle::EndpointType),
    #[error("Handle is closed")]
    HandleClosed,
    #[error("Channel is closed")]
    ChannelClosed,
    #[error("Handle is not a socket")]
    NotSocket,
    #[error("Not enough type information to use handle as channel")]
    ChannelTypeUndetermined,
    #[error("Raw channel {0} are unimplemented")]
    RawChannelUnimplemented(&'static str),
    #[error("Handle type is too ambiguous to identify")]
    NoHandleID,
}

/// Combines the [`lib::Type`] object from the FIDL codec with the [`lib::LookupResult`]
/// object. Basically the former contains base types and also handles to more
/// complex types whereas the latter contains detailed specifications of the
/// more complex types.
enum LookupResultOrType<'a> {
    LookupResult(lib::LookupResult<'a>),
    Type(lib::Type),
}

impl<'a> std::fmt::Display for LookupResultOrType<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn nullable_mark(nullable: &bool) -> &'static str {
            if *nullable { "?" } else { "" }
        }

        match self {
            LookupResultOrType::LookupResult(lib::LookupResult::Bits(lib::Bits {
                name, ..
            }))
            | LookupResultOrType::LookupResult(lib::LookupResult::Enum(lib::Enum {
                name, ..
            }))
            | LookupResultOrType::LookupResult(lib::LookupResult::Protocol(lib::Protocol {
                name,
                ..
            }))
            | LookupResultOrType::LookupResult(lib::LookupResult::Struct(lib::Struct {
                name,
                ..
            }))
            | LookupResultOrType::LookupResult(lib::LookupResult::Table(lib::TableOrUnion {
                name,
                ..
            }))
            | LookupResultOrType::LookupResult(lib::LookupResult::Union(lib::TableOrUnion {
                name,
                ..
            })) => {
                write!(f, "{}", name)
            }
            LookupResultOrType::Type(lib::Type::Unknown(_))
            | LookupResultOrType::Type(lib::Type::UnknownString(_)) => write!(f, "<UNKNOWN>"),
            LookupResultOrType::Type(lib::Type::Identifier { name, nullable }) => {
                write!(f, "{}{}", name, nullable_mark(nullable))
            }
            LookupResultOrType::Type(lib::Type::Endpoint {
                protocol,
                role: _,
                rights: _,
                nullable,
            }) => {
                write!(f, "{}{}", protocol, nullable_mark(nullable))
            }
            LookupResultOrType::Type(lib::Type::Bool) => write!(f, "bool"),
            LookupResultOrType::Type(lib::Type::U8) => write!(f, "u8"),
            LookupResultOrType::Type(lib::Type::U16) => write!(f, "u16"),
            LookupResultOrType::Type(lib::Type::U32) => write!(f, "u32"),
            LookupResultOrType::Type(lib::Type::U64) => write!(f, "u64"),
            LookupResultOrType::Type(lib::Type::I8) => write!(f, "i8"),
            LookupResultOrType::Type(lib::Type::I16) => write!(f, "i16"),
            LookupResultOrType::Type(lib::Type::I32) => write!(f, "i32"),
            LookupResultOrType::Type(lib::Type::I64) => write!(f, "i64"),
            LookupResultOrType::Type(lib::Type::F32) => write!(f, "f32"),
            LookupResultOrType::Type(lib::Type::F64) => write!(f, "f64"),
            LookupResultOrType::Type(lib::Type::Array(ty, size)) => {
                write!(f, "{}[{}]", LookupResultOrType::Type((**ty).clone()), size)
            }
            LookupResultOrType::Type(lib::Type::Vector { ty, element_count: _, nullable }) => {
                write!(
                    f,
                    "{}[]{}",
                    LookupResultOrType::Type((**ty).clone()),
                    nullable_mark(nullable)
                )
            }
            LookupResultOrType::Type(lib::Type::String { nullable, byte_count: _ }) => {
                write!(f, "String{}", nullable_mark(nullable))
            }
            LookupResultOrType::Type(lib::Type::Handle { object_type, .. }) => match object_type {
                &ObjectType::Bti => write!(f, "bti"),
                &ObjectType::Channel => write!(f, "channel"),
                &ObjectType::Clock => write!(f, "clock"),
                &ObjectType::Log => write!(f, "debuglog"),
                &ObjectType::Event => write!(f, "event"),
                &ObjectType::Eventpair => write!(f, "eventpair"),
                &ObjectType::Exception => write!(f, "exception"),
                &ObjectType::Interrupt => write!(f, "interrupt"),
                &ObjectType::Iommu => write!(f, "iommu"),
                &ObjectType::Job => write!(f, "job"),
                &ObjectType::Msi => write!(f, "msi"),
                &ObjectType::None => write!(f, "handle"),
                &ObjectType::Pager => write!(f, "pager"),
                &ObjectType::PciDevice => write!(f, "pci_device"),
                &ObjectType::Pmt => write!(f, "pmt"),
                &ObjectType::Port => write!(f, "port"),
                &ObjectType::Process => write!(f, "process"),
                &ObjectType::Profile => write!(f, "profile"),
                &ObjectType::Resource => write!(f, "resource"),
                &ObjectType::Socket => write!(f, "socket"),
                &ObjectType::Stream => write!(f, "stream"),
                &ObjectType::SuspendToken => write!(f, "suspend_token"),
                &ObjectType::Thread => write!(f, "thread"),
                &ObjectType::Timer => write!(f, "timer"),
                &ObjectType::Vcpu => write!(f, "vcpu"),
                &ObjectType::Vmar => write!(f, "vmar"),
                &ObjectType::Vmo => write!(f, "vmo"),
                _ => write!(f, "unknown_type_handle"),
            },
            LookupResultOrType::Type(lib::Type::FrameworkError) => {
                write!(f, "<<FrameworkError>>")
            }
        }
    }
}

/// An invocable value, such as a function or closure.
#[derive(Clone)]
pub struct Invocable(
    Arc<dyn Fn(Vec<Value>, Option<Value>) -> BoxFuture<'static, Result<Value>> + Send + Sync>,
);

impl Invocable {
    /// Construct a new invocable.
    pub fn new(
        f: Arc<
            dyn Fn(Vec<Value>, Option<Value>) -> BoxFuture<'static, Result<Value>> + Send + Sync,
        >,
    ) -> Self {
        Self(f)
    }

    /// Invoke this invocable.
    pub async fn invoke(self, args: Vec<Value>, underscore: Option<Value>) -> Result<Value> {
        self.0(args, underscore).await
    }
}

pub type Value = FidlValue<PlaygroundValue>;

pub trait ValueExt: Sized {
    /// Clones this value. May modify the original value to introduce reference counting.
    fn duplicate(&mut self) -> Self;

    /// Convert this value to a raw `BigRational` if possible.
    fn try_big_num(self) -> Result<BigRational, Self>;

    /// Convert this value to a `num::BigInt` if possible.
    fn try_big_int(self) -> Result<BigInt, Value>;

    /// Convert this value to a `usize` if possible.
    fn try_usize(self) -> Result<usize, Self>;

    /// If this object is a client with the given protocol, return the raw channel.
    fn try_client_channel(
        self,
        ns: &lib::Namespace,
        expected_protocol: &str,
    ) -> Result<fdomain_client::Channel, Self>;

    /// If this object is a server, return the raw channel and protocol name.
    fn try_server_channel(self) -> Result<fdomain_client::Channel, Self>;

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// Convert a playground value to one that is ready for transfer via FIDL by
    /// converting playground-specific types. This performs *minimal* type checking; only
    /// what happens in the course of figuring out what conversion is appropriate.
    /// The FIDL codec should catch the rest.
    fn to_fidl_value(self, ns: &lib::Namespace, ty: &lib::Type) -> Result<FidlValue>;

    /// Get an [`InUseHandle`] from this value. Will convert directly-stored
    /// handle values or return the wrapped [`InUseHandle`] if applicable.
    fn to_in_use_handle(self) -> Option<InUseHandle>;

    /// Whether this value represents a client endpoint for the given service.
    fn is_client(&self, service: &str) -> bool;

    /// Get whether this value can be invoked as a command
    fn is_invocable(&self) -> bool;
}

impl ValueExt for Value {
    fn duplicate(&mut self) -> Self {
        match self {
            Value::Null => Value::Null,
            Value::Bool(a) => Value::Bool(*a),
            Value::U8(a) => Value::U8(*a),
            Value::U16(a) => Value::U16(*a),
            Value::U32(a) => Value::U32(*a),
            Value::U64(a) => Value::U64(*a),
            Value::I8(a) => Value::I8(*a),
            Value::I16(a) => Value::I16(*a),
            Value::I32(a) => Value::I32(*a),
            Value::I64(a) => Value::I64(*a),
            Value::F32(a) => Value::F32(*a),
            Value::F64(a) => Value::F64(*a),
            Value::String(a) => Value::String(a.clone()),
            Value::Object(a) => {
                Value::Object(a.iter_mut().map(|(a, b)| (a.clone(), b.duplicate())).collect())
            }
            Value::Bits(a, b) => Value::Bits(a.clone(), Box::new(b.duplicate())),
            Value::Enum(a, b) => Value::Enum(a.clone(), Box::new(b.duplicate())),
            Value::Union(a, b, c) => Value::Union(a.clone(), b.clone(), Box::new(c.duplicate())),
            Value::List(a) => Value::List(a.iter_mut().map(|x| x.duplicate()).collect()),

            Value::ServerEnd(_, _, _) => {
                let Value::ServerEnd(a, b, _) = std::mem::replace(self, Value::Null) else {
                    unreachable!();
                };
                let mut playground_value =
                    PlaygroundValue::InUseHandle(InUseHandle::server_end(a, b));
                *self = Value::OutOfLine(playground_value.duplicate());
                Value::OutOfLine(playground_value)
            }
            Value::ClientEnd(_, _, _) => {
                let Value::ClientEnd(a, b, _) = std::mem::replace(self, Value::Null) else {
                    unreachable!();
                };
                let mut playground_value =
                    PlaygroundValue::InUseHandle(InUseHandle::client_end(a, b));
                *self = Value::OutOfLine(playground_value.duplicate());
                Value::OutOfLine(playground_value)
            }
            Value::Handle(_, _, _) => {
                let Value::Handle(a, b, _) = std::mem::replace(self, Value::Null) else {
                    unreachable!();
                };
                let mut playground_value =
                    PlaygroundValue::InUseHandle(InUseHandle::handle(a, b.as_platform()));
                *self = Value::OutOfLine(playground_value.duplicate());
                Value::OutOfLine(playground_value)
            }
            Value::OutOfLine(a) => Value::OutOfLine(a.duplicate()),
        }
    }

    fn try_big_num(self) -> Result<BigRational, Value> {
        match self {
            Value::U8(i) => Ok(BigRational::from_integer(i.into())),
            Value::U16(i) => Ok(BigRational::from_integer(i.into())),
            Value::U32(i) => Ok(BigRational::from_integer(i.into())),
            Value::U64(i) => Ok(BigRational::from_integer(i.into())),
            Value::I8(i) => Ok(BigRational::from_integer(i.into())),
            Value::I16(i) => Ok(BigRational::from_integer(i.into())),
            Value::I32(i) => Ok(BigRational::from_integer(i.into())),
            Value::I64(i) => Ok(BigRational::from_integer(i.into())),
            Value::Bits(s, v) => v.try_big_num().map_err(|v| Value::Bits(s, Box::new(v))),
            Value::Enum(s, v) => v.try_big_num().map_err(|v| Value::Enum(s, Box::new(v))),
            Value::OutOfLine(PlaygroundValue::Num(s)) => Ok(s),
            _ => Err(self),
        }
    }

    fn try_big_int(self) -> Result<BigInt, Value> {
        match self {
            Value::U8(i) => Ok(i.into()),
            Value::U16(i) => Ok(i.into()),
            Value::U32(i) => i.try_into().map_err(|_| Value::U32(i)),
            Value::U64(i) => i.try_into().map_err(|_| Value::U64(i)),
            Value::I8(i) => i.try_into().map_err(|_| Value::I8(i)),
            Value::I16(i) => i.try_into().map_err(|_| Value::I16(i)),
            Value::I32(i) => i.try_into().map_err(|_| Value::I32(i)),
            Value::I64(i) => i.try_into().map_err(|_| Value::I64(i)),
            Value::Bits(s, v) => v.try_big_int().map_err(|v| Value::Bits(s, Box::new(v))),
            Value::Enum(s, v) => v.try_big_int().map_err(|v| Value::Enum(s, Box::new(v))),
            Value::OutOfLine(PlaygroundValue::Num(s)) => {
                if s.is_integer() {
                    return Ok(s.to_integer());
                }
                Err(Value::OutOfLine(PlaygroundValue::Num(s)))
            }
            _ => Err(self),
        }
    }

    fn try_usize(self) -> Result<usize, Value> {
        match self {
            Value::U8(i) => Ok(i.into()),
            Value::U16(i) => Ok(i.into()),
            Value::U32(i) => i.try_into().map_err(|_| Value::U32(i)),
            Value::U64(i) => i.try_into().map_err(|_| Value::U64(i)),
            Value::I8(i) => i.try_into().map_err(|_| Value::I8(i)),
            Value::I16(i) => i.try_into().map_err(|_| Value::I16(i)),
            Value::I32(i) => i.try_into().map_err(|_| Value::I32(i)),
            Value::I64(i) => i.try_into().map_err(|_| Value::I64(i)),
            Value::Bits(s, v) => v.try_usize().map_err(|v| Value::Bits(s, Box::new(v))),
            Value::Enum(s, v) => v.try_usize().map_err(|v| Value::Enum(s, Box::new(v))),
            Value::OutOfLine(PlaygroundValue::Num(s)) => {
                if s.is_integer() {
                    if let Ok(x) = s.to_integer().try_into() {
                        return Ok(x);
                    }
                }
                Err(Value::OutOfLine(PlaygroundValue::Num(s)))
            }
            _ => Err(self),
        }
    }

    fn try_client_channel(
        self,
        ns: &lib::Namespace,
        expected_protocol: &str,
    ) -> Result<fdomain_client::Channel, Self> {
        match self {
            Value::ClientEnd(c, proto, _) if ns.inherits(&proto, expected_protocol) => Ok(c),
            Value::OutOfLine(PlaygroundValue::InUseHandle(ref i)) => {
                if let Ok(c) = i.take_client(Some(expected_protocol)) { Ok(c) } else { Err(self) }
            }
            other => Err(other),
        }
    }

    fn try_server_channel(self) -> Result<fdomain_client::Channel, Self> {
        match self {
            Value::ServerEnd(c, _, _) => Ok(c),
            Value::OutOfLine(PlaygroundValue::InUseHandle(ref i)) => {
                if let Ok(c) = i.take_server(None) { Ok(c) } else { Err(self) }
            }
            other => Err(other),
        }
    }

    fn to_fidl_value(self, ns: &lib::Namespace, ty: &lib::Type) -> Result<FidlValue> {
        let ret = match self {
            Value::Null => FidlValue::Null,
            Value::Bool(a) => FidlValue::Bool(a),
            Value::U8(a) => FidlValue::U8(a),
            Value::U16(a) => FidlValue::U16(a),
            Value::U32(a) => FidlValue::U32(a),
            Value::U64(a) => FidlValue::U64(a),
            Value::I8(a) => FidlValue::I8(a),
            Value::I16(a) => FidlValue::I16(a),
            Value::I32(a) => FidlValue::I32(a),
            Value::I64(a) => FidlValue::I64(a),
            Value::F32(a) => FidlValue::F32(a),
            Value::F64(a) => FidlValue::F64(a),
            Value::String(a) => FidlValue::String(a),
            Value::Object(mut a) => match ty {
                lib::Type::Identifier { name, nullable: _ } => match ns
                    .lookup(name)
                    .map_err(|e| ValueError::FIDLLookupFailed(name.clone(), Arc::new(e)))?
                {
                    lib::LookupResult::Struct(st) => FidlValue::Object(
                        a.into_iter()
                            .map(|(key, val)| {
                                let member =
                                    st.members.iter().find(|x| x.name == key).ok_or_else(|| {
                                        ValueError::MemberMissing(name.clone(), key.clone())
                                    })?;
                                Ok((key, val.to_fidl_value(ns, &member.ty)?))
                            })
                            .collect::<Result<_>>()?,
                    ),
                    lib::LookupResult::Table(tbl) => FidlValue::Object(
                        a.into_iter()
                            .map(|(key, val)| {
                                let (_, member) =
                                    tbl.members.iter().find(|(_, x)| x.name == key).ok_or_else(
                                        || ValueError::MemberMissing(name.clone(), key.clone()),
                                    )?;
                                Ok((key, val.to_fidl_value(ns, &member.ty)?))
                            })
                            .collect::<Result<_>>()?,
                    ),
                    lib::LookupResult::Union(un) => {
                        let item = a.pop();
                        let (key, value) = if let Some((key, value)) = item.filter(|_| a.is_empty())
                        {
                            (key, value)
                        } else {
                            return Err(ValueError::StructToUnionWrongNumberOfMembers.into());
                        };
                        let (_, member) =
                            un.members.iter().find(|(_, x)| x.name == key).ok_or_else(|| {
                                ValueError::MemberMissing(name.clone(), key.clone())
                            })?;
                        FidlValue::Union(
                            name.clone(),
                            key,
                            Box::new(value.to_fidl_value(ns, &member.ty)?),
                        )
                    }
                    other => {
                        return Err(ValueError::BadObjectConversion(
                            format!("{}", Value::Object(a)),
                            format!("{}", LookupResultOrType::LookupResult(other)),
                        )
                        .into());
                    }
                },
                other => {
                    return Err(ValueError::BadObjectConversion(
                        format!("{}", Value::Object(a)),
                        format!("{:?}", other),
                    )
                    .into());
                }
            },
            Value::Bits(a, b) => {
                let lib::LookupResult::Bits(bits) = ns
                    .lookup(&a)
                    .map_err(|e| ValueError::FIDLLookupFailed(a.clone(), Arc::new(e)))?
                else {
                    return Err(ValueError::BadBitsType(a).into());
                };
                FidlValue::Bits(a, Box::new(b.to_fidl_value(ns, &bits.ty)?))
            }
            Value::Enum(a, b) => {
                let lib::LookupResult::Enum(en) = ns
                    .lookup(&a)
                    .map_err(|e| ValueError::FIDLLookupFailed(a.clone(), Arc::new(e)))?
                else {
                    return Err(ValueError::BadEnumType(a).into());
                };
                FidlValue::Enum(a, Box::new(b.to_fidl_value(ns, &en.ty)?))
            }
            Value::Union(a, b, c) => {
                let lib::LookupResult::Union(union) = ns
                    .lookup(&a)
                    .map_err(|e| ValueError::FIDLLookupFailed(a.clone(), Arc::new(e)))?
                else {
                    return Err(ValueError::BadUnionType(a).into());
                };
                for member in union.members.values() {
                    if member.name == b {
                        return Ok(FidlValue::Union(
                            a,
                            b,
                            Box::new(c.to_fidl_value(ns, &member.ty)?),
                        ));
                    }
                }

                return Err(ValueError::BadUnionMember(a, b).into());
            }
            Value::List(a) => match ty {
                lib::Type::Identifier { name, nullable: _ } => match ns
                    .lookup(name)
                    .map_err(|e| ValueError::FIDLLookupFailed(name.clone(), Arc::new(e)))?
                {
                    lib::LookupResult::Struct(st) => {
                        if a.len() > st.members.len() {
                            return Err(ValueError::ListStructTooShort(name.clone()).into());
                        }
                        FidlValue::Object(
                            st.members
                                .iter()
                                .zip(a.into_iter().chain(std::iter::repeat_with(|| Value::Null)))
                                .map(|(x, y)| {
                                    y.to_fidl_value(ns, &x.ty).map(|y| (x.name.to_owned(), y))
                                })
                                .collect::<Result<_>>()?,
                        )
                    }
                    lib::LookupResult::Table(tbl) => {
                        let mut members = Vec::with_capacity(a.len());
                        for (i, value) in a.into_iter().enumerate() {
                            let ordinal = i as u64 + 1;
                            let Some(member) = tbl.members.get(&ordinal) else {
                                if matches!(value, Value::Null) {
                                    continue;
                                } else {
                                    return Err(
                                        ValueError::TableFromListOrdinalMissing(ordinal).into()
                                    );
                                }
                            };
                            members.push((
                                member.name.to_owned(),
                                value.to_fidl_value(ns, &member.ty)?,
                            ));
                        }
                        FidlValue::Object(members)
                    }
                    _ => return Err(ValueError::TypeFromList(ty.clone()).into()),
                },
                lib::Type::Array(ty, _) => FidlValue::List(
                    a.into_iter().map(|x| x.to_fidl_value(ns, &*ty)).collect::<Result<_>>()?,
                ),
                lib::Type::Vector { ty, .. } => FidlValue::List(
                    a.into_iter().map(|x| x.to_fidl_value(ns, ty)).collect::<Result<_>>()?,
                ),
                _ => return Err(ValueError::TypeFromList(ty.clone()).into()),
            },
            Value::ServerEnd(a, b, c) => FidlValue::ServerEnd(a, b, c),
            Value::ClientEnd(a, b, c) => FidlValue::ClientEnd(a, b, c),
            Value::Handle(a, b, c) => FidlValue::Handle(a, b, c),
            Value::OutOfLine(s) => s.to_fidl_value(ns, ty)?,
        };

        Ok(ret)
    }

    fn to_in_use_handle(self) -> Option<InUseHandle> {
        match self {
            Value::ServerEnd(h, p, _) => Some(InUseHandle::server_end(h, p)),
            Value::ClientEnd(h, p, _) => Some(InUseHandle::client_end(h, p)),
            Value::Handle(h, t, _) => Some(InUseHandle::handle(h, t.as_platform())),
            Value::OutOfLine(PlaygroundValue::InUseHandle(s)) => Some(s),
            _ => None,
        }
    }

    fn is_client(&self, service: &str) -> bool {
        match self {
            Value::ClientEnd(_, s, _) => s == service,
            Value::OutOfLine(PlaygroundValue::InUseHandle(s)) => {
                s.get_client_protocol().ok().map(|x| &x == service).unwrap_or(false)
            }
            _ => false,
        }
    }

    fn is_invocable(&self) -> bool {
        match self {
            Value::OutOfLine(PlaygroundValue::Invocable(_)) => true,
            Value::OutOfLine(PlaygroundValue::InUseHandle(h)) => h.is_client_end(),
            Value::OutOfLine(PlaygroundValue::TypeHinted(_, v)) => v.is_invocable(),
            _ => false,
        }
    }
}

/// These are value types which aren't strictly part of the FIDL value structure
/// but instead serve a purpose within the playground environment.
pub enum PlaygroundValue {
    /// A function or closure.
    Invocable(Invocable),
    /// A handle which can have multiple owners.
    InUseHandle(InUseHandle),
    /// A number with no precision limits.
    Num(BigRational),
    /// An iterator.
    Iterator(ReplayableIterator),
    /// A value with a type hint associated with it.
    TypeHinted(String, Box<Value>),
}

impl PlaygroundValue {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// Convert this playground value to a raw FIDL value if possible.
    fn to_fidl_value_by_type_or_lookup(
        self,
        ns: &lib::Namespace,
        ty: LookupResultOrType<'_>,
    ) -> Result<FidlValue> {
        match (ty, self) {
            (LookupResultOrType::Type(ty), PlaygroundValue::TypeHinted(_, y)) => {
                y.to_fidl_value(ns, &ty)
            }
            (LookupResultOrType::Type(lib::Type::U8), PlaygroundValue::Num(x)) => x
                .to_u8()
                .map(FidlValue::U8)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::U8).into()),
            (LookupResultOrType::Type(lib::Type::U16), PlaygroundValue::Num(x)) => x
                .to_u16()
                .map(FidlValue::U16)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::U16).into()),
            (LookupResultOrType::Type(lib::Type::U32), PlaygroundValue::Num(x)) => x
                .to_u32()
                .map(FidlValue::U32)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::U32).into()),
            (LookupResultOrType::Type(lib::Type::U64), PlaygroundValue::Num(x)) => x
                .to_u64()
                .map(FidlValue::U64)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::U64).into()),
            (LookupResultOrType::Type(lib::Type::I8), PlaygroundValue::Num(x)) => x
                .to_i8()
                .map(FidlValue::I8)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::I8).into()),
            (LookupResultOrType::Type(lib::Type::I16), PlaygroundValue::Num(x)) => x
                .to_i16()
                .map(FidlValue::I16)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::I16).into()),
            (LookupResultOrType::Type(lib::Type::I32), PlaygroundValue::Num(x)) => x
                .to_i32()
                .map(FidlValue::I32)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::I32).into()),
            (LookupResultOrType::Type(lib::Type::I64), PlaygroundValue::Num(x)) => x
                .to_i64()
                .map(FidlValue::I64)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::I64).into()),
            (LookupResultOrType::Type(lib::Type::F32), PlaygroundValue::Num(x)) => x
                .to_f32()
                .map(FidlValue::F32)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::F32).into()),
            (LookupResultOrType::Type(lib::Type::F64), PlaygroundValue::Num(x)) => x
                .to_f64()
                .map(FidlValue::F64)
                .ok_or_else(|| ValueError::ConversionOverflow(lib::Type::F64).into()),
            (LookupResultOrType::Type(lib::Type::Identifier { name, .. }), v) => v
                .to_fidl_value_by_type_or_lookup(
                    ns,
                    LookupResultOrType::LookupResult(
                        ns.lookup(&name)
                            .map_err(|e| ValueError::FIDLLookupFailed(name.clone(), Arc::new(e)))?,
                    ),
                ),
            (
                LookupResultOrType::LookupResult(lib::LookupResult::Bits(lib::Bits { ty, .. })),
                v,
            ) => v.to_fidl_value_by_type_or_lookup(ns, LookupResultOrType::Type(ty.clone())),
            (
                LookupResultOrType::LookupResult(lib::LookupResult::Enum(lib::Enum { ty, .. })),
                v,
            ) => v.to_fidl_value_by_type_or_lookup(ns, LookupResultOrType::Type(ty.clone())),
            (
                LookupResultOrType::Type(lib::Type::Endpoint {
                    role: lib::EndpointRole::Server,
                    protocol,
                    ..
                }),
                PlaygroundValue::InUseHandle(h),
            ) => h
                .take_server(Some(&protocol))
                .map(|x| FidlValue::ServerEnd(x, protocol.to_owned(), None))
                .map_err(|_| ValueError::ServerConversionFailed(protocol.clone()).into()),
            (
                LookupResultOrType::Type(lib::Type::Endpoint {
                    role: lib::EndpointRole::Client,
                    protocol,
                    ..
                }),
                PlaygroundValue::InUseHandle(h),
            ) => h
                .take_client(Some(&protocol))
                .map(|x| FidlValue::ClientEnd(x, protocol.to_owned(), None))
                .map_err(|_| ValueError::ClientConversionFailed(protocol.clone()).into()),
            (
                LookupResultOrType::Type(lib::Type::Handle {
                    object_type: fidl_codec_fdomain::ObjectType::Socket,
                    rights: _,
                    nullable: _,
                }),
                PlaygroundValue::InUseHandle(h),
            ) => h.take_socket().map_err(|_| ValueError::SocketConversionFailed.into()),
            (LookupResultOrType::Type(ty), PlaygroundValue::Invocable(_)) => {
                Err(ValueError::InvocableConversionFailed(ty).into())
            }
            (LookupResultOrType::Type(ty), PlaygroundValue::Num(_)) => {
                Err(ValueError::NumberConversionFailed(ty).into())
            }
            (LookupResultOrType::Type(ty), PlaygroundValue::Iterator(_)) => {
                Err(ValueError::IteratorConversionFailed(ty).into())
            }
            _ => Err(ValueError::MiscConversionFailed.into()),
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// Convert this playground value to a raw FIDL value if possible.
    fn to_fidl_value(self, ns: &lib::Namespace, ty: &lib::Type) -> Result<FidlValue> {
        self.to_fidl_value_by_type_or_lookup(ns, LookupResultOrType::Type(ty.clone()))
    }

    /// Duplicate this value. May modify the original value to convert
    /// single-owner forms to refcounted forms etc.
    fn duplicate(&mut self) -> Self {
        match self {
            PlaygroundValue::Invocable(a) => PlaygroundValue::Invocable(a.clone()),
            PlaygroundValue::InUseHandle(a) => PlaygroundValue::InUseHandle(a.clone()),
            PlaygroundValue::Num(a) => PlaygroundValue::Num(a.clone()),
            PlaygroundValue::Iterator(a) => PlaygroundValue::Iterator(a.clone()),
            PlaygroundValue::TypeHinted(a, b) => {
                PlaygroundValue::TypeHinted(a.clone(), Box::new(b.duplicate()))
            }
        }
    }
}

/// Compares two playground values. Automatically handles some niceties like upcasting integers etc.
pub fn playground_semantic_compare(this: &Value, other: &Value) -> Option<Ordering> {
    fn try_promote(a: &Value) -> Option<BigRational> {
        match a {
            Value::U8(x) => Some(BigRational::from_integer((*x).into())),
            Value::U16(x) => Some(BigRational::from_integer((*x).into())),
            Value::U32(x) => Some(BigRational::from_integer((*x).into())),
            Value::U64(x) => Some(BigRational::from_integer((*x).into())),
            Value::I8(x) => Some(BigRational::from_integer((*x).into())),
            Value::I16(x) => Some(BigRational::from_integer((*x).into())),
            Value::I32(x) => Some(BigRational::from_integer((*x).into())),
            Value::I64(x) => Some(BigRational::from_integer((*x).into())),
            Value::OutOfLine(PlaygroundValue::Num(x)) => Some(x.clone()),
            _ => None,
        }
    }

    fn cmp_to_fidl(a: &PlaygroundValue, b: &Value) -> Option<Ordering> {
        match (a, b) {
            (a, Value::Bits(_, b)) => cmp_to_fidl(a, b),
            (a, Value::Enum(_, b)) => cmp_to_fidl(a, b),
            (PlaygroundValue::Num(a), Value::U8(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::U16(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::U32(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::U64(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::I8(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::I16(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::I32(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            (PlaygroundValue::Num(a), Value::I64(b)) => {
                PartialOrd::partial_cmp(a, &BigRational::from_integer((*b).into()))
            }
            _ => None,
        }
    }

    match (this, other) {
        (a @ Value::OutOfLine(_), b @ Value::OutOfLine(_)) => {
            if let (Some(a), Some(b)) = (try_promote(a), try_promote(b)) {
                PartialOrd::partial_cmp(&a, &b)
            } else {
                None
            }
        }
        (Value::OutOfLine(a), b) => cmp_to_fidl(a, b),
        (a, Value::OutOfLine(b)) => cmp_to_fidl(b, a).map(Ordering::reverse),
        (Value::Bits(_, a), b) => playground_semantic_compare(a, b),
        (a, Value::Bits(_, b)) => playground_semantic_compare(a, b),
        (Value::Enum(_, a), b) => playground_semantic_compare(a, b),
        (a, Value::Enum(_, b)) => playground_semantic_compare(a, b),
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Bool(a), Value::Bool(b)) => PartialOrd::partial_cmp(a, b),
        (Value::String(a), Value::String(b)) => PartialOrd::partial_cmp(a, b),
        (Value::Object(a), Value::Object(b)) => {
            if a.len() != b.len() {
                None
            } else {
                let mut values = HashMap::new();

                for (name, value) in a.iter() {
                    values.insert(name, value);
                }

                let mut ret = Some(Ordering::Equal);

                for (name, value) in b.iter() {
                    if let Some(v) = values.get(name) {
                        if playground_semantic_compare(value, *v)
                            .map(|x| x.is_eq())
                            .unwrap_or(false)
                        {
                            continue;
                        }
                    }

                    ret = None;
                    break;
                }

                ret
            }
        }
        (Value::U8(a), Value::U8(b)) => PartialOrd::partial_cmp(a, b),
        (Value::U16(a), Value::U16(b)) => PartialOrd::partial_cmp(a, b),
        (Value::U32(a), Value::U32(b)) => PartialOrd::partial_cmp(a, b),
        (Value::U64(a), Value::U64(b)) => PartialOrd::partial_cmp(a, b),
        (Value::I8(a), Value::I8(b)) => PartialOrd::partial_cmp(a, b),
        (Value::I16(a), Value::I16(b)) => PartialOrd::partial_cmp(a, b),
        (Value::I32(a), Value::I32(b)) => PartialOrd::partial_cmp(a, b),
        (Value::I64(a), Value::I64(b)) => PartialOrd::partial_cmp(a, b),
        (Value::List(a), Value::List(b)) => a
            .iter()
            .zip(b.iter())
            .map(|(a, b)| playground_semantic_compare(a, b))
            .find(|x| *x != Some(Ordering::Equal))
            .unwrap_or_else(|| PartialOrd::partial_cmp(&a.len(), &b.len())),
        (a, b) => {
            if let (Some(a), Some(b)) = (try_promote(a), try_promote(b)) {
                PartialOrd::partial_cmp(&a, &b)
            } else {
                None
            }
        }
    }
}

impl std::fmt::Debug for PlaygroundValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for PlaygroundValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaygroundValue::Invocable(Invocable(a)) => write!(f, "<function@{:p}>", a),
            PlaygroundValue::Num(x) => {
                let mut x = x.clone();
                if x.numer() < &0.into() || x.denom() < &0.into() {
                    write!(f, "-")?;
                    x *= &BigInt::from(-1);
                }
                let int_part = x.to_integer();
                std::fmt::Display::fmt(&x.to_integer(), f)?;
                if x.is_integer() {
                    return Ok(());
                }
                x -= &int_part;
                write!(f, ".")?;
                for _ in 0..100 {
                    x *= &BigInt::from(10);
                    let int_part = x.to_integer();
                    write!(f, "{int_part}")?;
                    if x.is_integer() {
                        break;
                    }
                    x -= &int_part;
                }
                Ok(())
            }
            PlaygroundValue::Iterator(_) => write!(f, "<iterator>"),
            PlaygroundValue::InUseHandle(h) => std::fmt::Display::fmt(h, f),
            PlaygroundValue::TypeHinted(hint, v) => write!(f, "@{hint} {v}"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_client::HandleBased;
    use fidl_codec_fdomain::Rights;
    use futures::{AsyncReadExt, AsyncWriteExt, FutureExt};

    #[test]
    fn compare() {
        assert!(playground_semantic_compare(&Value::U8(2), &Value::U32(1)).unwrap().is_gt());
        assert!(playground_semantic_compare(&Value::U32(2), &Value::U8(1)).unwrap().is_gt());
        assert!(playground_semantic_compare(&Value::I32(-2), &Value::U8(1)).unwrap().is_lt());
        assert!(
            playground_semantic_compare(
                &Value::Bits("pumpkin".into(), Box::new(Value::U8(2))),
                &Value::U32(1)
            )
            .unwrap()
            .is_gt()
        );
        assert!(
            playground_semantic_compare(
                &Value::Enum("pumpkin".into(), Box::new(Value::U8(2))),
                &Value::U32(1)
            )
            .unwrap()
            .is_gt()
        );
        assert!(
            playground_semantic_compare(&Value::String("a".into()), &Value::String("a".into()))
                .unwrap()
                .is_eq()
        );
        assert!(
            playground_semantic_compare(&Value::String("a".into()), &Value::String("b".into()))
                .unwrap()
                .is_lt()
        );
        assert!(
            playground_semantic_compare(
                &Value::List(vec![Value::U8(1), Value::U8(2)]),
                &Value::List(vec![Value::U8(1), Value::U8(2)])
            )
            .unwrap()
            .is_eq()
        );
        assert!(
            playground_semantic_compare(
                &Value::List(vec![Value::U8(1), Value::U8(2)]),
                &Value::List(vec![Value::U8(1), Value::U8(3)])
            )
            .unwrap()
            .is_lt()
        );
        assert!(
            playground_semantic_compare(
                &Value::List(vec![Value::U8(1), Value::U8(2)]),
                &Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(0)])
            )
            .unwrap()
            .is_lt()
        );
        assert!(
            playground_semantic_compare(
                &Value::Object(vec![("foo".into(), Value::U8(1)), ("bar".into(), Value::U8(2))]),
                &Value::Object(vec![("bar".into(), Value::U8(2)), ("foo".into(), Value::U8(1))]),
            )
            .unwrap()
            .is_eq()
        );
        assert!(
            playground_semantic_compare(
                &Value::Object(vec![("foo".into(), Value::U8(1)), ("bar".into(), Value::U8(1))]),
                &Value::Object(vec![("bar".into(), Value::U8(2)), ("foo".into(), Value::U8(1))]),
            )
            .is_none()
        );
    }

    #[test]
    fn to_fidl_struct() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/PrimitiveTypes".to_owned(),
            nullable: false,
        };
        let fidl_value = Value::Object(vec![
            ("s".to_owned(), Value::String("floop".to_owned())),
            ("b".to_owned(), Value::Bool(true)),
            ("i8".to_owned(), Value::I8(1)),
            ("i16".to_owned(), Value::I16(2)),
            ("i32".to_owned(), Value::I32(3)),
            ("i64".to_owned(), Value::I64(4)),
            ("u8".to_owned(), Value::U8(5)),
            ("u16".to_owned(), Value::U16(6)),
            ("u32".to_owned(), Value::U32(7)),
            ("u64".to_owned(), Value::U64(8)),
            ("f32".to_owned(), Value::F32(9.0)),
            ("f64".to_owned(), Value::F64(10.0)),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap();
        let FidlValue::Object(fields) = fidl_value else { panic!() };

        let mut checks: HashMap<_, _> = [
            (
                "s",
                (&|x: &FidlValue| if let FidlValue::String(s) = x { s == "floop" } else { false })
                    as &dyn Fn(&FidlValue) -> _,
            ),
            ("b", &|x| matches!(x, FidlValue::Bool(true))),
            ("i8", &|x| matches!(x, FidlValue::I8(1))),
            ("i16", &|x| matches!(x, FidlValue::I16(2))),
            ("i32", &|x| matches!(x, FidlValue::I32(3))),
            ("i64", &|x| matches!(x, FidlValue::I64(4))),
            ("u8", &|x| matches!(x, FidlValue::U8(5))),
            ("u16", &|x| matches!(x, FidlValue::U16(6))),
            ("u32", &|x| matches!(x, FidlValue::U32(7))),
            ("u64", &|x| matches!(x, FidlValue::U64(8))),
            ("f32", &|x| if let FidlValue::F32(f) = x { *f == 9.0 } else { false }),
            ("f64", &|x| if let FidlValue::F64(f) = x { *f == 10.0 } else { false }),
        ]
        .into_iter()
        .collect();

        assert!(fields.len() == 12);
        for (key, value) in fields.into_iter() {
            let check = checks.remove(key.as_str()).unwrap();
            assert!(check(&value), "Field {key:?} had value {value:?}");
        }
    }

    #[test]
    fn to_fidl_struct_extra_field() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/PrimitiveTypes".to_owned(),
            nullable: false,
        };
        Value::Object(vec![
            ("s".to_owned(), Value::String("floop".to_owned())),
            ("b".to_owned(), Value::Bool(true)),
            ("i8".to_owned(), Value::I8(1)),
            ("i16".to_owned(), Value::I16(2)),
            ("i32".to_owned(), Value::I32(3)),
            ("i64".to_owned(), Value::I64(4)),
            ("u8".to_owned(), Value::U8(5)),
            ("u16".to_owned(), Value::U16(6)),
            ("u32".to_owned(), Value::U32(7)),
            ("u64".to_owned(), Value::U64(8)),
            ("f32".to_owned(), Value::F32(9.0)),
            ("f64".to_owned(), Value::F64(10.0)),
            ("extr_field".to_owned(), Value::F64(10.0)),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap_err();
    }

    #[test]
    fn to_fidl_table() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/ValueTable".to_owned(),
            nullable: false,
        };
        let fidl_value = Value::Object(vec![
            ("first_int16".to_owned(), Value::I16(2)),
            (
                "second_struct".to_owned(),
                Value::Object(vec![
                    ("value1".to_owned(), Value::String("pumpkin".to_owned())),
                    ("value2".to_owned(), Value::String("floop".to_owned())),
                ]),
            ),
            (
                "third_union".to_owned(),
                Value::Object(vec![("variant_i".to_owned(), Value::I32(3))]),
            ),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap();
        let FidlValue::Object(fields) = fidl_value else { panic!() };

        let mut checks: HashMap<_, _> = [
            (
                "first_int16",
                (&|x: &FidlValue| matches!(x, FidlValue::I16(2))) as &dyn Fn(&FidlValue) -> bool,
            ),
            ("second_struct", &|x| {
                if let FidlValue::Object(fields) = x {
                    fields.len() == 2
                        && fields.iter().any(|(k, v)| {
                            k == "value1"
                                && if let FidlValue::String(v) = v { v == "pumpkin" } else { false }
                        })
                        && fields.iter().any(|(k, v)| {
                            k == "value2"
                                && if let FidlValue::String(v) = v { v == "floop" } else { false }
                        })
                } else {
                    false
                }
            }),
            ("third_union", &|x| {
                if let FidlValue::Union(ident, variant, field) = x {
                    ident == "test.fidlcodec.examples/IntStructUnion"
                        && variant == "variant_i"
                        && matches!(**field, FidlValue::I32(3))
                } else {
                    false
                }
            }),
        ]
        .into_iter()
        .collect();

        assert!(fields.len() == 3);
        for (key, value) in fields.into_iter() {
            let check = checks.remove(key.as_str()).unwrap();
            assert!(check(&value), "Field {key:?} had value {value:?}");
        }
    }

    #[test]
    fn list_to_fidl_struct() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/PrimitiveTypes".to_owned(),
            nullable: false,
        };
        let fidl_value = Value::List(vec![
            Value::String("floop".to_owned()),
            Value::Bool(true),
            Value::I8(1),
            Value::I16(2),
            Value::I32(3),
            Value::I64(4),
            Value::U8(5),
            Value::U16(6),
            Value::U32(7),
            Value::U64(8),
            Value::F32(9.0),
            Value::F64(10.0),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap();
        let FidlValue::Object(fields) = fidl_value else { panic!() };

        let mut checks: HashMap<_, _> = [
            (
                "s",
                (&|x: &FidlValue| if let FidlValue::String(s) = x { s == "floop" } else { false })
                    as &dyn Fn(&FidlValue) -> _,
            ),
            ("b", &|x| matches!(x, FidlValue::Bool(true))),
            ("i8", &|x| matches!(x, FidlValue::I8(1))),
            ("i16", &|x| matches!(x, FidlValue::I16(2))),
            ("i32", &|x| matches!(x, FidlValue::I32(3))),
            ("i64", &|x| matches!(x, FidlValue::I64(4))),
            ("u8", &|x| matches!(x, FidlValue::U8(5))),
            ("u16", &|x| matches!(x, FidlValue::U16(6))),
            ("u32", &|x| matches!(x, FidlValue::U32(7))),
            ("u64", &|x| matches!(x, FidlValue::U64(8))),
            ("f32", &|x| if let FidlValue::F32(f) = x { *f == 9.0 } else { false }),
            ("f64", &|x| if let FidlValue::F64(f) = x { *f == 10.0 } else { false }),
        ]
        .into_iter()
        .collect();

        assert!(fields.len() == 12);
        for (key, value) in fields.into_iter() {
            let check = checks.remove(key.as_str()).unwrap();
            assert!(check(&value), "Field {key:?} had value {value:?}");
        }
    }

    #[test]
    fn list_to_fidl_table() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/ValueTable".to_owned(),
            nullable: false,
        };
        let fidl_value = Value::List(vec![
            Value::I16(2),
            Value::List(vec![
                Value::String("pumpkin".to_owned()),
                Value::String("floop".to_owned()),
            ]),
            Value::Null,
            Value::Object(vec![("variant_i".to_owned(), Value::I32(3))]),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap();
        let FidlValue::Object(fields) = fidl_value else { panic!() };

        let mut checks: HashMap<_, _> = [
            (
                "first_int16",
                (&|x: &FidlValue| matches!(x, FidlValue::I16(2))) as &dyn Fn(&FidlValue) -> bool,
            ),
            ("second_struct", &|x| {
                if let FidlValue::Object(fields) = x {
                    fields.len() == 2
                        && fields.iter().any(|(k, v)| {
                            k == "value1"
                                && if let FidlValue::String(v) = v { v == "pumpkin" } else { false }
                        })
                        && fields.iter().any(|(k, v)| {
                            k == "value2"
                                && if let FidlValue::String(v) = v { v == "floop" } else { false }
                        })
                } else {
                    false
                }
            }),
            ("third_union", &|x| {
                if let FidlValue::Union(ident, variant, field) = x {
                    ident == "test.fidlcodec.examples/IntStructUnion"
                        && variant == "variant_i"
                        && matches!(**field, FidlValue::I32(3))
                } else {
                    false
                }
            }),
        ]
        .into_iter()
        .collect();

        assert!(fields.len() == 3);
        for (key, value) in fields.into_iter() {
            let check = checks.remove(key.as_str()).unwrap();
            assert!(check(&value), "Field {key:?} had value {value:?}");
        }
    }

    #[test]
    fn list_to_fidl_table_reserved_field() {
        let mut ns = lib::Namespace::new();
        ns.load(test_fidl::TEST_FIDL).unwrap();

        let ty = lib::Type::Identifier {
            name: "test.fidlcodec.examples/ValueTable".to_owned(),
            nullable: false,
        };
        Value::List(vec![
            Value::I16(2),
            Value::List(vec![
                Value::String("pumpkin".to_owned()),
                Value::String("floop".to_owned()),
            ]),
            Value::I16(5),
            Value::Object(vec![("variant_i".to_owned(), Value::I32(3))]),
        ])
        .to_fidl_value(&ns, &ty)
        .unwrap_err();
    }

    #[fuchsia::test]
    async fn clonable_invocable() {
        let a = Invocable::new(Arc::new(|mut v, o| {
            async move {
                assert!(o.is_none());
                let value = v.pop().unwrap();
                assert!(v.is_empty());
                let Value::U8(v) = value else { panic!() };
                Ok(Value::U8(v + 2))
            }
            .boxed()
        }));
        let b = a.clone();
        let Ok(Value::U8(a)) = a.invoke(vec![Value::U8(7)], None).await else {
            panic!();
        };
        assert_eq!(9, a);
        let Ok(Value::U8(b)) = b.invoke(vec![Value::U8(3)], None).await else {
            panic!();
        };
        assert_eq!(5, b);
    }

    #[allow(clippy::unused_async)]
    #[fuchsia::test]
    async fn promote_handles() {
        let mut ns = lib::Namespace::new();
        let client = fdomain_local::local_client_empty();
        ns.load(test_fidl::TEST_FIDL).unwrap();
        let (a, b) = client.create_channel();

        let mut client =
            Value::ClientEnd(a, "test.fidlcodec.examples/FidlCodecTestProtocol".to_owned(), None);
        let mut server =
            Value::ServerEnd(b, "test.fidlcodec.examples/FidlCodecTestProtocol".to_owned(), None);
        let client_dup = client.duplicate();
        let server_dup = server.duplicate();

        assert!(matches!(client_dup, Value::OutOfLine(PlaygroundValue::InUseHandle(_))));
        assert!(matches!(server_dup, Value::OutOfLine(PlaygroundValue::InUseHandle(_))));

        let Ok(FidlValue::ClientEnd(_channel, protocol, None)) = client_dup.to_fidl_value(
            &ns,
            &lib::Type::Endpoint {
                role: lib::EndpointRole::Client,
                protocol: "test.fidlcodec.examples/FidlCodecTestProtocol".to_owned(),
                rights: Some(Rights::SAME_RIGHTS),
                nullable: false,
            },
        ) else {
            panic!();
        };
        assert_eq!("test.fidlcodec.examples/FidlCodecTestProtocol", &protocol);

        let Ok(FidlValue::ServerEnd(_channel, protocol, None)) = server_dup.to_fidl_value(
            &ns,
            &lib::Type::Endpoint {
                role: lib::EndpointRole::Server,
                protocol: "test.fidlcodec.examples/FidlCodecTestProtocol".to_owned(),
                rights: Some(Rights::SAME_RIGHTS),
                nullable: false,
            },
        ) else {
            panic!();
        };
        assert_eq!("test.fidlcodec.examples/FidlCodecTestProtocol", &protocol);

        let Value::OutOfLine(PlaygroundValue::InUseHandle(client)) = client else { panic!() };

        assert!(client.take_client(Some("test.fidlcodec.examples/FidlCodecTestProtocol")).is_err());

        let Value::OutOfLine(PlaygroundValue::InUseHandle(server)) = server else { panic!() };

        assert!(server.take_server(Some("test.fidlcodec.examples/FidlCodecTestProtocol")).is_err());
    }

    #[fuchsia::test]
    async fn promote_to_socket() {
        use fidl_codec_fdomain::zx;

        let socket_default_rights =
            zx::RIGHTS_BASIC | zx::RIGHTS_IO | Rights::SIGNAL | Rights::SIGNAL_PEER;

        let ns = lib::Namespace::new();
        let client = fdomain_local::local_client_empty();
        let (a, b) = InUseHandle::new_endpoints(Arc::clone(&client));
        let a = Value::OutOfLine(PlaygroundValue::InUseHandle(a));

        let a = a
            .to_fidl_value(
                &ns,
                &lib::Type::Handle {
                    object_type: ObjectType::Socket,
                    rights: Some(socket_default_rights),
                    nullable: false,
                },
            )
            .unwrap();
        let FidlValue::Handle(a, ObjectType::Socket, None) = a else { panic!() };
        let mut a = fdomain_client::Socket::from(a);
        let mut b = b.unwrap_socket();
        static CALL: &[u8] = b"What if we shared some extremely basic opinions?";
        static RESPONSE: &[u8] = b"I think pickles are alright.";
        futures::future::join(
            async move {
                a.write_all(CALL).await.unwrap();
                let mut buf = [0u8; 28];
                assert_eq!(buf.len(), RESPONSE.len());
                a.read_exact(&mut buf).await.unwrap();
                assert_eq!(RESPONSE, &buf);
            },
            async move {
                let mut buf = [0u8; 48];
                assert_eq!(buf.len(), CALL.len());
                b.read_exact(&mut buf).await.unwrap();
                assert_eq!(CALL, &buf);
                b.write_all(RESPONSE).await.unwrap();
            },
        )
        .await;
    }

    #[allow(clippy::unused_async)]
    #[fuchsia::test]
    async fn duplicate_raw_handle() {
        let client = fdomain_local::local_client_empty();
        let (socket, _b) = client.create_stream_socket();
        let socket = socket.into_handle();
        let mut socket = Value::Handle(socket, ObjectType::Socket, None);
        let socket_dup = socket.duplicate();
        let Value::OutOfLine(PlaygroundValue::InUseHandle(socket)) = socket else { panic!() };
        let Value::OutOfLine(PlaygroundValue::InUseHandle(socket_dup)) = socket_dup else {
            panic!()
        };

        assert_eq!(socket.id().unwrap(), socket_dup.id().unwrap());
        assert_eq!(Some(fidl::ObjectType::SOCKET), socket.object_type());
        assert_eq!(Some(fidl::ObjectType::SOCKET), socket_dup.object_type());
    }

    #[test]
    fn display_real() {
        assert_eq!(
            "1.5 -6.75 0.5 -0.25",
            &format!(
                "{} {} {} {}",
                Value::OutOfLine(PlaygroundValue::Num(BigRational::new(3.into(), 2.into()))),
                Value::OutOfLine(PlaygroundValue::Num(BigRational::new((-27).into(), 4.into()))),
                Value::OutOfLine(PlaygroundValue::Num(BigRational::new(1.into(), 2.into()))),
                Value::OutOfLine(PlaygroundValue::Num(BigRational::new((-1).into(), 4.into()))),
            )
        );
    }
}
