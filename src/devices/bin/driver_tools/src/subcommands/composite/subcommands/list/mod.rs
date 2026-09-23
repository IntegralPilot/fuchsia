// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::common;
use ansi_term::Colour;
use anyhow::Result;
use args::{CompositeFilter, ListCompositeCommand};
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use prettytable::format::consts::FORMAT_CLEAN;
use prettytable::{Table, row};
use std::collections::HashMap;
use std::io::Write;

enum CompositeState {
    Bound(String),
    Unbound,
    Incomplete(Option<String>),
}

impl CompositeState {
    fn to_colorized_string(&self, with_style: bool) -> String {
        match self {
            CompositeState::Bound(_) => common::colorized("Bound", Colour::Green, with_style),
            CompositeState::Unbound => common::colorized("Unbound", Colour::Yellow, with_style),
            CompositeState::Incomplete(_) => {
                common::colorized("Incomplete", Colour::Fixed(208), with_style)
            } // Orange-ish
        }
    }

    fn matches_filter(&self, filter: &CompositeFilter) -> bool {
        match (self, filter) {
            (CompositeState::Bound(_), CompositeFilter::Bound) => true,
            (CompositeState::Unbound, CompositeFilter::Unbound) => true,
            (CompositeState::Incomplete(_), CompositeFilter::Incomplete) => true,
            _ => false,
        }
    }
}

pub async fn list(
    cmd: ListCompositeCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let composite_infos = fuchsia_driver_dev::get_composite_info(&driver_development_proxy).await?;
    let nodes = fuchsia_driver_dev::get_device_info(&driver_development_proxy, &[], false).await?;
    let moniker_to_node: HashMap<String, fdd::NodeInfo> =
        nodes.into_iter().filter_map(|n| n.moniker.as_ref().cloned().map(|m| (m, n))).collect();

    let with_style = termion::is_tty(&std::io::stdout());

    let mut table = Table::new();
    table.set_format(*FORMAT_CLEAN);
    table.set_titles(row!("STATE", "NAME", "DRIVER"));

    let mut composite_list: Vec<(String, CompositeState)> = Vec::new();

    for info in composite_infos {
        let spec = info.composite.as_ref().and_then(|c| {
            let fdd::CompositeInfo::Composite(info) = c;
            info.spec.as_ref()
        });
        let name = spec.and_then(|s| s.name.clone()).unwrap_or_else(|| "N/A".to_string());

        if let Some(filter_name) = &cmd.name {
            if !name.contains(filter_name) {
                continue;
            }
        }

        let mut is_incomplete = false;
        if let Some(parent_monikers) = &info.parent_monikers {
            if parent_monikers.iter().any(|m| m.is_none()) {
                is_incomplete = true;
            }
        } else if let Some(parent_paths) = &info.parent_topological_paths {
            if parent_paths.iter().any(|m| m.is_none()) {
                is_incomplete = true;
            }
        } else {
            // If we have no parent info at all, and no composite node, assume incomplete.
            if info.moniker.is_none() && info.topological_path.is_none() {
                is_incomplete = true;
            }
        }

        let state = if is_incomplete {
            let matched_driver_url = info.composite.as_ref().and_then(|c| {
                let fdd::CompositeInfo::Composite(comp) = c;
                comp.matched_driver
                    .as_ref()
                    .and_then(|m| m.composite_driver.as_ref())
                    .and_then(|d| d.driver_info.as_ref())
                    .and_then(|i| i.url.clone())
            });
            CompositeState::Incomplete(matched_driver_url)
        } else {
            if let Some(moniker) = &info.moniker {
                if let Some(node) = moniker_to_node.get(moniker) {
                    match &node.bound_driver_url {
                        Some(url) if url != "unbound" && !url.is_empty() => {
                            CompositeState::Bound(url.clone())
                        }
                        _ => CompositeState::Unbound,
                    }
                } else {
                    CompositeState::Unbound
                }
            } else {
                CompositeState::Unbound
            }
        };

        if let Some(filter) = &cmd.filter {
            if !state.matches_filter(filter) {
                continue;
            }
        }

        composite_list.push((name, state));
    }

    composite_list.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, state) in composite_list {
        if !cmd.verbose {
            writeln!(writer, "{}", name)?;
            continue;
        }

        let driver = match &state {
            CompositeState::Bound(url) => url.clone(),
            CompositeState::Incomplete(Some(url)) => url.clone(),
            _ => "None".to_string(),
        };

        table.add_row(row!(state.to_colorized_string(with_style), name, driver));
    }

    if cmd.verbose {
        table.print(writer)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use argh::FromArgs;
    use flex_client::fidl::ServerEnd;
    use flex_fuchsia_driver_framework as fdf;
    use fuchsia_async as fasync;
    use futures::future::{Future, FutureExt};
    use futures::stream::StreamExt;

    async fn test_list_composite<F, Fut>(
        cmd: ListCompositeCommand,
        on_driver_development_request: F,
    ) -> Result<String>
    where
        F: Fn(fdd::ManagerRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + Sync,
    {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(not(feature = "fdomain"))]
        let client = flex_client::fidl::ZirconClient;
        let (driver_development_proxy, mut driver_development_requests) =
            client.create_proxy_and_stream::<fdd::ManagerMarker>();

        let mut writer = Vec::new();
        let request_handler_task = fasync::Task::spawn(async move {
            while let Some(res) = driver_development_requests.next().await {
                let request = res.context("Failed to get next request")?;
                on_driver_development_request(request).await.context("Failed to handle request")?;
            }
            anyhow::bail!("Driver development request stream unexpectedly closed");
        });
        futures::select! {
            res = request_handler_task.fuse() => {
                res?;
                anyhow::bail!("Request handler task unexpectedly finished");
            }
            res = list(cmd, &mut writer, driver_development_proxy).fuse() => res.context("List composite command failed")?,
        }

        String::from_utf8(writer).context("Failed to convert list composite output to a string")
    }

    async fn run_composite_info_iterator_server(
        mut infos: Vec<fdd::CompositeNodeInfo>,
        iterator: ServerEnd<fdd::CompositeInfoIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::CompositeInfoIteratorRequest::GetNext { responder } => {
                    responder
                        .send(&infos)
                        .context("Failed to send composite infos to responder")?;
                    infos.clear();
                }
            }
        }
        Ok(())
    }

    async fn run_node_info_iterator_server(
        mut nodes: Vec<fdd::NodeInfo>,
        iterator: ServerEnd<fdd::NodeInfoIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::NodeInfoIteratorRequest::GetNext { responder } => {
                    responder.send(&nodes).context("Failed to send node infos to responder")?;
                    nodes.clear();
                }
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_list_sorting_and_states() {
        let cmd = ListCompositeCommand::from_args(&["list"], &[]).unwrap();

        let output = test_list_composite(cmd, |request: fdd::ManagerRequest| async move {
            match request {
                fdd::ManagerRequest::GetCompositeInfo { iterator, .. } => {
                    run_composite_info_iterator_server(
                        vec![
                            fdd::CompositeNodeInfo {
                                composite: Some(fdd::CompositeInfo::Composite(
                                    fdf::CompositeInfo {
                                        spec: Some(fdf::CompositeNodeSpec {
                                            name: Some("zebra".to_string()),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                )),
                                moniker: Some("dev/sys/zebra".to_string()),
                                ..Default::default()
                            },
                            fdd::CompositeNodeInfo {
                                composite: Some(fdd::CompositeInfo::Composite(
                                    fdf::CompositeInfo {
                                        spec: Some(fdf::CompositeNodeSpec {
                                            name: Some("apple".to_string()),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                )),
                                parent_monikers: Some(vec![None]), // Incomplete
                                ..Default::default()
                            },
                        ],
                        iterator,
                    )
                    .await
                }
                fdd::ManagerRequest::GetNodeInfo { iterator, .. } => {
                    run_node_info_iterator_server(
                        vec![fdd::NodeInfo {
                            moniker: Some("dev/sys/zebra".to_string()),
                            bound_driver_url: Some(
                                "fuchsia-boot:///zebra#meta/zebra.cm".to_string(),
                            ),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                }
                _ => Ok(()),
            }
        })
        .await
        .unwrap();

        // Should be sorted: apple, zebra
        assert_eq!(output, "apple\nzebra\n");
    }

    #[fuchsia::test]
    async fn test_list_verbose_incomplete() {
        let cmd = ListCompositeCommand::from_args(&["list"], &["-v"]).unwrap();

        let output = test_list_composite(cmd, |request: fdd::ManagerRequest| async move {
            match request {
                fdd::ManagerRequest::GetCompositeInfo { iterator, .. } => {
                    run_composite_info_iterator_server(
                        vec![fdd::CompositeNodeInfo {
                            composite: Some(fdd::CompositeInfo::Composite(fdf::CompositeInfo {
                                spec: Some(fdf::CompositeNodeSpec {
                                    name: Some("incomplete_spec".to_string()),
                                    ..Default::default()
                                }),
                                matched_driver: Some(fdf::CompositeDriverMatch {
                                    composite_driver: Some(fdf::CompositeDriverInfo {
                                        driver_info: Some(fdf::DriverInfo {
                                            url: Some(
                                                "fuchsia-boot:///incomplete#meta/incomplete.cm"
                                                    .to_string(),
                                            ),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })),
                            parent_monikers: Some(vec![None]),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                }
                fdd::ManagerRequest::GetNodeInfo { iterator, .. } => {
                    run_node_info_iterator_server(vec![], iterator).await
                }
                _ => Ok(()),
            }
        })
        .await
        .unwrap();

        assert!(output.contains("Incomplete"));
        assert!(output.contains("incomplete_spec"));
        assert!(output.contains("fuchsia-boot:///incomplete#meta/incomplete.cm"));
    }
}
