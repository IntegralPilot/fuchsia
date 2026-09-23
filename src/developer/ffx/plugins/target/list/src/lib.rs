// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use analytics::add_custom_event;
use async_trait::async_trait;

use ffx_config::EnvironmentContext;
use ffx_list_args::ListCommand;
use ffx_target::{TargetInfo, TargetInfoQuery};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxError, FfxMain, FfxTool};
use fuchsia_async::TimeoutExt as _;
use target_behavior::{ConnectionBehavior, target_interface};
use target_formatter::{JsonTarget, JsonTargetFormatter, TargetFormatter};
use thiserror::Error;

#[derive(FfxError, thiserror::Error, Debug)]
pub enum ShowTargetsError {
    #[exit_with_code(2)]
    #[error("Device {0} not found.")]
    DeviceNotFound(String),

    #[exit_with_code(1)]
    #[error("Invalid arguments, you must allow at least one address type")]
    NoAddressTypes,

    #[user]
    #[error("Writer error: {0}")]
    Writer(#[from] std::io::Error),

    #[user]
    #[error("FFX Writer error: {0}")]
    FfxWriter(#[from] ffx_writer::Error),

    #[user]
    #[error("Formatter error: {0}")]
    Formatter(#[from] target_formatter::FormatterError),
}

#[derive(FfxError, Error, Debug)]
pub enum ListError {
    #[unexpected]
    #[error("Query parse error: {0}")]
    QueryParse(String),

    #[user]
    #[error("Target collection FIDL error: {0}")]
    Fidl(#[from] fidl::Error),

    #[user]
    #[error("Target formatter error: {0}")]
    Formatter(#[from] target_formatter::FormatterError),

    #[unexpected]
    #[error("Failed to get default target specifier: {0}")]
    GetDefaultTargetSpecifier(#[source] anyhow::Error),

    #[user]
    #[error("Failed to resolve target address: {0}")]
    TargetResolution(#[from] target_behavior::TargetResolutionError),

    #[user]
    #[error("Failed to list targets: {0}")]
    ListTargets(#[from] ffx_target::FfxTargetCrateError),

    #[transparent]
    #[error(transparent)]
    ShowTargets(#[from] ShowTargetsError),

    #[unexpected]
    #[error("FHO error: {0}")]
    Fho(#[from] fho::Error),

    #[user]
    #[error("Required argument `-c ssh.priv=` not specified. Unable to verify RCS state.")]
    MissingSshPrivKey,
}

#[derive(FfxTool)]
#[target(None)]
#[main_error(ListError)]
pub struct ListTool {
    #[command]
    cmd: ListCommand,
    context: EnvironmentContext,
    fho_env: fho::FhoEnvironment,
}

fho::embedded_plugin!(ListTool, ListError);

#[async_trait(?Send)]
impl FfxMain for ListTool {
    type Error = ListError;

    type Writer = VerifiedMachineWriter<Vec<JsonTarget>>;
    async fn main(self, writer: Self::Writer) -> std::result::Result<(), ListError> {
        self.main_impl(writer).await
    }
}

impl ListTool {
    pub fn new(
        cmd: ListCommand,
        context: EnvironmentContext,
        fho_env: fho::FhoEnvironment,
    ) -> Self {
        Self { cmd, context, fho_env }
    }

    async fn main_impl(self, mut writer: <Self as FfxMain>::Writer) -> Result<(), ListError> {
        let list_query = TargetInfoQuery::try_from(self.cmd.nodename.clone())
            .map_err(|e| ListError::QueryParse(e.to_string()))?;

        let mut infos = self.list_targets_direct(list_query).await?;

        let spec = ffx_target::get_target_specifier(&self.context)
            .map_err(ListError::GetDefaultTargetSpecifier)?;
        let default_query =
            TargetInfoQuery::try_from(spec).map_err(|e| ListError::QueryParse(e.to_string()))?;

        for ti in infos.iter_mut() {
            ti.is_default = (!matches!(default_query, TargetInfoQuery::First)
                && ti.match_query(&default_query))
            .then_some(true);
        }

        emit_device_stats_event(infos.len(), &self.cmd.nodename).await;
        show_targets(self.cmd, infos, &mut writer).await.map_err(ListError::ShowTargets)?;
        Ok(())
    }

    async fn list_targets_direct(
        &self,
        query: TargetInfoQuery,
    ) -> Result<Vec<TargetInfo>, ListError> {
        let is_addresses_format = matches!(
            self.cmd.format,
            ffx_list_args::Format::Addresses | ffx_list_args::Format::AddressesWithLexicalScope
        );
        let connect_to_rcs = !self.cmd.no_probe && !is_addresses_format;
        if self.context.is_strict() && connect_to_rcs {
            let keys: std::result::Result<Vec<std::path::PathBuf>, _> =
                self.context.get(ffx_config::keys::SSH_PRIVATE_KEY);
            match keys {
                Ok(ref paths) if paths.iter().any(|p| !p.as_os_str().is_empty()) => {}
                _ => return Err(ListError::MissingSshPrivKey),
            }
        }
        Ok(match query.get_target_addr() {
            Some(addr) => {
                if connect_to_rcs {
                    // We don't need to do discovery, and in fact may not be able to
                    // discover the device. So instead, just query the information
                    // directly.  We're going to assume this device is in product mode.
                    // (Note: we check whether we explicitly told _not_ to connect to RCS, in
                    // which case we're not going to get anything useful from trying to do an IdentifyHost.
                    // If the device is undiscoverable _and_ we cannot connect to RCS,
                    // then there's not much to be done. Unfortunately we can't
                    // know if a device is undiscoverable or not, so we can't give the
                    // user useful guidance in that situation.)
                    let mut context = self.context.clone();
                    context.override_target_specifier(&self.cmd.nodename);
                    let target_env = target_interface(&self.fho_env);
                    let behavior = target_env.init_connection_behavior(&context).await?;
                    let ConnectionBehavior::Direct(ref connector) = *behavior;
                    let resolution = connector.resolution().await?;
                    let discovery_timeout = std::time::Duration::from_millis(
                        context.get(ffx_config::keys::DISCOVERY_TIMEOUT_MS).unwrap_or(2000),
                    );
                    let target_info = resolution
                        .get_target_info(addr, &context)
                        .on_timeout(discovery_timeout, || {
                            Err(anyhow::anyhow!("Connection timed out"))
                        })
                        .await
                        .unwrap_or_else(|e| {
                            log::debug!("Failed to get target info for {addr:?}: {e:?}");
                            TargetInfo {
                                nodename: self.cmd.nodename.clone(),
                                addresses: vec![addr],
                                rcs_state: ffx_target::info::RemoteControlState::Down,
                                target_state: ffx_target::info::TargetState::Unknown,
                                ..Default::default()
                            }
                        });
                    vec![target_info]
                } else {
                    // Short-circuit: We have the address, and were told not to probe (or format is addresses and we decided not to probe).
                    // Just return what we know without connecting.
                    vec![TargetInfo {
                        nodename: self.cmd.nodename.clone(),
                        addresses: vec![addr],
                        rcs_state: ffx_target::info::RemoteControlState::Unknown,
                        target_state: ffx_target::info::TargetState::Unknown,
                        ..Default::default()
                    }]
                }
            }
            _ => {
                ffx_target::list_targets(
                    &self.context,
                    query.clone(),
                    !self.cmd.no_usb,
                    !self.cmd.no_mdns,
                    connect_to_rcs,
                )
                .await?
            }
        })
    }
}

async fn show_targets(
    cmd: ListCommand,
    mut infos: Vec<TargetInfo>,
    writer: &mut VerifiedMachineWriter<Vec<JsonTarget>>,
) -> Result<(), ShowTargetsError> {
    // Provide stable output. Use "unstable" since we don't care about the original ordering.
    infos.sort_unstable_by(|a, b| a.nodename.cmp(&b.nodename));
    match infos.len() {
        0 => {
            // Printed to stderr, so that if a user is parsing output, say from a formatted
            // output, that the message is not consumed. A stronger future strategy would
            // have richer behavior dependent upon whether the user has a controlling
            // terminal, which would require passing in more and richer IO delegates.
            if let Some(n) = cmd.nodename {
                return Err(ShowTargetsError::DeviceNotFound(n));
            } else {
                if !writer.is_machine() {
                    writeln!(writer.stderr(), "No devices found.")?;
                } else {
                    writer.machine(&Vec::new())?;
                }
            }
        }
        _ => {
            let address_types = cmd.address_types();
            if address_types.is_empty() {
                return Err(ShowTargetsError::NoAddressTypes);
            }
            if writer.is_machine() {
                let res = target_formatter::filter_targets_by_address_types(infos, address_types);
                let formatter = JsonTargetFormatter::from(res);
                writer.machine(&formatter.targets)?;
            } else {
                let formatter =
                    Box::<dyn TargetFormatter>::try_from((cmd.format, address_types, infos))?;
                writer.line(formatter.lines()?.join("\n"))?;
            }
        }
    }
    Ok(())
}

fn query_type(query: &str) -> &str {
    match TargetInfoQuery::try_from(query) {
        Ok(TargetInfoQuery::NodenameOrId(_)) => "nodename_or_id",
        Ok(TargetInfoQuery::Id(_)) => "id",
        Ok(TargetInfoQuery::Addr(_)) => "addr",
        Ok(TargetInfoQuery::VSock(_)) => "vsock",
        Ok(TargetInfoQuery::Usb(_)) => "usb",
        Ok(TargetInfoQuery::First) => "first",
        Err(_) => "invalid",
    }
}

/// Emit an event indicating how many devices were in the result.
pub async fn emit_device_stats_event(num_devices: usize, query: &Option<String>) {
    let query = query.as_ref().map_or("", |v| v);
    let _ = add_custom_event(
        Some("ffx_target_list_devices"),
        Some(query_type(query)),
        None,
        [("devices", (num_devices as u64).into())].into_iter().collect(),
    )
    .await;
} ///////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use addr::TargetAddr;
    use anyhow::Result;
    use ffx_command::FfxCommandLine;
    use ffx_config::environment::ExecutableKind;
    use ffx_list_args::AddressTypes;
    use ffx_target::info::{RemoteControlState, TargetState};
    use ffx_writer::TestBuffers;

    #[test]
    fn test_address_types_from_cmd() -> Result<()> {
        let cmd_none = ListCommand { no_ipv4: true, no_ipv6: true, ..Default::default() };
        assert_eq!(cmd_none.address_types(), AddressTypes::IP.complement());
        let cmd_ipv4_only = ListCommand { no_ipv4: false, no_ipv6: true, ..Default::default() };
        assert_eq!(cmd_ipv4_only.address_types(), AddressTypes::IPV6.complement());
        let cmd_ipv6_only = ListCommand { no_ipv4: true, no_ipv6: false, ..Default::default() };
        assert_eq!(cmd_ipv6_only.address_types(), AddressTypes::IPV4.complement());
        let cmd_all = ListCommand { no_ipv4: false, no_ipv6: false, ..Default::default() };
        assert_eq!(cmd_all.address_types(), AddressTypes::all());
        let cmd_all_default = ListCommand::default();
        assert_eq!(cmd_all_default.address_types(), AddressTypes::all());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_sorted_output() -> Result<()> {
        let cmd = ListCommand::default();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);
        let ti1 = TargetInfo {
            nodename: Some(String::from("z")),
            addresses: vec![],
            rcs_state: RemoteControlState::Unknown,
            target_state: TargetState::Unknown,
            ..Default::default()
        };
        let ti2 = TargetInfo { nodename: Some(String::from("a")), ..ti1.clone() };
        let infos = vec![ti1, ti2];
        show_targets(cmd, infos, &mut writer).await?;
        let out: Vec<String> =
            test_buffers.into_stdout_str().lines().map(|s| s.to_string()).collect();
        // Line 0 is the header
        assert!(out[1].starts_with("a"));
        assert!(out[2].starts_with("z"));
        Ok(())
    }

    async fn build_list_tool(
        cmd: ListCommand,
        env: &ffx_config::TestEnv,
        fho_env: fho::FhoEnvironment,
    ) -> ListTool {
        ListTool { cmd, fho_env, context: env.context.clone() }
    }

    #[fuchsia::test]
    async fn test_list_direct_uses_resolution() {
        let env = ffx_config::test_init().unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&env.context, &ffx_cmd_line);

        let resolution =
            ffx_target::Resolution::mock(|| Err(anyhow::anyhow!("MockConnectionError")));
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);

        let target_env = target_interface(&fho_env);
        target_env.set_behavior_for_test(behavior);

        let list_cmd = ListCommand::default();
        let tool = build_list_tool(list_cmd, &env, fho_env).await;

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());

        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Down);
    }

    #[fuchsia::test]
    async fn test_list_direct_timeout() {
        let env = ffx_config::test_env()
            .user_config(ffx_config::keys::DISCOVERY_TIMEOUT_MS, 100)
            .build()
            .unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&env.context, &ffx_cmd_line);

        let behavior =
            ConnectionBehavior::fake_direct_connector(ffx_target::Resolution::mock_async(|| {
                std::future::pending()
            }));
        let target_env = target_interface(&fho_env);
        target_env.set_behavior_for_test(behavior);

        let list_cmd = ListCommand::default();
        let tool = build_list_tool(list_cmd, &env, fho_env).await;

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());

        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Down);
    }

    #[fuchsia::test]
    async fn test_list_direct_short_circuits_when_not_probing() {
        let env = ffx_config::test_init().unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&env.context, &ffx_cmd_line);

        let behavior =
            ConnectionBehavior::fake_direct_connector(ffx_target::Resolution::mock(|| {
                panic!("Resolution should not be called when no_probe is true");
            }));
        let target_env = target_interface(&fho_env);
        target_env.set_behavior_for_test(behavior);

        let list_cmd = ListCommand { no_probe: true, ..Default::default() };
        let tool = build_list_tool(list_cmd, &env, fho_env).await;

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());

        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Unknown);
    }

    #[fuchsia::test]
    async fn test_list_direct_short_circuits_for_addresses_format() {
        let env = ffx_config::test_init().unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&env.context, &ffx_cmd_line);

        let behavior =
            ConnectionBehavior::fake_direct_connector(ffx_target::Resolution::mock(|| {
                panic!("Resolution should not be called when format is Addresses");
            }));
        let target_env = target_interface(&fho_env);
        target_env.set_behavior_for_test(behavior);

        let list_cmd =
            ListCommand { format: ffx_list_args::Format::Addresses, ..Default::default() };
        let tool = build_list_tool(list_cmd, &env, fho_env).await;

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());

        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Unknown);
    }

    #[test]
    fn test_list_error_conversion() {
        let err = ListError::ShowTargets(ShowTargetsError::DeviceNotFound("blarg".to_string()));
        let fho_err = fho::Error::from(err);
        assert_eq!(fho_err.to_string(), "Device blarg not found.");

        let err2 = ListError::ShowTargets(ShowTargetsError::NoAddressTypes);
        let fho_err2 = fho::Error::from(err2);
        assert_eq!(
            fho_err2.to_string(),
            "Invalid arguments, you must allow at least one address type"
        );

        let err3 = ListError::MissingSshPrivKey;
        let fho_err3 = fho::Error::from(err3);
        assert_eq!(
            fho_err3.to_string(),
            "Required argument `-c ssh.priv=` not specified. Unable to verify RCS state."
        );
    }

    #[fuchsia::test]
    async fn test_list_direct_strict_missing_ssh_priv() {
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, ffx_config::ConfigMap::new()).unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&context, &ffx_cmd_line);

        let list_cmd = ListCommand::default();
        let tool = ListTool { cmd: list_cmd, fho_env, context };

        let query = TargetInfoQuery::First;
        let res = tool.list_targets_direct(query).await;

        match res {
            Err(ListError::MissingSshPrivKey) => {}
            other => panic!("expected MissingSshPrivKey, got {other:?}"),
        }
    }

    #[fuchsia::test]
    async fn test_list_direct_strict_missing_ssh_priv_addr_query() {
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, ffx_config::ConfigMap::new()).unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&context, &ffx_cmd_line);

        let list_cmd = ListCommand::default();
        let tool = ListTool { cmd: list_cmd, fho_env, context };

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());
        let res = tool.list_targets_direct(query).await;

        match res {
            Err(ListError::MissingSshPrivKey) => {}
            other => panic!("expected MissingSshPrivKey, got {other:?}"),
        }
    }

    #[fuchsia::test]
    async fn test_list_direct_strict_with_ssh_priv() {
        let runtime_args =
            ffx_config::runtime::populate_runtime(&["ssh.priv=/path/to/key".to_string()], None)
                .unwrap();
        let context = EnvironmentContext::strict(ExecutableKind::Test, runtime_args).unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&context, &ffx_cmd_line);

        let resolution =
            ffx_target::Resolution::mock(|| Err(anyhow::anyhow!("MockConnectionError")));
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);

        let target_env = target_interface(&fho_env);
        target_env.set_behavior_for_test(behavior);

        let list_cmd = ListCommand::default();
        let tool = ListTool { cmd: list_cmd, fho_env, context };

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());
        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Down);
    }

    #[fuchsia::test]
    async fn test_list_direct_strict_no_probe_missing_ssh_priv() {
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, ffx_config::ConfigMap::new()).unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&context, &ffx_cmd_line);

        let list_cmd = ListCommand { no_probe: true, ..Default::default() };
        let tool = ListTool { cmd: list_cmd, fho_env, context };

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());
        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Unknown);
    }

    #[fuchsia::test]
    async fn test_list_direct_strict_format_addresses_missing_ssh_priv() {
        let context =
            EnvironmentContext::strict(ExecutableKind::Test, ffx_config::ConfigMap::new()).unwrap();
        let ffx_cmd_line = FfxCommandLine::default();
        let fho_env = fho::FhoEnvironment::new(&context, &ffx_cmd_line);

        let list_cmd =
            ListCommand { format: ffx_list_args::Format::Addresses, ..Default::default() };
        let tool = ListTool { cmd: list_cmd, fho_env, context };

        let query = TargetInfoQuery::Addr("127.0.0.1:8022".parse().unwrap());
        let res = tool.list_targets_direct(query).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].addresses, vec!["127.0.0.1:8022".parse::<TargetAddr>().unwrap()]);
        assert_eq!(res[0].rcs_state, RemoteControlState::Unknown);
    }
}
