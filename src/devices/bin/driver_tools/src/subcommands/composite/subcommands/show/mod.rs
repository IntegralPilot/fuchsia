// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::common::{node_property_value_to_string, write_node_properties};
use anyhow::{Context, Result, bail};
use args::ShowCompositeCommand;
use flex_client::ProxyHasDomain;
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use std::io::Write;

pub async fn show(
    cmd: ShowCompositeCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let composite_infos =
        fuchsia_driver_dev::get_composite_node_specs(&driver_development_proxy, None)
            .await
            .context("Failed to get composite node specs")?;

    let filtered_infos: Vec<_> = composite_infos
        .into_iter()
        .filter(|info| {
            info.spec
                .as_ref()
                .and_then(|spec| spec.name.as_ref())
                .map(|name| name.contains(&cmd.query))
                .unwrap_or(false)
        })
        .collect();

    if filtered_infos.is_empty() {
        writeln!(writer, "No composite node specs found matching {:?}.", cmd.query)?;
        return Ok(());
    }

    if filtered_infos.len() > 1 {
        let names: Vec<String> = filtered_infos
            .iter()
            .map(|info| {
                info.spec
                    .as_ref()
                    .and_then(|spec| spec.name.clone())
                    .unwrap_or_else(|| "N/A".to_string())
            })
            .collect();
        bail!(
            "The query {:?} matches more than one composite node spec:\n{}\n\nTo avoid ambiguity, use a more specific query.",
            cmd.query,
            names.join("\n")
        );
    }

    let info = &filtered_infos[0];
    let spec = info.spec.as_ref().context("Missing spec in composite info")?;
    let matched_driver = info.matched_driver.as_ref();

    writeln!(writer, "{0: <10}: {1}", "Name", spec.name.as_deref().unwrap_or("N/A"))?;

    let driver_url = matched_driver
        .and_then(|m| m.composite_driver.as_ref())
        .and_then(|cd| cd.driver_info.as_ref())
        .and_then(|di| di.url.as_deref());

    writeln!(writer, "{0: <10}: {1}", "Driver", driver_url.unwrap_or("None"))?;

    // Query for existing composite nodes to get their topological paths or monikers
    let (iterator, iterator_server) =
        driver_development_proxy.domain().create_proxy::<fdd::CompositeInfoIteratorMarker>();
    driver_development_proxy
        .get_composite_info(iterator_server)
        .context("Failed to call GetCompositeInfo(). This may be because the driver development component is not reachable.")?;

    let mut parent_paths = Vec::new();
    let mut parent_monikers = Vec::new();
    let mut spec_topological_path = None;
    let mut spec_moniker = None;

    loop {
        let composite_list =
            iterator.get_next().await.context("CompositeInfoIterator GetNext() failed")?;

        if composite_list.is_empty() {
            break;
        }

        for composite_node in composite_list {
            if let Some(fdd::CompositeInfo::Composite(node_info)) = composite_node.composite {
                if node_info.spec.and_then(|s| s.name) == spec.name {
                    parent_paths = composite_node.parent_topological_paths.unwrap_or_default();
                    parent_monikers = composite_node.parent_monikers.unwrap_or_default();
                    spec_topological_path = composite_node.topological_path;
                    spec_moniker = composite_node.moniker;
                    break;
                }
            }
        }
        if !parent_paths.is_empty() || !parent_monikers.is_empty() {
            break;
        }
    }

    let display_moniker = if let Some(moniker) = spec_moniker {
        moniker
    } else if let Some(path) = spec_topological_path {
        get_moniker_from_path(&path, &driver_development_proxy).await?
    } else {
        "N/A".to_string()
    };

    writeln!(writer, "{0: <10}: {1}", "Node", display_moniker)?;

    if let Some(parents) = &spec.parents2 {
        writeln!(writer, "{0: <10}: {1}", "Parents", parents.len())?;

        for (i, parent) in parents.iter().enumerate() {
            let parent_name = matched_driver
                .and_then(|m| m.parent_names.as_ref())
                .and_then(|names| names.get(i))
                .map(|s| s.as_str())
                .unwrap_or("N/A");

            let is_primary = matched_driver
                .and_then(|m| m.primary_parent_index)
                .map(|idx| idx == i as u32)
                .unwrap_or(false);

            let primary_tag = if is_primary { "(Primary)" } else { "" };
            writeln!(writer, "Parent {0} : {1} {2}", i, parent_name, primary_tag)?;

            let bound_node_moniker = if let Some(Some(moniker)) = parent_monikers.get(i) {
                moniker.clone()
            } else if let Some(Some(path)) = parent_paths.get(i) {
                get_moniker_from_path(path, &driver_development_proxy).await?
            } else {
                "Unbound".to_string()
            };

            writeln!(writer, "  Node    : {}", bound_node_moniker)?;

            let bind_rules_len = parent.bind_rules.len();
            writeln!(writer, "  {0} Bind Rules", bind_rules_len)?;

            for (j, bind_rule) in parent.bind_rules.iter().enumerate() {
                let key = &bind_rule.key;
                let values = bind_rule
                    .values
                    .iter()
                    .map(|value| node_property_value_to_string(value))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    writer,
                    "  [{0:>2}/{1:>2}] : {2:?} {3} {{ {4} }}",
                    j + 1,
                    bind_rules_len,
                    bind_rule.condition,
                    key,
                    values,
                )?;
            }

            write_node_properties(&parent.properties, writer)?;
        }
    }

    Ok(())
}

async fn get_moniker_from_path(path: &str, proxy: &fdd::ManagerProxy) -> Result<String> {
    let nodes = fuchsia_driver_dev::get_device_info(proxy, &[path.to_string()], true).await?;
    if let Some(node) = nodes.first() {
        if let Some(moniker) = &node.moniker {
            return Ok(moniker.clone());
        }
    }
    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use argh::FromArgs;
    use flex_client::fidl::ServerEnd;
    use flex_fuchsia_driver_framework as fdf;
    use fuchsia_async as fasync;
    use futures::future::{Future, FutureExt};
    use futures::stream::StreamExt;

    /// Invokes `show` with `cmd` and runs a mock driver development server that
    /// invokes `on_driver_development_request` whenever it receives a request.
    /// The output of `show` that is normally written to its `writer` parameter
    /// is returned.
    async fn test_show_composite<F, Fut>(
        cmd: ShowCompositeCommand,
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

        // Run the command and mock driver development server.
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
            res = show(cmd, &mut writer, driver_development_proxy).fuse() => res.context("Show composite command failed")?,
        }

        String::from_utf8(writer).context("Failed to convert show composite output to a string")
    }

    async fn run_specs_iterator_server(
        mut specs: Vec<fdf::CompositeInfo>,
        iterator: ServerEnd<fdd::CompositeNodeSpecIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::CompositeNodeSpecIteratorRequest::GetNext { responder } => {
                    responder
                        .send(&specs)
                        .context("Failed to send composite node specs to responder")?;
                    specs.clear();
                }
            }
        }
        Ok(())
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

    #[fuchsia::test]
    async fn test_show_partial_match() {
        let cmd = ShowCompositeCommand::from_args(&["show"], &["test"]).unwrap();

        let output = test_show_composite(cmd, |request: fdd::ManagerRequest| async move {
            match request {
                fdd::ManagerRequest::GetCompositeNodeSpecs {
                    name_filter,
                    iterator,
                    control_handle: _,
                } => {
                    // name_filter should be None because we want all specs for partial match
                    assert!(name_filter.is_none());
                    run_specs_iterator_server(
                        vec![fdf::CompositeInfo {
                            spec: Some(fdf::CompositeNodeSpec {
                                name: Some("test_spec".to_string()),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .context("Failed to run specs iterator server")?
                }
                fdd::ManagerRequest::GetCompositeInfo { iterator, control_handle: _ } => {
                    run_composite_info_iterator_server(vec![], iterator)
                        .await
                        .context("Failed to run composite info iterator server")?
                }
                _ => {}
            }
            Ok(())
        })
        .await
        .unwrap();

        assert!(output.contains("Name      : test_spec"));
    }
}
