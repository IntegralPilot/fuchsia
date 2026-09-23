// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_constants::{ALLOC_PRESENT_U32, ALLOC_PRESENT_U64};
use nom::bytes::complete::take;
use nom::combinator::{map, value, verify};
use nom::multi::count;
use nom::sequence::{pair, preceded, terminated};
use nom::{IResult, Parser};

use fidl_data_zx::{DEFAULT_CHANNEL_RIGHTS, ObjType as ObjectType, Rights};

use crate::error::{Error, Result};
use crate::handle::*;
use crate::library;
use crate::transaction::{TransactionHeader, decode_transaction_header};
use crate::util::*;
use crate::value::Value;

use std::str;

type DResult<'a, R> = IResult<&'a [u8], R, Error>;

/// This represents an action that will yield a Value when given further bytes to process and
/// handles to potentially consume. This is how we implement out-of-line data. The initial parse
/// takes the inline data, and when we're ready, the Defer can be fed the remaining bytes to take
/// the out of line data.
enum Defer<'d> {
    /// This Defer doesn't need any further processing. We can just offer up the value right now.
    Complete(Value),

    /// This Defer implements the actual deferred processing pattern described.
    Action(
        Box<
            dyn for<'a> FnOnce(
                    &'a [u8],
                    &mut Vec<HandleInfo>,
                    RecursionCounter,
                ) -> DResult<'a, Value>
                + 'd,
        >,
    ),
}

impl<'d> Defer<'d> {
    /// Completes a deferred parse and returns the result.
    fn complete<'a>(
        self,
        data: &'a [u8],
        handles: &mut Vec<HandleInfo>,
        counter: RecursionCounter,
    ) -> DResult<'a, Value> {
        match self {
            Defer::Complete(v) => Ok((data, v)),
            Defer::Action(act) => act(data, handles, counter),
        }
    }
}

impl<'d> From<Value> for Defer<'d> {
    fn from(v: Value) -> Defer<'d> {
        Defer::Complete(v)
    }
}

fn take_u8(data: &[u8]) -> DResult<'_, u8> {
    map(take(1usize), |x: &[u8]| x[0]).parse(data)
}

fn value_u8(data: &[u8]) -> DResult<'_, Value> {
    map(take_u8, Value::U8).parse(data)
}

fn value_bool(data: &[u8]) -> DResult<'_, Value> {
    map(verify(take_u8, |&x| x == 0 || x == 1), |x| Value::Bool(x != 0)).parse(data)
}

fn take_u16(data: &[u8]) -> DResult<'_, u16> {
    map(take(2usize), |x: &[u8]| u16::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_u16(data: &[u8]) -> DResult<'_, Value> {
    map(take_u16, Value::U16).parse(data)
}

fn take_u32(data: &[u8]) -> DResult<'_, u32> {
    map(take(4usize), |x: &[u8]| u32::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_u32(data: &[u8]) -> DResult<'_, Value> {
    map(take_u32, Value::U32).parse(data)
}

fn take_u64(data: &[u8]) -> DResult<'_, u64> {
    map(take(8usize), |x: &[u8]| u64::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_u64(data: &[u8]) -> DResult<'_, Value> {
    map(take_u64, Value::U64).parse(data)
}

fn take_i8(data: &[u8]) -> DResult<'_, i8> {
    map(take(1usize), |x: &[u8]| i8::from_le_bytes([x[0]])).parse(data)
}

fn value_i8(data: &[u8]) -> DResult<'_, Value> {
    map(take_i8, Value::I8).parse(data)
}

fn take_i16(data: &[u8]) -> DResult<'_, i16> {
    map(take(2usize), |x: &[u8]| i16::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_i16(data: &[u8]) -> DResult<'_, Value> {
    map(take_i16, Value::I16).parse(data)
}

fn take_i32(data: &[u8]) -> DResult<'_, i32> {
    map(take(4usize), |x: &[u8]| i32::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_i32(data: &[u8]) -> DResult<'_, Value> {
    map(take_i32, Value::I32).parse(data)
}

fn take_i64(data: &[u8]) -> DResult<'_, i64> {
    map(take(8usize), |x: &[u8]| i64::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_i64(data: &[u8]) -> DResult<'_, Value> {
    map(take_i64, Value::I64).parse(data)
}

fn take_f32(data: &[u8]) -> DResult<'_, f32> {
    map(take(4usize), |x: &[u8]| f32::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_f32(data: &[u8]) -> DResult<'_, Value> {
    map(take_f32, Value::F32).parse(data)
}

fn take_f64(data: &[u8]) -> DResult<'_, f64> {
    map(take(8usize), |x: &[u8]| f64::from_le_bytes(x.try_into().unwrap())).parse(data)
}

fn value_f64(data: &[u8]) -> DResult<'_, Value> {
    map(take_f64, Value::F64).parse(data)
}

fn transaction_header(data: &[u8]) -> DResult<'_, TransactionHeader> {
    decode_transaction_header(data)
        .map(|(a, b)| (b, a))
        .map_err(|e| Error::DecodeError(format!("Invalid FIDL transaction header ({e:?})")).into())
}

fn take_padding(amount: usize) -> impl Fn(&[u8]) -> DResult<'_, ()> {
    move |bytes| value((), verify(take(amount), |x: &[u8]| x.iter().all(|&x| x == 0))).parse(bytes)
}

fn decode_struct<'s>(
    ns: &'s library::Namespace,
    st: &'s library::Struct,
    nullable: bool,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'s>> {
    move |bytes: &[u8]| {
        if !nullable {
            return decode_struct_nonnull(ns, st).parse(bytes);
        }

        let (bytes, presence) = take_u64(bytes)?;

        if presence == 0 {
            Ok((bytes, Defer::Complete(Value::Null)))
        } else if presence != ALLOC_PRESENT_U64 {
            Err(Error::DecodeError("Bad presence indicator".to_owned()).into())
        } else {
            Ok((
                bytes,
                Defer::Action(Box::new(
                    move |bytes: &[u8],
                          handles: &mut Vec<HandleInfo>,
                          counter: RecursionCounter| {
                        let counter = counter.next()?;
                        let align = alignment_padding_for_size(st.size);
                        let (bytes, defer) =
                            terminated(decode_struct_nonnull(ns, st), take_padding(align))
                                .parse(bytes)?;
                        defer.complete(bytes, handles, counter)
                    },
                )),
            ))
        }
    }
}

fn decode_struct_nonnull<'s>(
    ns: &'s library::Namespace,
    st: &'s library::Struct,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'s>> {
    move |mut bytes: &[u8]| {
        let mut offset = 0;
        let mut fields = Vec::new();

        for member in &st.members {
            let (remaining, result) =
                preceded(take_padding(member.offset - offset), decode_type(ns, &member.ty))
                    .parse(bytes)?;
            fields.push((member.name.clone(), result));
            bytes = remaining;
            offset = member.offset + member.ty.inline_size(ns)?;
        }

        if offset < st.size {
            let (remaining, _) = take_padding(st.size - offset).parse(bytes)?;
            bytes = remaining;
        }

        Ok((
            bytes,
            Defer::Action(Box::new(
                move |mut bytes: &[u8],
                      handles: &mut Vec<HandleInfo>,
                      counter: RecursionCounter| {
                    let mut complete_fields = Vec::new();

                    for (name, defer) in fields {
                        let (remaining, value) = defer.complete(bytes, handles, counter)?;
                        bytes = remaining;
                        complete_fields.push((name, value))
                    }

                    Ok((bytes, Value::Object(complete_fields)))
                },
            )),
        ))
    }
}

fn decode_type<'t>(
    ns: &'t library::Namespace,
    ty: &'t library::Type,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'t>> {
    use library::Type;
    move |b: &[u8]| {
        match ty {
            Type::Bool => value_bool(b),
            Type::U8 => value_u8(b),
            Type::U16 => value_u16(b),
            Type::U32 => value_u32(b),
            Type::U64 => value_u64(b),
            Type::I8 => value_i8(b),
            Type::I16 => value_i16(b),
            Type::I32 => value_i32(b),
            Type::I64 => value_i64(b),
            Type::F32 => value_f32(b),
            Type::F64 => value_f64(b),
            Type::Array(ty, size) => return decode_array(ns, ty, *size).parse(b),
            Type::Vector { ty, nullable, element_count } => {
                return decode_vector(ns, ty, *nullable, *element_count).parse(b);
            }
            Type::String { nullable, byte_count } => {
                return decode_string(*nullable, *byte_count).parse(b);
            }
            Type::Identifier { name, nullable } => {
                return decode_identifier(ns, name, *nullable).parse(b);
            }
            Type::Handle { object_type, nullable, rights } => {
                return decode_handle(*object_type, *nullable, *rights).parse(b);
            }
            Type::Endpoint { protocol, rights, nullable, role } => match role {
                library::EndpointRole::Client => {
                    return decode_client_end(
                        protocol.clone(),
                        *nullable,
                        rights.or(Some(DEFAULT_CHANNEL_RIGHTS)),
                    )
                    .parse(b);
                }
                library::EndpointRole::Server => {
                    return decode_server_end(
                        protocol.clone(),
                        *nullable,
                        rights.or(Some(DEFAULT_CHANNEL_RIGHTS)),
                    )
                    .parse(b);
                }
            },
            Type::UnknownString(s) => {
                Err(Error::LibraryError(format!("Unresolved Type: {}", s)).into())
            }
            Type::Unknown(library::TypeInfo { identifier: s, .. }) => {
                return Err(Error::LibraryError(format!(
                    "Unresolved Type: {}",
                    s.as_ref().map_or("<unidentified>", String::as_str)
                ))
                .into());
            }
            Type::FrameworkError => map(take_u32, |_| Value::Null).parse(b),
        }
        .map(|(x, y)| (x, Defer::Complete(y)))
    }
}

/// Given a list of defers, complete them all and turn them into a list of complete values.
fn complete_deferred_list<'a>(
    bytes: &'a [u8],
    handles: &mut Vec<HandleInfo>,
    defers: Vec<Defer<'_>>,
    counter: RecursionCounter,
) -> DResult<'a, Value> {
    let mut bytes = bytes;
    let mut values = Vec::new();

    for defer in defers {
        let (next_bytes, value) = defer.complete(bytes, handles, counter)?;
        bytes = next_bytes;
        values.push(value)
    }

    Ok((bytes, Value::List(values)))
}

fn decode_array<'t>(
    ns: &'t library::Namespace,
    ty: &'t library::Type,
    size: usize,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'t>> {
    move |bytes: &[u8]| {
        let (bytes, defers) = count(decode_type(ns, ty), size).parse(bytes)?;

        Ok((
            bytes,
            Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    complete_deferred_list(bytes, handles, defers, counter)
                },
            )),
        ))
    }
}

fn decode_vector<'t>(
    ns: &'t library::Namespace,
    ty: &'t library::Type,
    nullable: bool,
    element_count: Option<usize>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'t>> {
    move |bytes: &[u8]| {
        let (bytes, (size, presence)) = pair(take_u64, take_u64).parse(bytes)?;
        let size = size as usize;
        let Some(byte_count) = size.checked_mul(ty.inline_size(ns)?) else {
            return Err(Error::DecodeError("Vector too long".to_owned()).into());
        };
        let align = alignment_padding_for_size(byte_count);

        if presence == 0 {
            if nullable {
                if size == 0 {
                    Ok((bytes, Defer::Complete(Value::Null)))
                } else {
                    Err(Error::DecodeError("Absent vector had a size".to_owned()).into())
                }
            } else {
                Err(Error::DecodeError("Missing non-nullable vector".to_owned()).into())
            }
        } else if presence != ALLOC_PRESENT_U64 {
            Err(Error::DecodeError("Bad presence indicator".to_owned()).into())
        } else if element_count.map(|x| x < size).unwrap_or(false) {
            Err(Error::DecodeError("Vector too long".to_owned()).into())
        } else {
            Ok((
                bytes,
                Defer::Action(Box::new(
                    move |bytes: &[u8],
                          handles: &mut Vec<HandleInfo>,
                          counter: RecursionCounter| {
                        let counter = counter.next()?;
                        let (bytes, defers) =
                            terminated(count(decode_type(ns, ty), size), take_padding(align))
                                .parse(bytes)?;

                        complete_deferred_list(bytes, handles, defers, counter)
                    },
                )),
            ))
        }
    }
}

fn decode_string(
    nullable: bool,
    byte_count: Option<usize>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'static>> {
    move |bytes: &[u8]| {
        let (bytes, (size, presence)) = pair(take_u64, take_u64).parse(bytes)?;
        let size = size as usize;
        let align = alignment_padding_for_size(size);

        if presence == 0 {
            if nullable {
                if size == 0 {
                    Ok((bytes, Defer::Complete(Value::Null)))
                } else {
                    Err(Error::DecodeError("Absent string had a size".to_owned()).into())
                }
            } else {
                Err(Error::DecodeError("Missing non-nullable string".to_owned()).into())
            }
        } else if presence != ALLOC_PRESENT_U64 {
            Err(Error::DecodeError("Bad presence indicator".to_owned()).into())
        } else if byte_count.map(|x| x < size).unwrap_or(false) {
            Err(Error::DecodeError("String too long".to_owned()).into())
        } else {
            Ok((
                bytes,
                Defer::Action(Box::new(
                    move |bytes: &[u8], _: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                        let _counter = counter.next()?;
                        let (bytes, data) =
                            terminated(take(size), take_padding(align)).parse(bytes)?;

                        match str::from_utf8(data) {
                            Ok(x) => Ok((bytes, Value::String(x.to_owned()))),
                            Err(x) => Err(Error::Utf8Error(x).into()),
                        }
                    },
                )),
            ))
        }
    }
}

fn decode_server_end(
    interface: String,
    nullable: bool,
    rights: Option<Rights>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'static>> {
    decode_handle_with(
        interface,
        nullable,
        &|x, y, z| Value::ServerEnd(x.into(), y, z),
        Some(ObjectType::Channel),
        rights,
    )
}

fn decode_client_end(
    interface: String,
    nullable: bool,
    rights: Option<Rights>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'static>> {
    decode_handle_with(
        interface,
        nullable,
        &|x, y, z| Value::ClientEnd(x.into(), y, z),
        Some(ObjectType::Channel),
        rights,
    )
}

fn decode_handle(
    handle_type: ObjectType,
    nullable: bool,
    rights: Option<Rights>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'static>> {
    decode_handle_with(handle_type, nullable, &Value::Handle, None, rights)
}

fn decode_handle_with<T: Clone + 'static>(
    handle_type: T,
    nullable: bool,
    value_builder: &'static (impl Fn(NullableHandle, T, Option<Rights>) -> Value + 'static),
    constrain_type: Option<ObjectType>,
    constrain_rights: Option<Rights>,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'static>> {
    move |bytes: &[u8]| {
        let handle_type = handle_type.clone();
        let (bytes, presence) = take_u32(bytes)?;

        if presence == 0 {
            if nullable {
                Ok((bytes, Defer::Complete(Value::Null)))
            } else {
                Err(Error::DecodeError("Missing non-nullable handle".to_owned()).into())
            }
        } else if presence != ALLOC_PRESENT_U32 {
            Err(Error::DecodeError("Bad presence indicator".to_owned()).into())
        } else {
            Ok((
                bytes,
                Defer::Action(Box::new(
                    move |bytes: &[u8], handles: &mut Vec<HandleInfo>, _: RecursionCounter| {
                        if !handles.is_empty() {
                            if constrain_type.map(|x| x == handles[0].object_type()).unwrap_or(true)
                            {
                                let handle_info = handles.remove(0);

                                let decoded_rights = match (handle_info.rights(), constrain_rights)
                                {
                                    (Rights::SAME_RIGHTS, Some(_)) => Rights::SAME_RIGHTS,
                                    (handle_rights, Some(Rights::SAME_RIGHTS)) => handle_rights,
                                    (handle_rights, None) => handle_rights,
                                    (handle_rights, Some(constrain_rights)) => {
                                        if handle_rights.contains(constrain_rights) {
                                            constrain_rights
                                        } else {
                                            let mut missing = constrain_rights;
                                            missing.remove(handle_rights);
                                            return Err(Error::DecodeError(format!(
                                                "Insufficient handle rights, need {missing:?}"
                                            ))
                                            .into());
                                        }
                                    }
                                };

                                Ok((
                                    bytes,
                                    value_builder(
                                        handle_info.into_handle(),
                                        handle_type,
                                        Some(decoded_rights),
                                    ),
                                ))
                            } else {
                                Err(Error::DecodeError("Wrong handle type".to_owned()).into())
                            }
                        } else {
                            Err(Error::DecodeError("Too few handles".to_owned()).into())
                        }
                    },
                )),
            ))
        }
    }
}

fn decode_enum<'e>(
    ns: &'e library::Namespace,
    en: &'e library::Enum,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'e>> {
    move |bytes: &[u8]| {
        let (bytes, defer) = decode_type(ns, &en.ty).parse(bytes)?;
        Ok((
            bytes,
            Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    let (bytes, value) = defer.complete(bytes, handles, counter)?;

                    for member in &en.members {
                        if value == member.value || !en.strict {
                            return Ok((bytes, Value::Enum(en.name.to_owned(), Box::new(value))));
                        }
                    }

                    if en.strict {
                        Err(Error::DecodeError("Unknown Enum Variant.".to_owned()).into())
                    } else {
                        Ok((bytes, Value::Enum(en.name.to_owned(), Box::new(value))))
                    }
                },
            )),
        ))
    }
}

fn decode_bits<'b>(
    ns: &'b library::Namespace,
    bits: &'b library::Bits,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'b>> {
    move |bytes: &[u8]| {
        let (bytes, defer) = decode_type(ns, &bits.ty).parse(bytes)?;
        Ok((
            bytes,
            Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    let (bytes, value) = defer.complete(bytes, handles, counter)?;

                    let data = value.bits().ok_or_else(|| {
                        Error::LibraryError("Bits with non-integer type.".to_owned())
                    })?;

                    if bits.strict && data != data & bits.mask {
                        Err(Error::DecodeError("Invalid value for bits field.".to_owned()).into())
                    } else {
                        Ok((bytes, Value::Bits(bits.name.to_owned(), Box::new(value))))
                    }
                },
            )),
        ))
    }
}

/// Contents of an envelope header.
enum Envelope {
    Present { bytes: u32, handles: u16 },
    Inline { bytes: [u8; 4], handles: u16 },
    Empty,
}

impl Envelope {
    fn skip(&self) -> Defer<'static> {
        let (envelope_bytes, envelope_handles) = match self {
            Envelope::Present { bytes, handles } => (*bytes, *handles),
            Envelope::Inline { handles, .. } => (0, *handles),
            Envelope::Empty => return Defer::Complete(Value::Null),
        };

        Defer::Action(Box::new(
            move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                let _counter = counter.next()?;
                if (envelope_bytes & 7u32) != 0 {
                    return Err(Error::DecodeError("Invalid envelope size".to_owned()).into());
                }
                let envelope_bytes = envelope_bytes as usize;
                let envelope_handles = envelope_handles as usize;

                if envelope_handles > handles.len() {
                    Err(Error::DecodeError("Insufficient handles for envelope".to_owned()).into())
                } else if envelope_bytes > bytes.len() {
                    Err(Error::DecodeError("Insufficient bytes for envelope".to_owned()).into())
                } else {
                    *handles = handles.split_off(envelope_handles);
                    Ok((&bytes[envelope_bytes as usize..], Value::Null))
                }
            },
        ))
    }

    fn decode_type<'s>(
        &self,
        ns: &'s library::Namespace,
        ty: &'s library::Type,
    ) -> Result<Defer<'s>> {
        if let &Envelope::Empty = self {
            Ok(self.skip())
        } else if !ty.is_resolved(ns) {
            Ok(self.skip())
        } else if let Envelope::Inline { bytes, handles } = self {
            let (padding, ret) = decode_type(ns, ty).parse(bytes)?;
            take_padding(padding.len()).parse(padding)?;
            let expect_handles = *handles as usize;
            Ok(Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    let handle_count = handles.len();
                    let v = ret.complete(bytes, handles, counter)?;

                    let handles_used = handle_count - handles.len();
                    if handles_used != expect_handles {
                        Err(Error::DecodeError("Wrong number of handles in envelope".to_owned())
                            .into())
                    } else {
                        Ok(v)
                    }
                },
            )))
        } else if ty.inline_size(ns)? <= 4 {
            Err(Error::DecodeError("Envelope should be inline".to_owned()))
        } else {
            let Envelope::Present { bytes, handles } = self else { unreachable!() };
            let expect_bytes = *bytes as usize;
            let expect_handles = *handles as usize;
            Ok(Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    let counter = counter.next()?;
                    let bytes_start = bytes.len();
                    let align = alignment_padding_for_size(ty.inline_size(ns)?);
                    let (bytes, defer) =
                        terminated(decode_type(ns, ty), take_padding(align)).parse(bytes)?;

                    let handle_count = handles.len();
                    let (bytes, value) = defer.complete(bytes, handles, counter)?;
                    let handles_used = handle_count - handles.len();
                    let bytes_used = bytes_start - bytes.len();
                    if handles_used != expect_handles {
                        Err(Error::DecodeError("Wrong number of handles in envelope".to_owned())
                            .into())
                    } else if bytes_used != expect_bytes {
                        Err(Error::DecodeError("Wrong number of bytes in envelope".to_owned())
                            .into())
                    } else {
                        Ok((bytes, value))
                    }
                },
            )))
        }
    }

    fn take<'a>(empty_ok: bool) -> impl Fn(&'a [u8]) -> DResult<'a, Envelope> {
        move |bytes: &[u8]| {
            let (bytes, (envelope_bytes, envelope_handles, envelope_flags)) =
                (take_u32, take_u16, take_u16).parse(bytes)?;

            if envelope_bytes == 0 && envelope_handles == 0 && envelope_flags == 0 {
                if !empty_ok {
                    Err(Error::DecodeError("Unexpected empty envelope.".to_owned()).into())
                } else {
                    Ok((bytes, Envelope::Empty))
                }
            } else if envelope_flags == 0 {
                Ok((bytes, Envelope::Present { bytes: envelope_bytes, handles: envelope_handles }))
            } else if envelope_flags == 1 {
                Ok((
                    bytes,
                    Envelope::Inline {
                        bytes: envelope_bytes.to_le_bytes(),
                        handles: envelope_handles,
                    },
                ))
            } else {
                Err(Error::DecodeError("Unknown evelope flags.".to_owned()).into())
            }
        }
    }
}

fn decode_union<'u>(
    ns: &'u library::Namespace,
    union: &'u library::TableOrUnion,
    nullable: bool,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'u>> {
    move |bytes: &[u8]| {
        let (bytes, (ordinal, envelope)) = (take_u64, Envelope::take(nullable)).parse(bytes)?;

        match (ordinal, &envelope) {
            (0, Envelope::Empty) => return Ok((bytes, envelope.skip())),
            (0, _) => return Err(Error::DecodeError("Invalid Union block.".to_owned()).into()),
            _ => (),
        };

        match union.members.get(&ordinal) {
            None if union.strict => {
                Err(Error::DecodeError("Invalid Union ordinal.".to_owned()).into())
            }
            None => Ok((bytes, envelope.skip())),
            Some(member) => Ok((
                bytes,
                Defer::Action(Box::new(
                    move |bytes: &[u8],
                          handles: &mut Vec<HandleInfo>,
                          counter: RecursionCounter| {
                        let (bytes, inner) = envelope
                            .decode_type(ns, &member.ty)?
                            .complete(bytes, handles, counter)?;
                        Ok((
                            bytes,
                            Value::Union(
                                union.name.to_owned(),
                                member.name.to_owned(),
                                Box::new(inner),
                            ),
                        ))
                    },
                )),
            )),
        }
    }
}

fn decode_table<'t>(
    ns: &'t library::Namespace,
    table: &'t library::TableOrUnion,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'t>> {
    move |bytes: &[u8]| {
        let (bytes, (size, data_ptr)) = pair(take_u64, take_u64).parse(bytes)?;

        if data_ptr != ALLOC_PRESENT_U64 {
            return Err(Error::DecodeError("Bad presence indicator.".to_owned()).into());
        }

        Ok((
            bytes,
            Defer::Action(Box::new(
                move |bytes: &[u8], handles: &mut Vec<HandleInfo>, counter: RecursionCounter| {
                    let counter = counter.next()?;
                    let (mut bytes, envelopes) =
                        count(Envelope::take(true), size as usize).parse(bytes)?;

                    let mut result = Vec::new();
                    let mut expect_ord = 1u64;
                    for envelope in envelopes {
                        let member = table.members.get(&expect_ord);
                        expect_ord += 1;

                        let next_bytes = if let Some(member) = member {
                            let (next_bytes, val) = envelope
                                .decode_type(ns, &member.ty)?
                                .complete(bytes, handles, counter)?;
                            if !matches!(val, Value::Null) {
                                result.push((member.name.clone(), val));
                            }
                            next_bytes
                        } else {
                            envelope.skip().complete(bytes, handles, counter)?.0
                        };

                        bytes = next_bytes;
                    }

                    Ok((bytes, Value::Object(result)))
                },
            )),
        ))
    }
}

fn decode_identifier<'s>(
    ns: &'s library::Namespace,
    name: &'s str,
    nullable: bool,
) -> impl Fn(&[u8]) -> DResult<'_, Defer<'s>> {
    move |bytes: &[u8]| match ns.lookup(name)? {
        library::LookupResult::Bits(b) => decode_bits(ns, b).parse(bytes),
        library::LookupResult::Enum(e) => decode_enum(ns, e).parse(bytes),
        library::LookupResult::Struct(s) => decode_struct(ns, s, nullable).parse(bytes),
        library::LookupResult::Union(u) => decode_union(ns, u, nullable).parse(bytes),
        library::LookupResult::Table(t) => decode_table(ns, t).parse(bytes),
        library::LookupResult::Protocol(_) => Err(Error::DecodeError(format!(
            "Protocol names cannot be used as identifiers: {}",
            name
        ))
        .into()),
    }
}

/// Decode a FIDL request or response, depending on the direction header.
fn decode_message<'a>(
    ns: &library::Namespace,
    direction: Direction,
    bytes: &'a [u8],
    mut handles: Vec<HandleInfo>,
) -> Result<(TransactionHeader, Value)> {
    let (bytes, header) = transaction_header(bytes)?;

    let (_, method) = ns.lookup_method_ordinal(header.ordinal)?;

    let (message, has) = match direction {
        Direction::Request => (method.request.as_ref(), method.has_request),
        Direction::Response => (method.response.as_ref(), method.has_response),
    };

    if let Some(message) = message {
        let (bytes, defer) = decode_type(ns, message).parse(bytes)?;
        let (bytes, value) = defer.complete(bytes, &mut handles, RecursionCounter::new())?;

        if !bytes.is_empty() && (bytes.len() >= 8 || bytes.iter().any(|x| *x != 0)) {
            Err(Error::DecodeError(format!("{} bytes left over.", bytes.len())))
        } else if !handles.is_empty() {
            Err(Error::DecodeError(format!("{} handles left over.", handles.len())))
        } else {
            Ok((header, value))
        }
    } else if !has {
        Err(Error::DecodeError(format!(
            "Header indicates method {}, which has no {}.",
            method.name,
            direction.to_string()
        )))
    } else {
        Ok((header, Value::Null))
    }
}

/// Decode a FIDL request from a byte buffer and a list of handles.
pub fn decode_request(
    ns: &library::Namespace,
    bytes: &[u8],
    handles: Vec<HandleInfo>,
) -> Result<(TransactionHeader, Value)> {
    decode_message(ns, Direction::Request, bytes, handles)
}

/// Decode a FIDL response from a byte buffer and a list of handles.
pub fn decode_response(
    ns: &library::Namespace,
    bytes: &[u8],
    handles: Vec<HandleInfo>,
) -> Result<(TransactionHeader, Value)> {
    decode_message(ns, Direction::Response, bytes, handles)
}

/// Decode a FIDL value.
pub fn decode<'a>(
    ns: &library::Namespace,
    ty: &str,
    bytes: &'a [u8],
    mut handles: Vec<HandleInfo>,
) -> Result<Value> {
    if bytes.len() % 8 != 0 {
        return Err(Error::DecodeError("Unaligned encoded object".to_owned()));
    }
    let (bytes, defer) = decode_identifier(ns, ty, false).parse(bytes)?;
    let (bytes, value) = defer.complete(bytes, &mut handles, RecursionCounter::new())?;
    take_padding(bytes.len()).parse(bytes)?;

    if !bytes.is_empty() && (bytes.len() >= 8 || bytes.iter().any(|x| *x != 0)) {
        Err(Error::DecodeError(format!("{} bytes left over.", bytes.len())))
    } else if !handles.is_empty() {
        Err(Error::DecodeError(format!("{} handles left over.", handles.len())))
    } else {
        Ok(value)
    }
}
