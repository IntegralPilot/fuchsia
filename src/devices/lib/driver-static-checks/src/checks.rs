// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::utils;
use anyhow::{Context, Result};
use bind::interpreter::decode_bind_rules::DecodedRules;
use cm_rust::ComponentDecl;
use fidl_fuchsia_data as fdata;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Eq, PartialEq)]
pub struct StaticCheckPass {}

// TODO(b/260630596): Convert to enum with variant for each distinct error type.
#[derive(Debug, Eq, PartialEq)]
pub struct StaticCheckFail {
    /// The reason for the failure, to be displayed to the user. The reason should be a
    /// complete sentence, i.e. it should begin with a capital letter and end with a period. It
    /// should give enough context for the user to be easily able to identify the problem and
    /// fix it.
    ///
    /// Example of a bad reason:
    ///
    ///     missing json field
    ///
    /// Example of a good reason:
    ///
    ///     The driver's component manifest is missing a 'foo' field in its 'program'
    ///     declaration. The 'foo' field should be a list of strings.
    pub reason: String,
}

pub type StaticCheckResult = std::result::Result<StaticCheckPass, StaticCheckFail>;

pub type CheckFunction =
    fn(far_reader: &mut utils::FarReader, component: &ComponentDecl) -> Result<StaticCheckResult>;

const FHCP_SCHEMA_STRING: &'static str = include_str!("../../../../../build/drivers/FHCP.json");

#[derive(Debug, Deserialize)]
struct FhcpSchema {
    device_category_types: HashMap<String, HashMap<String, serde_json::Value>>,
}

const DEVICE_CATEGORIES_HELP: &'static str = "The 'device_categories' field should be a list of { category: \"...\", subcategory: \"...\" } objects populated with values from //build/drivers/FHCP.json.";

/// Checks that a driver has at least one device category and that it is valid.
pub fn check_device_categories(
    _far_reader: &mut utils::FarReader,
    component: &ComponentDecl,
) -> Result<StaticCheckResult> {
    let fhcp_schema: FhcpSchema =
        serde_json::from_str(FHCP_SCHEMA_STRING).context("Could not deserialize FHCP.json")?;
    if let Some(device_categories) = get_device_categories(component) {
        if device_categories.is_empty() {
            return Ok(Err(StaticCheckFail {
                reason: format!("The 'device_categories' field in the 'program' section of the driver's component manifest is empty. {}", DEVICE_CATEGORIES_HELP),
            }));
        }

        for device_category in device_categories {
            let result = is_device_category_valid(&fhcp_schema, &device_category);
            if result.is_err() {
                return Ok(result);
            }
        }
        Ok(Ok(StaticCheckPass {}))
    } else {
        Ok(Err(StaticCheckFail {
            reason: format!("The 'device_categories' field in the 'program' section of the driver's component manifest is missing or an invalid type. {}", DEVICE_CATEGORIES_HELP),
        }))
    }
}

/// Checks that a driver's compiled bind rules are present and valid.
pub fn check_bind_rules(
    far_reader: &mut utils::FarReader,
    component: &ComponentDecl,
) -> Result<StaticCheckResult> {
    if let Some(bind_path) = get_bindbc_path(component) {
        let contents = far_reader.read_file(&bind_path)?;
        if let Err(e) = DecodedRules::new(contents) {
            Ok(Err(StaticCheckFail {
                reason: format!("The bind decoder returned an error while decoding the driver's bind rules at {}: {}", bind_path, e),
            }))
        } else {
            Ok(Ok(StaticCheckPass {}))
        }
    } else {
        Ok(Err(StaticCheckFail {
            reason: "The 'bind' field in the 'program' section of the driver's component manifest is missing or is not a string.".to_string(),
        }))
    }
}

/// Returns the path to a driver's bindbc file from its component manifest.
fn get_bindbc_path(component: &ComponentDecl) -> Option<String> {
    if let Some(program) = &component.program {
        match utils::get_dictionary_value(&program.info, "bind") {
            Some(fdata::DictionaryValue::Str(path)) => Some(path.clone()),
            _ => None,
        }
    } else {
        None
    }
}

#[derive(Clone, Eq, Debug, PartialEq)]
struct CmlDeviceCategory {
    category: String,
    subcategory: String,
}

fn get_device_categories(component: &ComponentDecl) -> Option<Vec<CmlDeviceCategory>> {
    if component.program.is_none() {
        return None;
    }

    let program = component.program.as_ref().unwrap();
    let dict_value = utils::get_dictionary_value(&program.info, "device_categories");
    if let Some(fdata::DictionaryValue::ObjVec(objs)) = dict_value {
        let mut categories = Vec::new();
        for obj in objs {
            if let Some(fdata::DictionaryValue::Str(category)) =
                utils::get_dictionary_value(&obj, "category")
            {
                if let Some(fdata::DictionaryValue::Str(subcategory)) =
                    utils::get_dictionary_value(&obj, "subcategory")
                {
                    categories.push(CmlDeviceCategory {
                        category: category.clone(),
                        subcategory: subcategory.clone(),
                    });
                }
            }
        }
        Some(categories)
    } else {
        None
    }
}

fn is_device_category_valid(
    fhcp_schema: &FhcpSchema,
    device_category: &CmlDeviceCategory,
) -> StaticCheckResult {
    if let Some(json_category) = fhcp_schema.device_category_types.get(&device_category.category) {
        #[allow(clippy::map_entry)] // TODO(https://fxbug.dev/407087100)
        if json_category.contains_key(&device_category.subcategory) {
            Ok(StaticCheckPass {})
        } else {
            Err(StaticCheckFail  {
                reason: format!(
                    "In the 'device_categories' field of the driver's component manifest, '{}' is not a known subcategory of '{}'.",
                    device_category.subcategory, device_category.category
                ),
            })
        }
    } else {
        Err(StaticCheckFail {
            reason: format!("In the 'device_categories' field of the driver's component manifest, '{}' is not a known device category.", device_category.category),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bind::bytecode_constants;
    use cm_rust::ProgramDecl;
    use fuchsia_archive::Utf8Reader;
    use std::io::Cursor;

    const BIND_PATH: &'static str = "meta/bind/driver.bindbc";
    // Copied from src/devices/lib/bind/src/interpreter/decode_bind_rules.rs
    const BIND_HEADER: [u8; 8] = [0x42, 0x49, 0x4E, 0x44, 0x02, 0, 0, 0];

    #[test]
    fn test_check_bind_rules_valid() {
        let mut bytecode = BIND_HEADER.to_vec();
        bytecode.push(bytecode_constants::BYTECODE_DISABLE_DEBUG);
        bytecode.extend_from_slice(&bytecode_constants::SYMB_MAGIC_NUM.to_be_bytes());
        bytecode.extend_from_slice(&0_u32.to_le_bytes());
        bytecode.extend_from_slice(&bytecode_constants::INSTRUCTION_MAGIC_NUM.to_be_bytes());
        bytecode.extend_from_slice(&0_u32.to_le_bytes());

        let mut far_reader = make_test_far(bytecode);
        let component = make_test_component();

        let result = check_bind_rules(&mut far_reader, &component).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_bind_rules_invalid() {
        let mut far_reader = make_test_far(BIND_HEADER.to_vec());
        let component = make_test_component();

        let result = check_bind_rules(&mut far_reader, &component).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_check_device_categories_empty() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_check_device_categories_unknown_category() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![CmlDeviceCategory {
            category: "unknown_category".to_string(),
            subcategory: "unknown_subcategory".to_string(),
        }]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_check_device_categories_unknown_subcategory() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![CmlDeviceCategory {
            category: "input".to_string(),
            subcategory: "unknown_subcategory".to_string(),
        }]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_check_device_categories_valid() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![CmlDeviceCategory {
            category: "input".to_string(),
            subcategory: "touchpad".to_string(),
        }]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_device_categories_valid_multiple_categories() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![
            CmlDeviceCategory {
                category: "input".to_string(),
                subcategory: "touchpad".to_string(),
            },
            CmlDeviceCategory { category: "board".to_string(), subcategory: "i2c".to_string() },
        ]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_device_categories_mix_of_valid_and_invalid_categories() {
        let mut far_reader = make_test_far(vec![]);
        let component = make_test_component_with_device_categories(vec![
            CmlDeviceCategory {
                category: "input".to_string(),
                subcategory: "touchpad".to_string(),
            },
            CmlDeviceCategory {
                category: "board".to_string(),
                subcategory: "fake_subcategory".to_string(),
            },
        ]);

        let result = check_device_categories(&mut far_reader, &component).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn test_get_bindbc_path() {
        let component = make_test_component();
        assert_eq!(get_bindbc_path(&component), Some(BIND_PATH.to_string()));
    }

    #[test]
    fn test_get_device_categories() {
        let device_categories = vec![CmlDeviceCategory {
            category: "audio".to_string(),
            subcategory: "speaker".to_string(),
        }];
        let component = make_test_component_with_device_categories(device_categories.clone());
        assert_eq!(get_device_categories(&component), Some(device_categories));
    }

    fn make_test_component() -> ComponentDecl {
        return make_test_component_with_device_categories(vec![]);
    }

    fn make_test_component_with_device_categories(
        device_categories: Vec<CmlDeviceCategory>,
    ) -> ComponentDecl {
        let objs = device_categories
            .iter()
            .map(|category| fdata::Dictionary {
                entries: Some(vec![
                    fdata::DictionaryEntry {
                        key: "category".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(
                            category.category.clone(),
                        ))),
                    },
                    fdata::DictionaryEntry {
                        key: "subcategory".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(
                            category.subcategory.clone(),
                        ))),
                    },
                ]),
                ..Default::default()
            })
            .collect();

        ComponentDecl {
            program: Some(ProgramDecl {
                runner: None,
                info: fdata::Dictionary {
                    entries: Some(vec![
                        fdata::DictionaryEntry {
                            key: "bind".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::Str(
                                BIND_PATH.to_string(),
                            ))),
                        },
                        fdata::DictionaryEntry {
                            key: "device_categories".to_string(),
                            value: Some(Box::new(fdata::DictionaryValue::ObjVec(objs))),
                        },
                    ]),
                    ..Default::default()
                },
            }),
            uses: vec![],
            exposes: vec![],
            offers: vec![],
            capabilities: vec![],
            children: vec![],
            collections: vec![],
            facets: None,
            environments: vec![],
            config: None,
        }
    }

    fn make_test_far(bind_rules: Vec<u8>) -> Utf8Reader<Cursor<Vec<u8>>> {
        let mut far_bytes = Vec::new();
        fuchsia_archive::write(
            &mut far_bytes,
            std::collections::BTreeMap::from_iter([(
                BIND_PATH,
                (
                    bind_rules.len().try_into().unwrap(),
                    Box::new(Cursor::new(bind_rules)) as Box<dyn std::io::Read>,
                ),
            )]),
        )
        .unwrap();

        fuchsia_archive::Utf8Reader::new(Cursor::new(far_bytes)).unwrap()
    }
}
