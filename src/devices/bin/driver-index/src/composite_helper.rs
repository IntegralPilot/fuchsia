// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::match_common::{get_composite_rules_from_composite_driver, node_to_device_property};
use crate::resolved_driver::ResolvedDriver;
use crate::rkyv_ext;
use bind::compiler::Symbol;
use bind::compiler::symbol_table::{get_deprecated_key_identifier, get_deprecated_key_value};
use bind::interpreter::match_bind::{DeviceProperties, MatchBindData, PropertyKey, match_bind};
use fidl_fuchsia_driver_framework as fdf;
use std::collections::{BTreeMap, HashMap, HashSet};
use zx::Status;
use zx::sys::zx_status_t;

pub type BindRules = BTreeMap<PropertyKey, BindRuleCondition>;

#[derive(Clone, Debug, Eq, Hash, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Hash, PartialEq, Eq))]
pub struct BindRuleCondition {
    #[rkyv(with = rkyv_ext::ConditionDef)]
    pub condition: fdf::Condition,
    #[rkyv(with = rkyv::with::Map<rkyv_ext::SymbolDef>)]
    pub values: Vec<Symbol>,
}

pub fn find_composite_driver_match<'a>(
    parents: &'a Vec<fdf::ParentSpec2>,
    composite_drivers: &Vec<&ResolvedDriver>,
) -> Option<fdf::CompositeDriverMatch> {
    for composite_driver in composite_drivers {
        let matched_composite = match_composite_properties(composite_driver, parents);
        if let Ok(Some(matched_composite)) = matched_composite {
            return Some(matched_composite);
        }
    }
    None
}

pub fn node_matches_composite_driver(
    node: &fdf::ParentSpec2,
    bind_rules_node: &Vec<u8>,
    symbol_table: &HashMap<u32, String>,
    expected_parent_name: &str,
) -> bool {
    for prop in &node.properties {
        if prop.key == "fuchsia.NAME" {
            if let fdf::NodePropertyValue::StringValue(ref val) = prop.value {
                if val != expected_parent_name {
                    return false;
                }
            }
        }
    }

    match node_to_device_property(&node.properties) {
        Err(_) => false,
        Ok(props) => {
            if let Some(Symbol::StringValue(val)) =
                props.get(&PropertyKey::StringKey("fuchsia.NAME".to_string()))
            {
                if val != expected_parent_name {
                    return false;
                }
            }
            match_bind(MatchBindData { symbol_table, instructions: bind_rules_node }, &props)
                .unwrap_or(false)
        }
    }
}

pub fn convert_fidl_to_bind_rules(
    fidl_bind_rules: &Vec<fdf::BindRule2>,
) -> Result<BindRules, zx_status_t> {
    if fidl_bind_rules.is_empty() {
        return Err(Status::INVALID_ARGS.into_raw());
    }

    let mut bind_rules = BTreeMap::new();
    for fidl_rule in fidl_bind_rules {
        let key = PropertyKey::StringKey(fidl_rule.key.clone());

        // Check if the properties contain duplicate keys.
        if bind_rules.contains_key(&key) {
            return Err(Status::INVALID_ARGS.into_raw());
        }

        let first_val = fidl_rule.values.first().ok_or_else(|| Status::INVALID_ARGS.into_raw())?;
        let values = fidl_rule
            .values
            .iter()
            .map(|val| {
                // Check that the properties are all the same type.
                if std::mem::discriminant(first_val) != std::mem::discriminant(val) {
                    return Err(Status::INVALID_ARGS.into_raw());
                }
                Ok(node_property_to_symbol(val)?)
            })
            .collect::<Result<Vec<Symbol>, zx_status_t>>()?;

        bind_rules
            .insert(key, BindRuleCondition { condition: fidl_rule.condition, values: values });
    }
    Ok(bind_rules)
}

pub fn node_property_to_symbol(value: &fdf::NodePropertyValue) -> Result<Symbol, zx_status_t> {
    match value {
        fdf::NodePropertyValue::IntValue(i) => {
            Ok(bind::compiler::Symbol::NumberValue(i.clone().into()))
        }
        fdf::NodePropertyValue::StringValue(s) => {
            Ok(bind::compiler::Symbol::StringValue(s.clone()))
        }
        fdf::NodePropertyValue::EnumValue(s) => Ok(bind::compiler::Symbol::EnumValue(s.clone())),
        fdf::NodePropertyValue::BoolValue(b) => Ok(bind::compiler::Symbol::BoolValue(b.clone())),
        _ => Err(Status::INVALID_ARGS.into_raw()),
    }
}

pub fn get_driver_url(composite: &fdf::CompositeDriverMatch) -> String {
    return composite
        .composite_driver
        .as_ref()
        .and_then(|driver| driver.driver_info.as_ref())
        .and_then(|driver_info| driver_info.url.clone())
        .unwrap_or_else(|| "".to_string());
}

pub fn match_node(bind_rules: &BindRules, device_properties: &DeviceProperties) -> bool {
    for (key, node_prop_values) in bind_rules.iter() {
        let mut dev_prop_contains_value = match device_properties.get(key) {
            Some(val) => node_prop_values.values.contains(val),
            None => false,
        };

        // If the properties don't contain the key, try to convert it to a deprecated
        // key and check the properties with it.
        if !dev_prop_contains_value && !device_properties.contains_key(key) {
            let deprecated_key = match key {
                PropertyKey::NumberKey(int_key) => get_deprecated_key_identifier(*int_key as u32)
                    .map(|key| PropertyKey::StringKey(key)),
                PropertyKey::StringKey(str_key) => {
                    get_deprecated_key_value(str_key).map(|key| PropertyKey::NumberKey(key as u64))
                }
            };

            if let Some(key) = deprecated_key {
                dev_prop_contains_value = match device_properties.get(&key) {
                    Some(val) => node_prop_values.values.contains(val),
                    None => false,
                };
            }
        }

        let evaluate_condition = match node_prop_values.condition {
            fdf::Condition::Accept => {
                // If the node property accepts a false boolean value and the property is
                // missing from the device properties, then we should evaluate the condition
                // as true.
                dev_prop_contains_value
                    || node_prop_values.values.contains(&Symbol::BoolValue(false))
            }
            fdf::Condition::Reject => !dev_prop_contains_value,
            fdf::Condition::Unknown => {
                log::error!("Invalid condition type in bind rules.");
                return false;
            }
        };

        if !evaluate_condition {
            return false;
        }
    }

    true
}

pub fn match_composite_properties<'a>(
    composite_driver: &'a ResolvedDriver,
    parents: &'a Vec<fdf::ParentSpec2>,
) -> Result<Option<fdf::CompositeDriverMatch>, i32> {
    // The spec must have at least 1 node to match a composite driver.
    if parents.len() < 1 {
        return Ok(None);
    }

    let composite = get_composite_rules_from_composite_driver(composite_driver)?;

    // The composite driver bind rules should have a total node count of more than or equal to the
    // total node count of the spec. This is to account for optional nodes in the
    // composite driver bind rules.
    if composite.optional_parents.len() + composite.additional_parents.len() + 1 < parents.len() {
        return Ok(None);
    }

    // First find a matching primary node.
    let mut primary_parent_index = 0;
    let mut primary_matches = false;
    let primary_name = &composite.symbol_table[&composite.primary_parent.name_id];
    for i in 0..parents.len() {
        primary_matches = node_matches_composite_driver(
            &parents[i],
            &composite.primary_parent.instructions,
            &composite.symbol_table,
            primary_name,
        );
        if primary_matches {
            primary_parent_index = i as u32;
            break;
        }
    }

    if !primary_matches {
        return Ok(None);
    }

    // The remaining nodes in the properties can match the
    // additional nodes in the bind rules in any order.
    //
    // This logic has one issue that we are accepting as a tradeoff for simplicity:
    // If a properties node can match to multiple bind rule
    // additional nodes, it is going to take the first one, even if there is a less strict
    // node that it can take. This can lead to false negative matches.
    //
    // Example:
    // properties[1] can match both additional_nodes[0] and additional_nodes[1]
    // properties[2] can only match additional_nodes[0]
    //
    // This algorithm will return false because it matches up properties[1] with
    // additional_nodes[0], and so properties[2] can't match the remaining nodes
    // [additional_nodes[1]].
    //
    // If we were smarter here we could match up properties[1] with additional_nodes[1]
    // and properties[2] with additional_nodes[0] to return a positive match.
    // TODO(https://fxbug.dev/42058532): Disallow ambiguity with spec matching. We should log
    // a warning and return false if a spec node matches with multiple composite
    // driver nodes, and vice versa.
    let mut unmatched_additional_indices =
        (0..composite.additional_parents.len()).collect::<HashSet<_>>();
    let mut unmatched_optional_indices =
        (0..composite.optional_parents.len()).collect::<HashSet<_>>();

    let mut parent_names = vec![];

    for i in 0..parents.len() {
        if i == primary_parent_index as usize {
            parent_names.push(composite.symbol_table[&composite.primary_parent.name_id].clone());
            continue;
        }

        let mut matched = None;
        let mut matched_name: Option<String> = None;
        let mut from_optional = false;

        // First check if any of the additional nodes match it.
        for &j in &unmatched_additional_indices {
            let additional_name = &composite.symbol_table[&composite.additional_parents[j].name_id];
            let matches = node_matches_composite_driver(
                &parents[i],
                &composite.additional_parents[j].instructions,
                &composite.symbol_table,
                additional_name,
            );
            if matches {
                matched = Some(j);
                matched_name = Some(additional_name.clone());
                break;
            }
        }

        // If no additional nodes matched it, then look in the optional nodes.
        if matched.is_none() {
            for &j in &unmatched_optional_indices {
                let optional_name = &composite.symbol_table[&composite.optional_parents[j].name_id];
                let matches = node_matches_composite_driver(
                    &parents[i],
                    &composite.optional_parents[j].instructions,
                    &composite.symbol_table,
                    optional_name,
                );
                if matches {
                    from_optional = true;
                    matched = Some(j);
                    matched_name = Some(optional_name.clone());
                    break;
                }
            }
        }

        if matched.is_none() {
            return Ok(None);
        }

        if from_optional {
            unmatched_optional_indices.remove(&matched.unwrap());
        } else {
            unmatched_additional_indices.remove(&matched.unwrap());
        }

        parent_names.push(matched_name.unwrap());
    }

    // If we didn't consume all of the additional nodes in the bind rules then this is not a match.
    if !unmatched_additional_indices.is_empty() {
        return Ok(None);
    }

    let driver = fdf::CompositeDriverInfo {
        composite_name: Some(composite.symbol_table[&composite.device_name_id].clone()),
        driver_info: Some(composite_driver.create_driver_info(false)),
        ..Default::default()
    };
    return Ok(Some(fdf::CompositeDriverMatch {
        composite_driver: Some(driver),
        parent_names: Some(parent_names),
        primary_parent_index: Some(primary_parent_index),
        ..Default::default()
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_matches_composite_driver_matching_name() {
        let parent = fdf::ParentSpec2 {
            bind_rules: vec![],
            properties: vec![fdf::NodeProperty2 {
                key: "fuchsia.NAME".to_string(),
                value: fdf::NodePropertyValue::StringValue("my_parent".to_string()),
            }],
        };
        assert!(node_matches_composite_driver(&parent, &vec![], &HashMap::new(), "my_parent"));
    }

    #[test]
    fn test_node_matches_composite_driver_mismatched_name() {
        let parent = fdf::ParentSpec2 {
            bind_rules: vec![],
            properties: vec![fdf::NodeProperty2 {
                key: "fuchsia.NAME".to_string(),
                value: fdf::NodePropertyValue::StringValue("wrong_parent".to_string()),
            }],
        };
        assert!(!node_matches_composite_driver(&parent, &vec![], &HashMap::new(), "my_parent"));
    }
}
