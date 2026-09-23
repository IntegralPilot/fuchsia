// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::realm::{Instance, get_all_instances};
use ansi_term::Colour;
use anyhow::Result;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};
use std::collections::HashSet;
use std::str::FromStr;

use flex_fuchsia_sys2 as fsys;

/// Filters that can be applied when listing components
#[derive(Debug, PartialEq)]
pub enum ListFilter {
    Running,
    Stopped,
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

impl FromStr for ListFilter {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(ListFilter::Running),
            "stopped" => Ok(ListFilter::Stopped),
            filter => match filter.split_once(":") {
                Some((function, arg)) => match function {
                    "ancestor" | "ancestors" => Ok(ListFilter::Ancestor(arg.to_string())),
                    "descendant" | "descendants" => Ok(ListFilter::Descendant(arg.to_string())),
                    "relative" | "relatives" => Ok(ListFilter::Relative(arg.to_string())),
                    _ => Err("unknown function for list filter."),
                },
                None => Err(
                    "list filter should be 'running', 'stopped', 'ancestors:<component_name>', 'descendants:<component_name>', or 'relatives:<component_name>'.",
                ),
            },
        }
    }
}

pub async fn list_cmd_print<W: std::io::Write>(
    filter: Option<ListFilter>,
    verbose: bool,
    realm_query: fsys::RealmQueryProxy,
    mut writer: W,
) -> Result<()> {
    let instances = get_instances_matching_filter(filter, &realm_query).await?;

    if verbose {
        let table = create_table(instances);
        table.print(&mut writer)?;
    } else {
        for instance in instances {
            writeln!(writer, "{}", instance.moniker)?;
        }
    }

    Ok(())
}

pub async fn list_cmd_serialized(
    filter: Option<ListFilter>,
    realm_query: fsys::RealmQueryProxy,
) -> Result<Vec<Instance>> {
    let basic_infos = get_instances_matching_filter(filter, &realm_query).await?;
    Ok(basic_infos)
}

/// Creates a verbose table containing information about all instances.
fn create_table(instances: Vec<Instance>) -> Table {
    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);
    table.set_titles(row!("State", "Moniker", "URL"));

    for instance in instances {
        let state = instance.resolved_info.map_or_else(
            || Colour::Red.paint("Stopped"),
            |r| {
                r.execution_info.map_or_else(
                    || Colour::Yellow.paint("Resolved"),
                    |_| Colour::Green.paint("Running"),
                )
            },
        );

        table.add_row(row!(state, instance.moniker.to_string(), instance.url));
    }
    table
}

pub async fn get_instances_matching_filter(
    filter: Option<ListFilter>,
    realm_query: &fsys::RealmQueryProxy,
) -> Result<Vec<Instance>> {
    let instances = get_all_instances(realm_query).await?;

    let mut instances = match filter {
        Some(ListFilter::Running) => instances
            .into_iter()
            .filter(|i| i.resolved_info.as_ref().map_or(false, |r| r.execution_info.is_some()))
            .collect(),
        Some(ListFilter::Stopped) => instances
            .into_iter()
            .filter(|i| i.resolved_info.as_ref().map_or(true, |r| r.execution_info.is_none()))
            .collect(),
        Some(ListFilter::Ancestor(m)) => filter_ancestors(instances, m),
        Some(ListFilter::Descendant(m)) => filter_descendants(instances, m),
        Some(ListFilter::Relative(m)) => filter_relatives(instances, m),
        _ => instances,
    };

    instances.sort_by_key(|c| c.moniker.to_string());

    Ok(instances)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use moniker::Moniker;
    use std::collections::HashMap;

    fn create_query() -> fsys::RealmQueryProxy {
        // Serve RealmQuery for CML components.
        let query = serve_realm_query(
            vec![
                fsys::Instance {
                    moniker: Some("./".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/root#meta/root.cm".to_string()),
                    instance_id: None,
                    resolved_info: Some(fsys::ResolvedInfo {
                        resolved_url: Some(
                            "fuchsia-pkg://fuchsia.com/root#meta/root.cm".to_string(),
                        ),
                        execution_info: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                fsys::Instance {
                    moniker: Some("./core".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/core#meta/core.cm".to_string()),
                    instance_id: None,
                    resolved_info: Some(fsys::ResolvedInfo {
                        resolved_url: Some(
                            "fuchsia-pkg://fuchsia.com/core#meta/core.cm".to_string(),
                        ),
                        execution_info: Some(fsys::ExecutionInfo {
                            start_reason: Some("Debugging Workflow".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                fsys::Instance {
                    moniker: Some("./core/appmgr".to_string()),
                    url: Some("fuchsia-pkg://fuchsia.com/appmgr#meta/appmgr.cm".to_string()),
                    instance_id: None,
                    resolved_info: Some(fsys::ResolvedInfo {
                        resolved_url: Some(
                            "fuchsia-pkg://fuchsia.com/appmgr#meta/appmgr.cm".to_string(),
                        ),
                        execution_info: Some(fsys::ExecutionInfo {
                            start_reason: Some("Debugging Workflow".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        query
    }

    #[fuchsia::test]
    async fn no_filter() {
        let query = create_query();

        let instances = get_instances_matching_filter(None, &query).await.unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            vec![
                Moniker::root(),
                Moniker::parse_str("/core").unwrap(),
                Moniker::parse_str("/core/appmgr").unwrap(),
            ]
        );
    }

    #[fuchsia::test]
    async fn running_only() {
        let query = create_query();

        let instances =
            get_instances_matching_filter(Some(ListFilter::Running), &query).await.unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            vec![Moniker::parse_str("/core").unwrap(), Moniker::parse_str("/core/appmgr").unwrap(),]
        );
    }

    #[fuchsia::test]
    async fn stopped_only() {
        let query = create_query();

        let instances =
            get_instances_matching_filter(Some(ListFilter::Stopped), &query).await.unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            [Moniker::root()]
        );
    }

    #[fuchsia::test]
    async fn descendants_only() {
        let query = create_query();

        let instances =
            get_instances_matching_filter(Some(ListFilter::Descendant("core".to_string())), &query)
                .await
                .unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            vec![Moniker::parse_str("/core").unwrap(), Moniker::parse_str("/core/appmgr").unwrap(),]
        );
    }

    #[fuchsia::test]
    async fn ancestors_only() {
        let query = create_query();

        let instances =
            get_instances_matching_filter(Some(ListFilter::Ancestor("core".to_string())), &query)
                .await
                .unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            vec![Moniker::root(), Moniker::parse_str("/core").unwrap()]
        );
    }

    #[fuchsia::test]
    async fn relative_only() {
        let query = create_query();

        let instances =
            get_instances_matching_filter(Some(ListFilter::Relative("core".to_string())), &query)
                .await
                .unwrap();
        assert_eq!(
            instances.iter().map(|i| i.moniker.clone()).collect::<Vec<_>>(),
            vec![
                Moniker::root(),
                Moniker::parse_str("/core").unwrap(),
                Moniker::parse_str("/core/appmgr").unwrap(),
            ]
        );
    }
}
