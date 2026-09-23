// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! Library for resolving and encoding the runtime configuration values of a component.

use cm_rust::{
    ConfigChecksum, ConfigDecl, ConfigField as ConfigFieldDecl, ConfigMutability,
    ConfigNestedValueType, ConfigOverride, ConfigSingleValue, ConfigValue, ConfigValueType,
    ConfigValuesData, ConfigVectorValue, NativeIntoFidl,
};
use dynfidl::{BasicField, Field, Structure, VectorField};
use fidl_fuchsia_component_decl as fdecl;
use flyweights::FlyStr;
use thiserror::Error;

/// The resolved configuration for a component.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigFields {
    /// A list of all resolved fields, in the order of the compiled manifest.
    pub fields: Vec<ConfigField>,
    /// A checksum from the compiled manifest.
    pub checksum: ConfigChecksum,
}

impl ConfigFields {
    /// Resolve a component's configuration values according to its declared schema. Should not fail if
    /// `decl` and `specs` are well-formed.
    pub fn resolve(
        decl: &ConfigDecl,
        base_values: ConfigValuesData,
        parent_overrides: Option<&[ConfigOverride]>,
    ) -> Result<Self, ResolutionError> {
        // base values must have been packaged for the exact same layout as the decl contains
        if decl.checksum != base_values.checksum {
            return Err(ResolutionError::ChecksumFailure {
                expected: decl.checksum.clone(),
                received: base_values.checksum,
            });
        }

        // ensure we have the correct number of base values for resolving by offset
        if decl.fields.len() != base_values.values.len() {
            return Err(ResolutionError::WrongNumberOfValues {
                expected: decl.fields.len(),
                received: base_values.values.len(),
            });
        }

        // reject overrides which don't match any fields in the schema
        if let Some(overrides) = parent_overrides {
            for key in overrides.iter().map(|o| &o.key) {
                if !decl.fields.iter().any(|f| &f.key == key) {
                    return Err(ResolutionError::FieldDoesNotExist { key: key.clone() });
                }
            }
        }

        let mut resolved_fields = vec![];
        for (decl_field, spec_field) in
            decl.fields.iter().zip(IntoIterator::into_iter(base_values.values))
        {
            // see if the parent component provided a value
            let mut from_parent = None;
            if let Some(parent_overrides) = parent_overrides {
                for o in parent_overrides {
                    // parent overrides are resolved according to string key, not index
                    if o.key == decl_field.key {
                        if !decl_field.mutability.contains(ConfigMutability::PARENT) {
                            return Err(ResolutionError::FieldIsNotMutable {
                                key: decl_field.key.clone(),
                            });
                        }
                        from_parent = Some(o.value.clone());
                    }
                }
            }

            // prefer parent-provided values over packaged values
            let value = if let Some(v) = from_parent { v } else { spec_field.value };
            let resolved_field = ConfigField::resolve(value, &decl_field).map_err(|source| {
                ResolutionError::InvalidValue { key: decl_field.key.clone(), source }
            })?;
            resolved_fields.push(resolved_field);
        }

        Ok(Self { fields: resolved_fields, checksum: decl.checksum.clone() })
    }

    /// Encode the resolved fields as a FIDL struct with every field non-nullable.
    ///
    /// The first two bytes of the encoded buffer are a little-endian unsigned integer which denotes
    /// the `checksum_length`. Bytes `2..2+checksum_length` are used to store the declaration
    /// checksum. All remaining bytes are used to store the FIDL header and struct.
    pub fn encode_as_fidl_struct(self) -> Vec<u8> {
        let Self { fields, checksum: ConfigChecksum::Sha256(checksum) } = self;
        let mut structure = Structure::default();
        for ConfigField { value, .. } in fields {
            structure = match value {
                ConfigValue::Single(ConfigSingleValue::Bool(b)) => {
                    structure.field(Field::Basic(BasicField::Bool(b)))
                }
                ConfigValue::Single(ConfigSingleValue::Uint8(n)) => {
                    structure.field(Field::Basic(BasicField::UInt8(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Uint16(n)) => {
                    structure.field(Field::Basic(BasicField::UInt16(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Uint32(n)) => {
                    structure.field(Field::Basic(BasicField::UInt32(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Uint64(n)) => {
                    structure.field(Field::Basic(BasicField::UInt64(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Int8(n)) => {
                    structure.field(Field::Basic(BasicField::Int8(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Int16(n)) => {
                    structure.field(Field::Basic(BasicField::Int16(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Int32(n)) => {
                    structure.field(Field::Basic(BasicField::Int32(n)))
                }
                ConfigValue::Single(ConfigSingleValue::Int64(n)) => {
                    structure.field(Field::Basic(BasicField::Int64(n)))
                }
                ConfigValue::Single(ConfigSingleValue::String(s)) => {
                    // TODO(https://fxbug.dev/42169377) improve string representation too
                    structure.field(Field::Vector(VectorField::UInt8Vector(s.as_bytes().to_vec())))
                }
                ConfigValue::Vector(ConfigVectorValue::BoolVector(b)) => {
                    structure.field(Field::Vector(VectorField::BoolVector(b.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Uint8Vector(n)) => {
                    structure.field(Field::Vector(VectorField::UInt8Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Uint16Vector(n)) => {
                    structure.field(Field::Vector(VectorField::UInt16Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Uint32Vector(n)) => {
                    structure.field(Field::Vector(VectorField::UInt32Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Uint64Vector(n)) => {
                    structure.field(Field::Vector(VectorField::UInt64Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Int8Vector(n)) => {
                    structure.field(Field::Vector(VectorField::Int8Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Int16Vector(n)) => {
                    structure.field(Field::Vector(VectorField::Int16Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Int32Vector(n)) => {
                    structure.field(Field::Vector(VectorField::Int32Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::Int64Vector(n)) => {
                    structure.field(Field::Vector(VectorField::Int64Vector(n.into())))
                }
                ConfigValue::Vector(ConfigVectorValue::StringVector(s)) => {
                    structure.field(Field::Vector(
                        // TODO(https://fxbug.dev/42169377) improve string representation too
                        VectorField::UInt8VectorVector(
                            IntoIterator::into_iter(s).map(|s| s.as_bytes().to_vec()).collect(),
                        ),
                    ))
                }
            };
        }

        let mut buf = Vec::new();
        buf.extend((checksum.len() as u16).to_le_bytes());
        buf.extend(checksum);
        buf.extend(structure.encode_persistent());
        buf
    }
}

impl Into<fdecl::ResolvedConfig> for ConfigFields {
    fn into(self) -> fdecl::ResolvedConfig {
        let checksum = self.checksum.native_into_fidl();
        let fields = self.fields.into_iter().map(|f| f.into()).collect();
        fdecl::ResolvedConfig { checksum, fields }
    }
}

/// A single resolved configuration field.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigField {
    /// The configuration field's key.
    pub key: FlyStr,

    /// The configuration field's value.
    pub value: ConfigValue,

    /// Ways this component's packaged values could have been overridden.
    pub mutability: ConfigMutability,
}

impl ConfigField {
    /// Reconciles a config field schema from the manifest with a value from the value file.
    /// If the types and constraints don't match, an error is returned.
    pub fn resolve(value: ConfigValue, decl_field: &ConfigFieldDecl) -> Result<Self, ValueError> {
        let key = decl_field.key.clone();

        match (&value, &decl_field.type_) {
            (ConfigValue::Single(ConfigSingleValue::Bool(_)), ConfigValueType::Bool)
            | (ConfigValue::Single(ConfigSingleValue::Uint8(_)), ConfigValueType::Uint8)
            | (ConfigValue::Single(ConfigSingleValue::Uint16(_)), ConfigValueType::Uint16)
            | (ConfigValue::Single(ConfigSingleValue::Uint32(_)), ConfigValueType::Uint32)
            | (ConfigValue::Single(ConfigSingleValue::Uint64(_)), ConfigValueType::Uint64)
            | (ConfigValue::Single(ConfigSingleValue::Int8(_)), ConfigValueType::Int8)
            | (ConfigValue::Single(ConfigSingleValue::Int16(_)), ConfigValueType::Int16)
            | (ConfigValue::Single(ConfigSingleValue::Int32(_)), ConfigValueType::Int32)
            | (ConfigValue::Single(ConfigSingleValue::Int64(_)), ConfigValueType::Int64) => (),
            (
                ConfigValue::Single(ConfigSingleValue::String(text)),
                ConfigValueType::String { max_size },
            ) => {
                let max_size = *max_size as usize;
                if text.len() > max_size {
                    return Err(ValueError::StringTooLong { max: max_size, actual: text.len() });
                }
            }
            (ConfigValue::Vector(list), ConfigValueType::Vector { nested_type, max_count }) => {
                let max_count = *max_count as usize;
                let actual_count = match (list, nested_type) {
                    (ConfigVectorValue::BoolVector(l), ConfigNestedValueType::Bool) => l.len(),
                    (ConfigVectorValue::Uint8Vector(l), ConfigNestedValueType::Uint8) => l.len(),
                    (ConfigVectorValue::Uint16Vector(l), ConfigNestedValueType::Uint16) => l.len(),
                    (ConfigVectorValue::Uint32Vector(l), ConfigNestedValueType::Uint32) => l.len(),
                    (ConfigVectorValue::Uint64Vector(l), ConfigNestedValueType::Uint64) => l.len(),
                    (ConfigVectorValue::Int8Vector(l), ConfigNestedValueType::Int8) => l.len(),
                    (ConfigVectorValue::Int16Vector(l), ConfigNestedValueType::Int16) => l.len(),
                    (ConfigVectorValue::Int32Vector(l), ConfigNestedValueType::Int32) => l.len(),
                    (ConfigVectorValue::Int64Vector(l), ConfigNestedValueType::Int64) => l.len(),
                    (
                        ConfigVectorValue::StringVector(l),
                        ConfigNestedValueType::String { max_size },
                    ) => {
                        let max_size = *max_size as usize;
                        for (i, s) in l.iter().enumerate() {
                            if s.len() > max_size {
                                return Err(ValueError::VectorElementInvalid {
                                    offset: i,
                                    source: Box::new(ValueError::StringTooLong {
                                        max: max_size,
                                        actual: s.len(),
                                    }),
                                });
                            }
                        }
                        l.len()
                    }
                    (other_list, other_ty) => {
                        return Err(ValueError::TypeMismatch {
                            expected: format!("{:?}", other_ty),
                            received: format!("{:?}", other_list),
                        });
                    }
                };

                if actual_count > max_count {
                    return Err(ValueError::VectorTooLong { max: max_count, actual: actual_count });
                }
            }
            (other_val, other_ty) => {
                return Err(ValueError::TypeMismatch {
                    expected: format!("{:?}", other_ty),
                    received: format!("{:?}", other_val),
                });
            }
        }

        Ok(ConfigField { key, value, mutability: decl_field.mutability })
    }
}

impl Into<fdecl::ResolvedConfigField> for ConfigField {
    fn into(self) -> fdecl::ResolvedConfigField {
        fdecl::ResolvedConfigField {
            key: self.key.to_string(),
            value: self.value.native_into_fidl(),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
#[allow(missing_docs)]
pub enum ResolutionError {
    #[error(
        "Checksums in declaration and value file do not match. Expected {expected:04x?}, received {received:04x?}"
    )]
    ChecksumFailure { expected: ConfigChecksum, received: ConfigChecksum },

    #[error(
        "Value file has a different number of values ({received}) than declaration has fields ({expected})."
    )]
    WrongNumberOfValues { expected: usize, received: usize },

    #[error("Provided a value for `{key}` which is not in the component's configuration schema.")]
    FieldDoesNotExist { key: FlyStr },

    #[error("Provided an override which is not mutable in the component's configuration schema.")]
    FieldIsNotMutable { key: FlyStr },

    #[error("Received invalid value for `{key}`:\n\t{source}")]
    InvalidValue {
        key: FlyStr,
        #[source]
        source: ValueError,
    },
}

#[derive(Clone, Debug, Error, PartialEq)]
#[allow(missing_docs)]
pub enum ValueError {
    #[error("Value of type `{received}` does not match declaration of type {expected}.")]
    TypeMismatch { expected: String, received: String },

    #[error("Received string of length {actual} for a field with a max of {max}.")]
    StringTooLong { max: usize, actual: usize },

    #[error("Received vector of length {actual} for a field with a max of {max}.")]
    VectorTooLong { max: usize, actual: usize },

    #[error("Vector element at {offset} index is invalid.")]
    VectorElementInvalid {
        offset: usize,
        #[source]
        source: Box<Self>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_component_config_ext::{config_decl, values_data};
    use fidl_test_config_encoder::BasicSuccessSchema;

    use ConfigSingleValue::*;
    use ConfigValue::*;
    use ConfigVectorValue::*;

    #[test]
    fn basic_success() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            my_flag: { bool },
            my_uint8: { uint8 },
            my_uint16: { uint16 },
            my_uint32: { uint32 },
            my_uint64: { uint64 },
            my_int8: { int8 },
            my_int16: { int16 },
            my_int32: { int32 },
            my_int64: { int64 },
            my_string: { string, max_size: 100 },
            my_vector_of_flag: { vector, element: bool, max_count: 100 },
            my_vector_of_uint8: { vector, element: uint8, max_count: 100 },
            my_vector_of_uint16: { vector, element: uint16, max_count: 100 },
            my_vector_of_uint32: { vector, element: uint32, max_count: 100 },
            my_vector_of_uint64: { vector, element: uint64, max_count: 100 },
            my_vector_of_int8: { vector, element: int8, max_count: 100 },
            my_vector_of_int16: { vector, element: int16, max_count: 100 },
            my_vector_of_int32: { vector, element: int32, max_count: 100 },
            my_vector_of_int64: { vector, element: int64, max_count: 100 },
            my_vector_of_string: {
                vector,
                element: { string, max_size: 100 },
                max_count: 100
            },
        };

        let specs = values_data![
            ck@ decl.checksum.clone(),
            Single(Bool(false)),
            Single(Uint8(255u8)),
            Single(Uint16(65535u16)),
            Single(Uint32(4000000000u32)),
            Single(Uint64(8000000000u64)),
            Single(Int8(-127i8)),
            Single(Int16(-32766i16)),
            Single(Int32(-2000000000i32)),
            Single(Int64(-4000000000i64)),
            Single(String("hello, world!".into())),
            Vector(BoolVector(Box::from([true, false]))),
            Vector(Uint8Vector(Box::from([1, 2, 3]))),
            Vector(Uint16Vector(Box::from([2, 3, 4]))),
            Vector(Uint32Vector(Box::from([3, 4, 5]))),
            Vector(Uint64Vector(Box::from([4, 5, 6]))),
            Vector(Int8Vector(Box::from([-1, -2, 3]))),
            Vector(Int16Vector(Box::from([-2, -3, 4]))),
            Vector(Int32Vector(Box::from([-3, -4, 5]))),
            Vector(Int64Vector(Box::from([-4, -5, 6]))),
            Vector(StringVector(Box::from(["valid".into(), "valid".into()]))),
        ];

        let resolved = ConfigFields::resolve(&decl, specs, None).unwrap();
        assert_eq!(
            resolved,
            ConfigFields {
                fields: vec![
                    ConfigField {
                        key: "my_flag".into(),
                        value: Single(Bool(false)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_uint8".into(),
                        value: Single(Uint8(255)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_uint16".into(),
                        value: Single(Uint16(65535)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_uint32".into(),
                        value: Single(Uint32(4000000000)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_uint64".into(),
                        value: Single(Uint64(8000000000)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_int8".into(),
                        value: Single(Int8(-127)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_int16".into(),
                        value: Single(Int16(-32766)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_int32".into(),
                        value: Single(Int32(-2000000000)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_int64".into(),
                        value: Single(Int64(-4000000000)),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_string".into(),
                        value: Single(String("hello, world!".into())),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_flag".into(),
                        value: Vector(BoolVector(Box::from([true, false]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_uint8".into(),
                        value: Vector(Uint8Vector(Box::from([1, 2, 3]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_uint16".into(),
                        value: Vector(Uint16Vector(Box::from([2, 3, 4]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_uint32".into(),
                        value: Vector(Uint32Vector(Box::from([3, 4, 5]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_uint64".into(),
                        value: Vector(Uint64Vector(Box::from([4, 5, 6]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_int8".into(),
                        value: Vector(Int8Vector(Box::from([-1, -2, 3]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_int16".into(),
                        value: Vector(Int16Vector(Box::from([-2, -3, 4]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_int32".into(),
                        value: Vector(Int32Vector(Box::from([-3, -4, 5]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_int64".into(),
                        value: Vector(Int64Vector(Box::from([-4, -5, 6]))),
                        mutability: Default::default(),
                    },
                    ConfigField {
                        key: "my_vector_of_string".into(),
                        value: Vector(StringVector(Box::from(["valid".into(), "valid".into()]))),
                        mutability: Default::default(),
                    },
                ],
                checksum: decl.checksum.clone(),
            }
        );

        let encoded = resolved.encode_as_fidl_struct();

        let checksum_len = u16::from_le_bytes(encoded[..2].try_into().unwrap());
        let struct_start = 2 + checksum_len as usize;
        assert_eq!(&encoded[2..struct_start], [0; 32]);

        let decoded: BasicSuccessSchema = fidl::unpersist(&encoded[struct_start..]).unwrap();
        assert_eq!(
            decoded,
            BasicSuccessSchema {
                my_flag: false,
                my_uint8: 255,
                my_uint16: 65535,
                my_uint32: 4000000000,
                my_uint64: 8000000000,
                my_int8: -127,
                my_int16: -32766,
                my_int32: -2000000000,
                my_int64: -4000000000,
                my_string: "hello, world!".into(),
                my_vector_of_flag: vec![true, false],
                my_vector_of_uint8: vec![1, 2, 3],
                my_vector_of_uint16: vec![2, 3, 4],
                my_vector_of_uint32: vec![3, 4, 5],
                my_vector_of_uint64: vec![4, 5, 6],
                my_vector_of_int8: vec![-1, -2, 3],
                my_vector_of_int16: vec![-2, -3, 4],
                my_vector_of_int32: vec![-3, -4, 5],
                my_vector_of_int64: vec![-4, -5, 6],
                my_vector_of_string: vec!["valid".into(), "valid".into()],
            }
        );
    }

    #[test]
    fn checksums_must_match() {
        let expected = ConfigChecksum::Sha256([0; 32]);
        let received = ConfigChecksum::Sha256([0xFF; 32]);
        let decl = config_decl! {
            ck@ expected.clone(),
            foo: { bool },
        };
        let specs = values_data! [
            ck@ received.clone(),
            ConfigValue::Single(ConfigSingleValue::Bool(true)),
        ];
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::ChecksumFailure { expected, received }
        );
    }

    #[test]
    fn too_many_values_fails() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            foo: { bool },
        };
        let specs = values_data! [
            ck@ decl.checksum.clone(),
            ConfigValue::Single(ConfigSingleValue::Bool(true)),
            ConfigValue::Single(ConfigSingleValue::Bool(false)),
        ];
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::WrongNumberOfValues { expected: 1, received: 2 }
        );
    }

    #[test]
    fn not_enough_values_fails() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            foo: { bool },
        };
        let specs = values_data! {
            ck@ decl.checksum.clone(),
        };
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::WrongNumberOfValues { expected: 1, received: 0 }
        );
    }

    #[test]
    fn string_length_is_validated() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            foo: { string, max_size: 10 },
        };
        let specs = values_data! [
            ck@ decl.checksum.clone(),
            ConfigValue::Single(ConfigSingleValue::String("hello, world!".into())),
        ];
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::InvalidValue {
                key: "foo".into(),
                source: ValueError::StringTooLong { max: 10, actual: 13 },
            }
        );
    }

    #[test]
    fn vector_length_is_validated() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            foo: { vector, element: uint8, max_count: 2 },
        };
        let specs = values_data! [
            ck@ decl.checksum.clone(),
            ConfigValue::Vector(ConfigVectorValue::Uint8Vector(Box::from([1, 2, 3]))),
        ];
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::InvalidValue {
                key: "foo".into(),
                source: ValueError::VectorTooLong { max: 2, actual: 3 },
            }
        );
    }

    #[test]
    fn vector_elements_validated() {
        let decl = config_decl! {
            ck@ ConfigChecksum::Sha256([0; 32]),
            foo: { vector, element: { string, max_size: 5 }, max_count: 2 },
        };
        let specs = values_data! [
            ck@ decl.checksum.clone(),
            ConfigValue::Vector(ConfigVectorValue::StringVector(Box::from([
                "valid".into(),
                "invalid".into(),
            ]))),
        ];
        assert_eq!(
            ConfigFields::resolve(&decl, specs, None).unwrap_err(),
            ResolutionError::InvalidValue {
                key: "foo".into(),
                source: ValueError::VectorElementInvalid {
                    offset: 1,
                    source: Box::new(ValueError::StringTooLong { max: 5, actual: 7 })
                },
            }
        );
    }

    #[test]
    fn parent_overrides_take_precedence() {
        let overridden_key = FlyStr::new("my_flag");
        let defaulted_key = FlyStr::new("my_other_flag");
        let decl = cm_rust::ConfigDecl {
            fields: Box::from([
                cm_rust::ConfigField {
                    key: overridden_key.clone(),
                    type_: cm_rust::ConfigValueType::Bool,
                    mutability: ConfigMutability::PARENT,
                },
                cm_rust::ConfigField {
                    key: defaulted_key.clone().into(),
                    type_: cm_rust::ConfigValueType::Bool,
                    mutability: ConfigMutability::empty(),
                },
            ]),
            checksum: ConfigChecksum::Sha256([0; 32]),
            value_source: cm_rust::ConfigValueSource::PackagePath("fake.cvf".into()),
        };

        let packaged = values_data![
            ck@ decl.checksum.clone(),
            Single(Bool(false)),
            Single(Bool(false)),
        ];

        let expected_value = Single(Bool(true));
        let overrides = vec![ConfigOverride {
            key: overridden_key.clone().into(),
            value: expected_value.clone(),
        }];

        assert_eq!(
            ConfigFields::resolve(&decl, packaged, Some(&overrides)).unwrap(),
            ConfigFields {
                fields: vec![
                    ConfigField {
                        key: overridden_key.clone().into(),
                        value: expected_value,
                        mutability: ConfigMutability::PARENT,
                    },
                    ConfigField {
                        key: defaulted_key.into(),
                        value: Single(Bool(false)),
                        mutability: ConfigMutability::empty(),
                    },
                ],
                checksum: decl.checksum.clone(),
            }
        );
    }

    #[test]
    fn overrides_must_match_declared_fields() {
        let decl = cm_rust::ConfigDecl {
            fields: Box::from([cm_rust::ConfigField {
                key: FlyStr::new("my_flag"),
                type_: cm_rust::ConfigValueType::Bool,
                mutability: ConfigMutability::PARENT,
            }]),
            checksum: ConfigChecksum::Sha256([0; 32]),
            value_source: cm_rust::ConfigValueSource::PackagePath("fake.cvf".into()),
        };

        let packaged = values_data![
            ck@ decl.checksum.clone(),
            Single(Bool(false)),
        ];

        let overridden_key = FlyStr::new("not_my_flag");
        let expected_value = Single(Bool(true));
        let overrides = vec![ConfigOverride {
            key: overridden_key.clone().into(),
            value: expected_value.clone(),
        }];

        assert_eq!(
            ConfigFields::resolve(&decl, packaged, Some(&overrides)).unwrap_err(),
            ResolutionError::FieldDoesNotExist { key: overridden_key.clone() },
        );
    }

    #[test]
    fn overrides_must_be_for_mutable_by_parent_fields() {
        let overridden_key = FlyStr::new("my_flag");
        let defaulted_key = FlyStr::new("my_other_flag");
        let decl = cm_rust::ConfigDecl {
            fields: Box::from([
                cm_rust::ConfigField {
                    key: overridden_key.clone().into(),
                    type_: cm_rust::ConfigValueType::Bool,
                    mutability: ConfigMutability::empty(),
                },
                // this field having parent mutability should not allow the other field to mutate
                cm_rust::ConfigField {
                    key: defaulted_key.clone().into(),
                    type_: cm_rust::ConfigValueType::Bool,
                    mutability: ConfigMutability::PARENT,
                },
            ]),
            checksum: ConfigChecksum::Sha256([0; 32]),
            value_source: cm_rust::ConfigValueSource::PackagePath("fake.cvf".into()),
        };

        let packaged = values_data![
            ck@ decl.checksum.clone(),
            Single(Bool(false)),
            Single(Bool(false)),
        ];

        let overrides =
            vec![ConfigOverride { key: overridden_key.clone(), value: Single(Bool(true)) }];

        assert_eq!(
            ConfigFields::resolve(&decl, packaged, Some(&overrides)).unwrap_err(),
            ResolutionError::FieldIsNotMutable { key: overridden_key.clone() },
        );
    }

    #[test]
    fn overrides_must_match_declared_type_exactly() {
        let overridden_key = FlyStr::new("my_flag");
        let decl = cm_rust::ConfigDecl {
            fields: Box::from([cm_rust::ConfigField {
                key: overridden_key.clone().into(),
                type_: cm_rust::ConfigValueType::Bool,
                mutability: ConfigMutability::PARENT,
            }]),
            checksum: ConfigChecksum::Sha256([0; 32]),
            value_source: cm_rust::ConfigValueSource::PackagePath("fake.cvf".into()),
        };

        let packaged = values_data![
            ck@ decl.checksum.clone(),
            Single(Bool(false)),
        ];

        let overrides =
            vec![ConfigOverride { key: overridden_key.clone().into(), value: Single(Uint8(1)) }];

        assert_eq!(
            ConfigFields::resolve(&decl, packaged, Some(&overrides)).unwrap_err(),
            ResolutionError::InvalidValue {
                key: overridden_key.into(),
                source: ValueError::TypeMismatch {
                    expected: "Bool".to_string(),
                    received: "Single(Uint8(1))".to_string()
                },
            },
        );
    }

    macro_rules! type_mismatch_test {
        ($test_name:ident: { $($ty_toks:tt)* }, $valid_spec:pat) => {
            #[test]
            fn $test_name() {
                let decl = ConfigFieldDecl {
                    key: "test_key".into(),
                    type_: fidl_fuchsia_component_config_ext::config_ty!($($ty_toks)*),
                    mutability: Default::default(),
                };
                for value in [
                    // one value of each type
                    Single(Bool(true)),
                    Single(Uint8(1)),
                    Single(Uint16(1)),
                    Single(Uint32(1)),
                    Single(Uint64(1)),
                    Single(Int8(1)),
                    Single(Int16(1)),
                    Single(Int32(1)),
                    Single(Int64(1)),
                    Single(String("".into())),
                    Vector(BoolVector(Box::from([]))),
                    Vector(Uint8Vector(Box::from([]))),
                    Vector(Uint16Vector(Box::from([]))),
                    Vector(Uint32Vector(Box::from([]))),
                    Vector(Uint64Vector(Box::from([]))),
                    Vector(Int8Vector(Box::from([]))),
                    Vector(Int16Vector(Box::from([]))),
                    Vector(Int32Vector(Box::from([]))),
                    Vector(Int64Vector(Box::from([]))),
                    Vector(StringVector(Box::from([]))),
                ] {
                    let should_succeed = matches!(value, $valid_spec);
                    match ConfigField::resolve(value, &decl) {
                        Ok(..) if should_succeed => (),
                        Err(ValueError::TypeMismatch { .. }) if !should_succeed => (),
                        other => panic!(
                            "test case {:?} received unexpected resolved value {:#?}",
                            decl, other
                        ),
                    }
                }
            }
        };
    }

    type_mismatch_test!(bool_type_mismatches: { bool }, Single(Bool(..)));
    type_mismatch_test!(uint8_type_mismatches:  { uint8 },  Single(Uint8(..)));
    type_mismatch_test!(uint16_type_mismatches: { uint16 }, Single(Uint16(..)));
    type_mismatch_test!(uint32_type_mismatches: { uint32 }, Single(Uint32(..)));
    type_mismatch_test!(uint64_type_mismatches: { uint64 }, Single(Uint64(..)));
    type_mismatch_test!(int8_type_mismatches:  { int8 },  Single(Int8(..)));
    type_mismatch_test!(int16_type_mismatches: { int16 }, Single(Int16(..)));
    type_mismatch_test!(int32_type_mismatches: { int32 }, Single(Int32(..)));
    type_mismatch_test!(int64_type_mismatches: { int64 }, Single(Int64(..)));
    type_mismatch_test!(string_type_mismatches: { string, max_size: 10 }, Single(String(..)));

    type_mismatch_test!(
        bool_vector_type_mismatches: { vector, element: bool, max_count: 1 }, Vector(BoolVector(..))
    );
    type_mismatch_test!(
        uint8_vector_type_mismatches:
        { vector, element: uint8, max_count: 1 },
        Vector(Uint8Vector(..))
    );
    type_mismatch_test!(
        uint16_vector_type_mismatches:
        { vector, element: uint16, max_count: 1 },
        Vector(Uint16Vector(..))
    );
    type_mismatch_test!(
        uint32_vector_type_mismatches:
        { vector, element: uint32, max_count: 1 },
        Vector(Uint32Vector(..))
    );
    type_mismatch_test!(
        uint64_vector_type_mismatches:
        { vector, element: uint64, max_count: 1 },
        Vector(Uint64Vector(..))
    );
    type_mismatch_test!(
        int8_vector_type_mismatches:
        { vector, element: int8, max_count: 1 },
        Vector(Int8Vector(..))
    );
    type_mismatch_test!(
        int16_vector_type_mismatches:
        { vector, element: int16, max_count: 1 },
        Vector(Int16Vector(..))
    );
    type_mismatch_test!(
        int32_vector_type_mismatches:
        { vector, element: int32, max_count: 1 },
        Vector(Int32Vector(..))
    );
    type_mismatch_test!(
        int64_vector_type_mismatches:
        { vector, element: int64, max_count: 1 },
        Vector(Int64Vector(..))
    );
    type_mismatch_test!(
        string_vector_type_mismatches:
        { vector, element: { string, max_size: 10 }, max_count: 1 },
        Vector(StringVector(..))
    );
}
