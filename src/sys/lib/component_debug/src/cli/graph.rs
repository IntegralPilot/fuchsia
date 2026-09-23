// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::realm::{Instance, get_all_instances};
use anyhow::Result;
use std::collections::HashSet;
use std::fmt::Write;
use std::str::FromStr;
use url::Url;

use flex_fuchsia_sys2 as fsys;

/// The starting part of our Graphviz graph output. This should be printed before any contents.
static GRAPHVIZ_START: &str = r##"digraph {
    graph [ pad = 0.2 ]
    node [ shape = "box" color = "#2a5b4f" penwidth = 2.25 fontname = "prompt medium" fontsize = 10 target = "_parent" margin = 0.22, ordering = out ];
    edge [ color = "#37474f" penwidth = 1 arrowhead = none target = "_parent" fontname = "roboto mono" fontsize = 10 ]
    splines = "ortho"
"##;

/// The ending part of our Graphviz graph output. This should be printed after `GRAPHVIZ_START` and the
/// contents of the graph.
static GRAPHVIZ_END: &str = "}";

/// Filters that can be applied when creating component graphs
#[derive(Debug, PartialEq)]
pub enum GraphFilter {
    /// Filters components that are an ancestor of the component with the given name.
    /// Includes the named component.
    Ancestor(String),
    /// Filters components that are a descendant of the component with the given name.
    /// Includes the named component.
    Descendant(String),
    /// Filters components that are a relative (either an ancestor or a descendant) of the
    /// component with the given name. Includes the named component.
    Relative(String),
}

impl FromStr for GraphFilter {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(":") {
            Some((function, arg)) => match function {
                "ancestor" | "ancestors" => Ok(Self::Ancestor(arg.to_string())),
                "descendant" | "descendants" => Ok(Self::Descendant(arg.to_string())),
                "relative" | "relatives" => Ok(Self::Relative(arg.to_string())),
                _ => Err("unknown function for list filter."),
            },
            None => Err(
                "list filter should be 'ancestors:<component_name>', 'descendants:<component_name>', or 'relatives:<component_name>'.",
            ),
        }
    }
}

/// Determines the visual orientation of the graph's nodes.
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphOrientation {
    /// The graph's nodes should be ordered from top to bottom.
    TopToBottom,
    /// The graph's nodes should be ordered from left to right.
    LeftToRight,
}

impl FromStr for GraphOrientation {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace("_", "").replace("-", "").as_str() {
            "tb" | "toptobottom" => Ok(GraphOrientation::TopToBottom),
            "lr" | "lefttoright" => Ok(GraphOrientation::LeftToRight),
            _ => Err("graph orientation should be 'toptobottom' or 'lefttoright'."),
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone)]
pub struct GraphResult {
    pub instances: Vec<Instance>,
    pub orientation: GraphOrientation,
}

impl std::fmt::Display for GraphResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let output = create_dot_graph(&self.instances, self.orientation);
        write!(f, "{}", output)
    }
}

pub async fn graph_cmd(
    filter: Option<GraphFilter>,
    orientation: GraphOrientation,
    realm_query: fsys::RealmQueryProxy,
) -> Result<GraphResult> {
    let mut instances = get_all_instances(&realm_query).await?;

    instances = match filter {
        Some(GraphFilter::Ancestor(m)) => filter_ancestors(instances, m),
        Some(GraphFilter::Descendant(m)) => filter_descendants(instances, m),
        Some(GraphFilter::Relative(m)) => filter_relatives(instances, m),
        _ => instances,
    };

    Ok(GraphResult { instances, orientation })
}

fn filter_ancestors(instances: Vec<Instance>, child_str: String) -> Vec<Instance> {
    let mut ancestors = HashSet::new();

    // Find monikers with this child as the leaf.
    for instance in &instances {
        if let Some(child) = instance.moniker.leaf() {
            if *child == child_str {
                // Add this moniker to ancestor list.
                let mut cur_moniker = instance.moniker.clone();
                ancestors.insert(cur_moniker.clone());

                // Loop over parents of this moniker and add them to ancestor list.
                while let Some(parent) = cur_moniker.parent() {
                    ancestors.insert(parent.clone());
                    cur_moniker = parent;
                }
            }
        }
    }

    instances.into_iter().filter(|i| ancestors.contains(&i.moniker)).collect()
}

fn filter_descendants(instances: Vec<Instance>, child_str: String) -> Vec<Instance> {
    let mut descendants = HashSet::new();

    // Find monikers with this child as the leaf.
    for instance in &instances {
        if let Some(child) = instance.moniker.leaf() {
            if *child == child_str {
                // Get all descendants of this moniker.
                for possible_child_instance in &instances {
                    if possible_child_instance.moniker.has_prefix(&instance.moniker) {
                        descendants.insert(possible_child_instance.moniker.clone());
                    }
                }
            }
        }
    }

    instances.into_iter().filter(|i| descendants.contains(&i.moniker)).collect()
}

fn filter_relatives(instances: Vec<Instance>, child_str: String) -> Vec<Instance> {
    let mut relatives = HashSet::new();

    // Find monikers with this child as the leaf.
    for instance in &instances {
        if let Some(child) = instance.moniker.leaf() {
            if *child == child_str {
                // Loop over parents of this moniker and add them to relatives list.
                let mut cur_moniker = instance.moniker.clone();
                while let Some(parent) = cur_moniker.parent() {
                    relatives.insert(parent.clone());
                    cur_moniker = parent;
                }

                // Get all descendants of this moniker and add them to relatives list.
                for possible_child_instance in &instances {
                    if possible_child_instance.moniker.has_prefix(&instance.moniker) {
                        relatives.insert(possible_child_instance.moniker.clone());
                    }
                }
            }
        }
    }

    instances.into_iter().filter(|i| relatives.contains(&i.moniker)).collect()
}

fn construct_codesearch_url(component_url: &str) -> String {
    // Extract the last part of the component URL
    let mut name_with_filetype = match component_url.rsplit_once("/") {
        Some(parts) => parts.1.to_string(),
        // No parts of the path contain `/`, this is already the last part of the component URL.
        // Out-of-tree components may be standalone.
        None => component_url.to_string(),
    };
    if name_with_filetype.ends_with(".cm") {
        name_with_filetype.push('l');
    }

    // We mix dashes and underscores between the manifest name and the instance name
    // sometimes, so search using both.
    let name_with_underscores = name_with_filetype.replace("-", "_");
    let name_with_dashes = name_with_filetype.replace("_", "-");

    let query = if name_with_underscores == name_with_dashes {
        format!("f:{}", name_with_underscores)
    } else {
        format!("f:{}|{}", name_with_underscores, name_with_dashes)
    };

    let mut code_search_url = Url::parse("https://cs.opensource.google/search").unwrap();
    code_search_url.query_pairs_mut().append_pair("q", &query).append_pair("ss", "fuchsia/fuchsia");

    code_search_url.into()
}

/// Create a graphviz dot graph from component instance information.
pub fn create_dot_graph(instances: &[Instance], orientation: GraphOrientation) -> String {
    let mut output = GRAPHVIZ_START.to_string();

    // Switch the orientation of the graph.
    match orientation {
        GraphOrientation::TopToBottom => writeln!(output, r#"    rankdir = "TB""#).unwrap(),
        GraphOrientation::LeftToRight => writeln!(output, r#"    rankdir = "LR""#).unwrap(),
    };

    for instance in instances {
        let moniker = instance.moniker.to_string();
        let label = if let Some(leaf) = instance.moniker.leaf() {
            leaf.to_string()
        } else {
            ".".to_string()
        };

        // Running components are filled.
        let running_attrs =
            if instance.resolved_info.as_ref().map_or(false, |r| r.execution_info.is_some()) {
                r##"style = "filled" fontcolor = "#ffffff""##
            } else {
                ""
            };

        // Components can be clicked to search for them on Code Search.
        let url_attrs = if !instance.url.is_empty() {
            let code_search_url = construct_codesearch_url(&instance.url);
            format!(r#"href = "{}""#, code_search_url.as_str())
        } else {
            String::new()
        };

        // Draw the component.
        writeln!(
            output,
            r#"    "{}" [ label = "{}" {} {} ]"#,
            moniker, label, running_attrs, url_attrs
        )
        .unwrap();

        // Component has a parent and the parent is also in the list of components
        if let Some(parent_moniker) = instance.moniker.parent() {
            if let Some(parent) = instances.iter().find(|i| i.moniker == parent_moniker) {
                // Connect parent to component
                writeln!(output, r#"    "{}" -> "{}""#, parent.moniker, moniker).unwrap();
            }
        }
    }

    writeln!(output, "{}", GRAPHVIZ_END).unwrap();
    output
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::realm::{ExecutionInfo, ResolvedInfo};
    use moniker::Moniker;

    fn instances_for_test() -> Vec<Instance> {
        vec![
            Instance {
                moniker: Moniker::root(),
                url: "fuchsia-boot:///#meta/root.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-boot:///#meta/root.cm".to_owned(),
                    execution_info: None,
                }),
            },
            Instance {
                moniker: Moniker::parse_str("appmgr").unwrap(),
                url: "fuchsia-pkg://fuchsia.com/appmgr#meta/appmgr.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-pkg://fuchsia.com/appmgr#meta/appmgr.cm".to_owned(),
                    execution_info: Some(ExecutionInfo {
                        start_reason: "Debugging Workflow".to_owned(),
                    }),
                }),
            },
            Instance {
                moniker: Moniker::parse_str("sys").unwrap(),
                url: "fuchsia-pkg://fuchsia.com/sys#meta/sys.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-pkg://fuchsia.com/sys#meta/sys.cm".to_owned(),
                    execution_info: None,
                }),
            },
            Instance {
                moniker: Moniker::parse_str("sys/baz").unwrap(),
                url: "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-pkg://fuchsia.com/baz#meta/baz.cm".to_owned(),
                    execution_info: Some(ExecutionInfo {
                        start_reason: "Debugging Workflow".to_owned(),
                    }),
                }),
            },
            Instance {
                moniker: Moniker::parse_str("sys/fuzz").unwrap(),
                url: "fuchsia-pkg://fuchsia.com/fuzz#meta/fuzz.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-pkg://fuchsia.com/fuzz#meta/fuzz.cm".to_owned(),
                    execution_info: None,
                }),
            },
            Instance {
                moniker: Moniker::parse_str("sys/fuzz/hello").unwrap(),
                url: "fuchsia-pkg://fuchsia.com/hello#meta/hello.cm".to_owned(),
                environment: None,
                instance_id: None,
                resolved_info: Some(ResolvedInfo {
                    resolved_url: "fuchsia-pkg://fuchsia.com/hello#meta/hello.cm".to_owned(),
                    execution_info: None,
                }),
            },
        ]
    }

    // The tests in this file are change-detectors because they will fail on
    // any style changes to the graph. This isn't great, but it makes it easy
    // to view the changes in a Graphviz visualizer.
    async fn test_graph_orientation(orientation: GraphOrientation, expected_rankdir: &str) {
        let instances = instances_for_test();

        let graph = create_dot_graph(&instances, orientation);
        pretty_assertions::assert_eq!(
            graph,
            format!(
                r##"digraph {{
    graph [ pad = 0.2 ]
    node [ shape = "box" color = "#2a5b4f" penwidth = 2.25 fontname = "prompt medium" fontsize = 10 target = "_parent" margin = 0.22, ordering = out ];
    edge [ color = "#37474f" penwidth = 1 arrowhead = none target = "_parent" fontname = "roboto mono" fontsize = 10 ]
    splines = "ortho"
    rankdir = "{}"
    "." [ label = "."  href = "https://cs.opensource.google/search?q=f%3Aroot.cml&ss=fuchsia%2Ffuchsia" ]
    "appmgr" [ label = "appmgr" style = "filled" fontcolor = "#ffffff" href = "https://cs.opensource.google/search?q=f%3Aappmgr.cml&ss=fuchsia%2Ffuchsia" ]
    "." -> "appmgr"
    "sys" [ label = "sys"  href = "https://cs.opensource.google/search?q=f%3Asys.cml&ss=fuchsia%2Ffuchsia" ]
    "." -> "sys"
    "sys/baz" [ label = "baz" style = "filled" fontcolor = "#ffffff" href = "https://cs.opensource.google/search?q=f%3Abaz.cml&ss=fuchsia%2Ffuchsia" ]
    "sys" -> "sys/baz"
    "sys/fuzz" [ label = "fuzz"  href = "https://cs.opensource.google/search?q=f%3Afuzz.cml&ss=fuchsia%2Ffuchsia" ]
    "sys" -> "sys/fuzz"
    "sys/fuzz/hello" [ label = "hello"  href = "https://cs.opensource.google/search?q=f%3Ahello.cml&ss=fuchsia%2Ffuchsia" ]
    "sys/fuzz" -> "sys/fuzz/hello"
}}
"##,
                expected_rankdir
            )
        );
    }

    #[fuchsia::test]
    async fn test_graph_top_to_bottom_orientation() {
        test_graph_orientation(GraphOrientation::TopToBottom, "TB").await;
    }

    #[fuchsia::test]
    async fn test_graph_left_to_right_orientation() {
        test_graph_orientation(GraphOrientation::LeftToRight, "LR").await;
    }
}
