// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow, bail};
use schemars::schema::{InstanceType, RootSchema, Schema, SchemaObject, SingleOrVec};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The data that describes the entire serde user interface.
/// This is extracted from a json schema.
#[derive(Debug, Serialize, PartialEq)]
pub struct AllData {
    /// The base url path to append to all links.
    pub url_path: String,
    /// The root data type.
    pub root: String,
    /// All data types, including the root.
    /// This contains the data for each struct/enum/etc.
    pub data_types: BTreeMap<String, DataType>,
}

impl AllData {
    /// Construct AllData from a root json schema.
    pub fn from_root_schema(url_path: &String, root_schema: &RootSchema) -> Result<Self> {
        let url_path = url_path.clone();
        let mut data_types = BTreeMap::new();
        let root_type = DataType::from_root_schema(root_schema)?;
        data_types.insert(root_type.rust_type.clone(), root_type.clone());
        for (rust_type, schema) in &root_schema.definitions {
            let child = DataType::from_schema(rust_type.clone(), schema)?;
            data_types.insert(child.rust_type.clone(), child.clone());
        }
        Ok(Self { url_path, root: root_type.rust_type, data_types })
    }
}

/// Data for a single struct/enum/etc.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DataType {
    /// The rust type.
    pub rust_type: String,
    /// The rust doc-comment.
    pub description: String,
    #[serde(flatten)]
    pub inner: DataTypeInner,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(untagged)]
pub enum DataTypeInner {
    Primitive(PrimitiveDataType),
    Enum(EnumDataType),
    Struct(StructDataType),
}

/// A primitive data type, such as a u8 or String.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PrimitiveDataType {
    /// The data type name.
    pub data_type: String,
}

/// An enum data type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EnumDataType {
    /// The variants of the enum.
    /// Note that this does not currently support complex enums or descriptions.
    /// The schemars crate does not populate descriptions for enums.
    pub variants: BTreeSet<String>,
}

/// A struct data type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StructDataType {
    /// The fields of the struct.
    pub fields: BTreeSet<StructFieldData>,
}

/// A single struct field, which should point to a sub-data-type.
#[derive(Debug, Clone, Eq, Serialize)]
pub struct StructFieldData {
    /// The name of the field.
    pub field_name: String,
    /// The doc-comment for the field.
    pub description: String,
    /// The default value of the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// The data type of the field.
    #[serde(flatten)]
    pub data_type: StructFieldType,
}

/// The type of a single struct field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum StructFieldType {
    Primitive { data_type: String },
    Custom { data_type: String },
}

impl PartialEq for StructFieldData {
    fn eq(&self, other: &Self) -> bool {
        self.field_name == other.field_name
    }
}

impl Ord for StructFieldData {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.field_name.cmp(&self.field_name)
    }
}

impl PartialOrd for StructFieldData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl DataType {
    /// Construct a DataType from a root schema object.
    fn from_root_schema(root_schema: &RootSchema) -> Result<Self> {
        let schema = root_schema.schema.clone();
        let metadata = root_schema
            .schema
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow!("missing metadata from root"))?;
        let rust_type =
            metadata.title.as_ref().ok_or_else(|| anyhow!("missing title from root"))?.clone();
        let description = metadata
            .description
            .as_ref()
            .ok_or_else(|| anyhow!("missing description from {}", rust_type))?
            .clone();
        Self::from_schema_object(rust_type, description, schema)
    }

    /// Construct a DataType from a non-root schema object.
    fn from_schema(rust_type: String, schema: &Schema) -> Result<Self> {
        let schema = schema.clone().into_object();
        let description = if let Some(metadata) = &schema.metadata {
            metadata
                .description
                .as_ref()
                .ok_or_else(|| anyhow!("missing description from {}", rust_type))?
                .clone()
        } else {
            "no description".to_string()
        };
        Self::from_schema_object(rust_type, description, schema)
    }

    /// Construct a DataType from a generic schema object.
    fn from_schema_object(
        rust_type: String,
        description: String,
        schema: SchemaObject,
    ) -> Result<Self> {
        // The description may be modified to add an error message.
        let mut description = description;

        // An enum.
        let inner = if let Some(enum_values) = schema.enum_values {
            let variants = enum_values.into_iter().map(|v| v.to_string()).collect();
            DataTypeInner::Enum(EnumDataType { variants })
        }
        // An enum with variants of different types.
        // TODO(b/332348955): Support this properly.
        // TODO(b/436293725): Support comments on enum variants.
        else if let Some(_) = schema.subschemas {
            let error_message = format!(
                "Failed to generate docs for complex {} enum: b/332348955 or b/436293725",
                rust_type
            );
            description = format!("{}\n\n{}", error_message, description);
            // println!("{}", error_message);
            DataTypeInner::Enum(EnumDataType { variants: BTreeSet::new() })
        }
        // A struct.
        else if let Some(object) = schema.object {
            let fields = object
                .properties
                .into_iter()
                .map(|(field_name, p)| {
                    let mut object = p.into_object();
                    let data_type = if let Some(format) = &object.format {
                        StructFieldType::Primitive { data_type: format.clone() }
                    } else if let Some(single_or_vec) = &object.instance_type {
                        StructFieldType::Primitive {
                            data_type: single_or_vec_to_string(&single_or_vec)?,
                        }
                    } else if let Some(reference) = &object.reference {
                        StructFieldType::Custom { data_type: reference.clone() }
                    } else {
                        let subschemas = object
                            .subschemas
                            .as_ref()
                            .ok_or_else(|| anyhow!("Missing subschemas for {}", rust_type))?;
                        let mut subobjects = Vec::<SchemaObject>::new();
                        if let Some(subs) = &subschemas.all_of {
                            for sub in subs {
                                subobjects.push(sub.clone().into_object());
                            }
                        }
                        if let Some(subs) = &subschemas.any_of {
                            for sub in subs {
                                subobjects.push(sub.clone().into_object());
                            }
                        }
                        let subobject = subobjects
                            .first()
                            .ok_or_else(|| anyhow!("Missing subobject for {}", rust_type))?
                            .clone();
                        let reference = subobject.reference.ok_or_else(|| {
                            anyhow!("Missing reference for field in {}", rust_type)
                        })?;
                        StructFieldType::Custom { data_type: reference }
                    };
                    let metadata = object.metadata();
                    let description = metadata.description.clone().unwrap_or_else(|| "".into());
                    let default = metadata.default.clone();
                    Ok(StructFieldData { field_name, data_type, description, default })
                })
                .collect::<Result<BTreeSet<StructFieldData>>>()?;
            DataTypeInner::Struct(StructDataType { fields })
        }
        // A primitive wrapped by a type.
        // e.g. ImageName(String)
        else if let Some(single_or_vec) = schema.instance_type {
            let data_type = single_or_vec_to_string(&single_or_vec)?;
            DataTypeInner::Primitive(PrimitiveDataType { data_type })
        }
        // Unsupported.
        else {
            anyhow::bail!("Unsupported schema type for {}", rust_type);
        };

        Ok(Self { rust_type, description, inner })
    }
}

/// Convert a SingleOrVec<InstanceType> to a user-friendly String.
fn single_or_vec_to_string(single_or_vec: &SingleOrVec<InstanceType>) -> Result<String> {
    match single_or_vec {
        SingleOrVec::Single(t) => Ok(format!("{}", instance_type_to_string(&t)?)),
        SingleOrVec::Vec(v) => {
            let t = v.first().ok_or_else(|| anyhow!("Missing instance type"))?;
            Ok(format!("[{}]", instance_type_to_string(&t)?))
        }
    }
}

/// Convert a schema InstanceType to a user-friendly String.
fn instance_type_to_string(instance_type: &InstanceType) -> Result<String> {
    let s = match instance_type {
        schemars::schema::InstanceType::Boolean => "bool",
        schemars::schema::InstanceType::Array => "vector",
        schemars::schema::InstanceType::String => "string",
        schemars::schema::InstanceType::Integer => "integer",
        schemars::schema::InstanceType::Number => "integer",
        schemars::schema::InstanceType::Object => "object",
        _ => bail!("unsupported type"),
    }
    .to_string();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::{
        AllData, DataType, DataTypeInner, EnumDataType, StructDataType, StructFieldData,
        StructFieldType, single_or_vec_to_string,
    };
    use pretty_assertions::assert_eq;
    use schemars::JsonSchema;
    use schemars::r#gen::SchemaSettings;
    use schemars::schema::{InstanceType, SingleOrVec};
    use serde::Serialize;
    use std::collections::{BTreeMap, BTreeSet};

    /// Mandatory description on root struct.
    #[derive(Serialize, JsonSchema)]
    struct RootStruct {
        /// Primitives should work
        field_1: u8,
        /// Nested enums should work
        field_2: MyEnum,
        /// Vectors should work
        field_3: Vec<String>,
        /// Nested structs should work
        field_4: MyStruct,
    }

    /// Really cool enum.
    #[allow(dead_code)]
    #[derive(Serialize, JsonSchema)]
    enum MyEnum {
        Variant1,
        #[serde(rename = "variant_2")]
        Variant2,
    }

    /// Really cool struct.
    #[derive(Serialize, JsonSchema)]
    struct MyStruct {
        /// Booleans should work
        field_5: bool,
    }

    #[test]
    fn test() {
        let settings = SchemaSettings::default().with(|s| {
            s.definitions_path = "".to_string();
        });
        let generator = settings.into_generator();
        let root_schema = generator.into_root_schema_for::<RootStruct>();
        let all_data = AllData::from_root_schema(&"url_path".into(), &root_schema).unwrap();
        let expected = AllData {
            url_path: "url_path".into(),
            root: "RootStruct".into(),
            data_types: BTreeMap::from([
                (
                    "RootStruct".into(),
                    DataType {
                        rust_type: "RootStruct".into(),
                        description: "Mandatory description on root struct.".into(),
                        inner: DataTypeInner::Struct(StructDataType {
                            fields: BTreeSet::from([
                                StructFieldData {
                                    field_name: "field_1".into(),
                                    description: "Primitives should work".into(),
                                    default: None,
                                    data_type: StructFieldType::Primitive {
                                        data_type: "uint8".into(),
                                    },
                                },
                                StructFieldData {
                                    field_name: "field_2".into(),
                                    description: "Nested enums should work".into(),
                                    default: None,
                                    data_type: StructFieldType::Custom {
                                        data_type: "MyEnum".into(),
                                    },
                                },
                                StructFieldData {
                                    field_name: "field_3".into(),
                                    description: "Vectors should work".into(),
                                    default: None,
                                    data_type: StructFieldType::Primitive {
                                        data_type: "vector".into(),
                                    },
                                },
                                StructFieldData {
                                    field_name: "field_4".into(),
                                    description: "Nested structs should work".into(),
                                    default: None,
                                    data_type: StructFieldType::Custom {
                                        data_type: "MyStruct".into(),
                                    },
                                },
                            ]),
                        }),
                    },
                ),
                (
                    "MyEnum".into(),
                    DataType {
                        rust_type: "MyEnum".into(),
                        description: "Really cool enum.".into(),
                        inner: DataTypeInner::Enum(EnumDataType {
                            variants: BTreeSet::from([
                                "\"Variant1\"".into(),
                                "\"variant_2\"".into(),
                            ]),
                        }),
                    },
                ),
                (
                    "MyStruct".into(),
                    DataType {
                        rust_type: "MyStruct".into(),
                        description: "Really cool struct.".into(),
                        inner: DataTypeInner::Struct(StructDataType {
                            fields: BTreeSet::from([StructFieldData {
                                field_name: "field_5".into(),
                                description: "Booleans should work".into(),
                                default: None,
                                data_type: StructFieldType::Primitive {
                                    data_type: "boolean".into(),
                                },
                            }]),
                        }),
                    },
                ),
            ]),
        };
        assert_eq!(expected, all_data);
    }

    #[test]
    fn test_instance_type_to_string() {
        let s = single_or_vec_to_string(&SingleOrVec::from(InstanceType::Integer)).unwrap();
        assert_eq!("integer", &s);
        let s = single_or_vec_to_string(&SingleOrVec::from(InstanceType::Boolean)).unwrap();
        assert_eq!("bool", &s);
        let s = single_or_vec_to_string(&SingleOrVec::from(vec![InstanceType::Boolean])).unwrap();
        assert_eq!("[bool]", &s);
    }
}
