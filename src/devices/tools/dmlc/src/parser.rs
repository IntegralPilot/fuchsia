// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
pub struct PciBind {
    pub vid: Option<Value>,
    pub did: Option<Value>,
    pub class: Option<Value>,
    pub subclass: Option<Value>,
    pub interface: Option<Value>,
    pub revision: Option<Value>,
    pub topo: Option<Value>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
pub struct UsbBind {
    pub vid: Option<Value>,
    pub pid: Option<Value>,
    pub class: Option<Value>,
    pub subclass: Option<Value>,
    pub protocol: Option<Value>,
    #[serde(alias = "interface")]
    pub interface_number: Option<Value>,
    pub bind_protocol: Option<String>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
pub struct AcpiBind {
    pub hid: Option<Value>,
    pub first_cid: Option<Value>,
    pub bus_type: Option<Value>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
pub struct BindPrimary {
    pub node: String,
    pub compat: Option<Value>,
    pub vid: Option<Value>,
    pub pid: Option<Value>,
    pub did: Option<Value>,
    pub protocol: Option<String>,
    pub service: Option<String>,
    pub banjo: Option<String>,
    pub transport: Option<String>,
    pub pci: Option<PciBind>,
    pub usb: Option<UsbBind>,
    pub acpi: Option<AcpiBind>,
    pub node_name: Option<Value>,
    pub match_name: Option<bool>,
    pub one_of: Option<Vec<DmlBind>>,
    pub rules: Option<HashMap<String, Value>>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
pub struct DmlBind {
    pub protocol: Option<String>,
    pub service: Option<String>,
    pub banjo: Option<String>,
    pub transport: Option<String>,
    pub vid: Option<Value>,
    pub pid: Option<Value>,
    pub did: Option<Value>,
    pub compat: Option<Value>,
    pub pci: Option<PciBind>,
    pub usb: Option<UsbBind>,
    pub acpi: Option<AcpiBind>,
    pub node_name: Option<Value>,
    pub match_name: Option<bool>,
    pub primary: Option<BindPrimary>,
    pub one_of: Option<Vec<DmlBind>>,
    pub rules: Option<HashMap<String, Value>>,
    #[serde(alias = "name")]
    pub composite_name: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct DmlProgram {
    pub driver_name: Option<String>,
    #[serde(alias = "requirements")]
    pub bind: Option<DmlBind>,
}

#[derive(Deserialize, Debug, Default)]
pub struct DmlInput {
    pub name: Option<String>,
    #[serde(alias = "composite_name")]
    #[allow(dead_code)]
    pub composite_name: Option<String>,
    #[serde(default)]
    pub program: DmlProgram,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub children: Vec<DmlChild>,
    #[serde(default, alias = "offers")]
    pub offer: Vec<DmlOffer>,
    #[serde(default)]
    pub metadata_mappings: Vec<MetadataMapping>,
    #[serde(default, rename = "use")]
    pub use_entries: Vec<Value>,
    #[serde(default)]
    pub capabilities: Vec<Value>,
    #[serde(default)]
    pub expose: Vec<Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DmlStaticMetadata {
    pub id: String,
    pub data: Option<Vec<u8>>,
}

#[derive(Deserialize, Debug)]
pub struct DmlChild {
    pub name: String,
    pub id: Option<u32>,
    pub url: Option<String>,
    pub compatible: Option<String>,
    #[serde(default)]
    pub metadata: Vec<DmlStaticMetadata>,
}

#[derive(Deserialize, Debug)]
pub struct DmlOffer {
    pub from: Option<String>,
    pub to: String,
    pub service: Option<String>,
    pub constraints: Option<Value>,
    pub name: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct DriverDml {
    pub name: String,
    #[serde(default, alias = "composite_name")]
    pub composite_name: Option<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub program: Value,
    #[serde(default)]
    pub capabilities: Vec<Value>,
    #[serde(default, rename = "use")]
    pub use_entries: Vec<Value>,
    #[serde(default)]
    pub expose: Vec<Value>,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Deserialize, Debug)]
pub struct DriverCapability {
    #[serde(rename = "service")]
    pub _service: Option<String>,
    pub metadata: Option<DriverMetadataSchemaDef>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DriverMetadataSchemaDef {
    pub id: String,
    pub schema: Option<Value>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MetadataAggregationDef {
    pub service: String,
    pub field: String,
    #[serde(default)]
    pub use_node_name: bool,
    pub auto_increment: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MetadataMapping {
    pub metadata_id: String,
    pub aggregations: Vec<MetadataAggregationDef>,
}

// Simplified AST for JSON Schema
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Bool,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Int8,
    Int16,
    Int32,
    Int64,
    String,
    Vector(Box<Type>),
    Struct(String),
    Enum(String),
    ProviderId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<String>,
}

pub const DEFAULT_ROOT_STRUCT_NAME: &str = "Root";

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub id: String,
    pub enums: HashMap<String, EnumDef>,
    pub structs: HashMap<String, StructDef>,
    pub root_layout: StructDef,
}

impl Schema {
    /// Determines the name for the generated root metadata struct.
    ///
    /// Naming precedence:
    /// 1. If the schema explicitly specifies a `title` property, use that title.
    /// 2. If there is only a single schema (`total_schemas == 1`), default to
    ///    `<DriverName>Metadata` (preserving backwards compatibility).
    /// 3. If there are multiple schemas and `title` is omitted, fall back to
    ///    the capitalized last dot-separated segment of the schema `id`.
    pub fn root_struct_name(&self, normalized_driver_name: &str, total_schemas: usize) -> String {
        if self.root_layout.name != DEFAULT_ROOT_STRUCT_NAME {
            self.root_layout.name.clone()
        } else if total_schemas == 1 {
            format!("{}Metadata", capitalize(normalized_driver_name))
        } else {
            let last_part = self.id.rsplit('.').next().unwrap_or(normalized_driver_name);
            capitalize(last_part)
        }
    }
}

pub fn load_dml_file(
    path: &Path,
    visiting: &mut HashSet<PathBuf>,
    processed: &mut HashSet<PathBuf>,
) -> Result<DmlInput, anyhow::Error> {
    let canonical = path.canonicalize()?;
    if visiting.contains(&canonical) {
        anyhow::bail!("Circular include detected: {}", canonical.display());
    }
    if processed.contains(&canonical) {
        return Ok(DmlInput::default());
    }

    visiting.insert(canonical.clone());

    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mut input: DmlInput = serde_json5::from_reader(file)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    let parent_dir = canonical
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Path has no parent: {}", canonical.display()))?;
    let mut all_children = Vec::new();
    let mut all_offers = Vec::new();
    let mut all_mappings = Vec::new();
    let mut all_use_entries = Vec::new();
    let mut all_capabilities = Vec::new();
    let mut all_expose = Vec::new();

    for include in &input.include {
        let include_path = parent_dir.join(include);
        let include_input = load_dml_file(&include_path, visiting, processed)?;
        all_children.extend(include_input.children);
        all_offers.extend(include_input.offer);
        all_mappings.extend(include_input.metadata_mappings);
        all_use_entries.extend(include_input.use_entries);
        all_capabilities.extend(include_input.capabilities);
        all_expose.extend(include_input.expose);
    }

    all_children.extend(input.children);
    all_offers.extend(input.offer);
    all_mappings.extend(input.metadata_mappings);
    all_use_entries.extend(input.use_entries);
    all_capabilities.extend(input.capabilities);
    all_expose.extend(input.expose);

    input.children = all_children;
    input.offer = all_offers;
    input.metadata_mappings = all_mappings;
    input.use_entries = all_use_entries;
    input.capabilities = all_capabilities;
    input.expose = all_expose;

    visiting.remove(&canonical);
    processed.insert(canonical);

    Ok(input)
}

pub fn load_dml_file_root(path: &Path) -> Result<DmlInput, anyhow::Error> {
    let mut visiting = HashSet::new();
    let mut processed = HashSet::new();
    load_dml_file(path, &mut visiting, &mut processed)
}

pub fn load_driver_dml(path: &Path) -> Result<DriverDml, anyhow::Error> {
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let driver_dml: DriverDml = serde_json5::from_reader(file)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(driver_dml)
}

pub fn parse_json_schema(schema_val: &Value, metadata_id: &str) -> Result<Schema, anyhow::Error> {
    let mut enums = HashMap::new();
    let mut structs = HashMap::new();

    if let Some(defs) = schema_val.get("definitions").and_then(|v| v.as_object()) {
        // Pass 1: Parse all enums
        for (name, def) in defs {
            if let Some(variants) = def.get("enum").and_then(|v| v.as_array()) {
                let variants: Vec<String> = variants
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .ok_or_else(|| anyhow::anyhow!("Enum variant must be string"))
                            .map(|s| s.to_string())
                    })
                    .collect::<Result<_, _>>()?;
                enums.insert(name.clone(), EnumDef { name: name.clone(), variants });
            }
        }
        // Pass 2: Parse all structs
        for (name, def) in defs {
            if def.get("type").and_then(|v| v.as_str()) == Some("object") {
                let fields = parse_properties(def, &enums)?;
                structs.insert(name.clone(), StructDef { name: name.clone(), fields });
            }
        }
    }

    if schema_val.get("type").and_then(|v| v.as_str()) != Some("object") {
        anyhow::bail!("Root schema must be of type 'object'");
    }
    let root_name = schema_val
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| DEFAULT_ROOT_STRUCT_NAME.to_string());
    let root_fields = parse_properties(schema_val, &enums)?;
    let root_layout = StructDef { name: root_name, fields: root_fields };

    Ok(Schema { id: metadata_id.to_string(), enums, structs, root_layout })
}

fn parse_properties(
    obj_val: &Value,
    enums: &HashMap<String, EnumDef>,
) -> Result<Vec<Field>, anyhow::Error> {
    let mut fields = Vec::new();
    let properties = obj_val
        .get("properties")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("Missing 'properties' in object definition"))?;
    let required: HashSet<String> = match obj_val.get("required") {
        Some(Value::Array(arr)) => {
            let mut required_fields = HashSet::new();
            for v in arr {
                let s = v.as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "JSON Schema 'required' array elements must be strings: {:?}",
                        v
                    )
                })?;
                required_fields.insert(s.to_string());
            }
            required_fields
        }
        Some(_) => anyhow::bail!("JSON Schema 'required' must be an array"),
        None => HashSet::new(),
    };

    for (name, prop) in properties {
        let ty = parse_type(prop, enums)?;
        let optional = !required.contains(name);
        fields.push(Field { name: name.clone(), ty, optional });
    }
    Ok(fields)
}

fn parse_type(prop_val: &Value, enums: &HashMap<String, EnumDef>) -> Result<Type, anyhow::Error> {
    if let Some(ref_path) = prop_val.get("$ref").and_then(|v| v.as_str()) {
        let name = ref_path
            .split('/')
            .next_back()
            .ok_or_else(|| anyhow::anyhow!("Invalid $ref: {}", ref_path))?;
        if enums.contains_key(name) {
            return Ok(Type::Enum(name.to_string()));
        } else {
            return Ok(Type::Struct(name.to_string()));
        }
    }

    let type_str = prop_val
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'type' in property: {:?}", prop_val))?;

    match type_str {
        "boolean" => Ok(Type::Bool),
        "string" => Ok(Type::String),
        "integer" => {
            if let Some(fuchsia_type) = prop_val.get("fuchsia_type").and_then(|v| v.as_str()) {
                match fuchsia_type {
                    "uint8" => Ok(Type::Uint8),
                    "uint16" => Ok(Type::Uint16),
                    "uint32" => Ok(Type::Uint32),
                    "uint64" => Ok(Type::Uint64),
                    "int8" => Ok(Type::Int8),
                    "int16" => Ok(Type::Int16),
                    "int32" => Ok(Type::Int32),
                    "int64" => Ok(Type::Int64),
                    "provider_id" => Ok(Type::ProviderId),
                    _ => anyhow::bail!("Unsupported fuchsia_type: {}", fuchsia_type),
                }
            } else {
                Ok(Type::Uint32)
            }
        }
        "array" => {
            let items_val = prop_val
                .get("items")
                .ok_or_else(|| anyhow::anyhow!("Missing 'items' in array property"))?;
            let item_ty = parse_type(items_val, enums)?;
            Ok(Type::Vector(Box::new(item_ty)))
        }
        _ => anyhow::bail!("Unsupported JSON Schema type: {}", type_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_circular_include_error() {
        let temp_dir = std::env::temp_dir().join("test_temp_circular");
        fs::create_dir_all(&temp_dir).unwrap();

        let file1 = temp_dir.join("a.dml");
        let file2 = temp_dir.join("b.dml");

        // a.dml includes b.dml
        fs::write(&file1, r#"{"include": ["b.dml"]}"#).unwrap();
        // b.dml includes a.dml
        fs::write(&file2, r#"{"include": ["a.dml"]}"#).unwrap();

        let res = load_dml_file_root(&file1);

        // Clean up
        let _ = fs::remove_file(&file1);
        let _ = fs::remove_file(&file2);
        let _ = fs::remove_dir(&temp_dir);

        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("Circular include detected"),
            "Expected circular include error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_diamond_include_success() {
        let temp_dir = std::env::temp_dir().join("test_temp_diamond");
        fs::create_dir_all(&temp_dir).unwrap();

        let file_a = temp_dir.join("a.dml");
        let file_b = temp_dir.join("b.dml");
        let file_c = temp_dir.join("c.dml");
        let file_d = temp_dir.join("d.dml");

        // d.dml has some children
        fs::write(&file_d, r#"{"children": [{"name": "child_d"}]}"#).unwrap();
        // b.dml includes d.dml
        fs::write(&file_b, r#"{"include": ["d.dml"]}"#).unwrap();
        // c.dml includes d.dml
        fs::write(&file_c, r#"{"include": ["d.dml"]}"#).unwrap();
        // a.dml includes b.dml and c.dml
        fs::write(&file_a, r#"{"include": ["b.dml", "c.dml"]}"#).unwrap();

        let res = load_dml_file_root(&file_a);

        // Clean up
        let _ = fs::remove_file(&file_a);
        let _ = fs::remove_file(&file_b);
        let _ = fs::remove_file(&file_c);
        let _ = fs::remove_file(&file_d);
        let _ = fs::remove_dir(&temp_dir);

        assert!(res.is_ok());
        let input = res.unwrap();
        // child_d should be included exactly once because of the processed cache
        let child_names: Vec<String> = input.children.iter().map(|c| c.name.clone()).collect();
        assert_eq!(child_names, vec!["child_d"]);
    }

    #[test]
    fn test_dml_schema_is_valid_json() {
        let schema_str = include_str!("../dml.schema.json");
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(schema_str);
        assert!(parsed.is_ok(), "dml.schema.json is not valid JSON: {:?}", parsed.err());
        let val = parsed.unwrap();
        assert_eq!(
            val.get("$schema").and_then(|v| v.as_str()),
            Some("http://json-schema.org/draft-07/schema#")
        );
        let defs = val
            .get("definitions")
            .and_then(|v| v.as_object())
            .expect("missing definitions in schema");
        for key in [
            "PciBind",
            "UsbBind",
            "AcpiBind",
            "DmlBind",
            "Program",
            "UseEntry",
            "BoardChild",
            "BoardOffer",
            "MetadataMapping",
        ] {
            assert!(defs.contains_key(key), "Missing key definition '{}' in dml.schema.json", key);
        }
    }

    #[test]
    fn test_board_dml_offer_and_offers_alias() {
        let json_offer = r##"{
            "offer": [
                {
                    "from": "parent",
                    "to": "#child",
                    "service": "fuchsia.hardware.gpio.Service"
                }
            ]
        }"##;
        let parsed_offer: DmlInput = serde_json5::from_str(json_offer).unwrap();
        assert_eq!(parsed_offer.offer.len(), 1);
        assert_eq!(parsed_offer.offer[0].to, "#child");

        let json_offers = r##"{
            "offers": [
                {
                    "from": "parent",
                    "to": "#child2",
                    "service": "fuchsia.hardware.gpio.Service"
                }
            ]
        }"##;
        let parsed_offers: DmlInput = serde_json5::from_str(json_offers).unwrap();
        assert_eq!(parsed_offers.offer.len(), 1);
        assert_eq!(parsed_offers.offer[0].to, "#child2");
    }

    #[test]
    fn test_board_dml_include_offer_merging() {
        let temp_dir = std::env::temp_dir().join("test_temp_offer_merging");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let shard_offer = temp_dir.join("shard_offer.dml");
        let shard_offers = temp_dir.join("shard_offers.dml");
        let root_file = temp_dir.join("root.dml");

        // shard_offer.dml uses modern "offer"
        fs::write(
            &shard_offer,
            r##"{
                "offer": [
                    {
                        "from": "parent",
                        "to": "#child1",
                        "service": "fuchsia.hardware.gpio.Service"
                    }
                ]
            }"##,
        )
        .unwrap();

        // shard_offers.dml uses legacy "offers"
        fs::write(
            &shard_offers,
            r##"{
                "offers": [
                    {
                        "from": "parent",
                        "to": "#child2",
                        "service": "fuchsia.hardware.i2c.Service"
                    }
                ]
            }"##,
        )
        .unwrap();

        // root.dml includes both shards and defines its own "offer"
        fs::write(
            &root_file,
            r##"{
                "include": ["shard_offer.dml", "shard_offers.dml"],
                "offer": [
                    {
                        "from": "parent",
                        "to": "#child3",
                        "service": "fuchsia.hardware.clock.Service"
                    }
                ]
            }"##,
        )
        .unwrap();

        let res = load_dml_file_root(&root_file);

        // Clean up
        let _ = fs::remove_file(&shard_offer);
        let _ = fs::remove_file(&shard_offers);
        let _ = fs::remove_file(&root_file);
        let _ = fs::remove_dir(&temp_dir);

        assert!(res.is_ok(), "Failed to load root file with included offers: {:?}", res.err());
        let input = res.unwrap();
        assert_eq!(input.offer.len(), 3);
        assert_eq!(input.offer[0].to, "#child1");
        assert_eq!(input.offer[1].to, "#child2");
        assert_eq!(input.offer[2].to, "#child3");
    }
}
