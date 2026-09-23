// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{Result, TraceError};
use ffx_config::EnvironmentContext;
use fidl_codec_pure::library::LookupResult;
use itertools::Itertools;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::Path;
use zerocopy::FromBytes;
use zx_types::{zx_info_handle_basic_t, zx_obj_type_t, zx_rights_t};

/// This holds the state we use for translating FIDL method ordinals into names
/// and decoding FIDL messages from traces.
#[derive(Default)]
pub struct FidlLibraries {
    ns: fidl_codec_pure::library::Namespace,
    obj_types: OnceCell<BTreeMap<zx_obj_type_t, String>>,
    rights: OnceCell<BTreeMap<zx_rights_t, String>>,
}

impl FidlLibraries {
    pub fn from_context(env_ctx: &EnvironmentContext) -> Result<Self> {
        let mut map = Self {
            ns: fidl_codec_pure::library::Namespace::new(),
            obj_types: OnceCell::new(),
            rights: OnceCell::new(),
        };

        if let Some(build_dir) = env_ctx.build_dir() {
            let all_fidl_json_path = build_dir.join("all_fidl_json.txt");
            if all_fidl_json_path.exists() {
                for line in std::fs::read_to_string(all_fidl_json_path)?.lines() {
                    let ir_file_path = build_dir.join(line);
                    map.add_ir_file(&ir_file_path)?;
                }
            } else {
                return Err(TraceError::AllFidlJsonNotFound { path: build_dir.to_path_buf() });
            }
            Ok(map)
        } else {
            Err(TraceError::NoBuildDirectory)
        }
    }

    pub fn add_ir_file(&mut self, ir_file_path: impl AsRef<Path> + std::fmt::Debug) -> Result<()> {
        self.ns.load(&std::fs::read_to_string(ir_file_path.as_ref().to_str().unwrap())?)?;
        Ok(())
    }

    fn obj_types(&self) -> &BTreeMap<zx_obj_type_t, String> {
        self.obj_types.get_or_init(|| {
            let mut map = BTreeMap::new();
            if let Ok(LookupResult::Enum(decl)) = self.ns.lookup("zx/ObjType") {
                for member in &decl.members {
                    map.insert(
                        member.value.bits().unwrap() as zx_obj_type_t,
                        member.name.to_string(),
                    );
                }
            } else {
                eprintln!("WARNING: Could't find zx.ObjType declaration.");
            }

            map
        })
    }

    fn rights(&self) -> &BTreeMap<zx_rights_t, String> {
        self.rights.get_or_init(|| {
            let mut map = BTreeMap::new();
            if let Ok(LookupResult::Bits(decl)) = self.ns.lookup("zx/Rights") {
                for member in &decl.members {
                    map.insert(
                        member.value.bits().unwrap() as zx_rights_t,
                        member.name.to_string(),
                    );
                }
            } else {
                eprintln!("WARNING: Could't find zx.Rights declaration.");
            }

            map
        })
    }

    pub fn get(&self, ordinal: u64) -> Option<String> {
        match self.ns.lookup_method_ordinal(ordinal) {
            Ok((protocol_name, method)) => Some(format!("{protocol_name}.{}", method.name)),
            Err(_) => None,
        }
    }

    pub fn contains_key(&self, ordinal: u64) -> bool {
        self.ns.lookup_method_ordinal(ordinal).is_ok()
    }

    pub fn ns(&self) -> &fidl_codec_pure::library::Namespace {
        &self.ns
    }

    pub fn decode_message(&self, ordinal: u64, bytes: &[u8], handles: &[u8]) -> String {
        if bytes.len() <= 16 {
            // just a FIDL header
            return "".to_string();
        }

        let message = match FidlMessage::new(
            ordinal,
            bytes,
            handles,
            self.ns(),
            self.obj_types(),
            self.rights(),
        ) {
            Ok(decoder) => decoder,
            Err(e) => return format!("Error decoding kernel object info: {e}"),
        };

        match message.decode_to_string() {
            Ok(message) => message,
            Err(e) => format!("Error decoding message: {e}"),
        }
    }
}

/// Trim the library portion off of a FIDL identifier.
fn trim_identifier<'s>(identifier: &'s str) -> &'s str {
    identifier.split_once('/').map_or(identifier, |(_, suffix)| suffix)
}

/// Format a handle for display to the user.
/// The actual handle value is just an index into the handle table.
fn fmt_handle(
    handle: fidl_codec_pure::NullableHandle,
    message: &FidlMessage<'_, '_>,
    f: &mut impl std::fmt::Write,
) -> std::fmt::Result {
    // Look up handle info from zx_handle_t index we passed into fidl_codec.
    let info = message.object_info[handle.as_raw() as usize];

    // Show handle type.
    if let Some(name) = message.obj_types.get(&info.type_) {
        write!(f, "{name}(")?;
    } else {
        write!(f, "Handle<{t}>(", t = info.type_)?;
    }

    // Show koid.
    write!(f, "{}", info.koid)?;

    // Show rights.
    write!(f, ", rights=")?;
    let mut first_right = true;
    for b in 0..zx_rights_t::BITS {
        let mask = (1 as zx_rights_t) << b;
        if mask & info.rights == mask {
            if !first_right {
                write!(f, "|")?;
            } else {
                first_right = false;
            }
            if let Some(name) = message.rights.get(&mask) {
                write!(f, "{name}")?;
            } else {
                write!(f, "{mask:#x}")?;
            }
        }
    }
    if first_right {
        write!(f, "NONE")?;
    }

    // Show related koid if any.
    if info.related_koid != 0 {
        write!(f, ", related_koid={}", info.related_koid)?;
    }
    write!(f, ")")
}

/// Format a fidl_codec_pure::Value for display to the user.
fn fmt_value(
    value: fidl_codec_pure::Value,
    message: &FidlMessage<'_, '_>,
    f: &mut impl std::fmt::Write,
) -> std::fmt::Result {
    use fidl_codec_pure::Value::*;
    match value {
        Null => write!(f, "null"),
        Bool(value) => write!(f, "{value}"),
        U8(value) => write!(f, "{value}"),
        U16(value) => write!(f, "{value}"),
        U32(value) => write!(f, "{value}"),
        U64(value) => write!(f, "{value}"),
        I8(value) => write!(f, "{value}"),
        I16(value) => write!(f, "{value}"),
        I32(value) => write!(f, "{value}"),
        I64(value) => write!(f, "{value}"),
        F32(value) => write!(f, "{value}"),
        F64(value) => write!(f, "{value}"),
        String(value) => write!(f, "{value:?}"),
        Object(items) => {
            write!(f, "{{")?;
            let mut first = true;
            for (k, v) in items {
                if !first {
                    write!(f, ", ")?;
                } else {
                    first = false;
                }
                write!(f, "{k}: ")?;
                fmt_value(v, message, f)?;
            }
            write!(f, "}}")
        }
        Bits(id, value) => {
            let name = trim_identifier(&id);

            if let Ok(LookupResult::Bits(bits)) = message.ns.lookup(&id) {
                let members: Vec<_> = bits
                    .members
                    .iter()
                    .filter(|m| (m.value.bits().unwrap() & value.bits().unwrap()) != 0)
                    .map(|m| &m.name)
                    .collect();
                match members.len() {
                    0 => write!(f, "{name}({value})", value = value.bits().unwrap()),
                    1 => write!(f, "{name}.{member}", member = members[0]),
                    _ => {
                        write!(f, "{name}.({members})", members = members.into_iter().join("|"))
                    }
                }
            } else {
                write!(f, "name({value})")
            }
        }
        Enum(id, value) => {
            let name = trim_identifier(&id);

            let member = if let Ok(LookupResult::Enum(e)) = message.ns.lookup(&id) {
                e.members.iter().find(|m| m.value == *value)
            } else {
                None
            };
            if let Some(member) = member {
                write!(f, "{name}.{member}", member = member.name)
            } else {
                write!(f, "{name}({value})")
            }
        }
        Union(id, variant, value) => {
            write!(f, "{union_type}.{variant}({value})", union_type = trim_identifier(&id))
        }
        List(values) => {
            write!(f, "[")?;
            let mut first = true;
            for value in values {
                if !first {
                    write!(f, ", ")?;
                } else {
                    first = false;
                }
                fmt_value(value, message, f)?;
            }
            write!(f, "]")
        }
        ServerEnd(handle, id, _) => {
            write!(f, "server_end<{id}>(")?;
            fmt_handle(handle.into(), message, f)?;
            write!(f, ")")
        }
        ClientEnd(handle, id, _) => {
            write!(f, "client_end<{id}>(")?;
            fmt_handle(handle.into(), message, f)?;
            write!(f, ")")
        }
        Handle(handle, object_type, _) => {
            write!(f, "handle<{object_type:?}>(")?;
            fmt_handle(handle, message, f)?;
            write!(f, ")")
        }
        OutOfLine(_) => todo!(),
    }
}

/// Format a fidl_codec_pure::Value for display to the user.
fn value_to_string(value: fidl_codec_pure::Value, message: &FidlMessage<'_, '_>) -> Result<String> {
    let mut string = String::new();
    fmt_value(value, message, &mut string)?;
    Ok(string)
}

/// A type that holds and knows how to decode a FIDL message that's come out of
/// a trace.
struct FidlMessage<'a, 'b> {
    ordinal: u64,
    bytes: &'b [u8],
    object_info: Vec<zx_info_handle_basic_t>,
    obj_types: &'a BTreeMap<zx_obj_type_t, String>,
    rights: &'a BTreeMap<zx_rights_t, String>,
    ns: &'a fidl_codec_pure::library::Namespace,
}
impl<'a, 'b> FidlMessage<'a, 'b> {
    fn new(
        ordinal: u64,
        bytes: &'b [u8],
        object_info_bytes: &[u8],
        ns: &'a fidl_codec_pure::library::Namespace,
        obj_types: &'a BTreeMap<zx_obj_type_t, String>,
        rights: &'a BTreeMap<zx_rights_t, String>,
    ) -> Result<Self> {
        let object_info = object_info_bytes
            .chunks_exact(std::mem::size_of::<zx_info_handle_basic_t>())
            .map(|chunk| {
                zx_info_handle_basic_t::ref_from_bytes(chunk).map(|x| x.clone()).map_err(|_| {
                    let static_chunk: &'static [u8] = Box::leak(chunk.to_vec().into_boxed_slice());
                    let err = zx_info_handle_basic_t::ref_from_bytes(static_chunk).unwrap_err();
                    TraceError::ObjectInfoDecode(err)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { ordinal, bytes, object_info, obj_types, rights, ns })
    }

    /// Return a Vec of HandleInfo, one for each handle in |object_info|, but
    /// with the handle id of each handle being the index into |object_info|.
    fn make_handle_info(&self) -> Vec<fidl_codec_pure::HandleInfo> {
        self.object_info
            .iter()
            .enumerate()
            .map(|(i, info)| {
                fidl_codec_pure::HandleInfo::new(
                    fidl_codec_pure::NullableHandle::from_raw(i as u32),
                    fidl_codec_pure::ObjectType::from_raw(info.type_)
                        .unwrap_or(fidl_codec_pure::ObjectType::None),
                    fidl_codec_pure::Rights::from_bits_truncate(info.rights),
                )
            })
            .collect()
    }

    fn decode_to_string(&self) -> Result<String> {
        if self.bytes.len() <= 16 {
            // No FIDL body
            return Ok("".to_string());
        }
        let value = self.decode_to_value()?;
        value_to_string(value, self)
    }

    fn decode_to_value<'m>(&'m self) -> Result<fidl_codec_pure::Value> {
        let method = if let Ok((_, method)) = self.ns.lookup_method_ordinal(self.ordinal) {
            method
        } else {
            // Unknown method ordinal, can't decode the body into anything.
            return Err(TraceError::UnknownMethodOrdinal { ordinal: self.ordinal });
        };

        // Try decoding (if appropriate) the message as a request...
        let as_request = if method.has_request && method.request.is_some() {
            Some(self.decode_as_request())
        } else {
            None
        };

        // ...and as a response.
        let as_response = if method.has_response && method.response.is_some() {
            Some(self.decode_as_response())
        } else {
            None
        };

        // Decide what we're going to show to the user:
        match (as_request, as_response) {
            (None, None) => unreachable!(
                "FIDL transactions should have at least one of a request and a response."
            ),
            (Some(Ok(request)), Some(Ok(response))) => {
                eprintln!("Warning: Ambiguous request/response in decoding.");
                return Err(TraceError::AmbiguousDecoding { request, response });
            }
            (Some(Ok(value)), _) | (_, Some(Ok(value))) => Ok(value),
            (Some(Err(e)), _) | (_, Some(Err(e))) => Err(e),
        }
    }

    /// Try to decode this message, assuming it's a request.
    fn decode_as_request(&self) -> Result<fidl_codec_pure::Value> {
        let (_, body) =
            fidl_codec_pure::decode_request(&self.ns, self.bytes, self.make_handle_info())?;
        Ok(body)
    }

    /// Try to decode this message, assuming it's a response.
    fn decode_as_response(&self) -> Result<fidl_codec_pure::Value> {
        let (_, body) =
            fidl_codec_pure::decode_response(&self.ns, self.bytes, self.make_handle_info())?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zx_types::*;

    /// Format a Handle (and similar) for display to the user.
    fn handle_to_string(
        handle: fidl_codec_pure::NullableHandle,
        message: &FidlMessage<'_, '_>,
    ) -> Result<String> {
        let mut string = String::new();
        fmt_handle(handle, message, &mut string)?;
        Ok(string)
    }

    #[derive(Default)]
    struct FidlMessageForTest {
        ordinal: u64,
        bytes: Vec<u8>,
        object_info_bytes: Vec<u8>,
        ns: fidl_codec_pure::library::Namespace,
        obj_types: BTreeMap<zx_obj_type_t, String>,
        rights: BTreeMap<zx_rights_t, String>,
    }

    impl FidlMessageForTest {
        fn message<'t>(&'t self) -> FidlMessage<'t, 't> {
            FidlMessage::new(
                self.ordinal,
                &self.bytes,
                &self.object_info_bytes,
                &self.ns,
                &self.obj_types,
                &self.rights,
            )
            .expect("Making FidlMessage for test")
        }

        fn populate_types_and_rights(&mut self) {
            self.obj_types.extend(
                [(ZX_OBJ_TYPE_NONE, "None"), (ZX_OBJ_TYPE_PORT, "Port")]
                    .into_iter()
                    .map(|(k, v)| (k, v.into())),
            );
            self.rights.extend(
                [(ZX_RIGHT_DUPLICATE, "DUPLICATE"), (ZX_RIGHT_READ, "READ")]
                    .into_iter()
                    .map(|(k, v)| (k, v.into())),
            );
        }

        fn add_handle(
            &mut self,
            koid: zx_koid_t,
            rights: zx_rights_t,
            type_: zx_obj_type_t,
            related_koid: zx_koid_t,
        ) -> fidl_codec_pure::NullableHandle {
            let mut info = zx_info_handle_basic_t::default();
            info.koid = koid;
            info.rights = rights;
            info.type_ = type_;
            info.related_koid = related_koid;
            const INFO_SIZE: usize = std::mem::size_of::<zx_info_handle_basic_t>();
            let new_handle = fidl_codec_pure::NullableHandle::from_raw(
                (self.object_info_bytes.len() / INFO_SIZE) as zx_handle_t,
            );
            let mut bytes = [0u8; INFO_SIZE];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    &info as *const zx_info_handle_basic_t as *const u8,
                    bytes.as_mut_ptr(),
                    INFO_SIZE,
                );
            }
            self.object_info_bytes.extend_from_slice(bytes.as_slice());
            new_handle
        }
    }

    #[test]
    fn test_handle_info() {
        let mut for_test = FidlMessageForTest::default();
        for_test.populate_types_and_rights();

        // Empty message
        let empty = for_test.message();
        assert_eq!(empty.make_handle_info().len(), 0);

        // Message with one handle
        let h = for_test.add_handle(1234, 5, 6, 7890);
        let one_handle = for_test.message();
        let one_handle_info = one_handle.make_handle_info();
        assert_eq!(one_handle_info.len(), 1);
        assert_eq!(one_handle_info[0].object_type, fidl_codec_pure::ObjectType::Port);
        assert_eq!(one_handle_info[0].rights, fidl_codec_pure::Rights::from_bits_truncate(5));
        assert_eq!(
            "Port(1234, rights=DUPLICATE|READ, related_koid=7890)",
            handle_to_string(h, &one_handle).unwrap()
        );
    }

    #[test]
    fn test_basic_types() {
        let for_test = FidlMessageForTest::default();
        let message = for_test.message();
        let to_string = |value| value_to_string(value, &message).unwrap();

        use fidl_codec_pure::Value::*;
        assert_eq!("null", to_string(Null));
        assert_eq!("true", to_string(Bool(true)));
        assert_eq!("false", to_string(Bool(false)));
        assert_eq!("1", to_string(U8(1)));
        assert_eq!("2", to_string(U16(2)));
        assert_eq!("3", to_string(U32(3)));
        assert_eq!("4", to_string(U64(4)));
        assert_eq!("5", to_string(I8(5)));
        assert_eq!("6", to_string(I16(6)));
        assert_eq!("7", to_string(I32(7)));
        assert_eq!("8", to_string(I64(8)));
        assert_eq!("0.1", to_string(F32(0.1)));
        assert_eq!("0.2", to_string(F64(0.2)));
        assert_eq!(r#""Hello, world""#, to_string(String("Hello, world".to_string())));
    }

    #[test]
    fn test_compound_types() {
        let for_test = FidlMessageForTest::default();
        let message = for_test.message();
        let to_string = |value| value_to_string(value, &message).unwrap();

        use fidl_codec_pure::Value::*;

        // Lists (ie: FIDL arrays and vectors)
        assert_eq!(
            "[2, 3, 5, 7, 11, 13, 17, 19, 23]",
            to_string(List(vec![
                U8(2),
                U8(3),
                U8(5),
                U8(7),
                U8(11),
                U8(13),
                U8(17),
                U8(19),
                U8(23)
            ]))
        );

        // Objects (ie: FIDL structs and tables)
        assert_eq!(
            "{John: 1940, Paul: 1942, George: 1943, Ringo: 1940}",
            to_string(Object(vec!(
                ("John".to_string(), I64(1940)),
                ("Paul".to_string(), I64(1942)),
                ("George".to_string(), I64(1943)),
                ("Ringo".to_string(), I64(1940))
            )))
        );
    }
}
