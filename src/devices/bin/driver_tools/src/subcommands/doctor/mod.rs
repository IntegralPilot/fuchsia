// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::{Context, Result, anyhow};
use bind::bytecode_constants::{FALSE_VAL, RawOp, RawValueType, TRUE_VAL};
use bind::compiler::Symbol;
use bind::compiler::symbol_table::get_deprecated_key_identifier;
use bind::interpreter::common::{BytecodeIter, next_u8, next_u32};
use bind::interpreter::decode_bind_rules::{DecodedCompositeBindRules, DecodedRules};
use bind::interpreter::match_bind::{DeviceProperties, MatchBindData, PropertyKey, match_bind};
use flex_client::ProxyHasDomain;
use flex_fuchsia_driver_development as fdd;
use flex_fuchsia_driver_framework as fdf;
#[cfg(not(feature = "fdomain"))]
use fuchsia_driver_dev as fdev;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fdev;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::io;

trait DiagnosableParent {
    fn to_properties(&self) -> DeviceProperties;
    fn evaluate_bind_rules(&self, properties: &DeviceProperties) -> Vec<Diagnostic>;
    fn is_fuzzy_match(&self, properties: &DeviceProperties) -> bool;
}

impl DiagnosableParent for fdf::ParentSpec {
    fn to_properties(&self) -> DeviceProperties {
        node_to_bind_properties(Some(&self.properties))
    }
    fn evaluate_bind_rules(&self, properties: &DeviceProperties) -> Vec<Diagnostic> {
        evaluate_bind_rules(&self.bind_rules, properties)
    }
    fn is_fuzzy_match(&self, properties: &DeviceProperties) -> bool {
        is_spec_fuzzy_match(&self.bind_rules, properties)
    }
}

impl DiagnosableParent for fdf::ParentSpec2 {
    fn to_properties(&self) -> DeviceProperties {
        node_to_bind_properties2(Some(&self.properties))
    }
    fn evaluate_bind_rules(&self, properties: &DeviceProperties) -> Vec<Diagnostic> {
        evaluate_bind_rules2(&self.bind_rules, properties)
    }
    fn is_fuzzy_match(&self, properties: &DeviceProperties) -> bool {
        is_spec_fuzzy_match2(&self.bind_rules, properties)
    }
}

pub async fn doctor(
    cmd: args::DoctorCommand,
    driver_dev_proxy: fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    match (cmd.driver.as_ref(), cmd.node.as_ref(), cmd.composite_node_spec.as_ref()) {
        (Some(driver_filter), Some(node_moniker), _) => {
            let driver = resolve_driver(driver_filter, &driver_dev_proxy, writer).await?;
            if let Some(driver) = driver {
                diagnose_driver_and_node(&driver, node_moniker, &driver_dev_proxy, writer).await?;
            }
        }
        (None, Some(node_moniker), Some(spec_name)) => {
            diagnose_spec_and_node(spec_name, node_moniker, &driver_dev_proxy, writer).await?;
        }
        (Some(driver_filter), None, Some(spec_name)) => {
            let driver = resolve_driver(driver_filter, &driver_dev_proxy, writer).await?;
            if let Some(driver) = driver {
                diagnose_driver_and_spec(&driver, spec_name, &driver_dev_proxy, writer).await?;
            }
        }
        (Some(driver_filter), None, None) => {
            let driver = resolve_driver(driver_filter, &driver_dev_proxy, writer).await?;
            if let Some(driver) = driver {
                diagnose_driver(&driver, &driver_dev_proxy, writer).await?;
            }
        }
        (None, Some(node_moniker), None) => {
            diagnose_node(node_moniker, &driver_dev_proxy, writer).await?;
        }
        (None, None, Some(spec_name)) => {
            diagnose_spec(spec_name, &driver_dev_proxy, writer).await?;
        }
        (None, None, None) => {
            diagnose_all(&driver_dev_proxy, writer).await?;
        }
    }

    Ok(())
}

async fn resolve_driver(
    filter: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<Option<fdf::DriverInfo>> {
    let matched_drivers = fdev::get_drivers_from_query(filter, driver_dev_proxy).await?;

    if matched_drivers.is_empty() {
        writeln!(writer, "ERROR: No drivers matched the filter '{}'.", filter)?;
        return Ok(None);
    }

    if matched_drivers.len() > 1 {
        if let Some(exact) = matched_drivers.iter().find(|d| d.url.as_deref() == Some(filter)) {
            return Ok(Some(exact.clone()));
        }

        writeln!(writer, "ERROR: Multiple drivers matched the filter '{}':", filter)?;
        for driver in &matched_drivers {
            writeln!(writer, "  {}", driver.url.as_deref().unwrap_or("unknown"))?;
        }
        writeln!(writer, "Please provide a more specific filter.")?;
        return Ok(None);
    }

    Ok(Some(matched_drivers[0].clone()))
}

async fn diagnose_driver_and_node(
    driver: &fdf::DriverInfo,
    node_moniker: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let driver_url = driver.url.as_deref().unwrap_or("unknown");
    writeln!(writer, "Diagnosing driver {} against node {}", driver_url, node_moniker)?;
    let nodes = fdev::get_device_info(driver_dev_proxy, &[node_moniker.to_string()], true)
        .await
        .context("Failed to find node")?;
    if nodes.is_empty() {
        return Err(anyhow!("Node not found: {}", node_moniker));
    }
    let node = &nodes[0];

    if !is_node_unbound(node) {
        if let Some(bound_url) = &node.bound_driver_url {
            if bound_url == driver_url {
                writeln!(writer, "  Node is already bound to this driver.")?;
            } else {
                writeln!(writer, "  Node is bound to a different driver: {}", bound_url)?;
            }
        }
    }

    if let Some(bytecode) = &driver.bind_rules_bytecode {
        match DecodedRules::new(bytecode.clone())? {
            DecodedRules::Normal(rules) => {
                let properties = node_to_bind_properties(node.node_property_list.as_deref());
                if match_bind(
                    MatchBindData {
                        symbol_table: &rules.symbol_table,
                        instructions: &rules.instructions,
                    },
                    &properties,
                )
                .unwrap_or(false)
                {
                    writeln!(writer, "  Matches!")?;
                } else {
                    let diagnostics = evaluate_rules_bytecode(
                        &rules.symbol_table,
                        &rules.instructions,
                        &properties,
                    );
                    report_diagnostics(&diagnostics, writer, 2)?;
                }
            }
            DecodedRules::Composite(_) => {
                writeln!(
                    writer,
                    "Driver is a composite driver. Use --composite-node-spec to diagnose if you know which spec it should match."
                )?;
            }
        }
    } else {
        writeln!(writer, "Driver has no bind rules bytecode.")?;
    }

    Ok(())
}

async fn diagnose_spec_and_node(
    spec_name: &str,
    node_moniker: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    writeln!(writer, "Diagnosing composite node spec {} against node {}", spec_name, node_moniker)?;
    let all_specs = fdev::get_composite_node_specs(driver_dev_proxy, None)
        .await
        .context("Failed to get composite node specs")?;
    let spec = all_specs
        .iter()
        .find(|s| {
            s.spec
                .as_ref()
                .and_then(|spec| spec.name.as_ref())
                .map(|name| name == spec_name)
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("Spec not found: {}", spec_name))?;

    let nodes = fdev::get_device_info(driver_dev_proxy, &[node_moniker.to_string()], true)
        .await
        .context("Failed to find node")?;
    if nodes.is_empty() {
        return Err(anyhow!("Node not found: {}", node_moniker));
    }
    let node = &nodes[0];
    let node_properties = node_to_bind_properties(node.node_property_list.as_deref());

    if let Some(spec) = &spec.spec {
        if let Some(parents) = &spec.parents {
            for (i, parent) in parents.iter().enumerate() {
                writeln!(writer, "\nComparing against spec parent {}:", i)?;
                let diagnostics = parent.evaluate_bind_rules(&node_properties);
                report_diagnostics(&diagnostics, writer, 2)?;
            }
        } else if let Some(parents2) = &spec.parents2 {
            for (i, parent) in parents2.iter().enumerate() {
                writeln!(writer, "\nComparing against spec parent {}:", i)?;
                let diagnostics = parent.evaluate_bind_rules(&node_properties);
                report_diagnostics(&diagnostics, writer, 2)?;
            }
        }
    }

    Ok(())
}

async fn diagnose_driver_and_spec(
    driver: &fdf::DriverInfo,
    spec_name: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let driver_url = driver.url.as_deref().unwrap_or("unknown");
    writeln!(writer, "Diagnosing driver {} against composite node spec {}", driver_url, spec_name)?;

    let all_specs = fdev::get_composite_node_specs(driver_dev_proxy, None)
        .await
        .context("Failed to get composite node specs")?;

    let spec = all_specs
        .iter()
        .find(|s| {
            s.spec
                .as_ref()
                .and_then(|spec| spec.name.as_ref())
                .map(|name| name == spec_name)
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("Spec not found: {}", spec_name))?;

    if let Some(bytecode) = &driver.bind_rules_bytecode {
        match DecodedRules::new(bytecode.clone())? {
            DecodedRules::Normal(_) => {
                writeln!(
                    writer,
                    "Driver is NOT a composite driver, but it is being compared against a composite node spec."
                )?;
            }
            DecodedRules::Composite(rules) => {
                if let Some(spec) = &spec.spec {
                    if let Some(parents) = &spec.parents {
                        diagnose_composite_match(&rules, parents, writer)?;
                    } else if let Some(parents2) = &spec.parents2 {
                        diagnose_composite_match(&rules, parents2, writer)?;
                    } else {
                        writeln!(writer, "  Spec '{}' has no parents.", spec_name)?;
                    }
                } else {
                    writeln!(writer, "  Spec '{}' is missing detailed information.", spec_name)?;
                }
            }
        }
    } else {
        writeln!(writer, "  Driver '{}' has no bind rules bytecode.", driver_url)?;
    }

    Ok(())
}

fn diagnose_composite_match<P: DiagnosableParent>(
    rules: &DecodedCompositeBindRules,
    parents: &[P],
    writer: &mut dyn io::Write,
) -> Result<()> {
    if parents.is_empty() {
        writeln!(writer, "  Spec has no parents.")?;
        return Ok(());
    }

    let mut matched_parents = HashSet::new();

    writeln!(writer, "\nMatching primary node...")?;

    let mut primary_matched = false;
    for (p_idx, parent) in parents.iter().enumerate() {
        let props = parent.to_properties();
        if match_bind(
            MatchBindData {
                symbol_table: &rules.symbol_table,
                instructions: &rules.primary_parent.instructions,
            },
            &props,
        )
        .unwrap_or(false)
        {
            writeln!(writer, "  Primary node matches parent {}.", p_idx)?;
            primary_matched = true;
            matched_parents.insert(p_idx);
            break;
        }
    }

    if !primary_matched {
        writeln!(
            writer,
            "  ERROR: Primary node matches no parent in spec. Mismatches against parent 0:"
        )?;
        let primary_props = parents[0].to_properties();
        let diagnostics = evaluate_rules_bytecode(
            &rules.symbol_table,
            &rules.primary_parent.instructions,
            &primary_props,
        );
        report_diagnostics(&diagnostics, writer, 4)?;
    }

    for (i, node) in rules.additional_parents.iter().enumerate() {
        let node_name = rules
            .symbol_table
            .get(&node.name_id)
            .cloned()
            .unwrap_or_else(|| format!("additional_{}", i));
        writeln!(writer, "\nMatching additional node {} ({})...", i, node_name)?;

        let mut matched = false;
        for (p_idx, parent) in parents.iter().enumerate() {
            if matched_parents.contains(&p_idx) {
                continue;
            }
            let props = parent.to_properties();
            if match_bind(
                MatchBindData {
                    symbol_table: &rules.symbol_table,
                    instructions: &node.instructions,
                },
                &props,
            )
            .unwrap_or(false)
            {
                writeln!(writer, "  Matches parent {}.", p_idx)?;
                matched = true;
                matched_parents.insert(p_idx);
                break;
            }
        }

        if !matched {
            writeln!(writer, "  ERROR: No parent in spec matches additional node {}.", node_name)?;

            let mut best_parent = None;
            let mut best_score = 0;
            let driver_keys = get_keys_from_instructions(&node.decoded_instructions);

            for (p_idx, parent) in parents.iter().enumerate() {
                if matched_parents.contains(&p_idx) {
                    continue;
                }
                let props = parent.to_properties();
                let score = driver_keys.iter().filter(|k| props.contains_key(k)).count();
                if score >= best_score {
                    best_score = score;
                    best_parent = Some(p_idx);
                }
            }

            if let Some(p_idx) = best_parent {
                writeln!(writer, "  Mismatches against parent {}:", p_idx)?;
                let props = parents[p_idx].to_properties();
                let diags =
                    evaluate_rules_bytecode(&rules.symbol_table, &node.instructions, &props);
                report_diagnostics(&diags, writer, 4)?;
            } else {
                writeln!(
                    writer,
                    "  No available parent in the spec to compare against. This can happen if the driver has more nodes than the spec has parents."
                )?;
            }
        }
    }

    let mut unmatched_parents = Vec::new();
    for p_idx in 0..parents.len() {
        if !matched_parents.contains(&p_idx) {
            unmatched_parents.push(p_idx);
        }
    }

    if !unmatched_parents.is_empty() {
        writeln!(
            writer,
            "\n  ERROR: Driver bind rules missing matches for {} parents in the spec.",
            unmatched_parents.len()
        )?;
        for p_idx in unmatched_parents {
            writeln!(writer, "  Parent {} is unmatched.", p_idx)?;
        }
    }

    Ok(())
}

async fn diagnose_driver(
    driver: &fdf::DriverInfo,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    let driver_url = driver.url.as_deref().unwrap_or("unknown");
    writeln!(writer, "Diagnosing driver {}", driver_url)?;

    let nodes = fdev::get_device_info(driver_dev_proxy, &[], false).await?;
    let bound_nodes = nodes
        .iter()
        .filter_map(|n| {
            if let Some(bound_url) = &n.bound_driver_url {
                if bound_url == driver_url {
                    return Some(n.moniker.as_deref().unwrap_or("unknown"));
                }
            }
            None
        })
        .collect_vec();

    if !bound_nodes.is_empty() {
        writeln!(writer, "\nDriver {} is bound to the following nodes:", driver_url)?;
        for node in bound_nodes {
            writeln!(writer, "  {}", node)?;
        }
    }

    if let Some(bytecode) = &driver.bind_rules_bytecode {
        let rules = DecodedRules::new(bytecode.clone())?;

        match &rules {
            DecodedRules::Normal(r) => {
                writeln!(writer, "\nFuzzy matching against all unbound nodes...")?;
                let unbound_nodes = nodes.into_iter().filter(|n| is_node_unbound(n)).collect_vec();
                for node in unbound_nodes {
                    let props = node_to_bind_properties(node.node_property_list.as_deref());
                    if is_fuzzy_match(&rules, &props) {
                        writeln!(
                            writer,
                            "\n------------------------------------------------------------\nPotential match found: node {}",
                            node.moniker.as_deref().unwrap_or("unknown")
                        )?;
                        if match_bind(
                            MatchBindData {
                                symbol_table: &r.symbol_table,
                                instructions: &r.instructions,
                            },
                            &props,
                        )
                        .unwrap_or(false)
                        {
                            writeln!(writer, "  Matches!")?;
                        } else {
                            let diags =
                                evaluate_rules_bytecode(&r.symbol_table, &r.instructions, &props);
                            report_diagnostics(&diags, writer, 2)?;
                        }
                    }
                }
            }
            DecodedRules::Composite(r) => {
                let specs = fdev::get_composite_node_specs(driver_dev_proxy, None).await?;
                let (matched_specs, unmatched_specs): (Vec<_>, Vec<_>) =
                    specs.into_iter().partition(|s| {
                        if let Some(matched) = &s.matched_driver {
                            if let Some(composite_driver) = &matched.composite_driver {
                                if let Some(info) = &composite_driver.driver_info {
                                    if let Some(url) = &info.url {
                                        return url == driver_url;
                                    }
                                }
                            }
                        }
                        false
                    });

                if !matched_specs.is_empty() {
                    writeln!(writer, "\nDriver is matched to the following composite node specs:")?;
                    for spec_info in matched_specs {
                        let spec_name = spec_info
                            .spec
                            .as_ref()
                            .and_then(|s| s.name.as_deref())
                            .unwrap_or("unknown");
                        writeln!(
                            writer,
                            "  {}\n  (Run `ffx driver doctor --composite-node-spec {}` to diagnose parents)",
                            spec_name, spec_name
                        )?;
                    }
                }

                writeln!(writer, "\nFuzzy matching against all unmatched composite node specs...")?;
                let unmatched_specs = unmatched_specs
                    .into_iter()
                    .filter(|s| s.matched_driver.is_none())
                    .collect_vec();
                for spec_info in unmatched_specs {
                    if let Some(spec) = &spec_info.spec {
                        let mut potential_match = false;
                        if let Some(parents) = &spec.parents {
                            if parents.iter().any(|p| is_fuzzy_match(&rules, &p.to_properties())) {
                                potential_match = true;
                            }
                        } else if let Some(parents2) = &spec.parents2 {
                            if parents2.iter().any(|p| is_fuzzy_match(&rules, &p.to_properties())) {
                                potential_match = true;
                            }
                        }

                        if potential_match {
                            writeln!(
                                writer,
                                "\n------------------------------------------------------------\nPotential match found: spec {}",
                                spec.name.as_deref().unwrap_or("unknown")
                            )?;
                            if let Some(parents) = &spec.parents {
                                diagnose_composite_match(r, parents, writer)?;
                            } else if let Some(parents2) = &spec.parents2 {
                                diagnose_composite_match(r, parents2, writer)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn is_fuzzy_match(rules: &DecodedRules, properties: &DeviceProperties) -> bool {
    let keys = match rules {
        DecodedRules::Normal(r) => get_keys_from_instructions(&r.decoded_instructions),
        DecodedRules::Composite(r) => {
            let mut k = get_keys_from_instructions(&r.primary_parent.decoded_instructions);
            for n in &r.additional_parents {
                k.extend(get_keys_from_instructions(&n.decoded_instructions));
            }
            k
        }
    };
    keys.iter().any(|k| properties.contains_key(k))
}

fn get_keys_from_instructions(
    insts: &[bind::interpreter::instruction_decoder::DecodedInstruction],
) -> Vec<PropertyKey> {
    let mut keys = Vec::new();
    for inst in insts {
        if let bind::interpreter::instruction_decoder::DecodedInstruction::Condition(cond) = inst {
            match &cond.lhs {
                Symbol::NumberValue(v) => keys.push(PropertyKey::NumberKey(*v)),
                Symbol::StringValue(v) => keys.push(PropertyKey::StringKey(v.clone())),
                Symbol::Key(v, _) => keys.push(PropertyKey::StringKey(v.clone())),
                _ => {}
            }
        }
    }
    keys
}

fn is_node_unbound(node: &fdd::NodeInfo) -> bool {
    match &node.bound_driver_url {
        None => true,
        Some(url) => url == "unbound",
    }
}

async fn diagnose_node_info(
    node: &fdd::NodeInfo,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    if !is_node_unbound(node) {
        writeln!(
            writer,
            "  Node is bound to {}",
            node.bound_driver_url.as_deref().unwrap_or("unknown")
        )?;
    } else {
        writeln!(writer, "  Node is UNBOUND.")?;
        if node.quarantined == Some(true) {
            writeln!(writer, "  Node is QUARANTINED (driver failed to start). Check driver logs.")?;
        }

        let properties = node_to_bind_properties(node.node_property_list.as_deref());

        writeln!(writer, "\nFuzzy matching against all drivers...")?;
        let drivers = fdev::get_driver_info(driver_dev_proxy, &[]).await?;
        for driver in drivers {
            if let Some(bytecode) = &driver.bind_rules_bytecode {
                if let Ok(rules) = DecodedRules::new(bytecode.clone()) {
                    if is_fuzzy_match(&rules, &properties) {
                        writeln!(
                            writer,
                            "\n------------------------------------------------------------\nPotential match: driver {}",
                            driver.url.as_deref().unwrap_or("unknown")
                        )?;
                        match rules {
                            DecodedRules::Normal(r) => {
                                if match_bind(
                                    MatchBindData {
                                        symbol_table: &r.symbol_table,
                                        instructions: &r.instructions,
                                    },
                                    &properties,
                                )
                                .unwrap_or(false)
                                {
                                    writeln!(writer, "  Matches!")?;
                                } else {
                                    let diags = evaluate_rules_bytecode(
                                        &r.symbol_table,
                                        &r.instructions,
                                        &properties,
                                    );
                                    report_diagnostics(&diags, writer, 2)?;
                                }
                            }
                            DecodedRules::Composite(_) => {
                                writeln!(
                                    writer,
                                    "  This is a composite driver. For a detailed analysis, run `ffx driver doctor --driver {}` or use --composite-node-spec if you know which spec it should match.",
                                    driver.url.as_deref().unwrap_or("unknown")
                                )?;
                            }
                        }
                    }
                }
            }
        }

        writeln!(writer, "\nFuzzy matching against all composite node specs...")?;
        let specs = fdev::get_composite_node_specs(driver_dev_proxy, None).await?;
        for spec_info in specs {
            if let Some(spec) = &spec_info.spec {
                if let Some(parents) = &spec.parents {
                    for (i, parent) in parents.iter().enumerate() {
                        if parent.is_fuzzy_match(&properties) {
                            let spec_name = spec.name.as_deref().unwrap_or("unknown");
                            writeln!(
                                writer,
                                "\n------------------------------------------------------------\nPotential match: spec {} parent {}",
                                spec_name, i
                            )?;
                            let diags = parent.evaluate_bind_rules(&properties);
                            report_diagnostics(&diags, writer, 2)?;
                        }
                    }
                } else if let Some(parents2) = &spec.parents2 {
                    for (i, parent) in parents2.iter().enumerate() {
                        if parent.is_fuzzy_match(&properties) {
                            let spec_name = spec.name.as_deref().unwrap_or("unknown");
                            writeln!(
                                writer,
                                "\n------------------------------------------------------------\nPotential match: spec {} parent {}",
                                spec_name, i
                            )?;
                            let diags = parent.evaluate_bind_rules(&properties);
                            report_diagnostics(&diags, writer, 2)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn diagnose_node(
    node_moniker: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    writeln!(writer, "Diagnosing node {}", node_moniker)?;
    let nodes = fdev::get_device_info(driver_dev_proxy, &[node_moniker.to_string()], true).await?;
    if nodes.is_empty() {
        writeln!(writer, "  ERROR: Node not found.")?;
        return Ok(());
    }
    diagnose_node_info(&nodes[0], driver_dev_proxy, writer).await
}

fn is_spec_fuzzy_match(rules: &[fdf::BindRule], properties: &DeviceProperties) -> bool {
    rules.iter().any(|rule| {
        let key = match &rule.key {
            fdf::NodePropertyKey::IntValue(v) => PropertyKey::NumberKey(*v as u64),
            fdf::NodePropertyKey::StringValue(v) => PropertyKey::StringKey(v.clone()),
        };
        properties.contains_key(&key)
    })
}

fn is_spec_fuzzy_match2(rules: &[fdf::BindRule2], properties: &DeviceProperties) -> bool {
    rules.iter().any(|rule| {
        let key = PropertyKey::StringKey(rule.key.clone());
        properties.contains_key(&key)
    })
}

async fn get_moniker_from_path(path: &str, proxy: &fdd::ManagerProxy) -> Result<String> {
    let nodes = fdev::get_device_info(proxy, &[path.to_string()], true).await.unwrap_or_default();
    if let Some(node) = nodes.first() {
        if let Some(moniker) = &node.moniker {
            return Ok(moniker.clone());
        }
    }
    Ok(path.to_string())
}

async fn diagnose_spec_info(
    spec_info: &fdf::CompositeInfo,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    if let Some(driver) = spec_info
        .matched_driver
        .as_ref()
        .and_then(|m| m.composite_driver.as_ref())
        .and_then(|cd| cd.driver_info.as_ref())
        .and_then(|di| di.url.as_deref())
    {
        writeln!(writer, "  Spec matched to driver: {}", driver)?;
    } else {
        writeln!(writer, "  Spec did NOT match any driver.")?;
        writeln!(writer, "\nFuzzy matching against all composite drivers...")?;
        if let Some(spec) = &spec_info.spec {
            let drivers = fdev::get_driver_info(driver_dev_proxy, &[]).await?;
            for driver in drivers {
                if let Some(bytecode) = &driver.bind_rules_bytecode {
                    if let Ok(DecodedRules::Composite(rules)) = DecodedRules::new(bytecode.clone())
                    {
                        let mut potential_match = false;
                        if let Some(parents) = &spec.parents {
                            if parents.iter().any(|p| {
                                is_fuzzy_match(
                                    &DecodedRules::Composite(rules.clone()),
                                    &p.to_properties(),
                                )
                            }) {
                                potential_match = true;
                            }
                        } else if let Some(parents2) = &spec.parents2 {
                            if parents2.iter().any(|p| {
                                is_fuzzy_match(
                                    &DecodedRules::Composite(rules.clone()),
                                    &p.to_properties(),
                                )
                            }) {
                                potential_match = true;
                            }
                        }

                        if potential_match {
                            writeln!(
                                writer,
                                "\n------------------------------------------------------------\nPotential match: driver {}",
                                driver.url.as_deref().unwrap_or("unknown")
                            )?;
                            if let Some(parents) = &spec.parents {
                                diagnose_composite_match(&rules, parents, writer)?;
                            } else if let Some(parents2) = &spec.parents2 {
                                diagnose_composite_match(&rules, parents2, writer)?;
                            }
                        }
                    }
                }
            }
        }
    }

    writeln!(writer, "\nFuzzy matching spec parents against all unbound nodes...")?;

    // Query for existing composite nodes to get their bound parents
    let mut parent_paths: Vec<Option<String>> = Vec::new();
    let mut parent_monikers: Vec<Option<String>> = Vec::new();
    if let Some(spec) = &spec_info.spec {
        let (iterator, iterator_server) =
            driver_dev_proxy.domain().create_proxy::<fdd::CompositeInfoIteratorMarker>();
        if driver_dev_proxy.get_composite_info(iterator_server).is_ok() {
            loop {
                if let Ok(composite_list) = iterator.get_next().await {
                    if composite_list.is_empty() {
                        break;
                    }
                    for composite_node in composite_list {
                        if let Some(fdd::CompositeInfo::Composite(node_info)) =
                            composite_node.composite
                        {
                            if node_info.spec.and_then(|s| s.name) == spec.name {
                                parent_paths =
                                    composite_node.parent_topological_paths.unwrap_or_default();
                                parent_monikers =
                                    composite_node.parent_monikers.unwrap_or_default();
                                break;
                            }
                        }
                    }
                    if !parent_paths.is_empty() || !parent_monikers.is_empty() {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
    }

    let nodes = fdev::get_device_info(driver_dev_proxy, &[], false).await?;
    let unbound_nodes = nodes.into_iter().filter(|n| is_node_unbound(n)).collect_vec();

    let parent_names = spec_info.matched_driver.as_ref().and_then(|m| m.parent_names.as_ref());

    if let Some(spec) = &spec_info.spec {
        if let Some(parents) = &spec.parents {
            for (i, parent) in parents.iter().enumerate() {
                let name =
                    parent_names.and_then(|n| n.get(i)).map(|s| s.as_str()).unwrap_or("unknown");
                writeln!(writer, "  Parent {} ({}):", i, name)?;

                let mut bound_node = None;
                if let Some(Some(moniker)) = parent_monikers.get(i) {
                    bound_node = Some(moniker.clone());
                } else if let Some(Some(path)) = parent_paths.get(i) {
                    bound_node = Some(
                        get_moniker_from_path(path, driver_dev_proxy)
                            .await
                            .unwrap_or_else(|_| path.clone()),
                    );
                }

                if let Some(bound_name) = bound_node {
                    writeln!(writer, "    Already bound to node: {}", bound_name)?;
                } else {
                    let mut found_match = false;
                    for node in &unbound_nodes {
                        let properties =
                            node_to_bind_properties(node.node_property_list.as_deref());
                        if parent.is_fuzzy_match(&properties) {
                            found_match = true;
                            writeln!(
                                writer,
                                "    Potential match: node {}",
                                node.moniker.as_deref().unwrap_or("unknown")
                            )?;
                            let diags = parent.evaluate_bind_rules(&properties);
                            report_diagnostics(&diags, writer, 4)?;
                        }
                    }
                    if !found_match {
                        writeln!(writer, "    No unbound nodes matched this parent.")?;
                    }
                }
            }
        } else if let Some(parents2) = &spec.parents2 {
            for (i, parent) in parents2.iter().enumerate() {
                let name =
                    parent_names.and_then(|n| n.get(i)).map(|s| s.as_str()).unwrap_or("unknown");
                writeln!(writer, "  Parent {} ({}):", i, name)?;

                let mut bound_node = None;
                if let Some(Some(moniker)) = parent_monikers.get(i) {
                    bound_node = Some(moniker.clone());
                } else if let Some(Some(path)) = parent_paths.get(i) {
                    bound_node = Some(
                        get_moniker_from_path(path, driver_dev_proxy)
                            .await
                            .unwrap_or_else(|_| path.clone()),
                    );
                }

                if let Some(bound_name) = bound_node {
                    writeln!(writer, "    Already bound to node: {}", bound_name)?;
                } else {
                    let mut found_match = false;
                    for node in &unbound_nodes {
                        let properties =
                            node_to_bind_properties(node.node_property_list.as_deref());
                        if parent.is_fuzzy_match(&properties) {
                            found_match = true;
                            writeln!(
                                writer,
                                "    Potential match: node {}",
                                node.moniker.as_deref().unwrap_or("unknown")
                            )?;
                            let diags = parent.evaluate_bind_rules(&properties);
                            report_diagnostics(&diags, writer, 4)?;
                        }
                    }
                    if !found_match {
                        writeln!(writer, "    No unbound nodes matched this parent.")?;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn diagnose_spec(
    spec_name: &str,
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    writeln!(writer, "Diagnosing composite node spec {}", spec_name)?;
    let specs = fdev::get_composite_node_specs(driver_dev_proxy, None).await?;
    let spec = specs.iter().find(|s| {
        s.spec
            .as_ref()
            .and_then(|spec| spec.name.as_ref())
            .map(|name| name == spec_name)
            .unwrap_or(false)
    });

    match spec {
        None => writeln!(writer, "  ERROR: Composite node spec not found.")?,
        Some(s) => {
            diagnose_spec_info(s, driver_dev_proxy, writer).await?;
        }
    }

    Ok(())
}

async fn diagnose_all(
    driver_dev_proxy: &fdd::ManagerProxy,
    writer: &mut dyn io::Write,
) -> Result<()> {
    writeln!(writer, "Diagnosing all unbound nodes")?;
    let nodes = fdev::get_device_info(driver_dev_proxy, &[], false).await?;
    let unbound_nodes = nodes.into_iter().filter(|n| is_node_unbound(n)).collect_vec();

    if unbound_nodes.is_empty() {
        writeln!(writer, "No unbound nodes found.")?;
    } else {
        for node in unbound_nodes {
            writeln!(writer, "Diagnosing node {}", node.moniker.as_deref().unwrap_or("unknown"))?;
            diagnose_node_info(&node, driver_dev_proxy, writer).await?;
        }
    }

    writeln!(writer, "\n\nDiagnosing all unmatched composite node specs")?;
    let specs = fdev::get_composite_node_specs(driver_dev_proxy, None).await?;
    let unmatched_specs = specs.into_iter().filter(|s| s.matched_driver.is_none()).collect_vec();

    if unmatched_specs.is_empty() {
        writeln!(writer, "No unmatched composite node specs found.")?;
    } else {
        for spec in unmatched_specs {
            let name = spec.spec.as_ref().and_then(|s| s.name.as_deref()).unwrap_or("unknown");
            writeln!(writer, "Diagnosing composite node spec {}", name)?;
            diagnose_spec_info(&spec, driver_dev_proxy, writer).await?;
        }
    }

    Ok(())
}

fn node_to_bind_properties(node_props: Option<&[fdf::NodeProperty]>) -> DeviceProperties {
    let mut props = HashMap::new();
    if let Some(node_props) = node_props {
        for prop in node_props {
            let key = match &prop.key {
                fdf::NodePropertyKey::IntValue(v) => PropertyKey::NumberKey(*v as u64),
                fdf::NodePropertyKey::StringValue(v) => PropertyKey::StringKey(v.clone()),
            };
            let value = match &prop.value {
                fdf::NodePropertyValue::IntValue(v) => Symbol::NumberValue(*v as u64),
                fdf::NodePropertyValue::StringValue(v) => Symbol::StringValue(v.clone()),
                fdf::NodePropertyValue::BoolValue(v) => Symbol::BoolValue(*v),
                fdf::NodePropertyValue::EnumValue(v) => Symbol::EnumValue(v.clone()),
                _ => continue,
            };
            props.insert(key, value);
        }
    }
    props
}

fn node_to_bind_properties2(node_props: Option<&[fdf::NodeProperty2]>) -> DeviceProperties {
    let mut props = HashMap::new();
    if let Some(node_props) = node_props {
        for prop in node_props {
            let key = PropertyKey::StringKey(prop.key.clone());
            let value = match &prop.value {
                fdf::NodePropertyValue::IntValue(v) => Symbol::NumberValue(*v as u64),
                fdf::NodePropertyValue::StringValue(v) => Symbol::StringValue(v.clone()),
                fdf::NodePropertyValue::BoolValue(v) => Symbol::BoolValue(*v),
                fdf::NodePropertyValue::EnumValue(v) => Symbol::EnumValue(v.clone()),
                _ => continue,
            };
            props.insert(key, value);
        }
    }
    props
}

#[derive(Debug)]
enum Diagnostic {
    Mismatch { key: PropertyKey, expected: Symbol, actual: Option<Symbol>, is_equal: bool },
    AbortReached,
}

fn evaluate_rules_bytecode(
    symbol_table: &HashMap<u32, String>,
    instructions: &[u8],
    properties: &DeviceProperties,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut iter = instructions.iter();

    while let Some(byte) = iter.next() {
        let op_byte_val = *byte;
        if op_byte_val == RawOp::EqualCondition as u8
            || op_byte_val == RawOp::InequalCondition as u8
        {
            let is_equal = op_byte_val == RawOp::EqualCondition as u8;
            let (res, diag) =
                read_and_evaluate_values(&mut iter, symbol_table, properties, is_equal);
            if !res {
                if let Some(d) = diag {
                    diagnostics.push(d);
                }
                break;
            }
        } else if op_byte_val == RawOp::Abort as u8 {
            diagnostics.push(Diagnostic::AbortReached);
            break;
        } else if op_byte_val == RawOp::UnconditionalJump as u8 {
            let offset = match next_u32(&mut iter) {
                Ok(o) => o,
                Err(_) => break,
            };
            for _ in 0..offset {
                if iter.next().is_none() {
                    break;
                }
            }
            if iter.next() != Some(&(RawOp::JumpLandPad as u8)) {
                break;
            }
        } else if op_byte_val == RawOp::JumpIfEqual as u8
            || op_byte_val == RawOp::JumpIfNotEqual as u8
        {
            let is_equal = op_byte_val == RawOp::JumpIfEqual as u8;
            let offset = match next_u32(&mut iter) {
                Ok(o) => o,
                Err(_) => break,
            };
            let (res, _) = read_and_evaluate_values(&mut iter, symbol_table, properties, is_equal);
            if res {
                for _ in 0..offset {
                    if iter.next().is_none() {
                        break;
                    }
                }
                if iter.next() != Some(&(RawOp::JumpLandPad as u8)) {
                    break;
                }
            }
        } else if op_byte_val == RawOp::JumpLandPad as u8 {
            // Nothing to do
        } else {
            // Unknown opcode, stop
            break;
        }
    }
    diagnostics
}

fn read_and_evaluate_values(
    iter: &mut BytecodeIter<'_>,
    symbol_table: &HashMap<u32, String>,
    properties: &DeviceProperties,
    is_equal: bool,
) -> (bool, Option<Diagnostic>) {
    let lhs = match read_next_value(iter, symbol_table) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };
    let rhs = match read_next_value(iter, symbol_table) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };

    let key = match lhs {
        Symbol::NumberValue(v) => PropertyKey::NumberKey(v),
        Symbol::StringValue(v) => PropertyKey::StringKey(v),
        Symbol::Key(v, _) => PropertyKey::StringKey(v),
        _ => return (false, None),
    };

    let actual = properties.get(&key).cloned();
    let mut effective_actual = actual;
    if effective_actual.is_none() {
        if let PropertyKey::NumberKey(int_key) = &key {
            if let Some(str_key) = get_deprecated_key_identifier(*int_key as u32) {
                effective_actual = properties.get(&PropertyKey::StringKey(str_key)).cloned();
            }
        }
    }

    let matches = match &effective_actual {
        Some(val) => {
            let mut val_for_compare = val.clone();
            if let Symbol::EnumValue(v) = val {
                val_for_compare = Symbol::StringValue(v.clone());
            }
            let mut rhs_for_compare = rhs.clone();
            if let Symbol::EnumValue(v) = &rhs {
                rhs_for_compare = Symbol::StringValue(v.clone());
            }

            val_for_compare == rhs_for_compare
        }
        None => &bind::compiler::Symbol::NumberValue(0) == &rhs,
    };

    if matches == is_equal {
        (true, None)
    } else {
        (
            false,
            Some(Diagnostic::Mismatch { key, expected: rhs, actual: effective_actual, is_equal }),
        )
    }
}

fn read_next_value(
    iter: &mut BytecodeIter<'_>,
    symbol_table: &HashMap<u32, String>,
) -> Result<Symbol, ()> {
    let value_type_byte = next_u8(iter).map_err(|_| ())?;
    let value_type_val = *value_type_byte;
    let value = next_u32(iter).map_err(|_| ())?;

    if value_type_val == RawValueType::NumberValue as u8 {
        Ok(Symbol::NumberValue(value as u64))
    } else if value_type_val == RawValueType::Key as u8 {
        let name = symbol_table.get(&value).ok_or(())?.clone();
        Ok(Symbol::Key(name, bind::parser::bind_library::ValueType::Str))
    } else if value_type_val == RawValueType::StringValue as u8 {
        let val = symbol_table.get(&value).ok_or(())?.clone();
        Ok(Symbol::StringValue(val))
    } else if value_type_val == RawValueType::BoolValue as u8 {
        match value {
            FALSE_VAL => Ok(Symbol::BoolValue(false)),
            TRUE_VAL => Ok(Symbol::BoolValue(true)),
            _ => Err(()),
        }
    } else if value_type_val == RawValueType::EnumValue as u8 {
        let val = symbol_table.get(&value).ok_or(())?.clone();
        Ok(Symbol::EnumValue(val))
    } else {
        Err(())
    }
}

fn evaluate_bind_rules(rules: &[fdf::BindRule], properties: &DeviceProperties) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for rule in rules {
        let key = match &rule.key {
            fdf::NodePropertyKey::IntValue(v) => PropertyKey::NumberKey(*v as u64),
            fdf::NodePropertyKey::StringValue(v) => PropertyKey::StringKey(v.clone()),
        };
        let actual = properties.get(&key).cloned();

        let mut matched = false;
        for val in &rule.values {
            let symbol_val = match val {
                fdf::NodePropertyValue::IntValue(v) => Symbol::NumberValue(*v as u64),
                fdf::NodePropertyValue::StringValue(v) => Symbol::StringValue(v.clone()),
                fdf::NodePropertyValue::BoolValue(v) => Symbol::BoolValue(*v),
                fdf::NodePropertyValue::EnumValue(v) => Symbol::EnumValue(v.clone()),
                _ => continue,
            };
            if actual.as_ref() == Some(&symbol_val) {
                matched = true;
                break;
            }
        }

        match rule.condition {
            fdf::Condition::Accept => {
                if !matched {
                    diagnostics.push(Diagnostic::Mismatch {
                        key,
                        expected: Symbol::StringValue("one of requested values".to_string()),
                        actual,
                        is_equal: true,
                    });
                }
            }
            fdf::Condition::Reject => {
                if matched {
                    diagnostics.push(Diagnostic::Mismatch {
                        key,
                        expected: Symbol::StringValue("none of requested values".to_string()),
                        actual,
                        is_equal: false,
                    });
                }
            }
            _ => {}
        }
    }
    diagnostics
}

fn evaluate_bind_rules2(
    rules: &[fdf::BindRule2],
    properties: &DeviceProperties,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for rule in rules {
        let key = PropertyKey::StringKey(rule.key.clone());
        let actual = properties.get(&key).cloned();

        let mut matched = false;
        for val in &rule.values {
            let symbol_val = match val {
                fdf::NodePropertyValue::IntValue(v) => Symbol::NumberValue(*v as u64),
                fdf::NodePropertyValue::StringValue(v) => Symbol::StringValue(v.clone()),
                fdf::NodePropertyValue::BoolValue(v) => Symbol::BoolValue(*v),
                fdf::NodePropertyValue::EnumValue(v) => Symbol::EnumValue(v.clone()),
                _ => continue,
            };
            if actual.as_ref() == Some(&symbol_val) {
                matched = true;
                break;
            }
        }

        match rule.condition {
            fdf::Condition::Accept => {
                if !matched {
                    diagnostics.push(Diagnostic::Mismatch {
                        key,
                        expected: Symbol::StringValue("one of requested values".to_string()),
                        actual,
                        is_equal: true,
                    });
                }
            }
            fdf::Condition::Reject => {
                if matched {
                    diagnostics.push(Diagnostic::Mismatch {
                        key,
                        expected: Symbol::StringValue("none of requested values".to_string()),
                        actual,
                        is_equal: false,
                    });
                }
            }
            _ => {}
        }
    }
    diagnostics
}

fn report_diagnostics(
    diagnostics: &[Diagnostic],
    writer: &mut dyn io::Write,
    indent: usize,
) -> Result<()> {
    let indent_str = " ".repeat(indent);
    if diagnostics.is_empty() {
        writeln!(writer, "{}No issues found with direct property matching.", indent_str)?;
    } else {
        for diag in diagnostics {
            match diag {
                Diagnostic::Mismatch { key, expected, actual, is_equal } => {
                    let key_str = match key {
                        PropertyKey::NumberKey(v) => format!("{:#x}", v),
                        PropertyKey::StringKey(v) => v.clone(),
                    };
                    let actual_str = match actual {
                        Some(v) => v.to_string(),
                        None => "missing".to_string(),
                    };
                    if *is_equal {
                        writeln!(
                            writer,
                            "{}Mismatch: key {} expected {} but found {}",
                            indent_str, key_str, expected, actual_str
                        )?;
                    } else {
                        writeln!(
                            writer,
                            "{}Mismatch: key {} expected NOT {} but found {}",
                            indent_str, key_str, expected, actual_str
                        )?;
                    }
                }
                Diagnostic::AbortReached => {
                    writeln!(writer, "{}Unconditional Abort reached in bind rules.", indent_str)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bind::interpreter::instruction_decoder::{DecodedCondition, DecodedInstruction};
    use bind::parser::bind_library::ValueType;

    #[test]
    fn test_evaluate_rules_mismatch() {
        let symbol_table = HashMap::new();
        let mut instructions = vec![RawOp::EqualCondition as u8];
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&100u32.to_le_bytes());

        let mut properties = HashMap::new();
        properties.insert(PropertyKey::NumberKey(1), Symbol::NumberValue(200));

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            Diagnostic::Mismatch { key, expected, actual, is_equal } => {
                assert_eq!(key, &PropertyKey::NumberKey(1));
                assert_eq!(expected, &Symbol::NumberValue(100));
                assert_eq!(actual, &Some(Symbol::NumberValue(200)));
                assert!(is_equal);
            }
            _ => panic!("Expected Mismatch"),
        }
    }

    #[test]
    fn test_evaluate_rules_match() {
        let symbol_table = HashMap::new();
        let mut instructions = vec![RawOp::EqualCondition as u8];
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&100u32.to_le_bytes());

        let mut properties = HashMap::new();
        properties.insert(PropertyKey::NumberKey(1), Symbol::NumberValue(100));

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 0);
    }

    #[test]
    fn test_evaluate_rules_missing_property() {
        let mut symbol_table = HashMap::new();
        symbol_table.insert(1, "key1".to_string());

        // IF key1 == 0 (should match if missing)
        let mut instructions = vec![RawOp::EqualCondition as u8];
        instructions.push(RawValueType::StringValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&0u32.to_le_bytes());

        let properties = HashMap::new();

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 0);

        // IF key1 != 0 (should mismatch if missing)
        let mut instructions = vec![RawOp::InequalCondition as u8];
        instructions.push(RawValueType::StringValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&0u32.to_le_bytes());

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            Diagnostic::Mismatch { key, expected, actual, is_equal } => {
                assert_eq!(key, &PropertyKey::StringKey("key1".to_string()));
                assert_eq!(expected, &Symbol::NumberValue(0));
                assert_eq!(actual, &None);
                assert!(!is_equal);
            }
            _ => panic!("Expected Mismatch"),
        }
    }

    #[test]
    fn test_evaluate_rules_control_flow() {
        let mut symbol_table = HashMap::new();
        symbol_table.insert(1, "protocol".to_string());
        symbol_table.insert(2, "vendor_id".to_string());

        // if protocol == PCI { vendor_id == GOOGLE }
        // PCI = 10, GOOGLE = 0x1234
        // Bytecode:
        // JumpIfNotEqual(protocol, 10, target)
        // EqualCondition(vendor_id, 0x1234)
        // JumpLandPad(target)

        let mut instructions = vec![RawOp::JumpIfNotEqual as u8];
        instructions.extend_from_slice(&11u32.to_le_bytes()); // Offset
        instructions.push(RawValueType::StringValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&10u32.to_le_bytes());

        instructions.push(RawOp::EqualCondition as u8);
        instructions.push(RawValueType::StringValue as u8);
        instructions.extend_from_slice(&2u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&0x1234u32.to_le_bytes());

        instructions.push(RawOp::JumpLandPad as u8);

        // Case 1: protocol is USB (20). Should jump over vendor_id check.
        let mut properties = HashMap::new();
        properties.insert(PropertyKey::StringKey("protocol".to_string()), Symbol::NumberValue(20));
        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 0);

        // Case 2: protocol is PCI (10). Should NOT jump, and report vendor_id mismatch if it's wrong.
        let mut properties = HashMap::new();
        properties.insert(PropertyKey::StringKey("protocol".to_string()), Symbol::NumberValue(10));
        properties
            .insert(PropertyKey::StringKey("vendor_id".to_string()), Symbol::NumberValue(0x5678));
        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            Diagnostic::Mismatch { key, .. } => {
                assert_eq!(key, &PropertyKey::StringKey("vendor_id".to_string()));
            }
            _ => panic!("Expected vendor_id mismatch"),
        }
    }

    #[test]
    fn test_evaluate_rules_abort() {
        let symbol_table = HashMap::new();
        let instructions = vec![RawOp::Abort as u8];
        let properties = HashMap::new();

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            Diagnostic::AbortReached => {}
            _ => panic!("Expected AbortReached"),
        }
    }

    #[test]
    fn test_evaluate_rules_deprecated_key() {
        let symbol_table = HashMap::new();
        // fuchsia.BIND_PROTOCOL is 0x0001
        let mut instructions = vec![RawOp::EqualCondition as u8];
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&1u32.to_le_bytes());
        instructions.push(RawValueType::NumberValue as u8);
        instructions.extend_from_slice(&85u32.to_le_bytes());

        let mut properties = HashMap::new();
        properties.insert(
            PropertyKey::StringKey("fuchsia.BIND_PROTOCOL".to_string()),
            Symbol::NumberValue(85),
        );

        let diagnostics = evaluate_rules_bytecode(&symbol_table, &instructions, &properties);
        assert_eq!(diagnostics.len(), 0);
    }

    #[test]
    fn test_evaluate_bind_rules_mismatch() {
        let rules = vec![fdf::BindRule {
            key: fdf::NodePropertyKey::IntValue(1),
            condition: fdf::Condition::Accept,
            values: vec![fdf::NodePropertyValue::IntValue(100)],
        }];
        let mut properties = HashMap::new();
        properties.insert(PropertyKey::NumberKey(1), Symbol::NumberValue(200));

        let diagnostics = evaluate_bind_rules(&rules, &properties);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn test_evaluate_bind_rules2_mismatch() {
        let rules = vec![fdf::BindRule2 {
            key: "key1".to_string(),
            condition: fdf::Condition::Accept,
            values: vec![fdf::NodePropertyValue::IntValue(100)],
        }];
        let mut properties = HashMap::new();
        properties.insert(PropertyKey::StringKey("key1".to_string()), Symbol::NumberValue(200));

        let diagnostics = evaluate_bind_rules2(&rules, &properties);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0] {
            Diagnostic::Mismatch { key, expected: _, actual, is_equal: _ } => {
                assert_eq!(key, &PropertyKey::StringKey("key1".to_string()));
                assert_eq!(actual, &Some(Symbol::NumberValue(200)));
            }
            _ => panic!("Expected Mismatch"),
        }
    }

    #[test]
    fn test_is_fuzzy_match() {
        let mut symbol_table = HashMap::new();
        symbol_table.insert(1, "key1".to_string());
        let rules = DecodedRules::Normal(bind::interpreter::decode_bind_rules::DecodedBindRules {
            symbol_table,
            instructions: vec![],
            decoded_instructions: vec![DecodedInstruction::Condition(DecodedCondition {
                lhs: Symbol::Key("key1".to_string(), ValueType::Str),
                rhs: Symbol::NumberValue(100),
                is_equal: true,
            })],
            debug_info: None,
        });
        let mut properties = HashMap::new();
        properties.insert(PropertyKey::StringKey("key1".to_string()), Symbol::NumberValue(100));
        assert!(is_fuzzy_match(&rules, &properties));
    }

    #[test]
    fn test_get_keys_from_instructions() {
        let instructions = vec![
            DecodedInstruction::Condition(DecodedCondition {
                lhs: Symbol::NumberValue(1),
                rhs: Symbol::NumberValue(100),
                is_equal: true,
            }),
            DecodedInstruction::Condition(DecodedCondition {
                lhs: Symbol::StringValue("key_str".to_string()),
                rhs: Symbol::NumberValue(200),
                is_equal: true,
            }),
        ];
        let keys = get_keys_from_instructions(&instructions);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&PropertyKey::NumberKey(1)));
        assert!(keys.contains(&PropertyKey::StringKey("key_str".to_string())));
    }

    #[test]
    fn test_diagnose_composite_match_missing_parents() {
        struct MockParent {
            properties: DeviceProperties,
        }

        impl DiagnosableParent for MockParent {
            fn to_properties(&self) -> DeviceProperties {
                self.properties.clone()
            }
            fn evaluate_bind_rules(&self, _properties: &DeviceProperties) -> Vec<Diagnostic> {
                vec![]
            }
            fn is_fuzzy_match(&self, _properties: &DeviceProperties) -> bool {
                true
            }
        }

        let symbol_table = HashMap::new();

        // Primary parent instructions: property 1 == 100
        let mut primary_instructions = vec![RawOp::EqualCondition as u8];
        primary_instructions.push(RawValueType::NumberValue as u8);
        primary_instructions.extend_from_slice(&1u32.to_le_bytes());
        primary_instructions.push(RawValueType::NumberValue as u8);
        primary_instructions.extend_from_slice(&100u32.to_le_bytes());

        // Additional parent instructions: property 2 == 200
        let mut additional_instructions = vec![RawOp::EqualCondition as u8];
        additional_instructions.push(RawValueType::NumberValue as u8);
        additional_instructions.extend_from_slice(&2u32.to_le_bytes());
        additional_instructions.push(RawValueType::NumberValue as u8);
        additional_instructions.extend_from_slice(&200u32.to_le_bytes());

        let rules = DecodedCompositeBindRules {
            symbol_table,
            device_name_id: 0,
            primary_parent: bind::interpreter::decode_bind_rules::Parent {
                name_id: 0,
                instructions: primary_instructions,
                decoded_instructions: vec![],
            },
            additional_parents: vec![bind::interpreter::decode_bind_rules::Parent {
                name_id: 0,
                instructions: additional_instructions,
                decoded_instructions: vec![],
            }],
            optional_parents: vec![],
            debug_info: None,
        };

        let mut props1 = HashMap::new();
        props1.insert(PropertyKey::NumberKey(1), Symbol::NumberValue(100));

        let mut props2 = HashMap::new();
        props2.insert(PropertyKey::NumberKey(2), Symbol::NumberValue(200));

        let mut props3 = HashMap::new();
        props3.insert(PropertyKey::NumberKey(3), Symbol::NumberValue(300));

        let parents = vec![
            MockParent { properties: props1 },
            MockParent { properties: props2 },
            MockParent { properties: props3 }, // This one should be unmatched
        ];

        let mut output = Vec::new();
        let result = diagnose_composite_match(&rules, &parents, &mut output);
        assert!(result.is_ok());

        let output_str = String::from_utf8(output).unwrap();
        assert!(
            output_str
                .contains("ERROR: Driver bind rules missing matches for 1 parents in the spec.")
        );
        assert!(output_str.contains("Parent 2 is unmatched."));
    }
}
