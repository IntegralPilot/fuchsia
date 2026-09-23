// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use driver_manager_config::Config;
use driver_manager_core::{DriverRunner, OfferInjector, PowerOffersConfig};
use driver_manager_development::DriverDevelopmentService;
use driver_manager_devfs::{Devfs, OutgoingDirectoryMsg};
use driver_manager_firmware_crash::FirmwareCrashService;
use driver_manager_shutdown::ShutdownManager;
use driver_manager_utils::DictionaryUtil;
use fidl::endpoints::{Proxy, create_endpoints};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::select;
use log::{error, info};
use std::ops::ControlFlow;
use std::rc::Rc;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_sandbox as fsandbox,
    fidl_fuchsia_driver_index as fdi, fidl_fuchsia_io as fio, fidl_fuchsia_ldsvc as fldsvc,
};

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    // Redirect standard out to debuglog.
    if stdout_to_debuglog::init().await.is_err() {
        log::warn!(
            "Failed to redirect stdout to debuglog, assuming test environment and continuing"
        );
    }

    let config = Config::take_from_startup_handle();

    info!("driver_manager_rust is starting.");

    let mut fs = ServiceFs::new_local();

    // Connect to required services.
    let realm = connect_to_protocol::<fcomponent::RealmMarker>()?;
    let introspector = connect_to_protocol::<fcomponent::IntrospectorMarker>()?;
    let capability_store = connect_to_protocol::<fsandbox::CapabilityStoreMarker>()?;
    let driver_index = connect_to_protocol::<fdi::DriverIndexMarker>()?;

    let lib_dir = fuchsia_fs::directory::open_in_namespace(
        "/pkg/lib",
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
    )
    .expect("Failed to open /pkg/lib");

    let (loader_service_factory, mut rx) = mpsc::unbounded::<oneshot::Sender<_>>();
    let loader_task = async move {
        while let Some(sender) = rx.next().await {
            let (client, server) = create_endpoints::<fldsvc::LoaderMarker>();

            library_loader::start(Clone::clone(&lib_dir), server.into_channel());

            let client = async move || {
                // TODO(https://fxbug.dev/42076026): Find a better way to set this config.
                if let Some(config) = option_env!("DRIVERHOST_LDSVC_CONFIG") {
                    let loader = client.into_proxy();
                    let status = loader.config(config).await.map_err(|_| zx::Status::INTERNAL)?;
                    zx::Status::ok(status)?;
                    Ok(loader.into_client_end().unwrap())
                } else {
                    Ok(client)
                }
            };
            let _ = sender.send(client().await);
        }
    }
    .fuse();
    futures::pin_mut!(loader_task);

    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded();

    let driver_runner = DriverRunner::new(
        realm,
        introspector,
        DictionaryUtil::new(capability_store),
        driver_index,
        loader_service_factory,
        config.enable_test_shutdown_delays,
        OfferInjector::new(PowerOffersConfig {
            power_inject_offer: config.power_inject_offer,
            power_suspend_enabled: config.power_suspend_enabled,
        }),
        Devfs::new(outgoing_tx),
    );
    driver_runner.register_notifier()?;

    let inspector = fuchsia_inspect::component::inspector();
    let driver_runner_inspect = driver_runner.clone();
    inspector.root().record_lazy_child_with_thread_local("driver_runner", move || {
        let driver_runner = driver_runner_inspect.clone();
        async move { Ok(driver_runner.inspect()) }.boxed_local()
    });

    driver_runner.publish(&mut fs);

    driver_runner.start_devfs_driver();

    info!("Starting DriverRunner with root driver URL: {}", config.root_driver);
    let root_driver = driver_runner.start_root_driver(config.root_driver).fuse();
    futures::pin_mut!(root_driver);

    let dds = Rc::new(DriverDevelopmentService::new(driver_runner.clone()));
    dds.publish(&mut fs);

    let firmware_crash_service = Rc::new(FirmwareCrashService::default());
    firmware_crash_service.publish(&mut fs);

    let shutdown_manager = ShutdownManager::new(driver_runner.clone());
    shutdown_manager.publish(&mut fs);

    // Serve devfs from outgoing directory.
    fs.add_remote("dev", driver_runner.devfs.as_ref().serve());

    #[cfg(feature = "heapdump")]
    heapdump::bind_with_fdio();

    fs.take_and_serve_directory_handle()?;

    let driver_runner_clone = driver_runner.clone();
    let purge_task = async move {
        driver_runner_clone.bootup_tracker.wait_for_bootup().await;
        let _ = scudo::mallopt(scudo::M_PURGE_ALL, 0);
    }
    .fuse();
    futures::pin_mut!(purge_task);

    let handle_msg = |fs: &mut ServiceFs<_>, msg| match msg {
        Some(OutgoingDirectoryMsg::Connect(channel)) => {
            if let Err(e) = fs.serve_connection(channel) {
                error!("Failed to serve outgoing with error {e}");
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
        Some(OutgoingDirectoryMsg::AddServiceInstance(service_name, instance_name, directory)) => {
            fs.dir("svc").dir(service_name).add_remote(instance_name, directory);
            ControlFlow::Continue(())
        }
        None => ControlFlow::Break(()),
    };

    info!("driver_manager main future is running");
    loop {
        select! {
            _ = loader_task => (),
            _ = purge_task => (),
            result = fs.next().fuse() => {
                if result.is_none() {
                    break;
                }
            },
            msg = outgoing_rx.next().fuse() => {
                if let ControlFlow::Break(_) = handle_msg(&mut fs, msg) { break }
            }
            result = root_driver => result?,
        }
    }

    error!("Driver Manager exited unexpectedly");
    Ok(())
}
