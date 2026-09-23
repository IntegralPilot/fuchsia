// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::composite_helper::*;
use crate::resolved_driver::ResolvedDriver;
use crate::rkyv_ext;
use bind::interpreter::decode_bind_rules::DecodedRules;
use bind::interpreter::match_bind::DeviceProperties;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index as fdi;
use std::collections::HashMap;
use zx::Status;
use zx::sys::zx_status_t;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone)]
pub struct CompositeParentRef {
    // This is the spec name. Corresponds with the key of spec_list.
    pub name: String,

    // This is the parent index.
    pub index: u32,
}

// The CompositeNodeSpecManager struct is responsible of managing a list of specs
// for matching.
#[derive(Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CompositeNodeSpecManager {
    // Maps a list of specs to the bind rules of their nodes. This is to handle multiple
    // specs that share a node with the same bind rules. Used for matching nodes.
    //
    // This requires the custom serializer/deserializer as serde does not handle maps
    // with non-string keys. Both the root HashMap and the inner BTree (BindRules type)
    // get converted to vectors in the custom serialization and from vectors in the deserialization.
    #[rkyv(with = rkyv::with::MapKV<
        rkyv::with::MapKV<rkyv_ext::PropertyKeyDef, rkyv::with::Identity>,
        rkyv::with::Identity,
    >)]
    pub parent_refs: HashMap<BindRules, Vec<CompositeParentRef>>,

    // Maps specs to the name. This list ensures that we don't add multiple specs with
    // the same name.
    //
    // This requires a custom serializer/deserializer as the value type is a fidl type without
    // built-in serde support. We manually create a local duplicate of the type using serde's remote
    // types feature and use a wrapper type to do the serialization/deserialization for us.
    #[rkyv(with = rkyv::with::MapKV<rkyv::with::Identity, rkyv_ext::CompositeInfoDef>)]
    pub spec_list: HashMap<String, fdf::CompositeInfo>,
}

/// Checks if name follows `^[A-Za-z0-9-_]*$` regex pattern;
fn is_name_valid(name: &str) -> bool {
    name.as_bytes()
        .iter()
        .all(|&b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'))
}

impl CompositeNodeSpecManager {
    pub fn new() -> Self {
        CompositeNodeSpecManager { parent_refs: HashMap::new(), spec_list: HashMap::new() }
    }

    pub fn add_composite_node_spec(
        &mut self,
        mut spec: fdf::CompositeNodeSpec,
        composite_drivers: Vec<&ResolvedDriver>,
    ) -> Result<(), i32> {
        // Get and validate the name.
        let name = spec.name.clone().ok_or_else(|| Status::INVALID_ARGS.into_raw())?;
        if !is_name_valid(&name) {
            log::error!("Invalid spec name. Name can only contain [A-Za-z0-9-_] characters");
            return Err(Status::INVALID_ARGS.into_raw());
        }

        let parents = match (spec.parents.take(), spec.parents2.take()) {
            (Some(parents), None) => parents
                .into_iter()
                .map(|parent| {
                    Ok(fdf::ParentSpec2 {
                        bind_rules: parent
                            .bind_rules
                            .into_iter()
                            .map(|rule| {
                                if let fdf::NodePropertyKey::StringValue(key) = rule.key {
                                    Ok(fdf::BindRule2 {
                                        key,
                                        condition: rule.condition,
                                        values: rule.values,
                                    })
                                } else {
                                    Err(Status::NOT_SUPPORTED.into_raw())
                                }
                            })
                            .collect::<Result<_, _>>()?,
                        properties: parent
                            .properties
                            .into_iter()
                            .map(|prop| {
                                if let fdf::NodePropertyKey::StringValue(key) = prop.key {
                                    Ok(fdf::NodeProperty2 { key, value: prop.value })
                                } else {
                                    Err(Status::NOT_SUPPORTED.into_raw())
                                }
                            })
                            .collect::<Result<_, _>>()?,
                    })
                })
                .collect::<Result<_, i32>>()?,
            (None, Some(parents2)) => parents2,
            (Some(_), Some(_)) => {
                log::error!(
                    "Both parents and parents2 were specified. Only one must be specified."
                );
                return Err(Status::INVALID_ARGS.into_raw());
            }
            (None, None) => {
                log::error!("Neither parents and parents2 were specified, but one is required.");
                return Err(Status::INVALID_ARGS.into_raw());
            }
        };

        if self.spec_list.contains_key(&name) {
            return Err(Status::ALREADY_EXISTS.into_raw());
        }

        if parents.is_empty() {
            log::error!("parents contains no entries.");
            return Err(Status::INVALID_ARGS.into_raw());
        }

        // Collect parent refs in a separate vector before adding them to the
        // CompositeNodeSpecManager. This is to ensure that we add the parent refs after
        // they're all verified to be valid.
        // TODO(https://fxbug.dev/42056805): Update tests so that we can verify that properties exists in
        // each parent ref.
        let mut parent_refs: Vec<(BindRules, CompositeParentRef)> = vec![];
        for (idx, parent) in parents.iter().enumerate() {
            let bind_rules = convert_fidl_to_bind_rules(&parent.bind_rules)?;
            parent_refs
                .push((bind_rules, CompositeParentRef { name: name.clone(), index: idx as u32 }));
        }

        // Add each parent ref into the map.
        for (bind_rules, parent_ref) in parent_refs {
            self.parent_refs
                .entry(bind_rules)
                .and_modify(|refs| refs.push(parent_ref.clone()))
                .or_insert_with(|| vec![parent_ref]);
        }

        let matched_composite_result = find_composite_driver_match(&parents, &composite_drivers);

        if let Some(matched_composite) = &matched_composite_result {
            log::info!(
                "Matched '{}' to composite node spec '{}'",
                get_driver_url(matched_composite),
                name
            );
        }

        spec.parents2 = Some(parents);

        self.spec_list.insert(
            name,
            fdf::CompositeInfo {
                spec: Some(spec),
                matched_driver: matched_composite_result,
                ..Default::default()
            },
        );
        Ok(())
    }

    // Match the given device properties to all the nodes. Returns a list of specs for all the
    // nodes that match.
    pub fn match_parent_specs(
        &self,
        properties: &DeviceProperties,
    ) -> Option<fdi::MatchDriverResult> {
        let mut matching_refs: Vec<CompositeParentRef> = vec![];
        for (node_props, parent_ref_list) in self.parent_refs.iter() {
            if match_node(&node_props, properties) {
                matching_refs.extend_from_slice(parent_ref_list.as_slice());
            }
        }

        if matching_refs.is_empty() {
            return None;
        }

        // Put in the matched composite info for this spec that we have stored in
        // |spec_list|.
        let mut composite_parents_result: Vec<fdf::CompositeParent> = vec![];
        for matching_ref in matching_refs {
            let composite_info = self.spec_list.get(&matching_ref.name);
            match composite_info {
                Some(info) => {
                    // TODO(https://fxbug.dev/42058749): Only return specs that have a matched composite using
                    // info.matched_driver.is_some()
                    composite_parents_result.push(fdf::CompositeParent {
                        composite: Some(fdf::CompositeInfo {
                            spec: Some(strip_parents_from_spec(&info.spec)),
                            matched_driver: info.matched_driver.clone(),
                            ..Default::default()
                        }),
                        index: Some(matching_ref.index),
                        ..Default::default()
                    });
                }
                None => {}
            }
        }

        if composite_parents_result.is_empty() {
            return None;
        }

        Some(fdi::MatchDriverResult::CompositeParents(composite_parents_result))
    }

    pub fn new_driver_available(&mut self, resolved_driver: &ResolvedDriver) {
        // Only composite drivers should be matched against composite node specs.
        if matches!(resolved_driver.bind_rules, DecodedRules::Normal(_)) {
            return;
        }

        for (name, composite_info) in self.spec_list.iter_mut() {
            if composite_info.matched_driver.is_some() {
                continue;
            }

            let parents = composite_info.spec.as_ref().unwrap().parents2.as_ref().unwrap();
            let matched_composite_result = match_composite_properties(resolved_driver, parents);
            if let Ok(Some(matched_composite)) = matched_composite_result {
                log::info!(
                    "Matched '{}' to composite node spec '{}'",
                    get_driver_url(&matched_composite),
                    name
                );
                composite_info.matched_driver = Some(matched_composite);
            }
        }
    }

    pub fn rebind(
        &mut self,
        spec_name: String,
        composite_drivers: Vec<&ResolvedDriver>,
    ) -> Result<(), zx_status_t> {
        let composite_info =
            self.spec_list.get(&spec_name).ok_or_else(|| Status::NOT_FOUND.into_raw())?;
        let parents = composite_info
            .spec
            .as_ref()
            .ok_or_else(|| Status::INTERNAL.into_raw())?
            .parents2
            .as_ref()
            .ok_or_else(|| Status::INTERNAL.into_raw())?;
        let new_match = find_composite_driver_match(parents, &composite_drivers);
        self.spec_list.entry(spec_name).and_modify(|spec| {
            spec.matched_driver = new_match;
        });
        Ok(())
    }

    pub fn rebind_composites_with_driver(
        &mut self,
        driver: String,
        composite_drivers: Vec<&ResolvedDriver>,
    ) -> Result<(), zx_status_t> {
        let specs_to_rebind = self
            .spec_list
            .iter()
            .filter_map(|(spec_name, spec_info)| {
                spec_info
                    .matched_driver
                    .as_ref()
                    .and_then(|matched_driver| matched_driver.composite_driver.as_ref())
                    .and_then(|composite_driver| composite_driver.driver_info.as_ref())
                    .and_then(|driver_info| driver_info.url.as_ref())
                    .and_then(|url| if &driver == url { Some(spec_name.to_string()) } else { None })
            })
            .collect::<Vec<_>>();

        for spec_name in specs_to_rebind {
            let composite_info =
                self.spec_list.get(&spec_name).ok_or_else(|| Status::NOT_FOUND.into_raw())?;
            let parents = composite_info
                .spec
                .as_ref()
                .ok_or_else(|| Status::INTERNAL.into_raw())?
                .parents2
                .as_ref()
                .ok_or_else(|| Status::INTERNAL.into_raw())?;
            let new_match = find_composite_driver_match(parents, &composite_drivers);
            self.spec_list.entry(spec_name).and_modify(|spec| {
                spec.matched_driver = new_match;
            });
        }

        Ok(())
    }

    pub fn get_specs(&self, name_filter: Option<String>) -> Vec<fdf::CompositeInfo> {
        if let Some(name) = name_filter {
            match self.spec_list.get(&name) {
                Some(item) => return vec![item.clone()],
                None => return vec![],
            }
        };

        let specs = self
            .spec_list
            .iter()
            .map(|(_name, composite_info)| composite_info.clone())
            .collect::<Vec<_>>();

        return specs;
    }
}

pub fn strip_parents_from_spec(spec: &Option<fdf::CompositeNodeSpec>) -> fdf::CompositeNodeSpec {
    // Strip the parents of the rules and properties since they are not needed by
    // the driver manager.
    let parents_stripped = spec.as_ref().and_then(|spec| spec.parents.as_ref()).map(|parents| {
        parents
            .iter()
            .map(|_parent| fdf::ParentSpec { bind_rules: vec![], properties: vec![] })
            .collect::<Vec<_>>()
    });
    let parents2_stripped = spec.as_ref().and_then(|spec| spec.parents2.as_ref()).map(|parents| {
        parents
            .iter()
            .map(|_parent| fdf::ParentSpec2 { bind_rules: vec![], properties: vec![] })
            .collect::<Vec<_>>()
    });

    fdf::CompositeNodeSpec {
        name: spec.as_ref().and_then(|spec| spec.name.clone()),
        parents: parents_stripped,
        parents2: parents2_stripped,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolved_driver::DriverPackageType;
    use crate::test_common::*;
    use bind::compiler::test_lib::*;
    use bind::compiler::{
        CompiledBindRules, CompositeBindRules, CompositeParent, Symbol, SymbolicInstructionInfo,
    };
    use bind::interpreter::match_bind::PropertyKey;

    use bind::parser::bind_library::ValueType;
    const TEST_DEVICE_NAME: &str = "test_device";
    const TEST_PRIMARY_NAME: &str = "primary_node";
    const TEST_ADDITIONAL_A_NAME: &str = "node_a";
    const TEST_ADDITIONAL_B_NAME: &str = "node_b";
    const TEST_OPTIONAL_NAME: &str = "optional_node";

    fn make_accept_list(key: &str, values: Vec<fdf::NodePropertyValue>) -> fdf::BindRule2 {
        fdf::BindRule2 { key: key.to_string(), condition: fdf::Condition::Accept, values }
    }

    fn make_reject(key: &str, value: fdf::NodePropertyValue) -> fdf::BindRule2 {
        fdf::BindRule2 {
            key: key.to_string(),
            condition: fdf::Condition::Reject,
            values: vec![value],
        }
    }

    fn make_reject_list(key: &str, values: Vec<fdf::NodePropertyValue>) -> fdf::BindRule2 {
        fdf::BindRule2 { key: key.to_string(), condition: fdf::Condition::Reject, values }
    }

    // TODO(https://fxbug.dev/42071377): Update tests so that they use the test data functions more often.
    fn create_test_parent_spec_1() -> fdf::ParentSpec2 {
        let bind_rules = vec![
            make_accept("testkey", fdf::NodePropertyValue::IntValue(200)),
            make_accept("testkey3", fdf::NodePropertyValue::BoolValue(true)),
            make_accept("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
        ];
        let properties = vec![make_property("testkey2", fdf::NodePropertyValue::BoolValue(false))];
        make_parent_spec(bind_rules, properties)
    }

    fn create_test_parent_spec_2() -> fdf::ParentSpec2 {
        let bind_rules = vec![
            make_reject("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
            make_accept(
                "flycatcher",
                fdf::NodePropertyValue::EnumValue("flycatcher.phoebe".to_string()),
            ),
            make_reject("yellowlegs", fdf::NodePropertyValue::BoolValue(true)),
        ];
        let properties = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];
        make_parent_spec(bind_rules, properties)
    }

    fn create_driver<'a>(
        composite_name: String,
        primary_parent: (&str, Vec<SymbolicInstructionInfo<'a>>),
        additionals: Vec<(&str, Vec<SymbolicInstructionInfo<'a>>)>,
        optionals: Vec<(&str, Vec<SymbolicInstructionInfo<'a>>)>,
    ) -> ResolvedDriver {
        let mut additional_parents = vec![];
        let mut optional_parents = vec![];
        for additional in additionals {
            additional_parents.push(CompositeParent {
                name: additional.0.to_string(),
                instructions: additional.1,
            });
        }
        for optional in optionals {
            optional_parents
                .push(CompositeParent { name: optional.0.to_string(), instructions: optional.1 });
        }
        let bind_rules = CompositeBindRules {
            device_name: composite_name,
            symbol_table: HashMap::new(),
            primary_parent: CompositeParent {
                name: primary_parent.0.to_string(),
                instructions: primary_parent.1,
            },
            additional_parents: additional_parents,
            optional_parents: optional_parents,
            enable_debug: false,
        };

        let bytecode = CompiledBindRules::CompositeBind(bind_rules).encode_to_bytecode().unwrap();
        let rules = DecodedRules::new(bytecode).unwrap();

        ResolvedDriver {
            component_url: cm_types::Url::new(
                "fuchsia-pkg://fuchsia.com/package#driver/my-driver.cm",
            )
            .unwrap(),
            bind_rules: rules,
            bind_bytecode: vec![],
            colocate: false,
            device_categories: vec![],
            fallback: false,
            package_type: DriverPackageType::Base,
            package_hash: None,
            is_dfv2: None,
            disabled: false,
        }
    }

    fn create_driver_with_rules<'a>(
        primary_parent: (&str, Vec<SymbolicInstructionInfo<'a>>),
        additionals: Vec<(&str, Vec<SymbolicInstructionInfo<'a>>)>,
        optionals: Vec<(&str, Vec<SymbolicInstructionInfo<'a>>)>,
    ) -> ResolvedDriver {
        create_driver(TEST_DEVICE_NAME.to_string(), primary_parent, additionals, optionals)
    }

    #[fuchsia::test]
    async fn test_property_match_node() {
        let nodes = Some(vec![create_test_parent_spec_1(), create_test_parent_spec_2()]);

        let composite_spec = fdf::CompositeNodeSpec {
            name: Some("test_spec".to_string()),
            parents2: nodes.clone(),
            ..Default::default()
        };

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec.clone(), vec![])
        );

        assert_eq!(1, composite_node_spec_manager.get_specs(None).len());
        assert_eq!(0, composite_node_spec_manager.get_specs(Some("not_there".to_string())).len());
        let specs = composite_node_spec_manager.get_specs(Some("test_spec".to_string()));
        assert_eq!(1, specs.len());
        let composite_node_spec = &specs[0];
        let expected_composite_node_spec =
            fdf::CompositeInfo { spec: Some(composite_spec.clone()), ..Default::default() };

        assert_eq!(&expected_composite_node_spec, composite_node_spec);

        // Match node 1.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(200));
        device_properties_1.insert(
            PropertyKey::StringKey("kingfisher".to_string()),
            Symbol::StringValue("kookaburra".to_string()),
        );
        device_properties_1
            .insert(PropertyKey::StringKey("testkey3".to_string()), Symbol::BoolValue(true));
        device_properties_1.insert(
            PropertyKey::StringKey("killdeer".to_string()),
            Symbol::StringValue("plover".to_string()),
        );

        let expected_parent = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                ..Default::default()
            }),
            index: Some(0),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );

        // Match node 2.
        let mut device_properties_2: DeviceProperties = HashMap::new();
        device_properties_2
            .insert(PropertyKey::StringKey("yellowlegs".to_string()), Symbol::BoolValue(false));
        device_properties_2.insert(
            PropertyKey::StringKey("killdeer".to_string()),
            Symbol::StringValue("lapwing".to_string()),
        );
        device_properties_2.insert(
            PropertyKey::StringKey("flycatcher".to_string()),
            Symbol::EnumValue("flycatcher.phoebe".to_string()),
        );

        let expected_parent_2 = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                ..Default::default()
            }),
            index: Some(1),
            ..Default::default()
        };

        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent_2])),
            composite_node_spec_manager.match_parent_specs(&device_properties_2)
        );
    }

    #[fuchsia::test]
    async fn test_property_match_bool_edgecase() {
        let bind_rules = vec![
            make_accept("testkey", fdf::NodePropertyValue::IntValue(200)),
            make_accept("testkey3", fdf::NodePropertyValue::BoolValue(false)),
        ];

        let properties = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];
        let composite_spec =
            make_composite_spec("test_spec", vec![make_parent_spec(bind_rules, properties)]);

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec.clone(), vec![])
        );

        // Match node.
        let mut device_properties: DeviceProperties = HashMap::new();
        device_properties
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(200));

        let expected_parent = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                ..Default::default()
            }),
            index: Some(0),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties)
        );
    }

    #[fuchsia::test]
    async fn test_multiple_spec_match() {
        let bind_rules_2_rearranged = vec![
            make_accept(
                "flycatcher",
                fdf::NodePropertyValue::EnumValue("flycatcher.phoebe".to_string()),
            ),
            make_reject("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
            make_reject("yellowlegs", fdf::NodePropertyValue::BoolValue(true)),
        ];

        let properties_2 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];
        let bind_rules_3 = vec![make_accept("cormorant", fdf::NodePropertyValue::BoolValue(true))];
        let properties_3 = vec![make_property("anhinga", fdf::NodePropertyValue::BoolValue(false))];
        let composite_spec_1 = make_composite_spec(
            "test_spec",
            vec![create_test_parent_spec_1(), create_test_parent_spec_2()],
        );

        let composite_spec_2 = make_composite_spec(
            "test_spec2",
            vec![
                make_parent_spec(bind_rules_2_rearranged, properties_2),
                make_parent_spec(bind_rules_3, properties_3),
            ],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec_1.clone(), vec![])
        );

        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec_2.clone(), vec![])
        );

        // Match node.
        let mut device_properties: DeviceProperties = HashMap::new();
        device_properties
            .insert(PropertyKey::StringKey("yellowlegs".to_string()), Symbol::BoolValue(false));
        device_properties.insert(
            PropertyKey::StringKey("killdeer".to_string()),
            Symbol::StringValue("lapwing".to_string()),
        );
        device_properties.insert(
            PropertyKey::StringKey("flycatcher".to_string()),
            Symbol::EnumValue("flycatcher.phoebe".to_string()),
        );
        let match_result =
            composite_node_spec_manager.match_parent_specs(&device_properties).unwrap();

        assert!(
            if let fdi::MatchDriverResult::CompositeParents(matched_node_info) = match_result {
                assert_eq!(2, matched_node_info.len());

                assert!(matched_node_info.contains(&fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec_1.clone()))),
                        ..Default::default()
                    }),
                    index: Some(1),
                    ..Default::default()
                }));

                assert!(matched_node_info.contains(&fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec_2.clone()))),
                        ..Default::default()
                    }),
                    index: Some(0),
                    ..Default::default()
                }));

                true
            } else {
                false
            }
        );
    }

    #[fuchsia::test]
    async fn test_multiple_spec_nodes_match() {
        let bind_rules_1 = vec![
            make_accept("testkey", fdf::NodePropertyValue::IntValue(200)),
            make_accept("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
        ];

        let properties_1 =
            vec![make_property("testkey2", fdf::NodePropertyValue::BoolValue(false))];

        let bind_rules_1_rearranged = vec![
            make_accept("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
            make_accept("testkey", fdf::NodePropertyValue::IntValue(200)),
        ];

        let bind_rules_3 = vec![make_accept("cormorant", fdf::NodePropertyValue::BoolValue(true))];

        let properties_3 =
            vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(false))];

        let bind_rules_4 = vec![make_accept_list(
            "testkey",
            vec![fdf::NodePropertyValue::IntValue(10), fdf::NodePropertyValue::IntValue(200)],
        )];

        let properties_4 = vec![make_property("testkey2", fdf::NodePropertyValue::BoolValue(true))];

        let composite_spec_1 = make_composite_spec(
            "test_spec",
            vec![make_parent_spec(bind_rules_1, properties_1.clone()), create_test_parent_spec_2()],
        );

        let composite_spec_2 = make_composite_spec(
            "test_spec2",
            vec![
                make_parent_spec(bind_rules_3, properties_3),
                make_parent_spec(bind_rules_1_rearranged, properties_1),
            ],
        );

        let composite_spec_3 =
            make_composite_spec("test_spec3", vec![make_parent_spec(bind_rules_4, properties_4)]);

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec_1.clone(), vec![])
        );

        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec_2.clone(), vec![])
        );

        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec_3.clone(), vec![])
        );

        // Match node.
        let mut device_properties: DeviceProperties = HashMap::new();
        device_properties
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(200));
        device_properties.insert(
            PropertyKey::StringKey("killdeer".to_string()),
            Symbol::StringValue("plover".to_string()),
        );
        let match_result =
            composite_node_spec_manager.match_parent_specs(&device_properties).unwrap();

        assert!(
            if let fdi::MatchDriverResult::CompositeParents(matched_node_info) = match_result {
                assert_eq!(3, matched_node_info.len());

                assert!(matched_node_info.contains(&fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec_1.clone()))),
                        ..Default::default()
                    }),
                    index: Some(0),
                    ..Default::default()
                }));

                assert!(matched_node_info.contains(&fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec_2.clone()))),
                        ..Default::default()
                    }),
                    index: Some(1),
                    ..Default::default()
                }));

                assert!(matched_node_info.contains(&fdf::CompositeParent {
                    composite: Some(fdf::CompositeInfo {
                        spec: Some(strip_parents_from_spec(&Some(composite_spec_3.clone()))),
                        ..Default::default()
                    }),
                    index: Some(0),
                    ..Default::default()
                }));

                true
            } else {
                false
            }
        );
    }

    #[fuchsia::test]
    async fn test_property_mismatch() {
        let bind_rules_2 = vec![
            make_accept("killdeer", fdf::NodePropertyValue::StringValue("plover".to_string())),
            make_reject("yellowlegs", fdf::NodePropertyValue::BoolValue(false)),
        ];

        let properties_2 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec(
                    "test_spec",
                    vec![create_test_parent_spec_1(), make_parent_spec(bind_rules_2, properties_2),]
                ),
                vec![]
            )
        );

        let mut device_properties: DeviceProperties = HashMap::new();
        device_properties
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(200));
        device_properties.insert(
            PropertyKey::StringKey("kingfisher".to_string()),
            Symbol::StringValue("bee-eater".to_string()),
        );
        device_properties
            .insert(PropertyKey::StringKey("yellowlegs".to_string()), Symbol::BoolValue(false));
        device_properties.insert(
            PropertyKey::StringKey("killdeer".to_string()),
            Symbol::StringValue("plover".to_string()),
        );

        assert_eq!(None, composite_node_spec_manager.match_parent_specs(&device_properties));
    }

    #[fuchsia::test]
    async fn test_property_match_list() {
        let bind_rules_1 = vec![
            make_reject_list(
                "testkey10",
                vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::IntValue(150)],
            ),
            make_accept_list(
                "plover",
                vec![
                    fdf::NodePropertyValue::StringValue("killdeer".to_string()),
                    fdf::NodePropertyValue::StringValue("lapwing".to_string()),
                ],
            ),
        ];

        let properties_1 = vec![make_property("testkey", fdf::NodePropertyValue::IntValue(100))];

        let bind_rules_2 = vec![
            make_reject_list(
                "testkey11",
                vec![fdf::NodePropertyValue::IntValue(20), fdf::NodePropertyValue::IntValue(10)],
            ),
            make_accept("dunlin", fdf::NodePropertyValue::BoolValue(true)),
        ];

        let properties_2 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];

        let composite_spec = make_composite_spec(
            "test_spec",
            vec![
                make_parent_spec(bind_rules_1, properties_1),
                make_parent_spec(bind_rules_2, properties_2),
            ],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(composite_spec.clone(), vec![])
        );

        // Match node 1.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey10".to_string()), Symbol::NumberValue(20));
        device_properties_1.insert(
            PropertyKey::StringKey("plover".to_string()),
            Symbol::StringValue("lapwing".to_string()),
        );

        let expected_parent_1 = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                ..Default::default()
            }),
            index: Some(0),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent_1])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );

        // Match node 2.
        let mut device_properties_2: DeviceProperties = HashMap::new();
        device_properties_2
            .insert(PropertyKey::StringKey("testkey5".to_string()), Symbol::NumberValue(20));
        device_properties_2
            .insert(PropertyKey::StringKey("dunlin".to_string()), Symbol::BoolValue(true));

        let expected_parent_2 = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                ..Default::default()
            }),
            index: Some(1),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent_2])),
            composite_node_spec_manager.match_parent_specs(&device_properties_2)
        );
    }

    #[fuchsia::test]
    async fn test_property_mismatch_list() {
        let bind_rules_1 = vec![
            make_reject_list(
                "testkey10",
                vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::IntValue(150)],
            ),
            make_accept_list(
                "plover",
                vec![
                    fdf::NodePropertyValue::StringValue("killdeer".to_string()),
                    fdf::NodePropertyValue::StringValue("lapwing".to_string()),
                ],
            ),
        ];

        let properties_1 = vec![make_property("testkey", fdf::NodePropertyValue::IntValue(100))];

        let bind_rules_2 = vec![
            make_reject_list(
                "testkey11",
                vec![fdf::NodePropertyValue::IntValue(20), fdf::NodePropertyValue::IntValue(10)],
            ),
            make_accept("testkey2", fdf::NodePropertyValue::BoolValue(true)),
        ];

        let properties_2 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec(
                    "test_spec",
                    vec![
                        make_parent_spec(bind_rules_1, properties_1),
                        make_parent_spec(bind_rules_2, properties_2),
                    ]
                ),
                vec![]
            )
        );

        // Match node 1.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey10".to_string()), Symbol::NumberValue(200));
        device_properties_1.insert(
            PropertyKey::StringKey("plover".to_string()),
            Symbol::StringValue("lapwing".to_string()),
        );
        assert_eq!(None, composite_node_spec_manager.match_parent_specs(&device_properties_1));

        // Match node 2.
        let mut device_properties_2: DeviceProperties = HashMap::new();
        device_properties_2
            .insert(PropertyKey::StringKey("testkey11".to_string()), Symbol::NumberValue(10));
        device_properties_2
            .insert(PropertyKey::StringKey("testkey2".to_string()), Symbol::BoolValue(true));

        assert_eq!(None, composite_node_spec_manager.match_parent_specs(&device_properties_2));
    }

    #[fuchsia::test]
    async fn test_property_multiple_value_types() {
        let bind_rules = vec![make_reject_list(
            "testkey10",
            vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::BoolValue(false)],
        )];

        let properties = vec![make_property("testkey", fdf::NodePropertyValue::IntValue(100))];

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test_spec", vec![make_parent_spec(bind_rules, properties)]),
                vec![]
            )
        );

        assert!(composite_node_spec_manager.parent_refs.is_empty());
        assert!(composite_node_spec_manager.spec_list.is_empty());
    }

    #[fuchsia::test]
    async fn test_property_duplicate_key() {
        let bind_rules = vec![
            make_reject_list(
                "testkey10",
                vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::IntValue(150)],
            ),
            make_accept("testkey10", fdf::NodePropertyValue::IntValue(10)),
        ];

        let properties = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test_spec", vec![make_parent_spec(bind_rules, properties)]),
                vec![]
            )
        );

        assert!(composite_node_spec_manager.parent_refs.is_empty());
        assert!(composite_node_spec_manager.spec_list.is_empty());
    }

    #[fuchsia::test]
    async fn test_missing_bind_rules() {
        let bind_rules = vec![
            make_reject_list(
                "testkey10",
                vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::IntValue(150)],
            ),
            make_accept("testkey10", fdf::NodePropertyValue::IntValue(10)),
        ];
        let properties_1 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];
        let properties_2 =
            vec![make_property("testkey10", fdf::NodePropertyValue::BoolValue(false))];

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec(
                    "test_spec",
                    vec![
                        make_parent_spec(bind_rules, properties_1),
                        make_parent_spec(vec![], properties_2),
                    ]
                ),
                vec![]
            )
        );
        assert!(composite_node_spec_manager.parent_refs.is_empty());
        assert!(composite_node_spec_manager.spec_list.is_empty());
    }

    #[fuchsia::test]
    async fn test_missing_composite_node_spec_fields() {
        let bind_rules = vec![
            make_reject_list(
                "testkey10",
                vec![fdf::NodePropertyValue::IntValue(200), fdf::NodePropertyValue::IntValue(150)],
            ),
            make_accept("testkey10", fdf::NodePropertyValue::IntValue(10)),
        ];

        let properties_1 = vec![make_property("testkey3", fdf::NodePropertyValue::BoolValue(true))];
        let properties_2 = vec![make_property("testkey", fdf::NodePropertyValue::BoolValue(false))];
        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                fdf::CompositeNodeSpec {
                    name: None,
                    parents2: Some(vec![
                        make_parent_spec(bind_rules, properties_1),
                        make_parent_spec(vec![], properties_2),
                    ]),
                    ..Default::default()
                },
                vec![]
            )
        );
        assert!(composite_node_spec_manager.parent_refs.is_empty());
        assert!(composite_node_spec_manager.spec_list.is_empty());

        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                fdf::CompositeNodeSpec {
                    name: Some("test_spec".to_string()),
                    parents2: None,
                    ..Default::default()
                },
                vec![]
            )
        );

        assert!(composite_node_spec_manager.parent_refs.is_empty());
        assert!(composite_node_spec_manager.spec_list.is_empty());
    }

    #[fuchsia::test]
    async fn test_composite_match() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];
        let additional_bind_rules_1 =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(10))];
        let additional_bind_rules_2 =
            vec![make_accept("test_10", fdf::NodePropertyValue::BoolValue(true))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let additional_a_key_1 = "additional_a_key";
        let additional_a_val_1 = 50;

        let additional_b_key_1 = "curlew";
        let additional_b_val_1 = 500;

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let additional_parent_spec_a = make_parent_spec(
            additional_bind_rules_1,
            vec![make_property(
                additional_a_key_1,
                fdf::NodePropertyValue::IntValue(additional_a_val_1),
            )],
        );

        let additional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(additional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(additional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let additional_parent_spec_b = make_parent_spec(
            additional_bind_rules_2,
            vec![make_property(
                additional_b_key_1,
                fdf::NodePropertyValue::IntValue(additional_b_val_1),
            )],
        );

        let additional_node_b_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(additional_b_key_1.to_string(), ValueType::Number),
            Symbol::NumberValue(additional_b_val_1.clone().into()),
        )];

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![
                (TEST_ADDITIONAL_A_NAME, additional_node_a_inst),
                (TEST_ADDITIONAL_B_NAME, additional_node_b_inst),
            ],
            vec![],
        );

        let composite_spec = make_composite_spec(
            "test_spec",
            vec![primary_parent_spec, additional_parent_spec_b, additional_parent_spec_a],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager
                .add_composite_node_spec(composite_spec.clone(), vec![&composite_driver])
        );

        assert_eq!(1, composite_node_spec_manager.get_specs(None).len());
        assert_eq!(0, composite_node_spec_manager.get_specs(Some("not_there".to_string())).len());
        let specs = composite_node_spec_manager.get_specs(Some("test_spec".to_string()));
        assert_eq!(1, specs.len());
        let composite_node_spec = &specs[0];

        let expected_spec = fdf::CompositeInfo {
            spec: Some(composite_spec.clone()),
            matched_driver: Some(fdf::CompositeDriverMatch {
                composite_driver: Some(fdf::CompositeDriverInfo {
                    composite_name: Some(TEST_DEVICE_NAME.to_string()),
                    driver_info: Some(composite_driver.clone().create_driver_info(false)),
                    ..Default::default()
                }),
                parent_names: Some(vec![
                    TEST_PRIMARY_NAME.to_string(),
                    TEST_ADDITIONAL_B_NAME.to_string(),
                    TEST_ADDITIONAL_A_NAME.to_string(),
                ]),
                primary_parent_index: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };

        let expected_spec_stripped_parents = fdf::CompositeInfo {
            spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
            matched_driver: Some(fdf::CompositeDriverMatch {
                composite_driver: Some(fdf::CompositeDriverInfo {
                    composite_name: Some(TEST_DEVICE_NAME.to_string()),
                    driver_info: Some(composite_driver.clone().create_driver_info(false)),
                    ..Default::default()
                }),
                parent_names: Some(vec![
                    TEST_PRIMARY_NAME.to_string(),
                    TEST_ADDITIONAL_B_NAME.to_string(),
                    TEST_ADDITIONAL_A_NAME.to_string(),
                ]),
                primary_parent_index: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(&expected_spec, composite_node_spec);

        // Match additional node A, the last node in the spec at index 2.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(10));

        let expected_parent = fdf::CompositeParent {
            composite: Some(expected_spec_stripped_parents.clone()),
            index: Some(2),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );
    }

    #[fuchsia::test]
    async fn test_composite_with_rearranged_primary_node() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let additional_bind_rules_1 =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(10))];

        let additional_bind_rules_2 =
            vec![make_accept("testkey10", fdf::NodePropertyValue::BoolValue(true))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let additional_a_key_1 = "additional_a_key";
        let additional_a_val_1 = 50;

        let additional_b_key_1 = "curlew";
        let additional_b_val_1 = 500;

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let additional_parent_spec_a = make_parent_spec(
            additional_bind_rules_1,
            vec![make_property(
                additional_a_key_1,
                fdf::NodePropertyValue::IntValue(additional_a_val_1),
            )],
        );

        let additional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(additional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(additional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let additional_parent_spec_b = make_parent_spec(
            additional_bind_rules_2,
            vec![make_property(
                additional_b_key_1,
                fdf::NodePropertyValue::IntValue(additional_b_val_1),
            )],
        );

        let additional_node_b_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(additional_b_key_1.to_string(), ValueType::Number),
            Symbol::NumberValue(additional_b_val_1.clone().into()),
        )];

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![
                (TEST_ADDITIONAL_A_NAME, additional_node_a_inst),
                (TEST_ADDITIONAL_B_NAME, additional_node_b_inst),
            ],
            vec![],
        );
        let composite_spec = make_composite_spec(
            "test_spec",
            vec![additional_parent_spec_b, additional_parent_spec_a, primary_parent_spec],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager
                .add_composite_node_spec(composite_spec.clone(), vec![&composite_driver])
        );

        assert_eq!(1, composite_node_spec_manager.get_specs(None).len());
        assert_eq!(0, composite_node_spec_manager.get_specs(Some("not_there".to_string())).len());
        let specs = composite_node_spec_manager.get_specs(Some("test_spec".to_string()));
        assert_eq!(1, specs.len());
        let composite_node_spec = &specs[0];

        let expected_spec = fdf::CompositeInfo {
            spec: Some(composite_spec.clone()),
            matched_driver: Some(fdf::CompositeDriverMatch {
                composite_driver: Some(fdf::CompositeDriverInfo {
                    composite_name: Some(TEST_DEVICE_NAME.to_string()),
                    driver_info: Some(composite_driver.clone().create_driver_info(false)),
                    ..Default::default()
                }),
                parent_names: Some(vec![
                    TEST_ADDITIONAL_B_NAME.to_string(),
                    TEST_ADDITIONAL_A_NAME.to_string(),
                    TEST_PRIMARY_NAME.to_string(),
                ]),
                primary_parent_index: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };

        let expected_spec_stripped_parents = fdf::CompositeInfo {
            spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
            matched_driver: Some(fdf::CompositeDriverMatch {
                composite_driver: Some(fdf::CompositeDriverInfo {
                    composite_name: Some(TEST_DEVICE_NAME.to_string()),
                    driver_info: Some(composite_driver.clone().create_driver_info(false)),
                    ..Default::default()
                }),
                parent_names: Some(vec![
                    TEST_ADDITIONAL_B_NAME.to_string(),
                    TEST_ADDITIONAL_A_NAME.to_string(),
                    TEST_PRIMARY_NAME.to_string(),
                ]),
                primary_parent_index: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(&expected_spec, composite_node_spec);

        // Match additional node A, the last node in the spec at index 2.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(10));

        let expected_parent = fdf::CompositeParent {
            composite: Some(expected_spec_stripped_parents.clone()),
            index: Some(1),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );
    }

    #[fuchsia::test]
    async fn test_composite_with_optional_match_without_optional() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let additional_bind_rules_1 =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(10))];

        let additional_bind_rules_2 =
            vec![make_accept("testkey10", fdf::NodePropertyValue::BoolValue(true))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let additional_a_key_1 = "additional_a_key";
        let additional_a_val_1 = 50;

        let additional_b_key_1 = "curlew";
        let additional_b_val_1 = 500;

        let optional_a_key_1 = "optional_a_key";
        let optional_a_val_1: u32 = 10;

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let additional_parent_spec_a = make_parent_spec(
            additional_bind_rules_1,
            vec![make_property(
                additional_a_key_1,
                fdf::NodePropertyValue::IntValue(additional_a_val_1),
            )],
        );

        let additional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(additional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(additional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let additional_parent_spec_b = make_parent_spec(
            additional_bind_rules_2,
            vec![make_property(
                additional_b_key_1,
                fdf::NodePropertyValue::IntValue(additional_b_val_1),
            )],
        );

        let additional_node_b_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(additional_b_key_1.to_string(), ValueType::Number),
            Symbol::NumberValue(additional_b_val_1.clone().into()),
        )];

        let optional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(optional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(optional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![
                (TEST_ADDITIONAL_A_NAME, additional_node_a_inst),
                (TEST_ADDITIONAL_B_NAME, additional_node_b_inst),
            ],
            vec![(TEST_OPTIONAL_NAME, optional_node_a_inst)],
        );

        let composite_spec = make_composite_spec(
            "test_spec",
            vec![primary_parent_spec, additional_parent_spec_b, additional_parent_spec_a],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager
                .add_composite_node_spec(composite_spec.clone(), vec![&composite_driver])
        );

        // Match additional node A, the last node in the spec at index 2.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(10));

        let expected_parent = fdf::CompositeParent {
            composite: Some(fdf::CompositeInfo {
                spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
                matched_driver: Some(fdf::CompositeDriverMatch {
                    composite_driver: Some(fdf::CompositeDriverInfo {
                        composite_name: Some(TEST_DEVICE_NAME.to_string()),
                        driver_info: Some(composite_driver.clone().create_driver_info(false)),
                        ..Default::default()
                    }),
                    parent_names: Some(vec![
                        TEST_PRIMARY_NAME.to_string(),
                        TEST_ADDITIONAL_B_NAME.to_string(),
                        TEST_ADDITIONAL_A_NAME.to_string(),
                    ]),
                    primary_parent_index: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            index: Some(2),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );
    }

    #[fuchsia::test]
    async fn test_composite_with_optional_match_with_optional() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let additional_bind_rules_1 =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(10))];

        let additional_bind_rules_2 =
            vec![make_accept("testkey10", fdf::NodePropertyValue::BoolValue(true))];

        let optional_bind_rules_1 =
            vec![make_accept("testkey1000", fdf::NodePropertyValue::IntValue(1000))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let additional_a_key_1 = "additional_a_key";
        let additional_a_val_1 = 50;

        let additional_b_key_1 = "curlew";
        let additional_b_val_1 = 500;

        let optional_a_key_1 = "optional_a_key";
        let optional_a_val_1 = 10;

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let additional_parent_spec_a = make_parent_spec(
            additional_bind_rules_1,
            vec![make_property(
                additional_a_key_1,
                fdf::NodePropertyValue::IntValue(additional_a_val_1),
            )],
        );

        let additional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(additional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(additional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let additional_parent_spec_b = make_parent_spec(
            additional_bind_rules_2,
            vec![make_property(
                additional_b_key_1,
                fdf::NodePropertyValue::IntValue(additional_b_val_1),
            )],
        );

        let additional_node_b_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(additional_b_key_1.to_string(), ValueType::Number),
            Symbol::NumberValue(additional_b_val_1.clone().into()),
        )];

        let optional_node_parent_a = make_parent_spec(
            optional_bind_rules_1,
            vec![make_property(
                optional_a_key_1,
                fdf::NodePropertyValue::IntValue(optional_a_val_1),
            )],
        );

        let optional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(optional_a_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(optional_a_val_1.clone().into()),
            ),
            make_abort_eq_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![
                (TEST_ADDITIONAL_A_NAME, additional_node_a_inst),
                (TEST_ADDITIONAL_B_NAME, additional_node_b_inst),
            ],
            vec![(TEST_OPTIONAL_NAME, optional_node_a_inst)],
        );

        let composite_spec = make_composite_spec(
            "test_spec",
            vec![
                primary_parent_spec,
                additional_parent_spec_b,
                optional_node_parent_a,
                additional_parent_spec_a,
            ],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager
                .add_composite_node_spec(composite_spec.clone(), vec![&composite_driver])
        );

        // Match additional node A, the last node in the spec at index 3.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey".to_string()), Symbol::NumberValue(10));

        let expected_composite = fdf::CompositeInfo {
            spec: Some(strip_parents_from_spec(&Some(composite_spec.clone()))),
            matched_driver: Some(fdf::CompositeDriverMatch {
                composite_driver: Some(fdf::CompositeDriverInfo {
                    composite_name: Some(TEST_DEVICE_NAME.to_string()),
                    driver_info: Some(composite_driver.clone().create_driver_info(false)),
                    ..Default::default()
                }),
                parent_names: Some(vec![
                    TEST_PRIMARY_NAME.to_string(),
                    TEST_ADDITIONAL_B_NAME.to_string(),
                    TEST_OPTIONAL_NAME.to_string(),
                    TEST_ADDITIONAL_A_NAME.to_string(),
                ]),
                primary_parent_index: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };

        let expected_parent = fdf::CompositeParent {
            composite: Some(expected_composite.clone()),
            index: Some(3),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );

        // Match optional node A, the second to last node in the spec at index 2.
        let mut device_properties_1: DeviceProperties = HashMap::new();
        device_properties_1
            .insert(PropertyKey::StringKey("testkey1000".to_string()), Symbol::NumberValue(1000));

        let expected_parent_2 = fdf::CompositeParent {
            composite: Some(expected_composite.clone()),
            index: Some(2),
            ..Default::default()
        };
        assert_eq!(
            Some(fdi::MatchDriverResult::CompositeParents(vec![expected_parent_2])),
            composite_node_spec_manager.match_parent_specs(&device_properties_1)
        );
    }

    #[fuchsia::test]
    async fn test_composite_mismatch() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let additional_bind_rules_1 =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(10))];

        let additional_bind_rules_2 =
            vec![make_accept("testkey10", fdf::NodePropertyValue::BoolValue(false))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let additional_a_key_1 = "additional_a_key";
        let additional_a_val_1 = 50;

        let additional_b_key_1 = "curlew";
        let additional_b_val_1 = 500;

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let additional_node_a_inst = vec![
            make_abort_ne_symbinst(
                Symbol::Key(additional_b_key_1.to_string(), ValueType::Number),
                Symbol::NumberValue(additional_b_val_1.clone().into()),
            ),
            // This does not exist in our properties so we expect it to not match.
            make_abort_ne_symbinst(
                Symbol::Key("NA".to_string(), ValueType::Number),
                Symbol::NumberValue(500),
            ),
        ];

        let additional_parent_spec_a = make_parent_spec(
            additional_bind_rules_1,
            vec![make_property(
                additional_b_key_1,
                fdf::NodePropertyValue::IntValue(additional_b_val_1),
            )],
        );

        let additional_node_b_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(additional_a_key_1.to_string(), ValueType::Number),
            Symbol::NumberValue(additional_a_val_1.clone().into()),
        )];

        let additional_parent_spec_b = make_parent_spec(
            additional_bind_rules_2,
            vec![make_property(
                additional_a_key_1,
                fdf::NodePropertyValue::IntValue(additional_a_val_1),
            )],
        );

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![
                (TEST_ADDITIONAL_A_NAME, additional_node_a_inst),
                (TEST_ADDITIONAL_B_NAME, additional_node_b_inst),
            ],
            vec![],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec(
                    "test_spec",
                    vec![primary_parent_spec, additional_parent_spec_a, additional_parent_spec_b]
                ),
                vec![&composite_driver]
            )
        );
    }

    #[fuchsia::test]
    async fn test_valid_name() {
        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();

        let node = make_parent_spec(
            vec![make_accept("wrybill", fdf::NodePropertyValue::IntValue(200))],
            vec![make_property(
                "dotteral",
                fdf::NodePropertyValue::StringValue("wrybill".to_string()),
            )],
        );
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test-spec", vec![node.clone()]),
                vec![]
            )
        );

        assert_eq!(
            Ok(()),
            composite_node_spec_manager
                .add_composite_node_spec(make_composite_spec("test_spec", vec![node]), vec![])
        );
    }

    #[fuchsia::test]
    async fn test_invalid_name() {
        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        let node = make_parent_spec(
            vec![make_accept("wrybill", fdf::NodePropertyValue::IntValue(200))],
            vec![make_property("dotteral", fdf::NodePropertyValue::IntValue(100))],
        );
        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test/spec", vec![node.clone()]),
                vec![]
            )
        );

        assert_eq!(
            Err(Status::INVALID_ARGS.into_raw()),
            composite_node_spec_manager
                .add_composite_node_spec(make_composite_spec("test/spec", vec![node]), vec![])
        );
    }

    #[fuchsia::test]
    async fn test_rebind() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let composite_driver = create_driver_with_rules(
            (TEST_PRIMARY_NAME, primary_node_inst.clone()),
            vec![],
            vec![],
        );

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test_spec", vec![primary_parent_spec]),
                vec![&composite_driver]
            )
        );

        let rebind_driver = create_driver(
            "rebind_composite".to_string(),
            (TEST_PRIMARY_NAME, primary_node_inst),
            vec![],
            vec![],
        );
        assert!(
            composite_node_spec_manager
                .rebind("test_spec".to_string(), vec![&rebind_driver])
                .is_ok()
        );
        assert_eq!(
            fdf::CompositeDriverInfo {
                composite_name: Some("rebind_composite".to_string()),
                driver_info: Some(rebind_driver.clone().create_driver_info(false)),
                ..Default::default()
            },
            composite_node_spec_manager
                .spec_list
                .get("test_spec")
                .unwrap()
                .matched_driver
                .as_ref()
                .unwrap()
                .composite_driver
                .as_ref()
                .unwrap()
                .clone()
        );
    }

    #[fuchsia::test]
    async fn test_rebind_no_match() {
        let primary_bind_rules =
            vec![make_accept("testkey", fdf::NodePropertyValue::IntValue(200))];

        let primary_key_1 = "whimbrel";
        let primary_val_1 = "sanderling";

        let primary_parent_spec = make_parent_spec(
            primary_bind_rules,
            vec![make_property(
                primary_key_1,
                fdf::NodePropertyValue::StringValue(primary_val_1.to_string()),
            )],
        );

        let primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key(primary_key_1.to_string(), ValueType::Str),
            Symbol::StringValue(primary_val_1.to_string()),
        )];

        let composite_driver =
            create_driver_with_rules((TEST_PRIMARY_NAME, primary_node_inst), vec![], vec![]);

        let mut composite_node_spec_manager = CompositeNodeSpecManager::new();
        assert_eq!(
            Ok(()),
            composite_node_spec_manager.add_composite_node_spec(
                make_composite_spec("test_spec", vec![primary_parent_spec]),
                vec![&composite_driver]
            )
        );

        // Create a composite driver for rebinding that won't match to the spec.
        let rebind_primary_node_inst = vec![make_abort_ne_symbinst(
            Symbol::Key("unmatched".to_string(), ValueType::Bool),
            Symbol::BoolValue(false),
        )];

        let rebind_driver = create_driver(
            "rebind_composite".to_string(),
            (TEST_PRIMARY_NAME, rebind_primary_node_inst),
            vec![],
            vec![],
        );
        assert!(
            composite_node_spec_manager
                .rebind("test_spec".to_string(), vec![&rebind_driver])
                .is_ok()
        );
        assert_eq!(
            None,
            composite_node_spec_manager.spec_list.get("test_spec").unwrap().matched_driver
        );
    }
}
