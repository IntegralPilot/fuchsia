// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use dml_config::BoardConfig;
use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fdf_fidl::DriverChannel;
use fidl_next_fuchsia_driver_framework as fdf_framework;
use fidl_next_fuchsia_hardware_platform_bus as fpbus;
use log::info;

use fidl_fuchsia_io as fio;

use anyhow::Context;
use fidl::Status;

use dml_config::parser::{DmlParserConfig, publish_dml_devices};

struct GenericBoardDriver {
    _node: Node,
    _pbus: fidl_next::Client<fpbus::PlatformBus, DriverChannel>,
}

driver_register!(GenericBoardDriver);

impl Driver for GenericBoardDriver {
    const NAME: &str = "generic-board";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("Starting generic-board driver");
        let node = context.take_node()?;

        // Read config from package
        let file = fuchsia_component::directory::open_file_async(
            &context.incoming,
            "pkg/config/board-config.fidl",
            fio::Rights::READ_BYTES,
        )
        .context("Failed to open board config file")?;
        let board_config_bytes =
            fuchsia_fs::file::read(&file).await.context("Failed to read board config file")?;
        let board_config = fidl::unpersist::<BoardConfig>(&board_config_bytes)
            .context("Failed to deserialize board config FIDL")?;
        info!("Loaded board config: {:?}", board_config);

        // Connect to PlatformBus service.
        let service = context
            .incoming
            .service::<fdf_component::ServiceInstance<fpbus::Service>>()
            .connect_next()
            .context("Failed to connect to PlatformBus service")?;

        let (client_end, server_end) = fdf_fidl::create_channel::<fpbus::PlatformBus>();
        service.platform_bus(server_end).context("Failed to connect to platform_bus member")?;

        let pbus = client_end.spawn();

        let composite_manager_client = context
            .incoming
            .connect_protocol_next::<fdf_framework::CompositeNodeManager>()
            .context("Failed to connect to CompositeNodeManager")?;
        let composite_manager = composite_manager_client.spawn();

        let board_info = pbus
            .get_board_info()
            .await
            .context("Failed to call GetBoardInfo")?
            .map_err(|e| e.err().unwrap_or(Status::INTERNAL))
            .context("GetBoardInfo returned error")?;
        info!("Board info: {:?}", board_info);

        publish_dml_devices(
            &pbus,
            &composite_manager,
            &board_config,
            &GENERIC_BOARD_PARSER_CONFIG,
            None,
        )
        .await
        .context("Failed to publish DML devices")?;

        info!("Generic board driver started successfully");
        Ok(Self { _node: node, _pbus: pbus })
    }

    async fn stop(&self) {}
}

static GENERIC_BOARD_PARSER_CONFIG: DmlParserConfig =
    DmlParserConfig { service_configs: phf::phf_map! {} };
