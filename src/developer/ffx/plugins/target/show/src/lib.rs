// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::show::TargetData;
use async_trait::async_trait;
use fdomain_fuchsia_buildinfo::ProviderProxy;
use fdomain_fuchsia_feedback::{DeviceIdProviderProxy, LastRebootInfoProviderProxy};
use fdomain_fuchsia_hwinfo::{Architecture, BoardProxy, DeviceProxy, ProductProxy};
use fdomain_fuchsia_update_channel as fupdate_channel;
use fdomain_fuchsia_update_channelcontrol::ChannelControlProxy;
use ffx_target;
use ffx_target_show_args as args;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{Deferred, FfxMain, FfxTool, FhoEnvironment, deferred};
use netext::ScopedSocketAddr;
use show::{
    AddressData, BoardData, BuildData, DeviceData, ProductData, TargetShowInfo, UpdateData,
};
use target_behavior::{ConnectionBehavior, DirectConnector};
use target_holders::{RemoteControlProxyHolder, moniker};

mod show;

#[derive(FfxTool)]
#[main_error(ShowError)]
pub struct ShowTool {
    #[command]
    cmd: args::TargetShow,
    fho_env: FhoEnvironment,
    rcs_proxy: RemoteControlProxyHolder,
    #[with(moniker("/core/system-update"))]
    channel_provider_proxy: fupdate_channel::ProviderProxy,
    #[with(moniker("/core/system-update"))]
    channel_control_proxy: ChannelControlProxy,
    #[with(moniker("/core/hwinfo"))]
    board_proxy: BoardProxy,
    #[with(moniker("/core/hwinfo"))]
    device_proxy: DeviceProxy,
    #[with(moniker("/core/hwinfo"))]
    product_proxy: ProductProxy,
    #[with(moniker("/core/build-info"))]
    build_info_proxy: ProviderProxy,
    #[with(deferred(moniker("/core/feedback_id")))]
    device_id_proxy: Deferred<DeviceIdProviderProxy>,
    #[with(moniker("/core/feedback"))]
    last_reboot_info_proxy: LastRebootInfoProviderProxy,
}

use fho::FfxError;
use thiserror::Error;

#[derive(FfxError, Error, Debug)]
pub enum ShowError {
    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("FDomain client error: {0}")]
    Fdomain(#[from] fdomain_client::Error),

    #[exit_with_code(1)]
    #[error("FIDL error: {0}")]
    FidlError(#[from] fidl::Error),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[exit_with_code(1)]
    #[error("Failed to get ssh address from target proxy: timeout")]
    TargetSshAddressTimeout(#[from] timeout::TimeoutError),

    #[exit_with_code(1)]
    #[error("Failed to identify host via Remote Control Service: {0:?}")]
    RcsHostIdentification(fdomain_fuchsia_developer_remotecontrol::IdentifyHostError),

    #[exit_with_code(1)]
    #[error("Failed to resolve target connection resolution: {0}")]
    TargetResolution(#[source] target_behavior::TargetResolutionError),

    #[exit_with_code(1)]
    #[error("Failed to establish target connection: {0}")]
    TargetConnection(#[source] ffx_target::FfxTargetCrateError),

    #[transparent]
    #[error(transparent)]
    Fho(#[from] fho::Error),
}

fho::embedded_plugin!(ShowTool, ShowError);

#[async_trait(?Send)]
impl FfxMain for ShowTool {
    type Writer = VerifiedMachineWriter<TargetShowInfo>;
    type Error = ShowError;

    /// Main entry point for the `show` subcommand.
    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        self.show_cmd(&mut writer).await
    }
}

impl ShowTool {
    async fn show_cmd(
        self,
        writer: &mut VerifiedMachineWriter<TargetShowInfo>,
    ) -> Result<(), ShowError> {
        let behavior = target_behavior::target_interface(&self.fho_env).behavior()?;
        let ConnectionBehavior::Direct(ref connector) = *behavior;
        // To add more show information, add a gather_*_show(*) call to this list
        // and add the result to the TargetShowInfo struct below.
        let (target, board, device, product, update, build) = futures::try_join!(
            gather_target_show(
                self.rcs_proxy,
                &self.fho_env,
                connector.clone(),
                self.last_reboot_info_proxy
            ),
            gather_board_show(self.board_proxy),
            gather_device_show(self.device_proxy, self.device_id_proxy),
            gather_product_show(self.product_proxy),
            gather_update_show(self.channel_provider_proxy, self.channel_control_proxy),
            gather_build_info_show(self.build_info_proxy),
        )?;
        let show = TargetShowInfo { target, board, device, product, update, build };
        if writer.is_machine() {
            writer.machine(&show)?;
        } else {
            show::output_for_human(&show, &self.cmd, writer)?;
        }
        Ok(())
    }
}

async fn gather_target_info_direct(
    connection: &ffx_target::Connection,
) -> Result<Option<AddressData>, ShowError> {
    // If we've gotten a connection, we must have an address we connected to
    let ad = match connection.device_address() {
        Some(addr) => match ScopedSocketAddr::from_socket_addr(addr) {
            Ok(ssaddr) => Some(AddressData {
                host: format!("{}", ssaddr.ip_string()),
                port: ssaddr.addr().port(),
            }),
            Err(e) => {
                log::warn!("Failed to create ScopedSocketAddr from {}: {:?}", addr, e);
                None
            }
        },
        None => None,
    };
    Ok(ad)
}

/// Determine target information.
async fn gather_target_show(
    rcs_proxy: RemoteControlProxyHolder,
    fho_env: &FhoEnvironment,
    connector: DirectConnector,
    last_reboot_info_proxy: LastRebootInfoProviderProxy,
) -> Result<TargetData, ShowError> {
    let host = rcs_proxy.identify_host().await?.map_err(ShowError::RcsHostIdentification)?;
    let name = host.nodename;
    let ssh_address = gather_target_info_direct(
        &*connector
            .resolution()
            .await
            .map_err(ShowError::TargetResolution)?
            .get_connection(fho_env.environment_context())
            .await
            .map_err(ShowError::TargetConnection)?,
    )
    .await?;

    let info = last_reboot_info_proxy.get().await?;

    Ok(TargetData {
        name: name.unwrap_or_else(|| "".into()),
        ssh_address,
        last_reboot_graceful: info.graceful.unwrap_or(false),
        last_reboot_reason: info.reason.map(|r| format!("{r:?}")),
        uptime_nanos: info.uptime.unwrap_or(-1),
    })
}

/// Determine the build info for the target.
async fn gather_build_info_show(build: ProviderProxy) -> Result<BuildData, ShowError> {
    let info = build.get_build_info().await?;

    Ok(BuildData {
        version: info.version,
        product: info.product_config,
        board: info.board_config,
        commit: info.latest_commit_date,
    })
}

fn arch_to_string(arch: Option<Architecture>) -> Option<String> {
    match arch {
        Some(Architecture::X64) => Some("x64".to_string()),
        Some(Architecture::Arm64) => Some("arm64".to_string()),
        _ => None,
    }
}

/// Determine the device info for the device.
async fn gather_board_show(board: BoardProxy) -> Result<BoardData, ShowError> {
    let info = board.get_info().await?;
    Ok(BoardData {
        name: info.name,
        revision: info.revision,
        instruction_set: arch_to_string(info.cpu_architecture),
    })
}

/// Determine the device info for the device.
async fn gather_device_show(
    device: DeviceProxy,
    device_id_proxy: Deferred<DeviceIdProviderProxy>,
) -> Result<DeviceData, ShowError> {
    let info = device.get_info().await?;
    let mut device = DeviceData {
        serial_number: info.serial_number,
        retail_sku: info.retail_sku,
        retail_demo: info.is_retail_demo,
        device_id: None,
    };
    match device_id_proxy.await {
        Ok(device_id) => {
            let id_info = device_id.get_id().await?;
            device.device_id = Some(id_info)
        }
        Err(e) => {
            log::warn!("Error getting device id proxy: {e}");
            device.device_id = None;
        }
    };
    Ok(device)
}

/// Determine the product info for the device.
async fn gather_product_show(product: ProductProxy) -> Result<ProductData, ShowError> {
    let info = product.get_info().await?;

    Ok(ProductData {
        audio_amplifier: info.audio_amplifier,
        build_date: info.build_date,
        build_name: info.build_name,
        colorway: info.colorway,
        display: info.display,
        emmc_storage: info.emmc_storage,
        language: info.language,
        regulatory_domain: info.regulatory_domain.map(|d| d.country_code.unwrap_or_default()),
        locale_list: info
            .locale_list
            .map(|l| l.iter().map(|ll| ll.id.to_string()).collect())
            .unwrap_or(vec![]),
        manufacturer: info.manufacturer,
        microphone: info.microphone,
        model: info.model,
        name: info.name,
        nand_storage: info.nand_storage,
        memory: info.memory,
        sku: info.sku,
    })
}

/// Determine the update show of the device, including update channels.
async fn gather_update_show(
    channel_provider: fupdate_channel::ProviderProxy,
    channel_control: ChannelControlProxy,
) -> Result<UpdateData, ShowError> {
    let current_channel = channel_provider.get_current().await?;
    let next_channel = match channel_control.get_target().await {
        Ok(channel) => Some(channel),
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx_status::Status::NOT_FOUND =>
        {
            None
        }
        Err(e) => Err(e)?,
    };

    Ok(UpdateData { current_channel, next_channel })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_buildinfo::{BuildInfo, ProviderRequest};

    use fdomain_fuchsia_feedback::{
        DeviceIdProviderRequest, LastReboot, LastRebootInfoProviderRequest, RebootReason,
    };
    use fdomain_fuchsia_hwinfo::{
        BoardInfo, BoardRequest, DeviceInfo, DeviceRequest, ProductInfo, ProductRequest,
    };
    use fdomain_fuchsia_intl::RegulatoryDomain;
    use fdomain_fuchsia_update_channelcontrol::ChannelControlRequest;
    use ffx_writer::{Format, TestBuffers};
    use serde_json::Value;
    use std::sync::Arc;
    use target_holders::fake_proxy;

    const TEST_OUTPUT_HUMAN: &'static str = "\
        Target: \
        \n    Name: \u{1b}[38;5;2m\"fake_fuchsia_device\"\u{1b}[m\
        \n    SSH Address: \u{1b}[38;5;2m\"127.0.0.1:22\"\u{1b}[m\
        \n    Last Reboot Graceful: \"true\"\
        \n    Last Reboot Reason: \"ZbiSwap\"\
        \n    Uptime (ns): \"65000\"\
        \nBoard: \
        \n    Name: \"fake_name\"\
        \n    Revision: \"fake_revision\"\
        \n    Instruction set: \"x64\"\
        \nDevice: \
        \n    Serial number: \"fake_serial\"\
        \n    Retail SKU: \"fake_sku\"\
        \n    Is retail demo: false\
        \n    Device ID: \"fake_device_id\"\
        \nProduct: \
        \n    Audio amplifier: \"fake_audio_amplifier\"\
        \n    Build date: \"fake_build_date\"\
        \n    Build name: \"fake_build_name\"\
        \n    Colorway: \"fake_colorway\"\
        \n    Display: \"fake_display\"\
        \n    EMMC storage: \"fake_emmc_storage\"\
        \n    Language: \"fake_language\"\
        \n    Regulatory domain: \"fake_regulatory_domain\"\
        \n    Locale list: []\
        \n    Manufacturer: \"fake_manufacturer\"\
        \n    Microphone: \"fake_microphone\"\
        \n    Model: \"fake_model\"\
        \n    Name: \"fake_name\"\
        \n    NAND storage: \"fake_nand_storage\"\
        \n    Memory: \"fake_memory\"\
        \n    SKU: \"fake_sku\"\
        \nUpdate: \
        \n    Current channel: \"fake_channel\"\
        \n    Next channel: \"fake_target\"\
        \nBuild: \
        \n    Version: \"fake_version\"\
        \n    Product: \"fake_product\"\
        \n    Board: \"fake_board\"\
        \n    Commit: \"fake_commit\"\
        \n";

    fn setup_fake_device_id_server(client: Arc<fdomain_client::Client>) -> DeviceIdProviderProxy {
        fake_proxy(client, move |req| match req {
            DeviceIdProviderRequest::GetId { responder } => {
                responder.send("fake_device_id").unwrap();
            }
        })
    }

    fn setup_fake_build_info_server(client: Arc<fdomain_client::Client>) -> ProviderProxy {
        fake_proxy(client, move |req| match req {
            ProviderRequest::GetBuildInfo { responder } => {
                responder
                    .send(&BuildInfo {
                        version: Some("fake_version".to_string()),
                        product_config: Some("fake_product".to_string()),
                        board_config: Some("fake_board".to_string()),
                        latest_commit_date: Some("fake_commit".to_string()),
                        ..Default::default()
                    })
                    .unwrap();
            }
        })
    }

    fn setup_fake_board_server(client: Arc<fdomain_client::Client>) -> BoardProxy {
        fake_proxy(client, move |req| match req {
            BoardRequest::GetInfo { responder } => {
                responder
                    .send(&BoardInfo {
                        name: Some("fake_name".to_string()),
                        revision: Some("fake_revision".to_string()),
                        cpu_architecture: Some(Architecture::X64),
                        ..Default::default()
                    })
                    .unwrap();
            }
        })
    }

    fn setup_fake_last_reboot_info_server(
        client: Arc<fdomain_client::Client>,
    ) -> LastRebootInfoProviderProxy {
        fake_proxy(client, move |req| match req {
            LastRebootInfoProviderRequest::Get { responder } => {
                responder
                    .send(&LastReboot {
                        graceful: Some(true),
                        uptime: Some(65000),
                        reason: Some(RebootReason::ZbiSwap),
                        ..Default::default()
                    })
                    .unwrap();
            }
        })
    }

    #[fuchsia::test]
    async fn test_show_cmd_impl() {
        let client = fdomain_local::local_client_empty();
        let buffers = TestBuffers::default();
        let output = VerifiedMachineWriter::<TargetShowInfo>::new_test(None, &buffers);
        let fho_env = FhoEnvironment::default();
        let target_env = target_behavior::target_interface(&fho_env);
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(
            target_behavior::setup_fake_resolution(None).await,
        ));
        let tool = ShowTool {
            cmd: args::TargetShow { ..Default::default() },
            fho_env,
            rcs_proxy: testing_lib::setup_fake_rcs(
                Arc::clone(&client),
                testing_lib::FakeRcsConfig::default(),
            )
            .into(),
            channel_provider_proxy: setup_fake_channel_provider_server(Arc::clone(&client)),
            channel_control_proxy: setup_fake_channel_control_server(Arc::clone(&client)),
            board_proxy: setup_fake_board_server(Arc::clone(&client)),
            device_proxy: setup_fake_device_server(Arc::clone(&client)),
            product_proxy: setup_fake_product_server(Arc::clone(&client)),
            build_info_proxy: setup_fake_build_info_server(Arc::clone(&client)),
            device_id_proxy: Deferred::from_output(Ok(setup_fake_device_id_server(Arc::clone(
                &client,
            )))),
            last_reboot_info_proxy: setup_fake_last_reboot_info_server(Arc::clone(&client)),
        };
        tool.main(output).await.expect("show tool main");
        // Convert to a readable string instead of using a byte string and comparing that. Unless
        // you can read u8 arrays well, this helps debug the output.
        let (stdout, _stderr) = buffers.into_strings();
        // Test line by line so it is easier to debug:
        let mut lineno = 0;
        let mut expected_iter = TEST_OUTPUT_HUMAN.lines().into_iter();
        for actual in stdout.lines() {
            lineno += 1;
            if let Some(expected) = expected_iter.next() {
                assert_eq!(
                    actual, expected,
                    "line {lineno} actual != expected {actual} vs. {expected}"
                )
            }
        }
        let remaining: Vec<&str> = expected_iter.collect();
        assert!(remaining.is_empty(), "Missing lines from actual input: {remaining:?}");
    }

    #[fuchsia::test]
    async fn test_gather_board_show() {
        let client = fdomain_local::local_client_empty();
        let test_proxy = setup_fake_board_server(client);
        let result = gather_board_show(test_proxy).await.expect("gather board show");
        assert_eq!(result.name, Some("fake_name".to_string()));
        assert_eq!(result.revision, Some("fake_revision".to_string()));
    }

    fn setup_fake_device_server(client: Arc<fdomain_client::Client>) -> DeviceProxy {
        fake_proxy(client, move |req| match req {
            DeviceRequest::GetInfo { responder } => {
                responder
                    .send(&DeviceInfo {
                        serial_number: Some("fake_serial".to_string()),
                        is_retail_demo: Some(false),
                        retail_sku: Some("fake_sku".to_string()),
                        ..Default::default()
                    })
                    .unwrap();
            }
        })
    }

    #[fuchsia::test]
    async fn test_gather_device_show() {
        let client = fdomain_local::local_client_empty();
        let test_proxy = setup_fake_device_server(Arc::clone(&client));
        let device_id_proxy = Deferred::from_output(Ok(setup_fake_device_id_server(client)));
        let result =
            gather_device_show(test_proxy, device_id_proxy).await.expect("gather device show");
        assert_eq!(result.serial_number, Some("fake_serial".to_string()));
        assert_eq!(result.retail_sku, Some("fake_sku".to_string()));
        assert_eq!(result.retail_demo, Some(false))
    }

    fn setup_fake_product_server(client: Arc<fdomain_client::Client>) -> ProductProxy {
        fake_proxy(client, move |req| match req {
            ProductRequest::GetInfo { responder } => {
                responder
                    .send(&ProductInfo {
                        sku: Some("fake_sku".to_string()),
                        language: Some("fake_language".to_string()),
                        regulatory_domain: Some(RegulatoryDomain {
                            country_code: Some("fake_regulatory_domain".to_string()),
                            ..Default::default()
                        }),
                        locale_list: Some(vec![]),
                        name: Some("fake_name".to_string()),
                        audio_amplifier: Some("fake_audio_amplifier".to_string()),
                        build_date: Some("fake_build_date".to_string()),
                        build_name: Some("fake_build_name".to_string()),
                        colorway: Some("fake_colorway".to_string()),
                        display: Some("fake_display".to_string()),
                        emmc_storage: Some("fake_emmc_storage".to_string()),
                        manufacturer: Some("fake_manufacturer".to_string()),
                        memory: Some("fake_memory".to_string()),
                        microphone: Some("fake_microphone".to_string()),
                        model: Some("fake_model".to_string()),
                        nand_storage: Some("fake_nand_storage".to_string()),
                        ..Default::default()
                    })
                    .unwrap();
            }
        })
    }

    #[fuchsia::test]
    async fn test_gather_product_show() {
        let client = fdomain_local::local_client_empty();
        let test_proxy = setup_fake_product_server(client);
        let result = gather_product_show(test_proxy).await.expect("gather product show");
        assert_eq!(result.audio_amplifier, Some("fake_audio_amplifier".to_string()));
        assert_eq!(result.build_date, Some("fake_build_date".to_string()));
        assert_eq!(result.name, Some("fake_name".to_string()));
        assert_eq!(result.build_name, Some("fake_build_name".to_string()));
        assert_eq!(result.colorway, Some("fake_colorway".to_string()));
    }

    fn setup_fake_channel_provider_server(
        client: Arc<fdomain_client::Client>,
    ) -> fupdate_channel::ProviderProxy {
        fake_proxy(client, move |req| match req {
            fupdate_channel::ProviderRequest::GetCurrent { responder } => {
                responder.send("fake_channel").unwrap();
            }
        })
    }

    fn setup_fake_channel_control_server(
        client: Arc<fdomain_client::Client>,
    ) -> ChannelControlProxy {
        fake_proxy(client, move |req| match req {
            ChannelControlRequest::GetTarget { responder } => {
                responder.send("fake_target").unwrap();
            }
            req => panic!("unexpected request {req:?}"),
        })
    }

    #[fuchsia::test]
    async fn test_gather_update_show() {
        let client = fdomain_local::local_client_empty();
        let provider_proxy = setup_fake_channel_provider_server(client.clone());
        let control_proxy = setup_fake_channel_control_server(client);
        let result =
            gather_update_show(provider_proxy, control_proxy).await.expect("gather update show");
        assert_eq!(result.current_channel, "fake_channel".to_string());
        assert_eq!(result.next_channel, Some("fake_target".to_string()));
    }

    #[fuchsia::test]
    async fn test_arch_to_string() {
        assert_eq!(arch_to_string(Some(Architecture::X64)), Some("x64".to_string()));
        assert_eq!(arch_to_string(Some(Architecture::Arm64)), Some("arm64".to_string()));
        assert_eq!(arch_to_string(None), None);
    }

    #[fuchsia::test]
    async fn test_verify_machine_schema() {
        let client = fdomain_local::local_client_empty();
        let buffers = TestBuffers::default();
        let mut output =
            VerifiedMachineWriter::<TargetShowInfo>::new_test(Some(Format::JsonPretty), &buffers);
        let fho_env = FhoEnvironment::default();
        let target_env = target_behavior::target_interface(&fho_env);
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(
            target_behavior::setup_fake_resolution(None).await,
        ));
        let tool = ShowTool {
            cmd: args::TargetShow { ..Default::default() },
            fho_env,
            rcs_proxy: testing_lib::setup_fake_rcs(
                Arc::clone(&client),
                testing_lib::FakeRcsConfig::default(),
            )
            .into(),
            channel_provider_proxy: setup_fake_channel_provider_server(Arc::clone(&client)),
            channel_control_proxy: setup_fake_channel_control_server(Arc::clone(&client)),
            board_proxy: setup_fake_board_server(Arc::clone(&client)),
            device_proxy: setup_fake_device_server(Arc::clone(&client)),
            product_proxy: setup_fake_product_server(Arc::clone(&client)),
            build_info_proxy: setup_fake_build_info_server(Arc::clone(&client)),
            device_id_proxy: Deferred::from_output(Ok(setup_fake_device_id_server(Arc::clone(
                &client,
            )))),
            last_reboot_info_proxy: setup_fake_last_reboot_info_server(Arc::clone(&client)),
        };
        tool.show_cmd(&mut output).await.expect("main");
        let (stdout, _stderr) = buffers.into_strings();
        let data: Value = serde_json::from_str(&stdout).expect("Valid JSON");
        match VerifiedMachineWriter::<TargetShowInfo>::verify_schema(&data) {
            Ok(_) => (),
            Err(e) => {
                println!("Error verifying schema: {e}");
                println!("{data:?}");
            }
        };
    }

    #[fuchsia::test]
    async fn test_show_cmd_impl_direct_connection() {
        let client = fdomain_local::local_client_empty();
        let buffers = TestBuffers::default();
        let output = VerifiedMachineWriter::<TargetShowInfo>::new_test(None, &buffers);
        let fho_env = FhoEnvironment::default();
        let target_env = target_behavior::target_interface(&fho_env);
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(
            target_behavior::setup_fake_resolution(None).await,
        ));
        let tool = ShowTool {
            cmd: args::TargetShow { ..Default::default() },
            fho_env,
            rcs_proxy: testing_lib::setup_fake_rcs(
                Arc::clone(&client),
                testing_lib::FakeRcsConfig::default(),
            )
            .into(),
            channel_provider_proxy: setup_fake_channel_provider_server(Arc::clone(&client)),
            channel_control_proxy: setup_fake_channel_control_server(Arc::clone(&client)),
            board_proxy: setup_fake_board_server(Arc::clone(&client)),
            device_proxy: setup_fake_device_server(Arc::clone(&client)),
            product_proxy: setup_fake_product_server(Arc::clone(&client)),
            build_info_proxy: setup_fake_build_info_server(Arc::clone(&client)),
            device_id_proxy: Deferred::from_output(Ok(setup_fake_device_id_server(Arc::clone(
                &client,
            )))),
            last_reboot_info_proxy: setup_fake_last_reboot_info_server(Arc::clone(&client)),
        };
        tool.main(output).await.expect("show tool main");
        // Convert to a readable string instead of using a byte string and comparing that. Unless
        // you can read u8 arrays well, this helps debug the output.
        let (stdout, _stderr) = buffers.into_strings();
        // Test line by line so it is easier to debug:
        let mut lineno = 0;
        let mut expected_iter = TEST_OUTPUT_HUMAN.lines().into_iter();
        for actual in stdout.lines() {
            lineno += 1;
            if let Some(expected) = expected_iter.next() {
                assert_eq!(
                    actual, expected,
                    "line {lineno} actual != expected {actual} vs. {expected}"
                )
            }
        }
        let remaining: Vec<&str> = expected_iter.collect();
        assert!(remaining.is_empty(), "Missing lines from actual input: {remaining:?}");
    }

    #[fuchsia::test]
    async fn test_show_rcs_host_identification_error() {
        let client = fdomain_local::local_client_empty();
        let buffers = TestBuffers::default();
        let output = VerifiedMachineWriter::<TargetShowInfo>::new_test(None, &buffers);
        let fho_env = FhoEnvironment::default();
        let target_env = target_behavior::target_interface(&fho_env);
        target_env.set_behavior_for_test(ConnectionBehavior::fake_direct_connector(
            target_behavior::setup_fake_resolution(None).await,
        ));

        let rcs_proxy = testing_lib::setup_fake_rcs(
            Arc::clone(&client),
            testing_lib::FakeRcsConfig {
                identify_host_handler: Some(std::rc::Rc::new(move |responder| {
                    responder
                        .send(Err(
                            fdomain_fuchsia_developer_remotecontrol::IdentifyHostError::ListInterfacesFailed,
                        ))
                        .unwrap();
                })),
                ..Default::default()
            },
        )
        .into();

        let tool = ShowTool {
            cmd: args::TargetShow { ..Default::default() },
            fho_env,
            rcs_proxy,
            channel_provider_proxy: setup_fake_channel_provider_server(Arc::clone(&client)),
            channel_control_proxy: setup_fake_channel_control_server(Arc::clone(&client)),
            board_proxy: setup_fake_board_server(Arc::clone(&client)),
            device_proxy: setup_fake_device_server(Arc::clone(&client)),
            product_proxy: setup_fake_product_server(Arc::clone(&client)),
            build_info_proxy: setup_fake_build_info_server(Arc::clone(&client)),
            device_id_proxy: Deferred::from_output(Ok(setup_fake_device_id_server(Arc::clone(
                &client,
            )))),
            last_reboot_info_proxy: setup_fake_last_reboot_info_server(Arc::clone(&client)),
        };
        let res = tool.main(output).await;
        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err(),
            ShowError::RcsHostIdentification(
                fdomain_fuchsia_developer_remotecontrol::IdentifyHostError::ListInterfacesFailed
            )
        ));
    }
}
