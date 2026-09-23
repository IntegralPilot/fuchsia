// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_constants::{
    ALLOC_ABSENT_U32, ALLOC_ABSENT_U64, ALLOC_PRESENT_U32, ALLOC_PRESENT_U64, MAGIC_NUMBER_INITIAL,
};
use fidl_data_zx::{DEFAULT_CHANNEL_RIGHTS, ObjType as ObjectType, Rights};

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::handle::*;
use crate::library;
use crate::transaction::*;
use crate::util::*;
use crate::value::Value;

/// Callback used to encode out-of-line data after in-line data has been returned.
type DeferCallback<'n, 't> = dyn FnOnce(&mut EncodeBuffer<'n>, RecursionCounter) -> Result<()> + 't;

/// Turn a list of callbacks into one callback that calls each of the list in sequence.
fn combine_calls<'n: 't, 't>(calls: Vec<Box<DeferCallback<'n, 't>>>) -> Box<DeferCallback<'n, 't>> {
    Box::new(move |this, counter| {
        for call in calls {
            call(this, counter)?;
        }

        Ok(())
    })
}

enum HandleType<'s> {
    ClientEnd(&'s str),
    ServerEnd(&'s str),
    Bare,
}

///  Buffer containing a partially or fully encoded value.
struct EncodeBuffer<'n> {
    ns: &'n library::Namespace,
    bytes: Vec<u8>,
    handles: Vec<HandleDisposition>,
}

impl<'n> EncodeBuffer<'n> {
    /// Pad the output buffer so the next thing we add will be aligned to 8 bytes.
    fn align_8(&mut self) {
        self.bytes
            .extend(std::iter::repeat(0u8).take(alignment_padding_for_size(self.bytes.len())));
    }

    fn encode_transaction<'n_i: 't, 't>(
        ns: &'n_i library::Namespace,
        txid: u32,
        protocol_name: &str,
        direction: Direction,
        method_name: &str,
        value: Value,
    ) -> Result<(Vec<u8>, Vec<HandleDisposition>)> {
        let mut buf = EncodeBuffer { ns, bytes: Vec::new(), handles: Vec::new() };

        let protocol = match ns.lookup(protocol_name)? {
            library::LookupResult::Protocol(i) => Ok(i),
            _ => Err(Error::LibraryError(format!("Could not find protocol '{}'.", protocol_name))),
        }?;

        let method = protocol.methods.get(method_name).ok_or_else(|| {
            Error::LibraryError(format!(
                "Could not find method '{}' on protocol '{}'",
                method_name, protocol_name
            ))
        })?;

        let (ty, has) = match direction {
            Direction::Request => {
                if !method.has_response && txid != 0 {
                    return Err(Error::EncodeError(
                        "Non-zero transaction ID for one-way method.".to_owned(),
                    ));
                }
                (method.request.as_ref(), method.has_request)
            }
            Direction::Response => (method.response.as_ref(), method.has_response),
        };

        let dynamic_flags =
            if method.strict { DynamicFlags::empty() } else { DynamicFlags::FLEXIBLE };

        if !has {
            return Err(Error::LibraryError(format!(
                "Method '{}' on protocol '{}' has no {}",
                method_name,
                protocol_name,
                direction.to_string()
            )));
        }

        buf.bytes.extend(&txid.to_le_bytes());
        buf.bytes.extend(&AtRestFlags::USE_V2_WIRE_FORMAT.bits().to_le_bytes());
        buf.bytes.push(dynamic_flags.bits());
        buf.bytes.push(MAGIC_NUMBER_INITIAL);
        buf.bytes.extend(&method.ordinal.to_le_bytes());

        if let Some(ty) = ty {
            buf.encode_type(ty, value)?(&mut buf, RecursionCounter::new())?
        } else if !matches!(value, Value::Null) {
            return Err(Error::EncodeError("Value must be null.".to_owned()));
        } else {
        };
        buf.align_8();
        Ok((buf.bytes, buf.handles))
    }

    fn encode_struct_nonnull<'t>(
        &mut self,
        st: &'n library::Struct,
        value: Value,
        start_offset: usize,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let start_offset = self.bytes.len() - start_offset;

        let values = match value {
            Value::Object(s) => Ok(s),
            _ => Err(Error::EncodeError("Value is not a struct.".to_owned())),
        }?;

        let mut values = {
            let mut map = HashMap::with_capacity(values.len());

            for (k, v) in values {
                map.insert(k, v);
            }

            map
        };

        let mut calls = Vec::new();

        for member in &st.members {
            let value = values.remove(&member.name).unwrap_or(Value::Null);
            self.bytes.extend(
                std::iter::repeat(0u8).take(member.offset - (self.bytes.len() - start_offset)),
            );
            calls.push(self.encode_type(&member.ty, value)?);
        }

        if let Some((name, _)) = values.into_iter().next() {
            Err(Error::EncodeError(format!("Unknown struct member: {}", name)))
        } else {
            self.bytes
                .extend(std::iter::repeat(0u8).take(st.size - (self.bytes.len() - start_offset)));
            Ok(combine_calls(calls))
        }
    }

    fn encode_type<'t>(
        &mut self,
        ty: &'n library::Type,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        use library::Type::*;

        match ty {
            Unknown(_) | UnknownString(_) => {
                return Err(Error::LibraryError("Unknown type".to_owned()));
            }
            Bool => self.encode_raw(if bool::try_from(value)? { &[1u8] } else { &[0u8] }),
            U8 => self.encode_raw(&u8::try_from(value)?.to_le_bytes()),
            U16 => self.encode_raw(&u16::try_from(value)?.to_le_bytes()),
            U32 => self.encode_raw(&u32::try_from(value)?.to_le_bytes()),
            U64 => self.encode_raw(&u64::try_from(value)?.to_le_bytes()),
            I8 => self.encode_raw(&i8::try_from(value)?.to_le_bytes()),
            I16 => self.encode_raw(&i16::try_from(value)?.to_le_bytes()),
            I32 => self.encode_raw(&i32::try_from(value)?.to_le_bytes()),
            I64 => self.encode_raw(&i64::try_from(value)?.to_le_bytes()),
            F32 => self.encode_raw(&f32::try_from(value)?.to_le_bytes()),
            F64 => self.encode_raw(&f64::try_from(value)?.to_le_bytes()),
            Array(ty, size) => self.encode_array(ty, *size, value),
            Vector { ty, nullable, element_count } => {
                self.encode_vector(ty, *nullable, value, *element_count)
            }
            String { nullable, byte_count } => self.encode_string(*nullable, value, *byte_count),
            Handle { object_type, rights, nullable } => {
                self.encode_handle(*object_type, *rights, HandleType::Bare, *nullable, value)
            }
            FrameworkError => self.encode_raw(&[0, 0, 0, 0]),
            Endpoint { role, protocol, rights, nullable } => self.encode_handle(
                ObjectType::Channel,
                Some(rights.unwrap_or(DEFAULT_CHANNEL_RIGHTS)),
                match role {
                    library::EndpointRole::Client => HandleType::ClientEnd(protocol),
                    library::EndpointRole::Server => HandleType::ServerEnd(protocol),
                },
                *nullable,
                value,
            ),
            Identifier { name, nullable } => self.encode_identifier(name.clone(), *nullable, value),
        }
    }

    fn encode_raw<'t>(&mut self, data: &[u8]) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        self.bytes.extend(data);
        Ok(Box::new(|_, _| Ok(())))
    }

    fn encode_array<'t>(
        &mut self,
        ty: &'n library::Type,
        size: usize,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let values = if let Value::List(v) = value {
            Ok(v)
        } else {
            Err(Error::EncodeError("Expected a list".to_owned()))
        }?;

        if values.len() != size {
            return Err(Error::EncodeError(format!("Expected list of length {}", size)));
        }

        let mut calls = Vec::with_capacity(size);

        for value in values {
            calls.push(self.encode_type(ty, value)?);
        }

        Ok(combine_calls(calls))
    }

    fn encode_vector<'t>(
        &mut self,
        ty: &'n library::Type,
        nullable: bool,
        value: Value,
        element_count: Option<usize>,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let values = match (value, nullable) {
            (Value::Null, true) => Ok(None),
            (Value::Null, false) => {
                Err(Error::EncodeError("Got null for non-nullable list".to_owned()))
            }
            (Value::List(v), _) => Ok(Some(v)),
            _ => Err(Error::EncodeError("Expected a list".to_owned())),
        }?;

        if let Some(values) = values {
            if element_count.map(|x| x < values.len()).unwrap_or(false) {
                return Err(Error::EncodeError("Vector exceeded max size".to_owned()));
            }

            self.bytes.extend(&(values.len() as u64).to_le_bytes());
            self.bytes.extend(&ALLOC_PRESENT_U64.to_le_bytes());
            Ok(Box::new(move |this, counter| {
                let counter = counter.next()?;
                let mut calls = Vec::with_capacity(values.len());

                for value in values {
                    calls.push(this.encode_type(ty, value)?);
                }

                this.align_8();

                for call in calls {
                    call(this, counter)?;
                }

                Ok(())
            }))
        } else {
            self.bytes.extend(std::iter::repeat(0u8).take(16));
            Ok(Box::new(|_, _| Ok(())))
        }
    }

    fn encode_string<'t>(
        &mut self,
        nullable: bool,
        value: Value,
        byte_count: Option<usize>,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let string = match (value, nullable) {
            (Value::Null, true) => Ok(None),
            (Value::Null, false) => {
                Err(Error::EncodeError("Got null for non-nullable string".to_owned()))
            }
            (Value::String(s), _) => Ok(Some(s)),
            _ => Err(Error::EncodeError("Expected a string".to_owned())),
        }?;

        if let Some(string) = string {
            if byte_count.map(|x| x < string.len()).unwrap_or(false) {
                return Err(Error::EncodeError("String exceeded max size".to_owned()));
            }

            self.bytes.extend(&(string.len() as u64).to_le_bytes());
            self.bytes.extend(&ALLOC_PRESENT_U64.to_le_bytes());
            Ok(Box::new(move |this, counter| {
                let _counter = counter.next()?;
                this.bytes.extend(string.as_bytes());
                this.align_8();
                Ok(())
            }))
        } else {
            self.bytes.extend(std::iter::repeat(0u8).take(16));
            Ok(Box::new(|_, _| Ok(())))
        }
    }

    fn encode_handle<'t>(
        &mut self,
        object_type: ObjectType,
        rights: Option<Rights>,
        expect: HandleType<'_>,
        nullable: bool,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let (handle, handle_rights) = match (value, nullable, expect) {
            (Value::Null, true, _) => Ok((None, None)),
            (Value::Handle(h, _, _), true, _) if h.is_invalid() => Ok((None, None)),
            (Value::ServerEnd(h, _, _), true, _) if h.is_invalid() => Ok((None, None)),
            (Value::ClientEnd(h, _, _), true, _) if h.is_invalid() => Ok((None, None)),
            (Value::Handle(h, _, _), false, _) if h.is_invalid() => {
                Err(Error::EncodeError("Got invalid handle for non-nullable handle".to_owned()))
            }
            (Value::ServerEnd(h, _, _), false, _) if h.is_invalid() => {
                Err(Error::EncodeError("Got invalid handle for non-nullable handle".to_owned()))
            }
            (Value::ClientEnd(h, _, _), false, _) if h.is_invalid() => {
                Err(Error::EncodeError("Got invalid handle for non-nullable handle".to_owned()))
            }
            (Value::Null, false, _) => {
                Err(Error::EncodeError("Got null for non-nullable handle".to_owned()))
            }
            (Value::Handle(h, s, r), _, _) => {
                if s != object_type && s != ObjectType::None {
                    Err(Error::EncodeError(format!(
                        "Expected object type {object_type:?} got {s:?}"
                    )))
                } else {
                    Ok((Some(h), r))
                }
            }
            (Value::ServerEnd(h, s, r), _, HandleType::ServerEnd(expect))
            | (Value::ClientEnd(h, s, r), _, HandleType::ClientEnd(expect)) => {
                if expect != s {
                    Err(Error::EncodeError(format!(
                        "Expected endpoint for protocol {expect}, got one for {s}"
                    )))
                } else if object_type != ObjectType::Channel {
                    Err(Error::EncodeError(format!(
                        "Expected object type {object_type:?} got channel for protocol {s}"
                    )))
                } else {
                    Ok((Some(h.into()), r))
                }
            }
            (Value::ServerEnd(_, s, _), _, HandleType::ClientEnd(expect))
            | (Value::ClientEnd(_, s, _), _, HandleType::ServerEnd(expect)) => {
                if expect != s {
                    Err(Error::EncodeError(format!(
                        "Expected endpoint for protocol {expect}, got one for {s}"
                    )))
                } else if object_type != ObjectType::Channel {
                    Err(Error::EncodeError(format!(
                        "Expected object type {object_type:?} got channel for protocol {s}"
                    )))
                } else {
                    Err(Error::EncodeError(format!("Got wrong end of channel for {expect}")))
                }
            }
            (Value::ServerEnd(h, s, r), _, HandleType::Bare)
            | (Value::ClientEnd(h, s, r), _, HandleType::Bare) => {
                if object_type != ObjectType::Channel {
                    Err(Error::EncodeError(format!(
                        "Expected object type {object_type:?} got channel for protocol {s}"
                    )))
                } else {
                    Ok((Some(h.into()), r))
                }
            }
            _ => Err(Error::EncodeError("Expected a handle".to_owned())),
        }?;

        let encoded_rights = match (handle_rights, rights) {
            (Some(_), Some(rights)) => rights,
            (Some(handle_rights), None) => handle_rights,
            (None, Some(rights)) => rights,
            (None, None) => Rights::SAME_RIGHTS,
        };

        if let Some(handle) = handle {
            self.bytes.extend(&ALLOC_PRESENT_U32.to_le_bytes());
            Ok(Box::new(move |this, _| {
                this.handles.push(HandleDisposition::move_op(handle, object_type, encoded_rights));
                Ok(())
            }))
        } else {
            self.bytes.extend(&ALLOC_ABSENT_U32.to_le_bytes());
            Ok(Box::new(|_, _| Ok(())))
        }
    }

    fn encode_identifier<'t>(
        &mut self,
        name: String,
        nullable: bool,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        use library::LookupResult::*;
        match (self.ns.lookup(&name)?, nullable) {
            (Bits(b), false) => self.encode_bits(b, value),
            (Enum(e), false) => self.encode_enum(e, value),
            (Table(t), false) => self.encode_table(t, value),
            (Struct(s), nullable) => self.encode_struct(s, nullable, value),
            (Union(u), nullable) => self.encode_union(u, nullable, value),
            (Protocol(_), _) => Err(Error::LibraryError(format!(
                "Protocol names cannot be used as identifiers: {}",
                name
            ))),
            _ => Err(Error::LibraryError(format!("Type {} shouldn't be nullable", name))),
        }
    }

    fn encode_bits<'t>(
        &mut self,
        bits: &'n library::Bits,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let value = match value {
            Value::Bits(name, inner) => {
                if name == bits.name {
                    *inner
                } else {
                    return Err(Error::EncodeError(format!(
                        "Expected {}, got {}",
                        bits.name, name
                    )));
                }
            }
            _ => value,
        };

        // If we can't get the bits, this is probably the wrong type of value. Use 0 so we ignore
        // the mask check and let encode_type return the error message.
        let data = u64::try_from(&value).unwrap_or(0);

        if bits.strict && data & !bits.mask != 0 {
            Err(Error::EncodeError(format!("Invalid bits set on {}", bits.name)))
        } else {
            self.encode_type(&bits.ty, value)
        }
    }

    fn encode_enum<'t>(
        &mut self,
        en: &'n library::Enum,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let value = match value {
            Value::Enum(name, inner) => {
                if name == en.name {
                    *inner
                } else {
                    return Err(Error::EncodeError(format!("Expected {}, got {}", en.name, name)));
                }
            }
            _ => value,
        };

        for item in &en.members {
            if !en.strict || item.value.cast_equals(&value) {
                return self.encode_type(&en.ty, value);
            }
        }

        Err(Error::EncodeError("Invalid enum variant".to_owned()))
    }

    fn encode_struct<'t>(
        &mut self,
        st: &'n library::Struct,
        nullable: bool,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let value = match (value, nullable) {
            (Value::Null, true) => Ok(None),
            (Value::Null, false) => Err(Error::EncodeError("Struct can't be null".to_owned())),
            (value, _) => Ok(Some(value)),
        }?;

        if let Some(value) = value {
            if nullable {
                self.bytes.extend(&ALLOC_PRESENT_U64.to_le_bytes());
                Ok(Box::new(move |this, counter| {
                    let counter = counter.next()?;
                    let call = this.encode_struct_nonnull(st, value, 0)?;
                    this.align_8();
                    call(this, counter)
                }))
            } else {
                self.encode_struct_nonnull(st, value, 0)
            }
        } else {
            self.bytes.extend(&ALLOC_ABSENT_U64.to_le_bytes());
            Ok(Box::new(|_, _| Ok(())))
        }
    }

    fn encode_envelope<'t>(
        &mut self,
        ty: &'n library::Type,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let header_pos = self.bytes.len();
        self.bytes.extend(&[0u8; 8]);

        if let Value::Null = value {
            Ok(Box::new(|_, _| Ok(())))
        } else {
            Ok(Box::new(move |this, counter| {
                let counter = counter.next()?;
                let start = this.bytes.len();
                let handle_start = this.handles.len();

                let header = if ty.inline_size(this.ns)? > 4 {
                    let call = this.encode_type(ty, value)?;

                    this.align_8();
                    call(this, counter)?;
                    let size = (this.bytes.len() - start) as u32;
                    let handle_count = (this.handles.len() - handle_start) as u16;

                    debug_assert!(size > 0 || handle_count > 0);
                    let mut header = Vec::new();
                    header.extend(&size.to_le_bytes());
                    header.extend(&handle_count.to_le_bytes());
                    header.extend(&0u16.to_le_bytes());
                    header
                } else {
                    let mut header_buf =
                        EncodeBuffer { ns: this.ns, bytes: Vec::new(), handles: Vec::new() };
                    header_buf.encode_type(ty, value)?(&mut header_buf, counter)?;
                    let EncodeBuffer { bytes: mut header, handles, .. } = header_buf;
                    header.resize(4, 0);
                    header.extend(&(handles.len() as u16).to_le_bytes());
                    header.extend(&1u16.to_le_bytes());
                    this.handles.extend(handles);
                    header
                };

                this.bytes.splice(header_pos..(header_pos + header.len()), header.into_iter());
                Ok(())
            }))
        }
    }

    fn encode_union<'t>(
        &mut self,
        union: &'n library::TableOrUnion,
        nullable: bool,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let entry = match value {
            Value::Null => Ok(None),
            Value::Union(u, n, b) if *u == union.name => Ok(Some((n, *b))),
            _ => Err(Error::EncodeError(format!("Expected {}", union.name))),
        }?;

        if let Some((variant, value)) = entry {
            for member in union.members.values() {
                if *member.name == *variant {
                    self.bytes.extend(&member.ordinal.to_le_bytes());
                    return self.encode_envelope(&member.ty, value);
                }
            }

            Err(Error::EncodeError(format!("Unrecognized union variant: '{}'", variant)))
        } else if nullable {
            self.bytes.extend(std::iter::repeat(0u8).take(16));
            Ok(Box::new(|_, _| Ok(())))
        } else {
            Err(Error::EncodeError("Got null for non-nullable Union".to_owned()))
        }
    }

    fn encode_table<'t>(
        &mut self,
        table: &'n library::TableOrUnion,
        value: Value,
    ) -> Result<Box<DeferCallback<'n, 't>>>
    where
        'n: 't,
    {
        let values = match value {
            Value::Object(values) => Ok(values),
            _ => Err(Error::EncodeError(format!("Could not convert to {}", table.name))),
        }?;

        let mut values_array = Vec::new();
        for (value_name, value) in values {
            for (&ord, member) in &table.members {
                let array_idx = usize::try_from(ord - 1).unwrap();
                if values_array.len() <= array_idx {
                    values_array.resize_with(array_idx + 1, || None);
                }
                if *member.name == value_name {
                    values_array[array_idx] = Some((&member.ty, value));
                    break;
                }
            }
        }

        while values_array.last().map(|x| x.is_none()).unwrap_or(false) {
            values_array.pop();
        }

        self.bytes.extend(&(values_array.len() as u64).to_le_bytes());
        self.bytes.extend(&ALLOC_PRESENT_U64.to_le_bytes());

        Ok(Box::new(move |this, counter| {
            let counter = counter.next()?;
            let mut calls = Vec::with_capacity(values_array.len());

            for slot in values_array.into_iter() {
                if let Some((ty, item)) = slot {
                    calls.push(this.encode_envelope(ty, item)?);
                } else {
                    this.bytes.extend(&ALLOC_ABSENT_U64.to_le_bytes());
                }
            }

            for call in calls {
                call(this, counter)?;
            }

            Ok(())
        }))
    }
}

/// Serialize a value into a FIDL request.
pub fn encode_request(
    ns: &library::Namespace,
    txid: u32,
    protocol_name: &str,
    method_name: &str,
    value: Value,
) -> Result<(Vec<u8>, Vec<HandleDisposition>)> {
    EncodeBuffer::encode_transaction(
        ns,
        txid,
        protocol_name,
        Direction::Request,
        method_name,
        value,
    )
}

/// Serialize a value into a FIDL response.
pub fn encode_response(
    ns: &library::Namespace,
    txid: u32,
    protocol_name: &str,
    method_name: &str,
    value: Value,
) -> Result<(Vec<u8>, Vec<HandleDisposition>)> {
    EncodeBuffer::encode_transaction(
        ns,
        txid,
        protocol_name,
        Direction::Response,
        method_name,
        value,
    )
}

/// Serialize a value into a FIDL serialized value.
pub fn encode(
    ns: &library::Namespace,
    type_name: &str,
    nullable: bool,
    value: Value,
) -> Result<(Vec<u8>, Vec<HandleDisposition>)> {
    let mut buf = EncodeBuffer { ns, bytes: Vec::new(), handles: Vec::new() };
    let cb = buf.encode_identifier(type_name.to_owned(), nullable, value)?;
    buf.align_8();
    cb(&mut buf, RecursionCounter::new()).map(|_| (buf.bytes, buf.handles))
}
