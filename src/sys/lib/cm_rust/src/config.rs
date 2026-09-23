// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust_derive::FidlDecl;
use fidl_fuchsia_component_decl as fdecl;
use flyweights::FlyStr;
use from_enum::FromEnum;
use std::fmt;
use std::hash::Hash;

use crate::{FidlIntoNative, NativeIntoFidl};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

impl FidlIntoNative<FlyStr> for String {
    fn fidl_into_native(self) -> FlyStr {
        FlyStr::new(&self)
    }
}

impl NativeIntoFidl<String> for FlyStr {
    fn native_into_fidl(self) -> String {
        self.to_string()
    }
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigValuesData")]
pub struct ConfigValuesData {
    pub values: Box<[ConfigValueSpec]>,
    pub checksum: ConfigChecksum,
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigValueSpec")]
pub struct ConfigValueSpec {
    pub value: ConfigValue,
}

#[derive(FromEnum, FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_union = "fdecl::ConfigValue")]
pub enum ConfigValue {
    Single(ConfigSingleValue),
    Vector(ConfigVectorValue),
}

impl ConfigValue {
    /// Return the type of this value.
    pub fn ty(&self) -> ConfigValueType {
        match self {
            Self::Single(sv) => sv.ty(),
            Self::Vector(vv) => vv.ty(),
        }
    }

    /// Check if this value matches the type of another value.
    pub fn matches_type(&self, other: &ConfigValue) -> bool {
        match (self, other) {
            (ConfigValue::Single(a), ConfigValue::Single(b)) => {
                std::mem::discriminant(a) == std::mem::discriminant(b)
            }
            (ConfigValue::Vector(a), ConfigValue::Vector(b)) => {
                std::mem::discriminant(a) == std::mem::discriminant(b)
            }
            _ => false,
        }
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> Self {
        ConfigValue::Single(ConfigSingleValue::String(FlyStr::new(value)))
    }
}

impl From<Vec<&str>> for ConfigValue {
    fn from(value: Vec<&str>) -> Self {
        let value: Box<[FlyStr]> = value.into_iter().map(FlyStr::new).collect();
        ConfigValue::Vector(ConfigVectorValue::StringVector(value))
    }
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigOverride")]
pub struct ConfigOverride {
    // NB: Name would make more sense here but it imposes a strictness that breaks clients
    pub key: FlyStr,
    pub value: ConfigValue,
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigSchema")]
pub struct ConfigDecl {
    pub fields: Box<[ConfigField]>,
    pub checksum: ConfigChecksum,
    pub value_source: ConfigValueSource,
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_union = "fdecl::ConfigChecksum")]
pub enum ConfigChecksum {
    Sha256([u8; 32]),
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
#[derive(FidlDecl, Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigSourceCapabilities")]
pub struct ConfigSourceCapabilities {}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_union = "fdecl::ConfigValueSource")]
pub enum ConfigValueSource {
    PackagePath(FlyStr),
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    Capabilities(ConfigSourceCapabilities),
}

#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_table = "fdecl::ConfigField")]
pub struct ConfigField {
    pub key: FlyStr,
    pub type_: ConfigValueType,

    // This field will not be present in compiled manifests which predate F12.
    #[fidl_decl(default)]
    pub mutability: ConfigMutability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigNestedValueType {
    Bool,
    Uint8,
    Int8,
    Uint16,
    Int16,
    Uint32,
    Int32,
    Uint64,
    Int64,
    String { max_size: u32 },
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValueType {
    Bool,
    Uint8,
    Int8,
    Uint16,
    Int16,
    Uint32,
    Int32,
    Uint64,
    Int64,
    String { max_size: u32 },
    Vector { nested_type: ConfigNestedValueType, max_count: u32 },
}

impl ConfigValueType {
    pub fn get_max_size(&self) -> Option<u32> {
        match self {
            ConfigValueType::String { max_size } => Some(*max_size),
            ConfigValueType::Bool
            | ConfigValueType::Uint8
            | ConfigValueType::Int8
            | ConfigValueType::Uint16
            | ConfigValueType::Int16
            | ConfigValueType::Uint32
            | ConfigValueType::Int32
            | ConfigValueType::Uint64
            | ConfigValueType::Int64
            | ConfigValueType::Vector { .. } => None,
        }
    }

    pub fn get_nested_type(&self) -> Option<ConfigNestedValueType> {
        match self {
            ConfigValueType::Vector { nested_type, .. } => Some(nested_type.clone()),
            ConfigValueType::Bool
            | ConfigValueType::Uint8
            | ConfigValueType::Int8
            | ConfigValueType::Uint16
            | ConfigValueType::Int16
            | ConfigValueType::Uint32
            | ConfigValueType::Int32
            | ConfigValueType::Uint64
            | ConfigValueType::Int64
            | ConfigValueType::String { .. } => None,
        }
    }

    pub fn get_max_count(&self) -> Option<u32> {
        match self {
            ConfigValueType::Vector { max_count, .. } => Some(*max_count),
            ConfigValueType::Bool
            | ConfigValueType::Uint8
            | ConfigValueType::Int8
            | ConfigValueType::Uint16
            | ConfigValueType::Int16
            | ConfigValueType::Uint32
            | ConfigValueType::Int32
            | ConfigValueType::Uint64
            | ConfigValueType::Int64
            | ConfigValueType::String { .. } => None,
        }
    }
}

macro_rules! generate_configvalue_from {
    ($name:expr, $type:ty) => {
        impl From<$type> for ConfigValue {
            fn from(value: $type) -> Self {
                $name(value.into())
            }
        }
    };
}

generate_configvalue_from!(ConfigValue::Single, bool);
generate_configvalue_from!(ConfigValue::Single, u8);
generate_configvalue_from!(ConfigValue::Single, u16);
generate_configvalue_from!(ConfigValue::Single, u32);
generate_configvalue_from!(ConfigValue::Single, u64);
generate_configvalue_from!(ConfigValue::Single, i8);
generate_configvalue_from!(ConfigValue::Single, i16);
generate_configvalue_from!(ConfigValue::Single, i32);
generate_configvalue_from!(ConfigValue::Single, i64);
generate_configvalue_from!(ConfigValue::Single, String);
generate_configvalue_from!(ConfigValue::Vector, Box<[bool]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[u8]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[u16]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[u32]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[u64]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[i8]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[i16]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[i32]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[i64]>);
generate_configvalue_from!(ConfigValue::Vector, Box<[String]>);
generate_configvalue_from!(ConfigValue::Vector, Vec<bool>);
generate_configvalue_from!(ConfigValue::Vector, Vec<u8>);
generate_configvalue_from!(ConfigValue::Vector, Vec<u16>);
generate_configvalue_from!(ConfigValue::Vector, Vec<u32>);
generate_configvalue_from!(ConfigValue::Vector, Vec<u64>);
generate_configvalue_from!(ConfigValue::Vector, Vec<i8>);
generate_configvalue_from!(ConfigValue::Vector, Vec<i16>);
generate_configvalue_from!(ConfigValue::Vector, Vec<i32>);
generate_configvalue_from!(ConfigValue::Vector, Vec<i64>);
generate_configvalue_from!(ConfigValue::Vector, Vec<String>);

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigValue::Single(sv) => sv.fmt(f),
            ConfigValue::Vector(lv) => lv.fmt(f),
        }
    }
}

impl From<String> for ConfigSingleValue {
    fn from(s: String) -> Self {
        Self::String(FlyStr::new(&s))
    }
}

#[derive(FromEnum, FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_union = "fdecl::ConfigSingleValue")]
pub enum ConfigSingleValue {
    Bool(bool),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    String(FlyStr),
}

impl ConfigSingleValue {
    fn ty(&self) -> ConfigValueType {
        match self {
            ConfigSingleValue::Bool(_) => ConfigValueType::Bool,
            ConfigSingleValue::Uint8(_) => ConfigValueType::Uint8,
            ConfigSingleValue::Uint16(_) => ConfigValueType::Uint16,
            ConfigSingleValue::Uint32(_) => ConfigValueType::Uint32,
            ConfigSingleValue::Uint64(_) => ConfigValueType::Uint64,
            ConfigSingleValue::Int8(_) => ConfigValueType::Int8,
            ConfigSingleValue::Int16(_) => ConfigValueType::Int16,
            ConfigSingleValue::Int32(_) => ConfigValueType::Int32,
            ConfigSingleValue::Int64(_) => ConfigValueType::Int64,
            // We substitute the max size limit because the value itself doesn't carry the info.
            ConfigSingleValue::String(_) => ConfigValueType::String { max_size: u32::MAX },
        }
    }
}

impl fmt::Display for ConfigSingleValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ConfigSingleValue::*;
        match self {
            Bool(v) => write!(f, "{}", v),
            Uint8(v) => write!(f, "{}", v),
            Uint16(v) => write!(f, "{}", v),
            Uint32(v) => write!(f, "{}", v),
            Uint64(v) => write!(f, "{}", v),
            Int8(v) => write!(f, "{}", v),
            Int16(v) => write!(f, "{}", v),
            Int32(v) => write!(f, "{}", v),
            Int64(v) => write!(f, "{}", v),
            String(v) => write!(f, "\"{}\"", v),
        }
    }
}

impl From<Box<[String]>> for ConfigVectorValue {
    fn from(v: Box<[String]>) -> Self {
        let fly_v: Box<[FlyStr]> = v.into_vec().into_iter().map(|s| FlyStr::new(&s)).collect();
        Self::StringVector(fly_v)
    }
}

#[derive(FromEnum, FidlDecl, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[fidl_decl(fidl_union = "fdecl::ConfigVectorValue")]
pub enum ConfigVectorValue {
    BoolVector(Box<[bool]>),
    Uint8Vector(Box<[u8]>),
    Uint16Vector(Box<[u16]>),
    Uint32Vector(Box<[u32]>),
    Uint64Vector(Box<[u64]>),
    Int8Vector(Box<[i8]>),
    Int16Vector(Box<[i16]>),
    Int32Vector(Box<[i32]>),
    Int64Vector(Box<[i64]>),
    StringVector(Box<[FlyStr]>),
}

impl From<Vec<bool>> for ConfigVectorValue {
    fn from(v: Vec<bool>) -> Self {
        Self::BoolVector(v.into())
    }
}

impl From<Vec<u8>> for ConfigVectorValue {
    fn from(v: Vec<u8>) -> Self {
        Self::Uint8Vector(v.into())
    }
}

impl From<Vec<u16>> for ConfigVectorValue {
    fn from(v: Vec<u16>) -> Self {
        Self::Uint16Vector(v.into())
    }
}

impl From<Vec<u32>> for ConfigVectorValue {
    fn from(v: Vec<u32>) -> Self {
        Self::Uint32Vector(v.into())
    }
}

impl From<Vec<u64>> for ConfigVectorValue {
    fn from(v: Vec<u64>) -> Self {
        Self::Uint64Vector(v.into())
    }
}

impl From<Vec<i8>> for ConfigVectorValue {
    fn from(v: Vec<i8>) -> Self {
        Self::Int8Vector(v.into())
    }
}

impl From<Vec<i16>> for ConfigVectorValue {
    fn from(v: Vec<i16>) -> Self {
        Self::Int16Vector(v.into())
    }
}

impl From<Vec<i32>> for ConfigVectorValue {
    fn from(v: Vec<i32>) -> Self {
        Self::Int32Vector(v.into())
    }
}

impl From<Vec<i64>> for ConfigVectorValue {
    fn from(v: Vec<i64>) -> Self {
        Self::Int64Vector(v.into())
    }
}

impl From<Vec<String>> for ConfigVectorValue {
    fn from(v: Vec<String>) -> Self {
        let shared_v: Box<[FlyStr]> = v.into_iter().map(|s| FlyStr::new(&s)).collect();
        Self::StringVector(shared_v)
    }
}

impl ConfigVectorValue {
    fn ty(&self) -> ConfigValueType {
        // We substitute the max size limit because the value itself doesn't carry the info.
        match self {
            ConfigVectorValue::BoolVector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Bool,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Uint8Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Uint8,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Uint16Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Uint16,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Uint32Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Uint32,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Uint64Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Uint64,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Int8Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Int8,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Int16Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Int16,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Int32Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Int32,
                max_count: u32::MAX,
            },
            ConfigVectorValue::Int64Vector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::Int64,
                max_count: u32::MAX,
            },
            ConfigVectorValue::StringVector(_) => ConfigValueType::Vector {
                nested_type: ConfigNestedValueType::String { max_size: u32::MAX },
                max_count: u32::MAX,
            },
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    // TODO(https://fxbug.dev/42075220) uncomment once bitflags is updated
    // pub struct ConfigMutability: <fdecl::ConfigMutability as bitflags::BitFlags>::Bits {
    pub struct ConfigMutability: u32 {
        const PARENT = fdecl::ConfigMutability::PARENT.bits();
    }
}

#[cfg(feature = "serde")]
bitflags_serde_legacy::impl_traits!(ConfigMutability);

impl NativeIntoFidl<fdecl::ConfigMutability> for ConfigMutability {
    fn native_into_fidl(self) -> fdecl::ConfigMutability {
        fdecl::ConfigMutability::from_bits_allow_unknown(self.bits())
    }
}

impl FidlIntoNative<ConfigMutability> for fdecl::ConfigMutability {
    fn fidl_into_native(self) -> ConfigMutability {
        ConfigMutability::from_bits_retain(self.bits())
    }
}

impl fmt::Display for ConfigVectorValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ConfigVectorValue::*;
        macro_rules! print_list {
            ($f:ident, $list:ident) => {{
                $f.write_str("[")?;

                for (i, item) in $list.iter().enumerate() {
                    if i > 0 {
                        $f.write_str(", ")?;
                    }
                    write!($f, "{}", item)?;
                }

                $f.write_str("]")
            }};
        }
        match self {
            BoolVector(l) => print_list!(f, l),
            Uint8Vector(l) => print_list!(f, l),
            Uint16Vector(l) => print_list!(f, l),
            Uint32Vector(l) => print_list!(f, l),
            Uint64Vector(l) => print_list!(f, l),
            Int8Vector(l) => print_list!(f, l),
            Int16Vector(l) => print_list!(f, l),
            Int32Vector(l) => print_list!(f, l),
            Int64Vector(l) => print_list!(f, l),
            StringVector(l) => {
                f.write_str("[")?;
                for (i, item) in l.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "\"{}\"", item)?;
                }
                f.write_str("]")
            }
        }
    }
}

impl FidlIntoNative<ConfigNestedValueType> for fdecl::ConfigType {
    fn fidl_into_native(mut self) -> ConfigNestedValueType {
        match self.layout {
            fdecl::ConfigTypeLayout::Bool => ConfigNestedValueType::Bool,
            fdecl::ConfigTypeLayout::Uint8 => ConfigNestedValueType::Uint8,
            fdecl::ConfigTypeLayout::Uint16 => ConfigNestedValueType::Uint16,
            fdecl::ConfigTypeLayout::Uint32 => ConfigNestedValueType::Uint32,
            fdecl::ConfigTypeLayout::Uint64 => ConfigNestedValueType::Uint64,
            fdecl::ConfigTypeLayout::Int8 => ConfigNestedValueType::Int8,
            fdecl::ConfigTypeLayout::Int16 => ConfigNestedValueType::Int16,
            fdecl::ConfigTypeLayout::Int32 => ConfigNestedValueType::Int32,
            fdecl::ConfigTypeLayout::Int64 => ConfigNestedValueType::Int64,
            fdecl::ConfigTypeLayout::String => {
                let max_size =
                    if let fdecl::LayoutConstraint::MaxSize(s) = self.constraints.remove(0) {
                        s
                    } else {
                        panic!("Unexpected constraint on String layout type for config field");
                    };
                ConfigNestedValueType::String { max_size }
            }
            fdecl::ConfigTypeLayout::Vector => {
                panic!("Nested vectors are not supported in structured config")
            }
            fdecl::ConfigTypeLayoutUnknown!() => panic!("Unknown layout type for config field"),
        }
    }
}

impl NativeIntoFidl<fdecl::ConfigType> for ConfigNestedValueType {
    fn native_into_fidl(self) -> fdecl::ConfigType {
        let layout = match self {
            ConfigNestedValueType::Bool => fdecl::ConfigTypeLayout::Bool,
            ConfigNestedValueType::Uint8 => fdecl::ConfigTypeLayout::Uint8,
            ConfigNestedValueType::Uint16 => fdecl::ConfigTypeLayout::Uint16,
            ConfigNestedValueType::Uint32 => fdecl::ConfigTypeLayout::Uint32,
            ConfigNestedValueType::Uint64 => fdecl::ConfigTypeLayout::Uint64,
            ConfigNestedValueType::Int8 => fdecl::ConfigTypeLayout::Int8,
            ConfigNestedValueType::Int16 => fdecl::ConfigTypeLayout::Int16,
            ConfigNestedValueType::Int32 => fdecl::ConfigTypeLayout::Int32,
            ConfigNestedValueType::Int64 => fdecl::ConfigTypeLayout::Int64,
            ConfigNestedValueType::String { .. } => fdecl::ConfigTypeLayout::String,
        };
        let constraints = match self {
            ConfigNestedValueType::String { max_size } => {
                vec![fdecl::LayoutConstraint::MaxSize(max_size)]
            }
            _ => vec![],
        };
        fdecl::ConfigType { layout, constraints, parameters: Some(vec![]) }
    }
}

impl FidlIntoNative<ConfigValueType> for fdecl::ConfigType {
    fn fidl_into_native(mut self) -> ConfigValueType {
        match self.layout {
            fdecl::ConfigTypeLayout::Bool => ConfigValueType::Bool,
            fdecl::ConfigTypeLayout::Uint8 => ConfigValueType::Uint8,
            fdecl::ConfigTypeLayout::Uint16 => ConfigValueType::Uint16,
            fdecl::ConfigTypeLayout::Uint32 => ConfigValueType::Uint32,
            fdecl::ConfigTypeLayout::Uint64 => ConfigValueType::Uint64,
            fdecl::ConfigTypeLayout::Int8 => ConfigValueType::Int8,
            fdecl::ConfigTypeLayout::Int16 => ConfigValueType::Int16,
            fdecl::ConfigTypeLayout::Int32 => ConfigValueType::Int32,
            fdecl::ConfigTypeLayout::Int64 => ConfigValueType::Int64,
            fdecl::ConfigTypeLayout::String => {
                let max_size = if let fdecl::LayoutConstraint::MaxSize(s) =
                    self.constraints.remove(0)
                {
                    s
                } else {
                    panic!(
                        "Unexpected constraint on String layout type for config field. Expected MaxSize."
                    );
                };
                ConfigValueType::String { max_size }
            }
            fdecl::ConfigTypeLayout::Vector => {
                let max_count = if let fdecl::LayoutConstraint::MaxSize(c) =
                    self.constraints.remove(0)
                {
                    c
                } else {
                    panic!(
                        "Unexpected constraint on Vector layout type for config field. Expected MaxSize."
                    );
                };
                let mut parameters =
                    self.parameters.expect("Config field must have parameters set");
                let nested_type = if let fdecl::LayoutParameter::NestedType(nested_type) =
                    parameters.remove(0)
                {
                    nested_type.fidl_into_native()
                } else {
                    panic!(
                        "Unexpected parameter on Vector layout type for config field. Expected NestedType."
                    );
                };
                ConfigValueType::Vector { max_count, nested_type }
            }
            fdecl::ConfigTypeLayoutUnknown!() => panic!("Unknown layout type for config field"),
        }
    }
}

impl NativeIntoFidl<fdecl::ConfigType> for ConfigValueType {
    fn native_into_fidl(self) -> fdecl::ConfigType {
        let layout = match self {
            ConfigValueType::Bool => fdecl::ConfigTypeLayout::Bool,
            ConfigValueType::Uint8 => fdecl::ConfigTypeLayout::Uint8,
            ConfigValueType::Uint16 => fdecl::ConfigTypeLayout::Uint16,
            ConfigValueType::Uint32 => fdecl::ConfigTypeLayout::Uint32,
            ConfigValueType::Uint64 => fdecl::ConfigTypeLayout::Uint64,
            ConfigValueType::Int8 => fdecl::ConfigTypeLayout::Int8,
            ConfigValueType::Int16 => fdecl::ConfigTypeLayout::Int16,
            ConfigValueType::Int32 => fdecl::ConfigTypeLayout::Int32,
            ConfigValueType::Int64 => fdecl::ConfigTypeLayout::Int64,
            ConfigValueType::String { .. } => fdecl::ConfigTypeLayout::String,
            ConfigValueType::Vector { .. } => fdecl::ConfigTypeLayout::Vector,
        };
        let (constraints, parameters) = match self {
            ConfigValueType::String { max_size } => {
                (vec![fdecl::LayoutConstraint::MaxSize(max_size)], vec![])
            }
            ConfigValueType::Vector { max_count, nested_type } => {
                let nested_type = nested_type.native_into_fidl();
                (
                    vec![fdecl::LayoutConstraint::MaxSize(max_count)],
                    vec![fdecl::LayoutParameter::NestedType(nested_type)],
                )
            }
            _ => (vec![], vec![]),
        };
        fdecl::ConfigType { layout, constraints, parameters: Some(parameters) }
    }
}
