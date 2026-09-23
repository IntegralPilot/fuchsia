// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::CompileBoardArgs;
use crate::bind_generator::generate_bind_file;
use crate::cml_generator::generate_board_cml_file;
use crate::parser::*;
use anyhow::Context;
use dml_config as fbdc;
use fidl_fuchsia_driver_metadata as fdr;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct LocalResourceEntry {
    node: String,
    constraint: Value,
    name: Option<String>,
    service: String,
}

/// Strips the leading '#' from a node/component reference name if present.
/// In DML/CML, child nodes are often referenced with a leading '#'.
fn strip_hash(s: &str) -> String {
    if s.starts_with('#') { s[1..].to_string() } else { s.to_string() }
}

pub fn compute_global_id(provider: &str, name: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for b in format!("{}:{}", provider, name).bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    if hash == 0 { 1 } else { hash }
}

/// Returns the index of the device with the given name in the `devices` vector,
/// creating a new entry if it doesn't exist.
///
/// The returned index is arbitrary and used solely for local lookup and tracking
/// within the compiler. It does not correspond to the device's runtime ID (which is
/// assigned separately). The order of devices in the vector is determined by the
/// order they are processed, which matches their appearance in the DML input.
fn get_or_create_device_idx(
    devices: &mut Vec<fbdc::Device>,
    name: &str,
    url: Option<String>,
) -> usize {
    if let Some(idx) = devices.iter().position(|d| d.name.as_deref() == Some(name)) {
        if url.is_some() && devices[idx].url.is_none() {
            devices[idx].url = url;
        }
        return idx;
    }
    let dev = fbdc::Device { name: Some(name.to_string()), url, ..Default::default() };
    devices.push(dev);
    devices.len() - 1
}

fn flatten_value(
    val: &Value,
    prefix: &str,
    entries: &mut Vec<fdr::DictionaryEntry>,
) -> Result<(), anyhow::Error> {
    match val {
        Value::Null => {}
        Value::Bool(b) => {
            entries.push(fdr::DictionaryEntry {
                key: prefix.to_string(),
                value: fdr::DictionaryValue::Boolean(*b),
            });
        }
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                entries.push(fdr::DictionaryEntry {
                    key: prefix.to_string(),
                    value: fdr::DictionaryValue::Int64(i),
                });
            } else if let Some(u) = n.as_u64() {
                entries.push(fdr::DictionaryEntry {
                    key: prefix.to_string(),
                    value: fdr::DictionaryValue::Int64(u as i64),
                });
            } else {
                anyhow::bail!("Unsupported number type: {}", n);
            }
        }
        Value::String(s) => {
            entries.push(fdr::DictionaryEntry {
                key: prefix.to_string(),
                value: fdr::DictionaryValue::Str(s.clone()),
            });
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                entries.push(fdr::DictionaryEntry {
                    key: format!("{}._count", prefix),
                    value: fdr::DictionaryValue::Int64(0),
                });
            } else {
                let first = &arr[0];
                match first {
                    Value::Number(_) => {
                        let mut vec = Vec::new();
                        for item in arr {
                            if let Value::Number(n) = item {
                                if let Some(i) = n.as_i64() {
                                    vec.push(i);
                                } else if let Some(u) = n.as_u64() {
                                    vec.push(u as i64);
                                } else {
                                    anyhow::bail!(
                                        "Unsupported number in metadata: {} (only 64-bit integers are supported)",
                                        n
                                    );
                                }
                            } else {
                                anyhow::bail!(
                                    "Heterogeneous array element: expected number, found: {:?}",
                                    item
                                );
                            }
                        }
                        entries.push(fdr::DictionaryEntry {
                            key: prefix.to_string(),
                            value: fdr::DictionaryValue::Int64Vec(vec),
                        });
                    }
                    Value::String(_) => {
                        let mut vec = Vec::new();
                        for item in arr {
                            if let Value::String(s) = item {
                                vec.push(s.clone());
                            } else {
                                anyhow::bail!(
                                    "Heterogeneous array element: expected string, found: {:?}",
                                    item
                                );
                            }
                        }
                        entries.push(fdr::DictionaryEntry {
                            key: prefix.to_string(),
                            value: fdr::DictionaryValue::StrVec(vec),
                        });
                    }
                    Value::Object(_) => {
                        entries.push(fdr::DictionaryEntry {
                            key: format!("{}._count", prefix),
                            value: fdr::DictionaryValue::Int64(arr.len() as i64),
                        });
                        for (idx, item) in arr.iter().enumerate() {
                            if !item.is_object() {
                                anyhow::bail!(
                                    "Heterogeneous array element: expected object, found: {:?}",
                                    item
                                );
                            }
                            let child_prefix = format!("{}.{}", prefix, idx);
                            flatten_value(item, &child_prefix, entries)?;
                        }
                    }
                    _ => anyhow::bail!("Unsupported array element type: {:?}", first),
                }
            }
        }
        Value::Object(obj) => {
            for (key, child_val) in obj {
                let child_prefix =
                    if prefix.is_empty() { key.clone() } else { format!("{}.{}", prefix, key) };
                flatten_value(child_val, &child_prefix, entries)?;
            }
        }
    }
    Ok(())
}

fn build_generic_metadata_value(
    mapping: &MetadataMapping,
    provider_id: u32,
    resources: &[LocalResourceEntry],
) -> Result<Value, anyhow::Error> {
    let mut root_obj = serde_json::Map::new();

    root_obj.insert(
        crate::workarounds::provider_id_key().to_string(),
        Value::Number(provider_id.into()),
    );

    for agg in &mapping.aggregations {
        root_obj.insert(agg.field.clone(), Value::Array(Vec::new()));
    }

    for agg in &mapping.aggregations {
        let mut arr = Vec::new();

        for res in resources {
            if res.service == agg.service {
                let mut val = res.constraint.clone();

                if let Some(obj) = val.as_object_mut() {
                    if !obj.contains_key("name") {
                        let name_to_insert = if !agg.use_node_name {
                            res.name.as_deref().unwrap_or(res.node.as_str())
                        } else {
                            res.node.as_str()
                        };
                        obj.insert("name".to_string(), Value::String(name_to_insert.to_string()));
                    }
                }
                arr.push(val);
            }
        }
        crate::workarounds::deduplicate_metadata_resources(&mapping.metadata_id, &mut arr);
        root_obj.insert(agg.field.clone(), Value::Array(arr));
    }

    Ok(Value::Object(root_obj))
}

pub fn compile_board(args: &CompileBoardArgs, year: &str) -> Result<(), anyhow::Error> {
    if args.out_dir.is_none()
        && (args.fidl_output.is_none() || args.bind_output.is_none() || args.cml_output.is_none())
    {
        return Err(anyhow::anyhow!(
            "Either out_dir or all of (fidl_output, bind_output, cml_output) must be specified"
        ));
    }

    let input_path = Path::new(&args.input_file);
    let out_dir = args.out_dir.as_deref().map(Path::new);

    if let Some(dir) = out_dir {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(path) = &args.fidl_output {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    if let Some(path) = &args.bind_output {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    if let Some(path) = &args.cml_output {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let board_dml = load_dml_file_root(input_path)?;

    let mut devices = Vec::new();
    let mut aggregates = Vec::new();

    // Process children
    for child in &board_dml.children {
        let idx = get_or_create_device_idx(&mut devices, &child.name, child.url.clone());
        devices[idx].compatible = child.compatible.clone();
        devices[idx].id = child.id;
        if !child.metadata.is_empty() {
            let new_meta: Vec<_> = child
                .metadata
                .iter()
                .map(|m| fbdc::StaticMetadata {
                    id: Some(m.id.clone()),
                    data: m.data.clone(),
                    ..Default::default()
                })
                .collect();
            let existing = devices[idx].metadata.get_or_insert_with(Vec::new);
            for new_item in new_meta {
                if existing.iter().any(|m| m.id == new_item.id) {
                    anyhow::bail!(
                        "Device '{}' has duplicate metadata entry for ID '{}'",
                        child.name,
                        new_item.id.as_ref().unwrap()
                    );
                }
                existing.push(new_item);
            }
        }
    }

    let mut auto_incrementer =
        crate::workarounds::AutoIncrementer::new(&board_dml.metadata_mappings);
    let mut aggregates_list = Vec::<((String, String), Vec<LocalResourceEntry>)>::new();

    // Process offers
    for offer in &board_dml.offer {
        let to_name = strip_hash(&offer.to);
        let _ = get_or_create_device_idx(&mut devices, &to_name, None);

        if let Some(service_name) = &offer.service {
            let from = offer.from.as_ref().ok_or_else(|| {
                anyhow::anyhow!("'from' is missing in service offer for '{}'", to_name)
            })?;

            let provider = if from == "parent" { "pdev".to_string() } else { strip_hash(from) };

            let mut constraint_val =
                offer.constraints.clone().unwrap_or_else(|| Value::Object(serde_json::Map::new()));

            auto_incrementer.apply(service_name, &provider, &mut constraint_val)?;

            let global_id = compute_global_id(&provider, offer.name.as_deref().unwrap_or(&to_name));
            if let Some(obj) = constraint_val.as_object_mut() {
                if !obj.contains_key("id") {
                    obj.insert("id".to_string(), Value::Number(global_id.into()));
                }
            }

            let entry = LocalResourceEntry {
                node: to_name.clone(),
                constraint: constraint_val,
                name: offer.name.clone(),
                service: service_name.clone(),
            };
            let key = (provider, service_name.clone());
            if let Some(existing) = aggregates_list.iter_mut().find(|(k, _)| *k == key) {
                existing.1.push(entry);
            } else {
                aggregates_list.push((key, vec![entry]));
            }
        }
    }

    for ((provider, service), resources) in &aggregates_list {
        let mut unique_resources = Vec::new();
        for res in resources {
            if !unique_resources.iter().any(|r: &LocalResourceEntry| {
                r.node == res.node && r.constraint == res.constraint && r.name == res.name
            }) {
                unique_resources.push(res.clone());
            }
        }

        let mut fidl_resources = Vec::new();
        for res in unique_resources {
            let mut constraint_entries = Vec::new();
            flatten_value(&res.constraint, "", &mut constraint_entries)?;
            let constraint_dict =
                fdr::Dictionary { entries: Some(constraint_entries), ..Default::default() };
            fidl_resources.push(fbdc::ResourceEntry {
                node: Some(res.node.clone()),
                constraint: Some(constraint_dict),
                name: res.name.clone(),
                ..Default::default()
            });
        }

        aggregates.push(fbdc::AggregateEntry {
            provider: Some(provider.clone()),
            service: Some(service.clone()),
            resources: Some(fidl_resources),
            ..Default::default()
        });
    }

    // 4. Serialize aggregated metadata to FIDL and attach to provider devices
    let mut service_to_metadata_id = std::collections::HashMap::new();
    for mapping in &board_dml.metadata_mappings {
        for agg in &mapping.aggregations {
            service_to_metadata_id.insert(agg.service.clone(), mapping.metadata_id.clone());
        }
    }

    let mut metadata_map = BTreeMap::<(String, String), Vec<LocalResourceEntry>>::new();
    for ((provider, service), resources) in &aggregates_list {
        if service == "fuchsia.hardware.platform.device.Service" {
            continue;
        }
        if let Some(metadata_id) = service_to_metadata_id.get(service.as_str()) {
            metadata_map
                .entry((provider.clone(), metadata_id.to_string()))
                .or_default()
                .extend(resources.clone());
        }
    }

    for ((provider, metadata_id), resources) in metadata_map {
        let dev_idx = get_or_create_device_idx(&mut devices, &provider, None);
        let provider_id = devices[dev_idx].id.unwrap_or(0);

        let mapping =
            board_dml.metadata_mappings.iter().find(|m| m.metadata_id == metadata_id).ok_or_else(
                || {
                    anyhow::anyhow!(
                        "No metadata mapping found in board DML for metadata ID: {}",
                        metadata_id
                    )
                },
            )?;

        let root_val = build_generic_metadata_value(mapping, provider_id, &resources)?;
        let mut entries = Vec::new();
        flatten_value(&root_val, "", &mut entries)?;
        let dictionary = fdr::Dictionary { entries: Some(entries), ..Default::default() };
        let serialized_bytes =
            fidl::persist(&dictionary).context("Failed to serialize Dictionary to FIDL")?;

        // Attach to provider device
        let metadata = devices[dev_idx].metadata.get_or_insert_with(Vec::new);
        metadata.push(fbdc::StaticMetadata {
            id: Some(metadata_id.clone()),
            data: Some(serialized_bytes),
            ..Default::default()
        });
    }

    // 5. Serialize BoardConfig to FIDL and write to file
    let board_config = fbdc::BoardConfig {
        devices: Some(devices),
        aggregates: Some(aggregates),
        ..Default::default()
    };
    let serialized_board_config =
        fidl::persist(&board_config).context("Failed to serialize BoardConfig to FIDL")?;

    let out_config_path = match &args.fidl_output {
        Some(path) => PathBuf::from(path),
        None => {
            out_dir.ok_or_else(|| anyhow::anyhow!("out_dir is missing"))?.join("board-config.fidl")
        }
    };
    let mut file = File::create(out_config_path)?;
    file.write_all(&serialized_board_config)?;

    // 6. Generate Bind and CML files for the board driver
    let board_name = board_dml.name.clone().unwrap_or_else(|| "board".to_string());

    let empty_bind = DmlBind::default();
    let bind_config = board_dml.program.bind.as_ref().unwrap_or(&empty_bind);
    let bind_code = generate_bind_file(&board_name, bind_config, &[], year)?;
    let cml_code = generate_board_cml_file(
        &board_name,
        &board_dml.program,
        &board_dml.use_entries,
        &board_dml.capabilities,
        &board_dml.expose,
    )?;

    let bind_output_path = match &args.bind_output {
        Some(path) => PathBuf::from(path),
        None => out_dir
            .ok_or_else(|| anyhow::anyhow!("out_dir is missing"))?
            .join(format!("{}-dml.bind", board_name)),
    };
    let cml_output_path = match &args.cml_output {
        Some(path) => PathBuf::from(path),
        None => out_dir
            .ok_or_else(|| anyhow::anyhow!("out_dir is missing"))?
            .join(format!("{}-dml.cml", board_name)),
    };

    std::fs::write(bind_output_path, bind_code).context("Failed to write generated bind file")?;
    let header = format!(
        r#"// Copyright {year} The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS GENERATED BY dmlc. DO NOT EDIT.

"#,
    );
    let final_cml_code = format!("{}{}", header, cml_code);
    std::fs::write(cml_output_path, final_cml_code)
        .context("Failed to write generated cml file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn flatten(val: &Value) -> BTreeMap<String, fdr::DictionaryValue> {
        let mut entries = Vec::new();
        flatten_value(val, "", &mut entries).unwrap();
        entries.into_iter().map(|e| (e.key, e.value)).collect()
    }

    #[test]
    fn test_flatten_value_primitives() {
        let val = serde_json::json!({
            "a_bool": true,
            "an_int": 42,
            "a_str": "hello",
            "large_uint": 18446744073709551615u64
        });
        let flat = flatten(&val);
        assert_eq!(flat.len(), 4);
        assert_eq!(flat.get("a_bool"), Some(&fdr::DictionaryValue::Boolean(true)));
        assert_eq!(flat.get("an_int"), Some(&fdr::DictionaryValue::Int64(42)));
        assert_eq!(flat.get("a_str"), Some(&fdr::DictionaryValue::Str("hello".to_string())));
        assert_eq!(flat.get("large_uint"), Some(&fdr::DictionaryValue::Int64(-1)));
    }

    #[test]
    fn test_flatten_value_arrays() {
        let val = serde_json::json!({
            "int_arr": [1, 2, 3],
            "str_arr": ["x", "y"],
            "empty_arr": [],
            "large_uint_arr": [18446744073709551615u64]
        });
        let flat = flatten(&val);
        assert_eq!(flat.len(), 4);
        assert_eq!(flat.get("int_arr"), Some(&fdr::DictionaryValue::Int64Vec(vec![1, 2, 3])));
        assert_eq!(
            flat.get("str_arr"),
            Some(&fdr::DictionaryValue::StrVec(vec!["x".to_string(), "y".to_string()]))
        );
        assert_eq!(flat.get("empty_arr._count"), Some(&fdr::DictionaryValue::Int64(0)));
        assert_eq!(flat.get("large_uint_arr"), Some(&fdr::DictionaryValue::Int64Vec(vec![-1])));
    }

    #[test]
    fn test_flatten_value_nested() {
        let val = serde_json::json!({
            "outer": {
                "inner_bool": false,
                "nested_obj": {
                    "val": "leaf"
                }
            }
        });
        let flat = flatten(&val);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat.get("outer.inner_bool"), Some(&fdr::DictionaryValue::Boolean(false)));
        assert_eq!(
            flat.get("outer.nested_obj.val"),
            Some(&fdr::DictionaryValue::Str("leaf".to_string()))
        );
    }

    #[test]
    fn test_flatten_value_obj_array() {
        let val = serde_json::json!({
            "arr": [
                { "id": 1, "name": "first" },
                { "id": 2, "name": "second" }
            ]
        });
        let flat = flatten(&val);
        assert_eq!(flat.len(), 5);
        assert_eq!(flat.get("arr._count"), Some(&fdr::DictionaryValue::Int64(2)));
        assert_eq!(flat.get("arr.0.id"), Some(&fdr::DictionaryValue::Int64(1)));
        assert_eq!(flat.get("arr.0.name"), Some(&fdr::DictionaryValue::Str("first".to_string())));
        assert_eq!(flat.get("arr.1.id"), Some(&fdr::DictionaryValue::Int64(2)));
        assert_eq!(flat.get("arr.1.name"), Some(&fdr::DictionaryValue::Str("second".to_string())));
    }

    #[test]
    fn test_flatten_value_heterogeneous_array_error() {
        let val = serde_json::json!({
            "arr": [1, "string"]
        });
        let mut entries = Vec::new();
        let res = flatten_value(&val, "", &mut entries);
        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("Heterogeneous array element"),
            "Expected heterogeneous error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_flatten_value_float_error() {
        let val = serde_json::json!({
            "arr": [1.2, 2.3]
        });
        let mut entries = Vec::new();
        let res = flatten_value(&val, "", &mut entries);
        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("Unsupported number in metadata"),
            "Expected float error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_flatten_value_heterogeneous_obj_array_error() {
        let val = serde_json::json!({
            "arr": [
                { "id": 1 },
                "not_an_object"
            ]
        });
        let mut entries = Vec::new();
        let res = flatten_value(&val, "", &mut entries);
        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("Heterogeneous array element"),
            "Expected heterogeneous error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_duplicate_metadata_id_error() {
        let temp_dir = std::env::temp_dir().join("test_temp_dup_metadata");
        fs::create_dir_all(&temp_dir).unwrap();

        let main_file = temp_dir.join("main.dml");
        let inc_file = temp_dir.join("inc.dml");

        // inc.dml defines a device with some metadata
        fs::write(
            &inc_file,
            r#"{
                "children": [
                    {
                        "name": "my_device",
                        "metadata": [
                            {
                                "id": "my_metadata_id",
                                "data": [1, 2, 3]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();

        // main.dml includes inc.dml and re-defines the same device with the same metadata ID
        fs::write(
            &main_file,
            r#"{
                "include": ["inc.dml"],
                "children": [
                    {
                        "name": "my_device",
                        "metadata": [
                            {
                                "id": "my_metadata_id",
                                "data": [4, 5, 6]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();

        let args = CompileBoardArgs {
            input_file: main_file.to_str().unwrap().to_string(),
            out_dir: Some(temp_dir.to_str().unwrap().to_string()),
            fidl_output: None,
            bind_output: None,
            cml_output: None,
            driver_dml: vec![],
        };

        let res = compile_board(&args, "2026");

        // Clean up
        let _ = fs::remove_file(&main_file);
        let _ = fs::remove_file(&inc_file);
        let _ = fs::remove_dir(&temp_dir);

        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("has duplicate metadata entry for ID"),
            "Expected duplicate metadata ID error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_auto_increment_non_object_constraints_error() {
        let temp_dir = std::env::temp_dir().join("test_temp_autoincrement");
        fs::create_dir_all(&temp_dir).unwrap();

        let main_file = temp_dir.join("main.dml");

        fs::write(
            &main_file,
            r#"{
                "name": "test_board",
                "metadata_mappings": [
                    {
                        "metadata_id": "fuchsia.hardware.gpio.Metadata",
                        "aggregations": [
                            {
                                "service": "fuchsia.hardware.gpio.Service",
                                "field": "pin",
                                "auto_increment": "pin"
                            }
                        ]
                    }
                ],
                "offer": [
                    {
                        "from": "parent",
                        "to": "gpio",
                        "service": "fuchsia.hardware.gpio.Service",
                        "constraints": "not_an_object"
                    }
                ]
            }"#,
        )
        .unwrap();

        let args = CompileBoardArgs {
            input_file: main_file.to_str().unwrap().to_string(),
            out_dir: Some(temp_dir.to_str().unwrap().to_string()),
            fidl_output: None,
            bind_output: None,
            cml_output: None,
            driver_dml: vec![],
        };

        let res = compile_board(&args, "2026");

        // Clean up
        let _ = fs::remove_file(&main_file);
        let _ = fs::remove_dir(&temp_dir);

        assert!(res.is_err());
        let err_msg = format!("{}", res.err().unwrap());
        assert!(
            err_msg.contains("Constraints must be a JSON object when auto-incrementing"),
            "Expected auto-increment constraints error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_large_uint64_round_trip() {
        let val = serde_json::json!({
            "large_uint": 18446744073709551615u64
        });
        let mut entries = Vec::new();
        flatten_value(&val, "", &mut entries).unwrap();
        let dict = fdr::Dictionary { entries: Some(entries), ..Default::default() };

        let retrieved = fbdc::get_uint64(&dict, "large_uint").unwrap();
        assert_eq!(retrieved, 18446744073709551615u64);
    }

    #[test]
    fn test_compute_global_id() {
        let id1 = compute_global_id("pdev", "gpio-pin-1");
        let id2 = compute_global_id("pdev", "gpio-pin-1");
        let id3 = compute_global_id("pdev", "gpio-pin-2");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert_ne!(id1, 0);
    }

    #[test]
    fn test_compile_board_minimal() {
        let temp_dir = std::env::temp_dir().join("test_temp_compile_board_minimal");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let main_file = temp_dir.join("main.dml");
        fs::write(
            &main_file,
            r##"{
                "name": "test_board",
                "children": [
                    {
                        "name": "clock",
                        "url": "fuchsia-pkg://fuchsia.com/clock#meta/clock.cm",
                        "compatible": "test,clock"
                    }
                ],
                "offer": [
                    {
                        "from": "parent",
                        "to": "#clock",
                        "service": "fuchsia.hardware.platform.device.Service"
                    }
                ]
            }"##,
        )
        .unwrap();

        let args = CompileBoardArgs {
            input_file: main_file.to_str().unwrap().to_string(),
            out_dir: Some(temp_dir.to_str().unwrap().to_string()),
            fidl_output: None,
            bind_output: None,
            cml_output: None,
            driver_dml: vec![],
        };

        let res = compile_board(&args, "2026");
        assert!(res.is_ok(), "compile_board failed: {:?}", res.err());

        let fidl_path = temp_dir.join("board-config.fidl");
        let bind_path = temp_dir.join("test_board-dml.bind");
        let cml_path = temp_dir.join("test_board-dml.cml");

        assert!(fidl_path.exists(), "Expected board-config.fidl to exist");
        assert!(bind_path.exists(), "Expected test_board-dml.bind to exist");
        assert!(cml_path.exists(), "Expected test_board-dml.cml to exist");

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
