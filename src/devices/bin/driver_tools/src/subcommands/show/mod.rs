// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use anyhow::Result;
use args::ShowCommand;
use bind::debugger::debug_dump::dump_bind_rules;
use flex_fuchsia_driver_development as fdd;
use fuchsia_driver_dev::GetSingleDriverError;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use std::collections::HashMap;
use std::io::Write;

pub async fn show(
    cmd: ShowCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    let driver_info = match fuchsia_driver_dev::get_single_driver_from_query(
        &cmd.query,
        &driver_development_proxy,
    )
    .await
    {
        Ok(info) => info,
        Err(GetSingleDriverError::NotFound(query)) => {
            return Err(anyhow::anyhow!("No drivers found matching {:?}.", query));
        }
        Err(e) => return Err(anyhow::anyhow!(e)),
    };

    // Query devices and create a map of drivers to devices.
    let empty: [String; 0] = [];
    let device_info = fuchsia_driver_dev::get_device_info(
        &driver_development_proxy,
        &empty,
        /* exact_match= */ false,
    )
    .await?;

    let mut driver_to_devices = HashMap::<String, Vec<String>>::new();
    for device in device_info.into_iter() {
        let driver = device.bound_driver_url.clone();
        let device_name = device.moniker.as_ref();
        if let (Some(driver), Some(device_name)) = (driver, device_name) {
            driver_to_devices
                .entry(driver)
                .and_modify(|v| v.push(device_name.to_string()))
                .or_insert_with(|| vec![device_name.to_string()]);
        }
    }

    if let Some(name) = driver_info.name {
        writeln!(writer, "{0: <10}: {1}", "Name", name)?;
    }
    if let Some(url) = driver_info.url.as_ref() {
        writeln!(writer, "{0: <10}: {1}", "URL", url)?;
    }

    // If the version isn't set, the value is assumed to be 1.
    writeln!(
        writer,
        "{0: <10}: {1}",
        "DF Version",
        driver_info.driver_framework_version.unwrap_or(1)
    )?;

    if let Some(node_categories) = driver_info.device_categories {
        write!(writer, "Node Categories: [")?;

        for (i, category_table) in node_categories.iter().enumerate() {
            if let Some(category) = &category_table.category {
                if let Some(subcategory) = &category_table.subcategory {
                    if !subcategory.is_empty() {
                        write!(writer, "{}::{}", category, subcategory)?;
                    } else {
                        write!(writer, "{}", category,)?;
                    }
                } else {
                    write!(writer, "{}", category,)?;
                }
            }

            if i != node_categories.len() - 1 {
                write!(writer, ", ")?;
            }
        }
        writeln!(writer, "]")?;
    }

    if let Some(url) = driver_info.url {
        if let Some(nodes) = driver_to_devices.get(&url) {
            writeln!(writer, "{0: <10}:\n  {1}", "Nodes", nodes.join("\n  "))?;
        }
    }
    let bind_rules =
        driver_info.bind_rules_bytecode.map(|bytecode| dump_bind_rules(bytecode).ok()).flatten();
    match bind_rules {
        Some(bind_rules) => {
            writeln!(writer, "{0: <10}: ", "Bind rules bytecode")?;
            writeln!(writer, "{}", bind_rules)?;
        }
        _ => writeln!(writer, "Issue parsing the bind rules bytecode")?,
    }
    writeln!(writer)?;

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

    /// Invokes `show` with `cmd` and runs a mock driver development server that
    /// invokes `on_driver_development_request` whenever it receives a request.
    /// The output of `show` that is normally written to its `writer` parameter
    /// is returned.
    async fn run_test_show<F, Fut>(
        #[cfg(feature = "fdomain")] client: std::sync::Arc<flex_client::Client>,
        cmd: ShowCommand,
        on_driver_development_request: F,
    ) -> Result<String>
    where
        F: Fn(fdd::ManagerRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + Sync,
    {
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
            res = show(cmd, &mut writer, driver_development_proxy).fuse() => res.context("Show command failed")?,
        }

        String::from_utf8(writer).context("Failed to convert show output to a string")
    }

    async fn run_driver_info_iterator_server(
        mut driver_infos: Vec<fdf::DriverInfo>,
        iterator: ServerEnd<fdd::DriverInfoIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::DriverInfoIteratorRequest::GetNext { responder } => {
                    responder
                        .send(&driver_infos)
                        .context("Failed to send driver infos to responder")?;
                    driver_infos.clear();
                }
            }
        }
        Ok(())
    }

    async fn run_device_info_iterator_server(
        mut device_infos: Vec<fdd::NodeInfo>,
        iterator: ServerEnd<fdd::NodeInfoIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::NodeInfoIteratorRequest::GetNext { responder } => {
                    responder
                        .send(&device_infos)
                        .context("Failed to send device infos to responder")?;
                    device_infos.clear();
                }
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_show() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = ShowCommand::from_args(&["show"], &["foo"]).unwrap();

        let output = run_test_show(
            #[cfg(feature = "fdomain")]
            client,
            cmd,
            |request: fdd::ManagerRequest| async move {
                match request {
                    fdd::ManagerRequest::GetDriverInfo {
                        driver_filter,
                        iterator,
                        control_handle: _,
                    } => {
                        let mut infos = vec![fdf::DriverInfo {
                            name: Some("foo".to_owned()),
                            url: Some(
                                "fuchsia-pkg://fuchsia.com/foo-package#meta/foo.cm".to_owned(),
                            ),
                            package_type: Some(fdf::DriverPackageType::Base),
                            device_categories: Some(vec![
                                fdf::DeviceCategory {
                                    category: Some("connectivity".to_string()),
                                    subcategory: Some("ethernet".to_string()),
                                    ..Default::default()
                                },
                                fdf::DeviceCategory {
                                    category: Some("usb".to_string()),
                                    subcategory: None,
                                    ..Default::default()
                                },
                            ]),
                            ..Default::default()
                        }];
                        if !driver_filter.is_empty()
                            && !infos[0].url.as_ref().unwrap().contains(&driver_filter[0])
                            && !infos[0].name.as_ref().unwrap().contains(&driver_filter[0])
                        {
                            infos.clear();
                        }

                        run_driver_info_iterator_server(infos, iterator)
                            .await
                            .context("Failed to run driver info iterator server")?
                    }
                    fdd::ManagerRequest::GetNodeInfo {
                        node_filter: _,
                        iterator,
                        control_handle: _,
                        exact_match: _,
                    } => run_device_info_iterator_server(
                        vec![fdd::NodeInfo {
                            bound_driver_url: Some(
                                "fuchsia-pkg://fuchsia.com/foo-package#meta/foo.cm".to_owned(),
                            ),
                            moniker: Some("dev.sys.foo".to_owned()),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .context("Failed to run device info iterator server")?,
                    _ => {}
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert_eq!(
            output,
            r#"Name      : foo
URL       : fuchsia-pkg://fuchsia.com/foo-package#meta/foo.cm
DF Version: 1
Node Categories: [connectivity::ethernet, usb]
Nodes     :
  dev.sys.foo
Issue parsing the bind rules bytecode

"#
        );
    }

    #[fuchsia::test]
    async fn test_show_no_match() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = ShowCommand::from_args(&["show"], &["nonexistent"]).unwrap();

        let result = run_test_show(
            #[cfg(feature = "fdomain")]
            client,
            cmd,
            |request: fdd::ManagerRequest| async move {
                match request {
                    fdd::ManagerRequest::GetDriverInfo { iterator, .. } => {
                        run_driver_info_iterator_server(vec![], iterator).await
                    }
                    _ => Ok(()),
                }
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            format!("{:?}", result.unwrap_err()),
            "Show command failed\n\nCaused by:\n    No drivers found matching \"nonexistent\"."
        );
    }

    #[fuchsia::test]
    async fn test_show_ambiguous() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = ShowCommand::from_args(&["show"], &["foo"]).unwrap();

        let result = run_test_show(
            #[cfg(feature = "fdomain")]
            client,
            cmd,
            |request: fdd::ManagerRequest| async move {
                match request {
                    fdd::ManagerRequest::GetDriverInfo { iterator, .. } => {
                        run_driver_info_iterator_server(
                            vec![
                                fdf::DriverInfo {
                                    name: Some("foo1".to_owned()),
                                    url: Some(
                                        "fuchsia-pkg://fuchsia.com/foo1#meta/foo1.cm".to_owned(),
                                    ),
                                    ..Default::default()
                                },
                                fdf::DriverInfo {
                                    name: Some("foo2".to_owned()),
                                    url: Some(
                                        "fuchsia-pkg://fuchsia.com/foo2#meta/foo2.cm".to_owned(),
                                    ),
                                    ..Default::default()
                                },
                            ],
                            iterator,
                        )
                        .await
                    }
                    _ => Ok(()),
                }
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(err_msg.contains("matches more than one driver"));
        assert!(err_msg.contains("foo1"));
        assert!(err_msg.contains("foo2"));
    }
}
