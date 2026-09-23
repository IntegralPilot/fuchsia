// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_data_zx::{ObjType as ObjectType, Rights};
use serde::de::{Deserialize as DeserializeIface, Deserializer};
use serde_derive::Deserialize;
use std::collections::HashMap;

use std::sync::{Arc, Weak};

use crate::error::{Error, Result};
use crate::value::Value;

/// Trait for deserialized structs that have a `name` field. Basically gives us a structural type
/// for "Has a String field named 'name'."
trait Named {
    fn name(&self) -> String;
}

impl<T: Named> Named for Arc<T> {
    fn name(&self) -> String {
        Arc::as_ref(self).name()
    }
}

macro_rules! named {
    ($ident:ty) => {
        impl Named for $ident {
            fn name(&self) -> String {
                self.name.clone()
            }
        }
    };
}

/// Deserializer helper for ObjectType
fn object_type<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<ObjectType>, D::Error> {
    Ok(Option::<u32>::deserialize(deserializer)?.map(|x| ObjectType::from_raw(x)).flatten())
}

/// Deserializer helper for composed protocols
fn composed_protocols<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    struct ComposedProtocol {
        name: String,
    }
    Ok(Vec::<ComposedProtocol>::deserialize(deserializer)?.into_iter().map(|x| x.name).collect())
}

/// Deserializer helper for ObjectType
fn rights<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Rights>, D::Error> {
    if let Some(rights) = Option::<u32>::deserialize(deserializer)? {
        Ok(Some(Rights::from_bits_truncate(rights)))
    } else {
        Ok(None)
    }
}

/// Deserializer helper that gets a list of values, but then packs them into a hashmap by name.
fn hash_by_name<'de, D: Deserializer<'de>, T: Named + DeserializeIface<'de>>(
    deserializer: D,
) -> std::result::Result<HashMap<String, T>, D::Error> {
    let mut ret = HashMap::new();
    for item in Vec::<T>::deserialize(deserializer)? {
        ret.insert(item.name(), item);
    }
    Ok(ret)
}

/// Same as `hash_by_name` but wraps the inserted values in an Arc.
fn hash_by_name_arc<'de, D: Deserializer<'de>, T: Named + DeserializeIface<'de>>(
    deserializer: D,
) -> std::result::Result<HashMap<String, Arc<T>>, D::Error> {
    let mut ret = HashMap::new();
    for item in Vec::<T>::deserialize(deserializer)? {
        ret.insert(item.name(), Arc::new(item));
    }
    Ok(ret)
}

/// Deserializer helper that gets a list of TableOrUnionMembers, but then packs them into a
/// hashmap by ordinal.
fn hash_by_ordinal<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<HashMap<u64, TableOrUnionMember>, D::Error> {
    let mut ret = HashMap::new();
    for item in Vec::<TableOrUnionMember>::deserialize(deserializer)? {
        ret.insert(item.ordinal, item);
    }
    Ok(ret)
}

/// Gets the size for a struct declaration by stripping away a field shape object.
fn extract_struct_size<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<usize, D::Error> {
    Ok(TypeShape::deserialize(deserializer)?.inline_size)
}

/// Gets the offset for a struct member by stripping away a field shape object.
fn extract_member_offset<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<usize, D::Error> {
    Ok(MemberShape::deserialize(deserializer)?.offset)
}

#[derive(Deserialize)]
struct Module {
    name: String,
    #[serde(deserialize_with = "hash_by_name")]
    bits_declarations: HashMap<String, Bits>,
    #[serde(deserialize_with = "hash_by_name")]
    enum_declarations: HashMap<String, Enum>,
    #[serde(deserialize_with = "hash_by_name")]
    protocol_declarations: HashMap<String, Protocol>,
    #[serde(deserialize_with = "hash_by_name")]
    table_declarations: HashMap<String, TableOrUnion>,
    #[serde(deserialize_with = "hash_by_name")]
    struct_declarations: HashMap<String, Struct>,
    #[serde(deserialize_with = "hash_by_name")]
    union_declarations: HashMap<String, TableOrUnion>,
    declarations: HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct TableOrUnion {
    pub name: String,
    pub strict: bool,
    #[serde(deserialize_with = "hash_by_ordinal")]
    pub members: HashMap<u64, TableOrUnionMember>,
}
named!(TableOrUnion);

#[derive(Deserialize)]
pub struct TableOrUnionMember {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
    pub ordinal: u64,
}

#[derive(Deserialize)]
#[serde(try_from = "EnumExpanded")]
pub struct Enum {
    pub name: String,
    pub ty: Type,
    pub strict: bool,
    pub members: Vec<ValueMember>,
}
named!(Enum);

#[derive(Deserialize)]
#[serde(try_from = "BitsExpanded")]
pub struct Bits {
    pub name: String,
    pub ty: Type,
    pub strict: bool,
    pub mask: u64,
    pub members: Vec<ValueMember>,
}
named!(Bits);

pub struct ValueMember {
    pub name: String,
    pub value: Value,
}

#[derive(Deserialize)]
struct EnumExpanded {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    strict: bool,
    members: Vec<ValueMemberExpanded>,
}

#[derive(Deserialize)]
struct BitsExpanded {
    name: String,
    #[serde(rename = "type")]
    ty: Type,
    strict: bool,
    members: Vec<ValueMemberExpanded>,
    mask: String,
}

// We want to un-nest some of the nonsense in the bits declarations. this is how.
#[derive(Deserialize)]
struct ValueMemberExpanded {
    name: String,
    value: ValueBlock,
}

#[derive(Deserialize)]
struct ValueBlock {
    value: String,
}

#[derive(Debug)]
// Note: The inner members are used as part of the `Debug` impl to provide
// additional context about the error. Prefix them with `_` to avoid dead code
// warnings as they're unused elsewhere.
enum ValueConvertError {
    FloatError { _error: std::num::ParseFloatError },
    IntError { _error: std::num::ParseIntError },
    BadPrimitive { _error: String },
}

impl From<std::num::ParseFloatError> for ValueConvertError {
    fn from(item: std::num::ParseFloatError) -> ValueConvertError {
        ValueConvertError::FloatError { _error: item }
    }
}

impl From<std::num::ParseIntError> for ValueConvertError {
    fn from(item: std::num::ParseIntError) -> ValueConvertError {
        ValueConvertError::IntError { _error: item }
    }
}

impl std::fmt::Display for ValueConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

fn convert_expanded_value_member(
    ty: &Type,
    container: &str,
    members: Vec<ValueMemberExpanded>,
) -> std::result::Result<Vec<ValueMember>, ValueConvertError> {
    type ConverterType = dyn Fn(String) -> std::result::Result<Value, ValueConvertError>;

    let converter = match ty {
        Type::U8 => (&|x: String| Ok(Value::U8(x.parse::<u8>()?))) as &ConverterType,
        Type::U16 => (&|x: String| Ok(Value::U16(x.parse::<u16>()?))) as &ConverterType,
        Type::U32 => (&|x: String| Ok(Value::U32(x.parse::<u32>()?))) as &ConverterType,
        Type::U64 => (&|x: String| Ok(Value::U64(x.parse::<u64>()?))) as &ConverterType,
        Type::I8 => (&|x: String| Ok(Value::I8(x.parse::<i8>()?))) as &ConverterType,
        Type::I16 => (&|x: String| Ok(Value::I16(x.parse::<i16>()?))) as &ConverterType,
        Type::I32 => (&|x: String| Ok(Value::I32(x.parse::<i32>()?))) as &ConverterType,
        Type::I64 => (&|x: String| Ok(Value::I64(x.parse::<i64>()?))) as &ConverterType,
        Type::F32 => (&|x: String| Ok(Value::F32(x.parse::<f32>()?))) as &ConverterType,
        Type::F64 => (&|x: String| Ok(Value::F64(x.parse::<f64>()?))) as &ConverterType,
        _ => return Err(ValueConvertError::BadPrimitive { _error: container.to_owned() }),
    };

    let mut result = Vec::new();

    for member in members {
        result.push(ValueMember { name: member.name, value: converter(member.value.value)? });
    }

    Ok(result)
}

impl TryFrom<EnumExpanded> for Enum {
    type Error = ValueConvertError;

    fn try_from(exp: EnumExpanded) -> std::result::Result<Enum, Self::Error> {
        let (name, ty, strict, members) = (exp.name, exp.ty.into(), exp.strict, exp.members);
        let members = convert_expanded_value_member(&ty, &name, members)?;
        Ok(Enum { name, ty, strict, members })
    }
}

impl TryFrom<BitsExpanded> for Bits {
    type Error = ValueConvertError;

    fn try_from(exp: BitsExpanded) -> std::result::Result<Bits, Self::Error> {
        let (name, ty, strict, members, mask) =
            (exp.name, exp.ty, exp.strict, exp.members, exp.mask);
        let members = convert_expanded_value_member(&ty, &name, members)?;
        let mask = mask.parse::<u64>()?;
        Ok(Bits { name, ty, strict, members, mask })
    }
}

#[derive(Deserialize)]
pub struct Protocol {
    pub name: String,
    #[serde(deserialize_with = "hash_by_name_arc")]
    pub methods: HashMap<String, Arc<Method>>,
    #[serde(deserialize_with = "composed_protocols")]
    pub composed_protocols: Vec<String>,
}
named!(Protocol);

#[derive(Debug, Clone, Deserialize)]
pub struct Struct {
    pub name: String,
    pub members: Vec<Member>,
    #[serde(rename = "type_shape_v2", deserialize_with = "extract_struct_size")]
    pub size: usize,
}
named!(Struct);

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(rename = "field_shape_v2", deserialize_with = "extract_member_offset")]
    pub offset: usize,
}
named!(Member);

#[derive(Deserialize)]
struct MemberShape {
    offset: usize,
}

#[derive(Deserialize)]
struct TypeShape {
    inline_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct Method {
    pub name: String,
    pub ordinal: u64,
    pub strict: bool,
    pub has_request: bool,
    #[serde(rename = "maybe_request_payload")]
    pub request: Option<Type>,
    pub has_response: bool,
    #[serde(rename = "maybe_response_payload")]
    pub response: Option<Type>,
}
named!(Method);

#[derive(Clone, Deserialize, Debug)]
#[serde(from = "TypeInfo")]
pub enum Type {
    Unknown(TypeInfo),
    UnknownString(String),
    FrameworkError,
    Identifier { name: String, nullable: bool },
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Array(Box<Type>, usize),
    Vector { ty: Box<Type>, nullable: bool, element_count: Option<usize> },
    String { nullable: bool, byte_count: Option<usize> },
    Handle { object_type: ObjectType, rights: Option<Rights>, nullable: bool },
    Endpoint { role: EndpointRole, protocol: String, rights: Option<Rights>, nullable: bool },
}

impl Type {
    pub fn inline_size(&self, ns: &Namespace) -> Result<usize> {
        use Type::*;

        match self {
            Bool | U8 | I8 => Ok(1),
            U16 | I16 => Ok(2),
            FrameworkError | U32 | I32 | F32 | Handle { .. } | Endpoint { .. } => Ok(4),
            U64 | I64 | F64 => Ok(8),
            Vector { .. } | String { .. } => Ok(16),
            Array(a, b) => Ok(a.inline_size(ns)? * b),
            Identifier { name, nullable } => match ns.lookup(name)? {
                LookupResult::Bits(b) => b.ty.inline_size(ns),
                LookupResult::Enum(e) => e.ty.inline_size(ns),
                LookupResult::Struct(s) => Ok(if *nullable { 8 } else { s.size }),
                LookupResult::Table(_) => Ok(16),
                LookupResult::Union(_) => Ok(16),
                LookupResult::Protocol(_) => Err(Error::LibraryError(format!(
                    "Protocol names cannot be used as identifiers: {}",
                    name
                ))),
            },
            Unknown(_) | UnknownString(_) => {
                Err(Error::LibraryError("Cannot get size for unknown type.".to_owned()))
            }
        }
    }

    pub fn is_resolved(&self, ns: &Namespace) -> bool {
        match self {
            Type::Unknown(_) | Type::UnknownString(_) => false,
            Type::Identifier { name, .. } => ns.lookup(name).is_ok(),
            _ => true,
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
pub struct TypeInfo {
    #[serde(rename = "kind_v2")]
    pub kind: TypeKind,
    #[serde(default)]
    #[serde(rename = "obj_type")]
    #[serde(deserialize_with = "object_type")]
    pub object_type: Option<ObjectType>,
    #[serde(default)]
    #[serde(deserialize_with = "rights")]
    pub rights: Option<Rights>,
    pub identifier: Option<String>,
    pub protocol: Option<String>,
    pub role: Option<EndpointRole>,
    pub subtype: Option<String>,
    pub element_type: Box<Option<TypeInfo>>,
    pub element_count: Option<usize>,
    pub maybe_element_count: Option<usize>,
    #[serde(default)]
    pub nullable: bool,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    Primitive,
    Vector,
    Array,
    Handle,
    Identifier,
    String,
    Endpoint,
    Internal,

    #[serde(untagged)]
    Unknown(String),
}

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum EndpointRole {
    Client,
    Server,
}

impl From<TypeInfo> for Type {
    fn from(info: TypeInfo) -> Type {
        let nullable = info.nullable;

        match info.kind {
            TypeKind::Primitive => {
                let subtype = if let Some(x) = info.subtype.clone() {
                    x
                } else {
                    return Type::Unknown(info);
                };
                match subtype.into() {
                    Type::UnknownString(_) => Type::Unknown(info),
                    ty @ _ => ty,
                }
            }
            TypeKind::Vector => match *info.element_type {
                Some(t) => Type::Vector {
                    ty: Box::new(t.into()),
                    nullable,
                    element_count: info.maybe_element_count,
                },
                _ => Type::Unknown(info),
            },
            TypeKind::Array => {
                if info.element_type.is_some() && info.element_count.is_some() {
                    Type::Array(
                        Box::new(info.element_type.unwrap().into()),
                        info.element_count.unwrap(),
                    )
                } else {
                    Type::Unknown(info)
                }
            }
            TypeKind::Handle => {
                if let Some(object_type) = info.object_type {
                    Type::Handle { object_type, rights: info.rights, nullable }
                } else {
                    Type::Unknown(info)
                }
            }
            TypeKind::Identifier => {
                if let Some(identifier) = info.identifier {
                    Type::Identifier { name: identifier, nullable }
                } else {
                    Type::Unknown(info)
                }
            }
            TypeKind::String => Type::String { nullable, byte_count: info.maybe_element_count },
            TypeKind::Endpoint => {
                let Some(role) = info.role.clone() else { return Type::Unknown(info) };
                let Some(protocol) = info.protocol.clone() else { return Type::Unknown(info) };
                Type::Endpoint { role, protocol, rights: info.rights, nullable: info.nullable }
            }
            TypeKind::Internal => {
                if info.subtype.as_deref() == Some("framework_error") {
                    Type::FrameworkError
                } else {
                    Type::Unknown(info)
                }
            }
            TypeKind::Unknown(_) => Type::Unknown(info),
        }
    }
}

impl From<&str> for Type {
    fn from(name: &str) -> Type {
        match name.as_ref() {
            "bool" => Type::Bool,
            "uint8" => Type::U8,
            "uint16" => Type::U16,
            "uint32" => Type::U32,
            "uint64" => Type::U64,
            "int8" => Type::I8,
            "int16" => Type::I16,
            "int32" => Type::I32,
            "int64" => Type::I64,
            "float32" => Type::F32,
            "float64" => Type::F64,
            _ => Type::UnknownString(name.to_owned()),
        }
    }
}

impl From<String> for Type {
    fn from(name: String) -> Type {
        name.as_str().into()
    }
}

pub enum LookupResult<'a> {
    Bits(&'a Bits),
    Enum(&'a Enum),
    Protocol(&'a Protocol),
    Struct(&'a Struct),
    Table(&'a TableOrUnion),
    Union(&'a TableOrUnion),
}

/// A collection of loaded modules.
#[derive(Default)]
pub struct Namespace {
    modules: HashMap<String, Module>,
    methods_by_ordinal: HashMap<u64, (String, Weak<Method>)>,
}

impl Namespace {
    /// Create a new, empty namespace.
    pub fn new() -> Self {
        Namespace { modules: HashMap::new(), methods_by_ordinal: HashMap::new() }
    }

    /// Load a module from the text of its FIDL JSON file.
    pub fn load(&mut self, data: &str) -> Result<()> {
        let new_mod: Module = serde_json::from_str(data)?;

        for protocol in new_mod.protocol_declarations.values() {
            for method in protocol.methods.values() {
                self.methods_by_ordinal
                    .insert(method.ordinal, (protocol.name(), Arc::downgrade(method)));
            }
        }

        self.modules.insert(new_mod.name.clone(), new_mod);
        Ok(())
    }

    /// Look up a name in the loaded modules.
    pub fn lookup(&self, name: &str) -> Result<LookupResult<'_>> {
        let halves: Vec<_> = name.split('/').collect();

        if halves.len() != 2 {
            return Err(Error::LibraryError(format!(
                "Wrong number of path components in {}, expected 2",
                name
            )));
        }

        let module: &Module = match self.modules.get(halves[0]) {
            Some(x) => x,
            None => return Err(Error::LibraryError(format!("Module {} not found!", halves[0]))),
        };

        match module.declarations.get(name).map(|x| x.as_ref()) {
            Some("bits") => match module.bits_declarations.get(name) {
                Some(x) => Ok(LookupResult::Bits(x)),
                None => {
                    Err(Error::LibraryError(format!("{} not found in bits declarations!", name)))
                }
            },
            Some("enum") => match module.enum_declarations.get(name) {
                Some(x) => Ok(LookupResult::Enum(x)),
                None => {
                    Err(Error::LibraryError(format!("{} not found in enum declarations!", name)))
                }
            },
            Some("protocol") => match module.protocol_declarations.get(name) {
                Some(x) => Ok(LookupResult::Protocol(x)),
                None => Err(Error::LibraryError(format!(
                    "{} not found in protocol declarations!",
                    name
                ))),
            },
            Some("struct") => match module.struct_declarations.get(name) {
                Some(x) => Ok(LookupResult::Struct(x)),
                None => {
                    Err(Error::LibraryError(format!("{} not found in struct declarations!", name)))
                }
            },
            Some("table") => match module.table_declarations.get(name) {
                Some(x) => Ok(LookupResult::Table(x)),
                None => {
                    Err(Error::LibraryError(format!("{} not found in table declarations!", name)))
                }
            },
            Some("union") => match module.union_declarations.get(name) {
                Some(x) => Ok(LookupResult::Union(x)),
                None => {
                    Err(Error::LibraryError(format!("{} not found in union declarations!", name)))
                }
            },
            Some(x) => Err(Error::LibraryError(format!("{} has unknown type {}!", name, x))),
            None => Err(Error::LibraryError(format!("{} not found in {}!", name, module.name))),
        }
    }

    /// Return whether a protocol inherits from another.
    pub fn inherits(&self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }

        let Ok(LookupResult::Protocol(protocol)) = self.lookup(a) else {
            return false;
        };

        for composed_protocol in protocol.composed_protocols.iter() {
            if self.inherits(composed_protocol, b) {
                return true;
            }
        }

        false
    }

    /// Look up a method ordinal in the loaded modules.
    pub fn lookup_method_ordinal(&self, ordinal: u64) -> Result<(String, Arc<Method>)> {
        self.methods_by_ordinal
            .get(&ordinal)
            .and_then(|(x, y)| y.upgrade().map(|y| (x.clone(), y)))
            .ok_or_else(|| Error::LibraryError(format!("No method with ordinal {}", ordinal)))
    }
}
