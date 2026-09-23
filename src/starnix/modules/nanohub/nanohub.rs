// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::datachannel_file::DataChannelDevice;
use crate::nanohub_comms_directory::{
    build_display_comms_directory, build_nanohub_comms_directory,
};
use crate::nanohub_gpio_stub::register_gpio_chip_device;
use crate::socket_tunnel_file::register_socket_tunnel_device;
use fidl_fuchsia_hardware_google_nanohub as fnanohub;
use fidl_fuchsia_hardware_serial as fserial;
use fuchsia_component::client::Service;
use futures::TryStreamExt;
use starnix_core::device::serial::SerialDevice;
use starnix_core::fs::sysfs::build_device_directory;
use starnix_core::task::Kernel;
use starnix_logging::{log_error, log_info, log_warn};
use starnix_uapi::auth::FsCred;

use std::sync::Arc;

const SERIAL_DIRECTORY: &str = "/dev/class/serial";

pub fn nanohub_device_init(kernel: &Arc<Kernel>) {
    register_gpio_chip_device(kernel, "gpiochipwake0");

    register_socket_tunnel_device(
        kernel,
        "/dev/display_comms".into(),
        "display_comms".into(),
        "display".into(),
        build_display_comms_directory,
    );

    // /dev/nanohub_comms requires a set of additional sysfs nodes, so create this route
    // with a specialized directory.
    register_socket_tunnel_device(
        kernel,
        "/dev/nanohub_comms".into(),
        "nanohub_comms".into(),
        "nanohub".into(),
        build_nanohub_comms_directory,
    );

    // Spawn future to bind and configure serial device
    kernel.kthreads.spawn_future(
        {
            let kernel = kernel.clone();
            move || async move { register_serial_device(kernel).await }
        },
        "register_serial_device",
    );

    kernel.kthreads.spawn_future(
        {
            let kernel = kernel.clone();
            move || async move { register_datachannel_devices(kernel).await }
        },
        "register_datachannel_devices",
    );
}

async fn register_datachannel_devices(kernel: Arc<Kernel>) {
    let service = match Service::open(fnanohub::StarnixDataChannelServiceMarker) {
        Ok(service) => service,
        Err(e) => {
            log_warn!("Failed to open DriverService: {:?}", e);
            return;
        }
    };
    let mut watcher = match service.watch().await {
        Ok(watcher) => watcher,
        Err(e) => {
            log_warn!("Failed to create watcher: {:?}", e);
            return;
        }
    };

    while let Ok(Some(data_channel_service_proxy)) = watcher.try_next().await {
        let name = match (|| {
            let device_proxy = data_channel_service_proxy.connect_to_waitable_sync()?;
            let id = device_proxy.get_identifier(zx::MonotonicInstant::INFINITE)?;
            Ok::<std::option::Option<std::string::String>, fidl::Error>(id.name)
        })() {
            Ok(Some(name)) => name,
            Ok(None) => {
                log_error!("Data channel device has no name, skipping registration");
                continue;
            }
            Err(e) => {
                log_error!("Failed to get device info: {:?}", e);
                continue;
            }
        };

        let registry = &kernel.device_registry;

        let device_class =
            registry.objects.get_or_create_class("nanohub".into(), registry.objects.virtual_bus());

        if let Err(e) = registry.register_dyn_device_with_dir(
            &kernel,
            name.as_bytes().into(),
            device_class,
            build_device_directory,
            DataChannelDevice::new(
                data_channel_service_proxy,
                kernel.suspend_resume_manager.clone(),
            ),
        ) {
            log_warn!("Failed to register datachannel device: {:?}", e);
        }
    }
}

async fn register_serial_device(kernel: Arc<Kernel>) {
    // TODO Move this to expect once test support is enabled
    let dir =
        match fuchsia_fs::directory::open_in_namespace(SERIAL_DIRECTORY, fuchsia_fs::PERM_READABLE)
        {
            Ok(dir) => dir,
            Err(e) => {
                log_error!("Failed to open serial directory: {:}", e);
                return;
            }
        };

    let mut watcher = match fuchsia_fs::directory::Watcher::new(&dir).await {
        Ok(watcher) => watcher,
        Err(e) => {
            log_info!("Failed to create directory watcher for serial device: {:}", e);
            return;
        }
    };

    loop {
        match watcher.try_next().await {
            Ok(Some(watch_msg)) => {
                let filename = watch_msg
                    .filename
                    .as_path()
                    .to_str()
                    .expect("Failed to convert watch_msg to str");
                if filename == "." {
                    continue;
                }
                if watch_msg.event == fuchsia_fs::directory::WatchEvent::ADD_FILE
                    || watch_msg.event == fuchsia_fs::directory::WatchEvent::EXISTING
                {
                    let instance_path = format!("{}/{}", SERIAL_DIRECTORY, filename);
                    let (client_channel, server_channel) = zx::Channel::create();
                    if let Err(_) = fdio::service_connect(&instance_path, server_channel) {
                        continue;
                    }

                    // `fuchsia.hardware.serial` exposes a `DeviceProxy` type used for binding with
                    // a `Device` type. This should not be confused with the `DeviceProxy` generated
                    // by FIDL
                    let device_proxy = fserial::DeviceProxy_SynchronousProxy::new(client_channel);
                    let (serial_proxy, server_end) =
                        fidl::endpoints::create_sync_proxy::<fserial::DeviceMarker>();

                    // Instruct the serial driver to bind the connection to the underlying device
                    if let Err(_) = device_proxy.get_channel(server_end) {
                        continue;
                    }

                    // Fetch the device class to see if this is the correct instance
                    let device_class = match serial_proxy.get_class(zx::MonotonicInstant::INFINITE)
                    {
                        Ok(class) => class,
                        Err(_) => continue,
                    };

                    if device_class == fserial::Class::Mcu {
                        let serial_device = SerialDevice::new(
                            &kernel,
                            serial_proxy.into_channel().into(),
                            FsCred::root(),
                        )
                        .expect("Can create SerialDevice wrapper");

                        // TODO This will register with an incorrect device number. We should be
                        // dynamically registering a major device and this should be minor device 1
                        // of that major device.
                        let registry = &kernel.device_registry;
                        registry
                            .register_dyn_device(
                                &kernel,
                                "ttyHS1".into(),
                                registry.objects.tty_class(),
                                serial_device,
                            )
                            .expect("Can register serial device");
                        break;
                    }
                }
            }
            Ok(None) => {
                break;
            }
            Err(e) => {
                log_error!("Serial driver stream ended with error: {:}", e);
                break;
            }
        }
    }
}
