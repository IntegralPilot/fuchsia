// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fdf_metadata::MetadataServer;
use fidl::Serializable;
use fspi_businfo::SpiBusMetadata;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::{error, info};
use pdev::{PdevExt, PlatformDevice};
use serde::Deserialize;
use zx::Status;

use fdf_power::PowerExt;
use fdf_resource::{ClockExt, GpioExt, ResetExt};

use fidl_next::ServerEnd;
use fidl_next::util::{Multiserver, multiserver};
use fidl_next_fuchsia_hardware_spiimpl as fspi_impl;

use fidl_next_fuchsia_hardware_platform_device as fpdev;

use fidl_fuchsia_hardware_spi_businfo as fspi_businfo;
use fidl_next_fuchsia_hardware_clock::ClockGetRateResponse;

mod spi_device;
use spi_device::DwSpiDevice;

#[derive(Deserialize, Debug, PartialEq)]
struct DwSpiConfig {
    dw_spi_rx_sample_delay_ns: u64,
}

struct SpiImplService {
    server: Multiserver<fspi_impl::SpiImpl>,
}

impl fspi_impl::ServiceHandler for SpiImplService {
    fn device(&self, server_end: ServerEnd<fspi_impl::SpiImpl>) {
        let _ = self.server.forward(server_end);
    }
}

struct DwSpiDriver {
    _node: Node,
    _scope: fasync::Scope,
    _businfo_server: Option<MetadataServer>,
}

driver_register!(DwSpiDriver);

impl DwSpiDriver {
    async fn get_max_bus_clock(pdev: &fidl_next::Client<fpdev::Device>) -> u64 {
        if let Ok(metadata) = pdev.get_typed_metadata::<SpiBusMetadata>().await {
            let channels = metadata.channels.unwrap_or(vec![]);
            channels.into_iter().filter_map(|c| c.max_frequency_hz).min().unwrap_or(0) as u64
        } else {
            0
        }
    }
}

impl Driver for DwSpiDriver {
    const NAME: &str = "dw-spi";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let powerdomain = context.connect_to_powerdomain("power-domain")?;
        powerdomain.enable().await?.map_err(|s| {
            anyhow::Error::new(s.err().unwrap_or(Status::INTERNAL))
                .context("Failed to enable power domain")
        })?;

        let clock_bus = context.connect_to_clock("bus")?;
        clock_bus.enable().await?.map_err(|s| {
            anyhow::Error::new(s.err().unwrap_or(Status::INTERNAL))
                .context("Failed to enable bus clock")
        })?;

        let parent_clock_hz = clock_bus
            .get_rate()
            .await
            .map_err(|_| Status::INTERNAL)
            .and_then(|res| res.map_err(|_| Status::INTERNAL))
            .inspect_err(|e| {
                error!("Failed to get bus clock rate: {e:?}");
            })
            .unwrap_or(ClockGetRateResponse { hz: 0 })
            .hz;

        let clock_regs = context.connect_to_clock("registers")?;
        clock_regs.enable().await?.map_err(|s| {
            anyhow::Error::new(s.err().unwrap_or(Status::INTERNAL))
                .context("Failed to enable registers clock")
        })?;

        let reset = context.connect_to_reset("reset")?;
        reset.toggle().await?.map_err(|s| {
            anyhow::Error::new(s.err().unwrap_or(Status::INTERNAL))
                .context("Failed to toggle reset")
        })?;

        let cs_gpio = {
            let cs_gpio = context.connect_to_gpio("cs-0")?;

            // The chip select GPIO is optional. Make a call on it do determine whether or not it
            // has been provided to us.
            match cs_gpio.release_interrupt().await {
                Ok(_) => Some(cs_gpio),
                _ => None,
            }
        };

        let pdev = context.connect_to_pdev()?;
        let interrupt = pdev
            .get_interrupt_by_id(0, 0)
            .await?
            .map_err(|s| {
                anyhow::Error::new(s.err().unwrap_or(Status::INTERNAL))
                    .context("Failed to get interrupt")
            })?
            .irq;
        let mut device = DwSpiDevice::new(pdev.map_mmio_by_id(0).await?, cs_gpio, interrupt);

        let max_bus_clock_hz = DwSpiDriver::get_max_bus_clock(&pdev).await;
        let config: DwSpiConfig = pdev
            .get_deserialized_metadata()
            .await
            .inspect_err(|e| {
                info!("dw-spi config was not provided ({e})");
            })
            .unwrap_or(DwSpiConfig { dw_spi_rx_sample_delay_ns: 0 });

        device.init_registers(
            parent_clock_hz,
            max_bus_clock_hz,
            config.dw_spi_rx_sample_delay_ns,
        )?;

        let mut outgoing = ServiceFs::new();

        let scope = fasync::Scope::new_with_name(Self::NAME);

        let (server, dispatcher) = multiserver();

        let offer = ServiceOffer::<fspi_impl::Service>::new_next()
            .add_default_named_next(&mut outgoing, "default", SpiImplService { server })
            .build_driver_offer();

        let mut node_args = NodeBuilder::new(Self::NAME).add_offer(offer);

        let businfo_server = MetadataServer::new(SpiBusMetadata::SERIALIZABLE_NAME)
            .forward_from_pdev_next(&pdev)
            .await;
        match businfo_server {
            Ok(ref server) => {
                if let Some(offer) = server.serve(&mut outgoing, scope.to_handle(), "default") {
                    node_args = node_args.add_offer(offer);
                }
            }
            // Metadata must either be provided or not provided; other results are considered fatal
            // errors.
            Err(e) if e == Status::NOT_FOUND => {}
            Err(e) => {
                error!("Failed to forward SPI bus metadata: {e}");
                return Err(e.into());
            }
        }

        let node = context.take_node()?;
        node.add_child(node_args.build()).await?;

        context.serve_outgoing(&mut outgoing)?;
        scope.spawn(outgoing.collect());

        scope.spawn_local(dispatcher.run(device));

        info!("dw-spi driver initialized successfully");

        Ok(Self { _node: node, _scope: scope, _businfo_server: businfo_server.ok() })
    }

    async fn stop(&self) {}
}

#[cfg(test)]
mod tests;
