// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod input_device;
mod keyboard;
mod mouse;
mod wire;

use crate::input_device::{InputDevice, InputHandler};
use crate::keyboard::KeyboardDevice;
use crate::mouse::MouseDevice;
use anyhow::{anyhow, Context};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_virtualization_hardware::{InputType, VirtioInputRequestStream};
use fuchsia_component::server;
use futures::{StreamExt, TryFutureExt, TryStreamExt};

async fn run_virtio_input(
    mut virtio_input_fidl: VirtioInputRequestStream,
) -> Result<(), anyhow::Error> {
    // Receive start info as first message.
    let (start_info, input_type, responder) = virtio_input_fidl
        .try_next()
        .await?
        .ok_or_else(|| anyhow!("Failed to read fidl message from the channel."))?
        .into_start()
        .ok_or_else(|| anyhow!("Start should be the first message sent."))?;

    // Prepare the device builder
    let (device_builder, guest_mem) = machina_virtio_device::from_start_info(start_info)?;

    responder.send()?;

    // Complete the setup of queues and get a device.
    let mut virtio_device_fidl = virtio_input_fidl.cast_stream();
    let (device, ready_responder) = machina_virtio_device::config_builder_from_stream(
        device_builder,
        &mut virtio_device_fidl,
        &[wire::EVENTQ, wire::STATUSQ][..],
        &guest_mem,
    )
    .await
    .context("Failed to initialize device.")?;

    let input_device = InputDevice::new(
        &guest_mem,
        device.take_stream(wire::EVENTQ)?,
        device.take_stream(wire::STATUSQ).ok(),
    );

    let mut input_handler: Box<dyn InputHandler> = match input_type {
        InputType::Keyboard(keyboard_listener) => {
            Box::new(KeyboardDevice::new(input_device, keyboard_listener.into_stream()))
        }
        InputType::Mouse(mouse_source) => {
            Box::new(MouseDevice::new(input_device, mouse_source.into_proxy()))
        }
    };

    ready_responder.send()?;

    futures::try_join!(
        device
            .run_device_notify(virtio_device_fidl)
            .map_err(|e| anyhow!("run_device_notify: {}", e)),
        input_handler.run(),
    )?;
    Ok(())
}

enum IncomingService {
    VirtioInput(VirtioInputRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), anyhow::Error> {
    let mut fs = server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingService::VirtioInput);
    fs.take_and_serve_directory_handle().context("Error starting server")?;

    // Only support one VirtioInput connection at a time
    fs.for_each(|service| async {
        match service {
            IncomingService::VirtioInput(stream) => {
                if let Err(e) = run_virtio_input(stream).await {
                    log::error!("Error running virtio_input service: {}", e);
                }
            }
        }
    })
    .await;
    Ok(())
}
