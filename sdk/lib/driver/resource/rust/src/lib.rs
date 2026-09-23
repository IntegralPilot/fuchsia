// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]
//! Helper library for connecting to common driver resources using `fidl_next`.

use fdf_component::{DriverContext, DriverError};
use fidl_next::Client;
use fidl_next_fuchsia_hardware_clock as fclock;
use fidl_next_fuchsia_hardware_gpio as fgpio;
use fidl_next_fuchsia_hardware_i2c as fi2c;
use fidl_next_fuchsia_hardware_mailbox as fmailbox;
use fidl_next_fuchsia_hardware_pci as fpci;
use fidl_next_fuchsia_hardware_reset as freset;
use fidl_next_fuchsia_hardware_spi as fspi;

/// Extension trait for [`DriverContext`] to simplify connecting to clock resources.
pub trait ClockExt {
    /// Connects to a clock resource with the given instance name.
    fn connect_to_clock(&self, instance: &str) -> Result<Client<fclock::Clock>, DriverError>;
}

impl ClockExt for DriverContext {
    fn connect_to_clock(&self, instance: &str) -> Result<Client<fclock::Clock>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fclock::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.clock(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to GPIO resources.
pub trait GpioExt {
    /// Connects to a GPIO resource with the given instance name.
    fn connect_to_gpio(&self, instance: &str) -> Result<Client<fgpio::Gpio>, DriverError>;
}

impl GpioExt for DriverContext {
    fn connect_to_gpio(&self, instance: &str) -> Result<Client<fgpio::Gpio>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fgpio::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.device(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to I2C resources.
pub trait I2cExt {
    /// Connects to an I2C resource with the given instance name.
    fn connect_to_i2c(&self, instance: &str) -> Result<Client<fi2c::Device>, DriverError>;
}

impl I2cExt for DriverContext {
    fn connect_to_i2c(&self, instance: &str) -> Result<Client<fi2c::Device>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fi2c::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.device(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to mailbox resources.
pub trait MailboxExt {
    /// Connects to a mailbox resource with the given instance name.
    fn connect_to_mailbox(&self, instance: &str) -> Result<Client<fmailbox::Channel>, DriverError>;
}

impl MailboxExt for DriverContext {
    fn connect_to_mailbox(&self, instance: &str) -> Result<Client<fmailbox::Channel>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fmailbox::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.channel(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to reset resources.
pub trait ResetExt {
    /// Connects to a reset resource with the given instance name.
    fn connect_to_reset(&self, instance: &str) -> Result<Client<freset::Reset>, DriverError>;
}

impl ResetExt for DriverContext {
    fn connect_to_reset(&self, instance: &str) -> Result<Client<freset::Reset>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<freset::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.reset(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to SPI resources.
pub trait SpiExt {
    /// Connects to a SPI resource with the given instance name.
    fn connect_to_spi(&self, instance: &str) -> Result<Client<fspi::Device>, DriverError>;
}

impl SpiExt for DriverContext {
    fn connect_to_spi(&self, instance: &str) -> Result<Client<fspi::Device>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fspi::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.device(server)?;
        Ok(client.spawn())
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to PCI
/// resources.
pub trait PciExt {
    /// Connects to a PCI resource with the given instance name.
    fn connect_to_pci(&self, instance: &str) -> Result<Client<fpci::Device>, DriverError>;
}

impl PciExt for DriverContext {
    fn connect_to_pci(&self, instance: &str) -> Result<Client<fpci::Device>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fpci::Service>>()
            .instance(instance)
            .connect_next()?;
        let (client, server) = fidl_next::fuchsia::create_channel();
        service.device(server)?;
        Ok(client.spawn())
    }
}
