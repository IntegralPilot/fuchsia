// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::{list_cmd_print, list_cmd_serialized};
use component_debug_fdomain::realm::Instance;
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_list_args::ComponentListCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::Serialize;
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct ListTool {
    #[command]
    cmd: ComponentListCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(ListTool);

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ListOutput {
    instances: Vec<Instance>,
}

#[async_trait(?Send)]
impl FfxMain for ListTool {
    type Writer = VerifiedMachineWriter<ListOutput>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;
        // All errors from component_debug library are user-visible.
        if writer.is_machine() {
            let instances = list_cmd_serialized(self.cmd.filter, realm_query)
                .await
                .map_err(|e| ffx_error!(e))?;
            let output = ListOutput { instances };
            writer.machine(&output)?;
        } else {
            list_cmd_print(self.cmd.filter, self.cmd.verbose, realm_query, writer)
                .await
                .map_err(|e| ffx_error!(e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ListTool;

    use anyhow::Result;
    use component_debug_fdomain::realm::ResolvedInfo;

    use ffx_component_list_args::ComponentListCommand;
    use ffx_writer::{Format, TestBuffers};
    use fho::FfxMain;

    use moniker::Moniker;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[fuchsia::test]
    async fn test_schema() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let cmd = ComponentListCommand { filter: None, verbose: false };
        let tool = ListTool {
            cmd,
            rcs: testing_lib::setup_fake_rcs(client, testing_lib::FakeRcsConfig::default()).into(),
        };
        let buffers = TestBuffers::default();

        let writer = <ListTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = buffers.into_stdout_str();

        let err = format!("schema not valid {output}");
        let json = serde_json::from_str(&output).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <ListTool as FfxMain>::Writer::verify_schema(&json).expect(&err);

        let want = ListOutput {
            instances: vec![
                Instance {
                    environment: None,
                    moniker: Moniker::parse_str("example/component")?,
                    resolved_info: Some(ResolvedInfo {
                        execution_info: None,
                        resolved_url: "".to_string(),
                    }),
                    url: "".to_string(),
                    instance_id: None,
                },
                Instance {
                    environment: None,
                    moniker: Moniker::parse_str("foo/bar/thing:instance")?,
                    resolved_info: Some(ResolvedInfo {
                        execution_info: None,
                        resolved_url: "".to_string(),
                    }),
                    url: "".to_string(),
                    instance_id: None,
                },
                Instance {
                    environment: None,
                    moniker: Moniker::parse_str("foo/component")?,
                    resolved_info: Some(ResolvedInfo {
                        execution_info: None,
                        resolved_url: "".to_string(),
                    }),
                    url: "".to_string(),
                    instance_id: None,
                },
                Instance {
                    environment: None,
                    moniker: Moniker::parse_str("other/component")?,
                    resolved_info: Some(ResolvedInfo {
                        execution_info: None,
                        resolved_url: "".to_string(),
                    }),
                    url: "".to_string(),
                    instance_id: None,
                },
            ],
        };
        assert_eq!(json, json!(want));

        Ok(())
    }
}
