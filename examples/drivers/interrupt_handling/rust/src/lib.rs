// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::pin::Pin;

use anyhow::Context;
use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fidl_fuchsia_hardware_gpio::{InterruptOptions, ServiceMarker};
use fuchsia_async::{OnInterrupt, Task};
use futures::StreamExt;
use log::{error, info};
use zx::{Interrupt, Status};

struct InterruptHandlingDriver {
    _node: Node,

    /// Hold onto the task handle to keep the interrupt listener running. If
    /// the handle is dropped, the task is cancelled and the driver will stop
    /// receiving interrupts.
    _interrupt_handler: Task<()>,
}

impl InterruptHandlingDriver {
    /// Returns the interrupt associated with the parent GPIO FIDL service
    /// found in `context`.
    async fn get_interrupt(context: &DriverContext) -> anyhow::Result<Interrupt> {
        let gpio_client = context
            .incoming
            .service_marker(ServiceMarker)
            .connect()
            .context("Failed to connect to GPIO service")?
            .connect_to_device()
            .context("Failed to connect to device")?;

        let response = gpio_client
            .get_interrupt(InterruptOptions::default())
            .await
            .context("Failed to call get_interrupt")?;

        let handle =
            response.map_err(Status::err_from_raw).context("GPIO returned error status")?;

        Ok(Interrupt::from(handle))
    }

    /// Creates a task that listens for interrupts from `interrupt`.
    fn create_interrupt_handler(interrupt: Interrupt) -> Task<()> {
        // We use Box::pin because OnInterrupt is not Unpin.
        let mut interrupt_stream = Box::pin(OnInterrupt::new(interrupt));

        // We use `Task::local` instead of `Task::spawn` because driver hosts
        // typically run on a single-threaded executor, and `Task::local`
        // avoids the requirement for the future to be `Send`.
        Task::local(async move {
            while let Some(Ok(_time)) = interrupt_stream.as_mut().next().await {
                info!("Received interrupt!");

                // IMPORTANT: Acknowledge the interrupt.
                // Failing to call ack() will prevent future interrupts from triggering.
                if let Err(e) = Pin::get_ref(interrupt_stream.as_ref()).as_ref().ack() {
                    error!("Failed to ack interrupt: {:?}", e);
                }
            }
        })
    }
}

impl Driver for InterruptHandlingDriver {
    const NAME: &str = "interrupt_handling_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node().map_err(DriverError::Status)?;

        let interrupt = Self::get_interrupt(&context).await.map_err(|e| {
            error!("Failed to request GPIO interrupt: {:?}", e);
            DriverError::Anyhow(e)
        })?;

        let interrupt_handler = Self::create_interrupt_handler(interrupt);

        Ok(Self { _node: node, _interrupt_handler: interrupt_handler })
    }

    async fn stop(&self) {
        info!("InterruptHandlingDriver::stop() was invoked.");
    }
}

driver_register!(InterruptHandlingDriver);
