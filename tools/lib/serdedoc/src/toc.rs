// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::data::{AllData, DataType, DataTypeInner, StructFieldType};
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::io::Write;

/// Helper class for generating a table of contents yaml file.
pub struct TableOfContents<'a> {
    /// The data for the interface.
    data: &'a AllData,
}

impl<'a> TableOfContents<'a> {
    /// Construct a TableOfContents from the `data`.
    pub fn new(data: &'a AllData) -> Self {
        Self { data }
    }

    /// Write the table of contents yaml file to `writer`.
    pub fn write(&self, writer: &mut impl Write) -> Result<()> {
        // Complete a depth-first-search and track the nodes on the path to
        // prevent cycles.
        let mut stack = Vec::<(String, String, usize)>::new();
        let mut path = HashSet::<String>::new();

        // Keep a set of all the types to detect types that are not in the graph.
        let mut all_types = HashSet::new();
        all_types.extend(self.data.data_types.keys());

        stack.push((self.data.root.clone(), "ProductConfig".to_string(), 0));
        path.insert(self.data.root.clone());

        let mut last_indent_level = 0;
        let mut last_rust_type = self.data.root.clone();
        while let Some((rust_type, name, indent_level)) = stack.pop() {
            // We just traversed downwards, so add a path node.
            if indent_level > last_indent_level {
                path.insert(rust_type.clone());
            }
            // Otherwise, we traversed upwards and need to remove a node.
            else {
                path.remove(&last_rust_type);
            }

            // Update the state.
            last_indent_level = indent_level;
            last_rust_type = rust_type.clone();

            // Add the children for structs.
            let node = self
                .data
                .data_types
                .get(&rust_type)
                .ok_or_else(|| anyhow!("node missing: {}", rust_type))?;
            let mut has_children = false;
            if let DataTypeInner::Struct(data) = &node.inner {
                for field in &data.fields {
                    if path.contains(&field.field_name) {
                        panic!("dependency cycle!");
                    }
                    if let StructFieldType::Custom { data_type, .. } = &field.data_type {
                        stack.push((data_type.clone(), field.field_name.clone(), indent_level + 1));
                        has_children = true;
                    }
                }
            }

            // Print the current node.
            self.write_node(writer, name, &node, has_children, indent_level)?;
            all_types.remove(&rust_type);
        }

        // TODO(b/332348955): Support complex enum variant types.
        // Write out types that are not connected to the graph yet due to b/332348955
        for ty in all_types {
            if let Some(node) = self.data.data_types.get(ty) {
                self.write_node(writer, ty.clone(), &node, false, 0)?;
            } else {
                anyhow::bail!("node missing for {ty}")
            }
        }

        Ok(())
    }

    /// Write a single content entry at a specific `indent_level`.
    /// If `has_children`, then an expandable section will be added.
    fn write_node(
        &self,
        writer: &mut impl Write,
        name: String,
        data_type: &DataType,
        has_children: bool,
        indent_level: usize,
    ) -> Result<()> {
        let spaces = indent_level * 2;
        write!(writer, "{: <2$}- title: {}\n", "", name, spaces)?;
        if !has_children {
            write!(
                writer,
                "{: <3$}  path: {}{}\n",
                "", self.data.url_path, data_type.rust_type, spaces
            )?;
        } else {
            write!(writer, "{: <1$}  section:\n", "", spaces)?;
            write!(writer, "{: <1$}  - title: Overview\n", "", spaces)?;
            write!(
                writer,
                "{: <3$}    path: {}{}\n",
                "", self.data.url_path, data_type.rust_type, spaces
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TableOfContents;
    use crate::data::{
        AllData, DataType, DataTypeInner, EnumDataType, PrimitiveDataType, StructDataType,
        StructFieldData, StructFieldType,
    };
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn test() {
        let data = AllData {
            url_path: "url/".into(),
            root: "root_type".into(),
            data_types: BTreeMap::from([
                (
                    "root_type".into(),
                    DataType {
                        rust_type: "root_type".into(),
                        description: "root description".into(),
                        inner: DataTypeInner::Struct(StructDataType {
                            fields: BTreeSet::from([
                                // primitives are ignored
                                StructFieldData {
                                    field_name: "field_1".into(),
                                    description: "description of field_1".into(),
                                    default: None,
                                    data_type: StructFieldType::Primitive {
                                        data_type: "primitive_1".into(),
                                    },
                                },
                                // custom types are not ignored
                                StructFieldData {
                                    field_name: "field_2".into(),
                                    description: "description of field_2".into(),
                                    default: None,
                                    data_type: StructFieldType::Custom {
                                        data_type: "custom_1".into(),
                                    },
                                },
                                StructFieldData {
                                    field_name: "field_3".into(),
                                    description: "description of field_3".into(),
                                    default: None,
                                    data_type: StructFieldType::Custom {
                                        data_type: "custom_2".into(),
                                    },
                                },
                                StructFieldData {
                                    field_name: "field_4".into(),
                                    description: "description of field_4".into(),
                                    default: None,
                                    data_type: StructFieldType::Custom {
                                        data_type: "custom_4".into(),
                                    },
                                },
                            ]),
                        }),
                    },
                ),
                (
                    "custom_1".into(),
                    DataType {
                        rust_type: "custom_1".into(),
                        description: "custom_1 description".into(),
                        inner: DataTypeInner::Primitive(PrimitiveDataType {
                            data_type: "int".into(),
                        }),
                    },
                ),
                (
                    "custom_2".into(),
                    DataType {
                        rust_type: "custom_2".into(),
                        description: "custom_2 description".into(),
                        inner: DataTypeInner::Struct(StructDataType {
                            fields: BTreeSet::from([StructFieldData {
                                field_name: "nested_field_1".into(),
                                description: "description of nested_field_1".into(),
                                default: None,
                                data_type: StructFieldType::Custom { data_type: "custom_3".into() },
                            }]),
                        }),
                    },
                ),
                (
                    "custom_3".into(),
                    DataType {
                        rust_type: "custom_3".into(),
                        description: "custom_3 description".into(),
                        inner: DataTypeInner::Primitive(PrimitiveDataType {
                            data_type: "string".into(),
                        }),
                    },
                ),
                (
                    "custom_4".into(),
                    DataType {
                        rust_type: "custom_4".into(),
                        description: "custom_4 description".into(),
                        inner: DataTypeInner::Enum(EnumDataType {
                            variants: BTreeSet::from(["Variant1".into(), "Variant2".into()]),
                        }),
                    },
                ),
            ]),
        };
        let toc = TableOfContents::new(&data);

        let mut output = vec![];
        toc.write(&mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        let expected = r"- title: ProductConfig
  section:
  - title: Overview
    path: url/root_type
  - title: field_2
    path: url/custom_1
  - title: field_3
    section:
    - title: Overview
      path: url/custom_2
    - title: nested_field_1
      path: url/custom_3
  - title: field_4
    path: url/custom_4
";
        assert_eq!(expected, &output);
    }
}
