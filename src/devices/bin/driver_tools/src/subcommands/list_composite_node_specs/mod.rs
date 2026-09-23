// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;

use crate::common::{node_property_value_to_string, write_node_properties};
use anyhow::{Context, Result};
use args::ListCompositeNodeSpecsCommand;
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use std::io::Write;

pub async fn list_composite_node_specs(
    cmd: ListCompositeNodeSpecsCommand,
    writer: &mut dyn Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    writeln!(
        writer,
        "WARNING: This command is deprecated. Use `ffx driver composite list` or `ffx driver composite show` instead."
    )?;

    let composite_infos =
        fuchsia_driver_dev::get_composite_node_specs(&driver_development_proxy, None)
            .await
            .context("Failed to get composite node specs")?;

    let filtered_infos: Vec<_> = if let Some(name_filter) = &cmd.name {
        composite_infos
            .into_iter()
            .filter(|info| {
                info.spec
                    .as_ref()
                    .and_then(|spec| spec.name.as_ref())
                    .map(|name| name.contains(name_filter))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        composite_infos
    };

    if !cmd.verbose {
        for composite_info in filtered_infos {
            let name =
                composite_info.spec.and_then(|spec| spec.name).unwrap_or_else(|| "N/A".to_string());
            let driver = composite_info
                .matched_driver
                .and_then(|matched_driver| matched_driver.composite_driver)
                .and_then(|composite_driver| composite_driver.driver_info)
                .and_then(|driver_info| driver_info.url)
                .unwrap_or_else(|| "None".to_string());
            writeln!(writer, "{:<20}: {}", name, driver)?;
        }
        return Ok(());
    }

    for composite_info in filtered_infos {
        let matched_driver = composite_info.matched_driver.unwrap_or_default();
        let spec = composite_info.spec.unwrap_or_default();
        if let Some(name) = &spec.name {
            writeln!(writer, "{0: <10}: {1}", "Name", name)?;
        }

        let url = matched_driver
            .composite_driver
            .as_ref()
            .and_then(|composite_driver| composite_driver.driver_info.as_ref())
            .and_then(|driver_info| driver_info.url.clone());

        if let Some(driver) = url {
            writeln!(writer, "{0: <10}: {1}", "Driver", driver)?;
        } else {
            writeln!(writer, "{0: <10}: {1}", "Driver", "None")?;
        }

        if let Some(nodes) = spec.parents2 {
            writeln!(writer, "{0: <10}: {1}", "Nodes", nodes.len())?;

            for (i, node) in nodes.into_iter().enumerate() {
                let name = match &matched_driver.parent_names {
                    Some(names) => format!("\"{}\"", names.get(i).unwrap()),
                    None => "None".to_string(),
                };

                if &matched_driver.primary_parent_index == &Some(i as u32) {
                    writeln!(writer, "{0: <10}: {1} (Primary)", format!("Node {}", i), name)?;
                } else {
                    writeln!(writer, "{0: <10}: {1}", format!("Node {}", i), name)?;
                }

                let bind_rules_len = node.bind_rules.len();
                writeln!(writer, "  {0} {1}", bind_rules_len, "Bind Rules")?;

                for (j, bind_rule) in node.bind_rules.into_iter().enumerate() {
                    let key = &bind_rule.key;
                    let values = bind_rule
                        .values
                        .into_iter()
                        .map(|value| node_property_value_to_string(&value))
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

                write_node_properties(&node.properties, writer)?;
            }
        }

        writeln!(writer)?;
    }
    Ok(())
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
    #[cfg(feature = "fdomain")]
    use std::sync::Arc;

    /// Invokes `list_composite_node_specs` with `cmd` and runs a mock driver development server that
    /// invokes `on_driver_development_request` whenever it receives a request.
    /// The output of `list_composite_node_specs` that is normally written to its `writer` parameter
    /// is returned.
    async fn test_list_composite_node_specs<F, Fut>(
        #[cfg(feature = "fdomain")] client: Arc<flex_client::Client>,
        cmd: ListCompositeNodeSpecsCommand,
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
                let request = res.unwrap();
                on_driver_development_request(request).await.context("Failed to handle request")?;
            }
            anyhow::bail!("Driver development request stream unexpectedly closed");
        });
        futures::select! {
            res = request_handler_task.fuse() => {
                res?;
                anyhow::bail!("Request handler task unexpectedly finished");
            }
            res = list_composite_node_specs(cmd, &mut writer, driver_development_proxy).fuse() => res.context("List composite node specs command failed")?,
        }

        String::from_utf8(writer)
            .context("Failed to convert list composite node specs output to a string")
    }

    async fn run_specs_iterator_server(
        mut specs: Vec<fdf::CompositeInfo>,
        iterator: ServerEnd<fdd::CompositeNodeSpecIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.unwrap();
            match request {
                fdd::CompositeNodeSpecIteratorRequest::GetNext { responder } => {
                    responder.send(&specs).unwrap();
                    specs.clear();
                }
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_verbose() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = ListCompositeNodeSpecsCommand::from_args(
            &["list-composite-node-specs"],
            &["--verbose"],
        )
        .unwrap();

        let output = test_list_composite_node_specs(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                match request {
                    fdd::ManagerRequest::GetCompositeNodeSpecs {
                        name_filter: _,
                        iterator,
                        control_handle: _,
                    } => run_specs_iterator_server(
                        vec![
                            fdf::CompositeInfo {
                                spec: Some(fdf::CompositeNodeSpec {
                                    name: Some("test_spec".to_string()),
                                    parents2: Some(vec![fdf::ParentSpec2 {
                                        bind_rules: vec![fdf::BindRule2 {
                                            key: "rule_key".to_string(),
                                            condition: fdf::Condition::Accept,
                                            values: vec![fdf::NodePropertyValue::StringValue(
                                                "rule_val".to_string(),
                                            )],
                                        }],
                                        properties: vec![fdf::NodeProperty2 {
                                            key: "prop_key".to_string(),
                                            value: fdf::NodePropertyValue::StringValue(
                                                "prop_val".to_string(),
                                            ),
                                        }],
                                    }]),
                                    ..Default::default()
                                }),
                                matched_driver: None,
                                ..Default::default()
                            },
                            fdf::CompositeInfo {
                                spec: Some(fdf::CompositeNodeSpec {
                                    name: Some("test_spec_with_driver".to_string()),
                                    parents2: Some(vec![
                                        fdf::ParentSpec2 {
                                            bind_rules: vec![fdf::BindRule2 {
                                                key: "rule_key".to_string(),
                                                condition: fdf::Condition::Accept,
                                                values: vec![
                                                    fdf::NodePropertyValue::StringValue(
                                                        "rule_val".to_string(),
                                                    ),
                                                    fdf::NodePropertyValue::StringValue(
                                                        "rule_val_2".to_string(),
                                                    ),
                                                ],
                                            }],
                                            properties: vec![fdf::NodeProperty2 {
                                                key: "prop_key_0".to_string(),
                                                value: fdf::NodePropertyValue::StringValue(
                                                    "prop_val_0".to_string(),
                                                ),
                                            }],
                                        },
                                        fdf::ParentSpec2 {
                                            bind_rules: vec![
                                                fdf::BindRule2 {
                                                    key: "0x0001".to_string(),
                                                    condition: fdf::Condition::Accept,
                                                    values: vec![
                                                        fdf::NodePropertyValue::IntValue(0x42),
                                                        fdf::NodePropertyValue::IntValue(0x123),
                                                        fdf::NodePropertyValue::IntValue(0x234),
                                                    ],
                                                },
                                                fdf::BindRule2 {
                                                    key: "0xdeadbeef".to_string(),
                                                    condition: fdf::Condition::Accept,
                                                    values: vec![fdf::NodePropertyValue::IntValue(
                                                        0xbeef,
                                                    )],
                                                },
                                            ],
                                            properties: vec![
                                                fdf::NodeProperty2 {
                                                    key: "prop_key_1".to_string(),
                                                    value: fdf::NodePropertyValue::EnumValue(
                                                        "prop_key_1.prop_val".to_string(),
                                                    ),
                                                },
                                                fdf::NodeProperty2 {
                                                    key: "prop_key_2".to_string(),
                                                    value: fdf::NodePropertyValue::IntValue(0x1),
                                                },
                                                fdf::NodeProperty2 {
                                                    key: "prop_key_3".to_string(),
                                                    value: fdf::NodePropertyValue::BoolValue(true),
                                                },
                                            ],
                                        },
                                    ]),
                                    ..Default::default()
                                }),
                                matched_driver: Some(fdf::CompositeDriverMatch {
                                    composite_driver: Some(fdf::CompositeDriverInfo {
                                        driver_info: Some(fdf::DriverInfo {
                                            url: Some("driver_url".to_string()),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    }),
                                    parent_names: Some(vec![
                                        "name_one".to_string(),
                                        "name_two".to_string(),
                                    ]),
                                    primary_parent_index: Some(1),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        ],
                        iterator,
                    )
                    .await
                    .context("Failed to run driver info iterator server")?,
                    _ => {}
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert_eq!(
            output,
            r#"WARNING: This command is deprecated. Use `ffx driver composite list` or `ffx driver composite show` instead.
Name      : test_spec
Driver    : None
Nodes     : 1
Node 0    : None
  1 Bind Rules
  [ 1/ 1] : Accept rule_key { "rule_val" }
  1 Properties
  [ 1/ 1] : Key prop_key                       Value "prop_val"

Name      : test_spec_with_driver
Driver    : driver_url
Nodes     : 2
Node 0    : "name_one"
  1 Bind Rules
  [ 1/ 1] : Accept rule_key { "rule_val", "rule_val_2" }
  1 Properties
  [ 1/ 1] : Key prop_key_0                     Value "prop_val_0"
Node 1    : "name_two" (Primary)
  2 Bind Rules
  [ 1/ 2] : Accept 0x0001 { 0x000042, 0x000123, 0x000234 }
  [ 2/ 2] : Accept 0xdeadbeef { 0x00beef }
  3 Properties
  [ 1/ 3] : Key prop_key_1                     Value Enum(prop_key_1.prop_val)
  [ 2/ 3] : Key prop_key_2                     Value 0x000001
  [ 3/ 3] : Key prop_key_3                     Value true

"#
        );
    }
}
