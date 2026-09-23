// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::env::EnvLike;
use crate::errors::BuildError;
use crate::name::Name;
use serde::Deserialize;
use serde_json::Value;
use serde_json5;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

const DEFAULT_SCHEMA_PATH: &str = "../../build/testing/test_config_schema.json5";
const SCHEMA_ARG_PREFIX: &str = "--schema=";

/// A JSON schema. `Schema` holds a test config schema in two forms: A serde_json Value suitable
/// for validating JSON against the schema, and an idiomatic form for guiding the processing of test
/// parameters in command lines and environment variables. In this latter form, the schema includes
/// only what's required for that purpose. For example, object types have no properties (because
/// object types are not allowed in command lines or environment variables). Descriptions are also
/// absent as well as enums, which are only used in formal validation.
#[derive(Deserialize, Debug, Default, PartialEq)]
pub struct Schema {
    // `Value` form of the schema for validation using `valico::json_schema``.
    #[serde(skip_deserializing)]
    pub as_value: Value,

    // Table of deserialized properties.
    pub properties: HashMap<Name, PropertyScheme>,
}

impl Schema {
    /// Gets a schema by reading the file specified in the '--schema=' command line argument in
    /// `env_like` or by reading from the default file specified by `DEFAULT_SCHEMA_PATH`.
    pub fn from_env_like<E: EnvLike>(env_like: &E) -> Result<Self, BuildError> {
        if let Some(arg) = env_like.args().find(|a| a.starts_with(SCHEMA_ARG_PREFIX)) {
            Self::read(PathBuf::from(arg.strip_prefix(SCHEMA_ARG_PREFIX).unwrap()))
        } else {
            Self::read(PathBuf::from(DEFAULT_SCHEMA_PATH))
        }
    }

    /// Creates a `Schema` by reading it from the schema from the specified path.
    pub fn read(path: PathBuf) -> Result<Self, BuildError> {
        let file = File::open(path.as_path())
            .map_err(|e| BuildError::FailedToOpenSchema { path: path.clone(), source: e })?;

        let mut reader = BufReader::new(file);

        Self::from_value(
            serde_json5::from_reader(&mut reader)
                .map_err(|e| BuildError::FailedToReadSchema { path: path.clone(), source: e })?,
            path,
        )
    }

    /// Creates a `Schema' from a `Value``.
    pub fn from_value(schema_value: Value, path: PathBuf) -> Result<Self, BuildError> {
        let mut schema: Schema = serde_json::from_value(schema_value.clone())
            .map_err(|e| BuildError::FailedToParseSchema { path: path.clone(), source: e })?;

        schema.as_value = schema_value;

        for (name, scheme) in &schema.properties {
            scheme.validate(name)?;
        }

        Ok(schema)
    }
}

/// Scheme for properties and array members.
#[derive(Deserialize, Debug, Default, PartialEq)]
pub struct PropertyScheme {
    // The type of the property.
    #[serde(rename = "type")]
    pub property_type: PropertyType,

    // The property scheme for array types. For other types, this field is None.
    pub items: Option<Box<PropertyScheme>>,
}

impl PropertyScheme {
    fn validate(&self, name: &Name) -> Result<(), BuildError> {
        match &self.items {
            Some(boxed_scheme) => {
                if self.property_type != PropertyType::Array {
                    return Err(BuildError::InvalidSchema(format!(
                        "Property {} has `items` but is not an array",
                        name
                    )));
                }

                (*boxed_scheme).validate(name)?;
            }
            None => {
                if self.property_type == PropertyType::Array {
                    return Err(BuildError::InvalidSchema(format!(
                        "Property {} is an array but has no `items`",
                        name
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Type of a property.
#[derive(Deserialize, Debug, Default, PartialEq)]
pub enum PropertyType {
    #[serde(rename = "string")]
    #[default]
    String,

    #[serde(rename = "number")]
    Number,

    #[serde(rename = "boolean")]
    Boolean,

    #[serde(rename = "array")]
    Array,

    // Object types are not supported, but object-typed parameters may still appear in the schema
    // without property information.
    #[serde(rename = "object")]
    Object,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde_json::json;
    use std::fs;
    use tempfile::NamedTempFile;

    pub fn fake_schema() -> Schema {
        Schema::from_value(
            json!({
                "properties": {
                    "true": { "type": "boolean" },
                    "false": { "type": "boolean" },
                    "true_simple": { "type": "boolean" },
                    "false_simple": { "type": "boolean" },
                    "no_negative_bool": { "type": "boolean" },
                    "zero": { "type": "number" },
                    "zero_point_one": { "type": "number" },
                    "negative_zero_point_one": { "type": "number" },
                    "string": { "type": "string" },
                    "array_of_number": {
                        "type": "array",
                        "items": {
                            "type": "number"
                        }
                    },
                    "array_of_string": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        }
                    },
                    "object": {
                        "type": "object",
                        "properties": {
                            "foo": { "type": "number" },
                            "bar": { "type": "number" }
                        }
                    },
                    "foo": { "type": "string" },
                    "bar": { "type": "string" },
                    "baz": { "type": "string" },
                    "a": { "type": "string" },
                    "b": { "type": "string" },
                    "c": { "type": "string" },
                    "d": { "type": "string" },
                    "e": { "type": "string" },
                    "output_directory": { "type": "string" },
                    "host_test_binary": { "type": "string" },
                    "host_test_args": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        }
                    }
                },
                "additionalProperties": false
            }),
            PathBuf::new(),
        )
        .expect("fake schema is valid")
    }

    #[test]
    fn test_read_schema() {
        let temp_file = NamedTempFile::new().expect("to create temporary file");
        let temp_file_path = temp_file.path().to_str().unwrap().to_string();
        fs::write(
            temp_file_path,
            r#"{
            type: "object",
            properties: {
                output_directory: { type: "string" },
                host_test_binary: { type: "string" },
                host_test_args: {
                    type: "array",
                    items: { type: "string", },
                },
            },
            required: [
                "output_directory",
                "host_test_binary",
            ],
            additionalProperties: false,
        }"#,
        )
        .expect("to write to temporary file");
        assert!(Schema::read(temp_file.path().to_path_buf()).is_ok());
        temp_file.close().expect("to close temporary file");

        let path_buf = PathBuf::from("/non_existent_file");
        let result = Schema::read(path_buf.clone());
        assert!(result.is_err());
        assert_matches!(
            result.unwrap_err(),
            BuildError::FailedToOpenSchema { path: p, source: _ }
                if p == path_buf
        );

        let temp_file = NamedTempFile::new().expect("to create temporary file");
        let temp_file_path = temp_file.path().to_str().unwrap().to_string();
        fs::write(temp_file_path, r#"not a valid schema"#).expect("to write to temporary file");
        let result = Schema::read(temp_file.path().to_path_buf());
        assert!(result.is_err());
        assert_matches!(
            result.unwrap_err(),
            BuildError::FailedToReadSchema { path: p, source: _ }
                if p == temp_file.path().to_path_buf()
        );
        temp_file.close().expect("to close temporary file");
    }
}
