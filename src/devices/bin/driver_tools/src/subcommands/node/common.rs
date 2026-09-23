// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common;
use ansi_term::Colour;
use anyhow::{Result, anyhow, bail, format_err};
use flex_fuchsia_driver_development as fdd;
use itertools::Itertools;
use std::collections::{BTreeMap, VecDeque};
use std::str::FromStr;

/// Filters that can be applied to nodes
#[derive(Debug, PartialEq, Clone)]
pub enum NodeFilter {
    /// Filters nodes that are bound to a driver or a composite parent,
    /// or their parent is the owner.
    Bound,
    /// Filters nodes that are not bound to anything and have no owner.
    Unbound,
    /// Filters nodes that are an ancestor of the node with the given name.
    /// Includes the named node.
    Ancestor(String, bool),
    /// Filters nodes that are a descendant of the node with the given name.
    /// Includes the named node.
    Descendant(String),
    /// Filters node that are a relative (either an ancestor or a descendant) of the
    /// node with the given name. Includes the named node.
    Relative(String, bool),
    /// Filters node that are a sibling of the node with the given name. Includes the named node.
    Sibling(String, bool),
}

impl FromStr for NodeFilter {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "bound" => Ok(NodeFilter::Bound),
            "unbound" => Ok(NodeFilter::Unbound),
            filter => match filter.split_once(":") {
                Some((function, arg)) => match function {
                    "ancestor" | "ancestors" => Ok(NodeFilter::Ancestor(arg.to_string(), false)),
                    "primary_ancestor" | "primary_ancestors" => {
                        Ok(NodeFilter::Ancestor(arg.to_string(), true))
                    }
                    "descendant" | "descendants" => Ok(NodeFilter::Descendant(arg.to_string())),
                    "relative" | "relatives" => Ok(NodeFilter::Relative(arg.to_string(), false)),
                    "primary_relative" | "primary_relatives" => {
                        Ok(NodeFilter::Relative(arg.to_string(), true))
                    }
                    "sibling" | "siblings" => Ok(NodeFilter::Sibling(arg.to_string(), false)),
                    "primary_sibling" | "primary_siblings" => {
                        Ok(NodeFilter::Sibling(arg.to_string(), true))
                    }
                    _ => Err("unknown function for node filter."),
                },
                None => Err(
                    "node filter should be 'bound', 'unbound', 'ancestors:<node_name>', 'primary_ancestors:<node_name>', 'descendants:<node_name>', 'relatives:<node_name>', 'primary_relatives:<node_name>', 'siblings:<node_name>', or 'primary_siblings:<node_name>'.",
                ),
            },
        }
    }
}

pub fn get_state_and_owner(
    quarantined: Option<bool>,
    url: &Option<String>,
    with_style: bool,
) -> (String, String) {
    let not_found = "Driver not found".to_string();
    let url = url.as_ref().unwrap_or(&not_found);
    let state = if url == "unbound" {
        common::colorized("Unbound", Colour::Yellow, with_style)
    } else if quarantined == Some(true) {
        common::colorized("Quarantined", Colour::Red, with_style)
    } else {
        common::colorized("Bound", Colour::Green, with_style)
    };

    let owner = if url == "unbound" {
        "none".to_string()
    } else if url == "owned by parent" {
        "parent".to_string()
    } else if url == "owned by composite(s)" {
        "composite(s)".to_string()
    } else {
        url.clone()
    };
    (state, owner)
}

pub async fn get_nodes_from_query(
    query: &str,
    nodes: Vec<fdd::NodeInfo>,
) -> Result<Vec<fdd::NodeInfo>> {
    // Try and find instances that contain the query in any of the identifiers
    // (moniker, URL).
    let mut filtered_instances: Vec<fdd::NodeInfo> = nodes
        .into_iter()
        .filter(|node| {
            let empty = "".to_string();
            let url_match = node.bound_driver_url.as_ref().unwrap_or(&empty).contains(&query);
            let moniker_match = node.moniker.as_ref().unwrap_or(&empty).contains(&query);
            url_match || moniker_match
        })
        .collect();

    // For stability sort the list by moniker.
    filtered_instances.sort_unstable_by(|a, b| a.moniker.as_ref().cmp(&b.moniker.as_ref()));

    let (exact_matches, others): (Vec<_>, Vec<_>) = filtered_instances
        .into_iter()
        .partition(|i| i.moniker.as_ref().map(|s| s.as_str()) == Some(query));

    if !exact_matches.is_empty() { Ok(exact_matches) } else { Ok(others) }
}

pub async fn get_single_node_from_query(
    query: &str,
    nodes: Vec<fdd::NodeInfo>,
) -> Result<fdd::NodeInfo> {
    // Get all instance monikers that match the query and ensure there is only one.
    let mut instances = get_nodes_from_query(&query, nodes).await?;
    if instances.len() > 1 {
        let monikers: Vec<String> = instances
            .into_iter()
            .map(|i| i.moniker.unwrap_or_else(|| "No moniker.".to_string()))
            .collect();
        let monikers = monikers.join("\n");
        bail!(
            "The query {:?} matches more than one node:\n{}\n\nTo avoid ambiguity, use one of the above monikers instead.",
            query,
            monikers
        );
    }
    if instances.is_empty() {
        bail!("No matching node found for query {:?}.", query);
    }
    let instance = instances.remove(0);
    Ok(instance)
}

pub fn filter_nodes(
    nodes: Vec<fdd::NodeInfo>,
    filter: Option<NodeFilter>,
) -> Result<Vec<fdd::NodeInfo>> {
    if let Some(filter) = filter {
        match filter {
            NodeFilter::Bound => Ok(nodes
                .into_iter()
                .filter(|n| {
                    let (state, _) = get_state_and_owner(n.quarantined, &n.bound_driver_url, false);
                    // Note: This returns both quarantined and bound nodes.
                    state != "Unbound"
                })
                .collect()),
            NodeFilter::Unbound => Ok(nodes
                .into_iter()
                .filter(|n| {
                    let (state, _) = get_state_and_owner(n.quarantined, &n.bound_driver_url, false);
                    state == "Unbound"
                })
                .collect()),
            NodeFilter::Ancestor(filter, primary_only) => {
                filter_ancestor(&nodes, filter, primary_only)
            }
            NodeFilter::Descendant(filter) => filter_descendant(&nodes, filter),
            NodeFilter::Relative(filter, primary_only) => {
                filter_relative(&nodes, filter, primary_only)
            }
            NodeFilter::Sibling(filter, primary_only) => {
                let node_map = create_node_map(&nodes)?;
                let target = match nodes
                    .iter()
                    .find(|n| n.moniker.as_ref().unwrap().eq_ignore_ascii_case(&filter))
                {
                    Some(node) => node,
                    None => return Err(anyhow!("Node with moniker {} not found", filter)),
                };

                let mut results: BTreeMap<u64, fdd::NodeInfo> = BTreeMap::new();

                // This collects all children of direct parents of the target node.
                // If |primary_only|, only children from the primary parent are collected.
                if let Some(parent_ids) = &target.parent_ids {
                    for parent_id in parent_ids {
                        if let Some(parent) = node_map.get(parent_id) {
                            if !primary_only
                                || target
                                    .moniker
                                    .as_ref()
                                    .unwrap()
                                    .starts_with(parent.moniker.as_ref().unwrap())
                            {
                                if let Some(child_ids) = &parent.child_ids {
                                    for child_id in child_ids {
                                        if let Some(child) = node_map.get(child_id) {
                                            results.insert(*child_id, child.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Ok(results.into_values().sorted_by_key(|a| a.moniker.clone()).collect())
            }
        }
    } else {
        Ok(nodes)
    }
}

pub fn create_node_map(nodes: &Vec<fdd::NodeInfo>) -> Result<BTreeMap<u64, fdd::NodeInfo>> {
    nodes
        .iter()
        .map(|node| {
            if let Some(id) = node.id {
                Ok((id, node.clone()))
            } else {
                Err(format_err!("Missing node id"))
            }
        })
        .collect::<Result<BTreeMap<_, _>>>()
}

fn filter_ancestor(
    nodes: &Vec<fdd::NodeInfo>,
    filter: String,
    primary_only: bool,
) -> Result<Vec<fdd::NodeInfo>> {
    let node_map = create_node_map(nodes)?;
    let root =
        match nodes.iter().find(|n| n.moniker.as_ref().unwrap().eq_ignore_ascii_case(&filter)) {
            Some(node) => node,
            None => return Err(anyhow!("Node with moniker {} not found", filter)),
        };

    let mut results: BTreeMap<u64, fdd::NodeInfo> = BTreeMap::new();
    let mut visit_q: VecDeque<&fdd::NodeInfo> = VecDeque::new();
    results.insert(root.id.ok_or_else(|| format_err!("Node missing id"))?, root.clone());
    visit_q.push_back(root);

    // Run a BFS through the parents of the node topology, to collect all nodes that are accessible
    // through parents. If |primary_only|, only traverse the primary parents of composite nodes.
    while let Some(node) = visit_q.pop_front() {
        if let Some(parent_ids) = &node.parent_ids {
            for parent_id in parent_ids {
                if let Some(parent) = node_map.get(parent_id) {
                    if !primary_only
                        || node
                            .moniker
                            .as_ref()
                            .unwrap()
                            .starts_with(parent.moniker.as_ref().unwrap())
                    {
                        if !results.contains_key(parent_id) {
                            results.insert(*parent_id, parent.clone());
                            visit_q.push_back(parent);
                        }
                    }
                }
            }
        }
    }

    Ok(results.into_values().sorted_by_key(|a| a.moniker.clone()).collect())
}

fn filter_descendant(nodes: &Vec<fdd::NodeInfo>, filter: String) -> Result<Vec<fdd::NodeInfo>> {
    let node_map = create_node_map(&nodes)?;
    let root =
        match nodes.iter().find(|n| n.moniker.as_ref().unwrap().eq_ignore_ascii_case(&filter)) {
            Some(node) => node,
            None => return Err(anyhow!("Node with moniker {} not found", filter)),
        };

    let mut results: BTreeMap<u64, fdd::NodeInfo> = BTreeMap::new();
    let mut visit_q: VecDeque<&fdd::NodeInfo> = VecDeque::new();
    results.insert(root.id.ok_or_else(|| format_err!("Node missing id"))?, root.clone());
    visit_q.push_back(root);

    // Run a BFS through the children of the node topology, to collect all nodes that are accessible
    // through children.
    while let Some(node) = visit_q.pop_front() {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                if let Some(child) = node_map.get(child_id) {
                    if !results.contains_key(child_id) {
                        results.insert(*child_id, child.clone());
                        visit_q.push_back(child);
                    }
                }
            }
        }
    }

    Ok(results.into_values().sorted_by_key(|a| a.moniker.clone()).collect())
}

fn filter_relative(
    nodes: &Vec<fdd::NodeInfo>,
    filter: String,
    primary_only: bool,
) -> Result<Vec<fdd::NodeInfo>> {
    let node_map = create_node_map(&nodes)?;
    let root =
        match nodes.iter().find(|n| n.moniker.as_ref().unwrap().eq_ignore_ascii_case(&filter)) {
            Some(node) => node,
            None => return Err(anyhow!("Node with moniker {} not found", filter)),
        };

    let mut results: BTreeMap<u64, fdd::NodeInfo> = BTreeMap::new();
    let mut visit_q: VecDeque<&fdd::NodeInfo> = VecDeque::new();
    results.insert(root.id.ok_or_else(|| format_err!("Node missing id"))?, root.clone());
    visit_q.push_back(root);

    // Same as filter_ancestor.
    // Run a BFS through the parents of the node topology, to collect all nodes that are accessible
    // through parents. If |primary_only|, only traverse the primary parents of composite nodes.
    while let Some(node) = visit_q.pop_front() {
        if let Some(parent_ids) = &node.parent_ids {
            for parent_id in parent_ids {
                if let Some(parent) = node_map.get(parent_id) {
                    if !primary_only
                        || node
                            .moniker
                            .as_ref()
                            .unwrap()
                            .starts_with(parent.moniker.as_ref().unwrap())
                    {
                        if !results.contains_key(parent_id) {
                            results.insert(*parent_id, parent.clone());
                            visit_q.push_back(parent);
                        }
                    }
                }
            }
        }
    }

    // Reset our BFS structures.
    visit_q = VecDeque::new();
    visit_q.push_back(root);

    // Same as filter_descendant.
    // Run a BFS through the children of the node topology, to collect all nodes that are accessible
    // through children.
    while let Some(node) = visit_q.pop_front() {
        if let Some(child_ids) = &node.child_ids {
            for child_id in child_ids {
                if let Some(child) = node_map.get(child_id) {
                    if !results.contains_key(child_id) {
                        results.insert(*child_id, child.clone());
                        visit_q.push_back(child);
                    }
                }
            }
        }
    }

    Ok(results.into_values().sorted_by_key(|a| a.moniker.clone()).collect())
}
