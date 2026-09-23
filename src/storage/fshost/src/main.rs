// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::environment::{DevicePublisher, Environment, FshostEnvironment};
use crate::inspect::register_stats;
use crate::watcher::{DirSource, PathSource, PathSourceType, WatchSource, Watcher};
use anyhow::{Context as _, Error, format_err};
use device::Parent;
use fidl::prelude::*;
use fidl_fuchsia_fshost as fshost;
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_runtime::{HandleType, take_startup_handle};
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{StreamExt, stream};
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::execution_scope::ExecutionScope;
use vfs::remote::remote_dir;
use zx::sys::zx_debug_write;

mod config;
mod crypt;
mod device;
mod environment;
mod fxblob;
mod inspect;
mod manager;
mod matcher;
mod ramdisk;
mod recovery;
mod service;
mod watcher;

const DEV_CLASS_BLOCK: &str = "/dev/class/block";
const DEV_CLASS_NAND: &str = "/dev/class/nand";
const VOLUME_SERVICE_PATH: &str = "/svc/fuchsia.hardware.block.volume.Service";

// Logs directly to the serial port.  To be used when it's expected that fshost will terminate
// shortly afterwards since messages via the log subsystem often don't make it.
fn debug_log(message: &str) {
    let message = format!("[fshost] {}\n", message);
    let message = message.as_bytes();
    unsafe {
        zx_debug_write(message.as_ptr(), message.len());
    }
}

async fn file_crash_report(signature: String) {
    let report = fidl_fuchsia_feedback::CrashReport {
        program_name: Some("fshost".to_string()),
        crash_signature: Some(signature),
        is_fatal: Some(true), // Automatically aggregate logs
        ..Default::default()
    };

    if let Ok(proxy) = connect_to_protocol::<fidl_fuchsia_feedback::CrashReporterMarker>() {
        // Wait entirely for the diagnostics service to cache & serialize report/logs before
        // reboot.
        match proxy.file_report(report).await {
            Ok(Ok(results)) => {
                log::info!("Fatal crash report successfully registered: {:?}", results);
            }
            Ok(Err(filing_error)) => {
                log::error!("Crash reporter returned application error: {:?}", filing_error);
            }
            Err(fidl_error) => {
                log::error!("FIDL error filing crash report: {:?}", fidl_error);
            }
        }
    } else {
        log::error!("Failed to connect to crash report service");
    }
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let config = Arc::new(fshost_config::Config::take_from_startup_handle());
    // NB There are tests that look for "fshost started".
    log::info!(config:?; "fshost started");

    let directory_request =
        take_startup_handle(HandleType::DirectoryRequest.into()).ok_or_else(|| {
            format_err!("missing DirectoryRequest startup handle - not launched as a component?")
        })?;

    let registered_devices = Arc::new(device::RegisteredDevices::default());
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<service::FshostShutdownResponder>(1);
    let (watcher, device_stream) = Watcher::new({
        // TODO(https://fxbug.dev/394968352): Don't watch /dev/class/nand
        let mut sources =
            vec![Box::new(PathSource::new(DEV_CLASS_NAND, PathSourceType::Nand, None))
                as Box<dyn WatchSource>];
        if config.watch_deprecated_v1_drivers {
            // TODO(https://fxbug.dev/394968352): Don't watch /dev/class/block
            sources.push(Box::new(PathSource::new(
                DEV_CLASS_BLOCK,
                PathSourceType::Block,
                Some(Arc::new(|_| Parent::Dev)),
            )) as Box<dyn WatchSource>);
        }
        sources.extend(
            fuchsia_fs::directory::open_in_namespace(
                VOLUME_SERVICE_PATH,
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
            )
            .map(|d| {
                Box::new(DirSource::new(d, VOLUME_SERVICE_PATH, Parent::Dev))
                    as Box<dyn WatchSource>
            }),
        );
        sources
    })
    .await?;
    // Potentially launch the boot items ramdisk. It's not fatal, so if it fails we print an error
    // and continue.
    let ramdisk_device = if config.ramdisk_image {
        log::info!("setting up ramdisk image from boot items");
        ramdisk::set_up_ramdisk().await.unwrap_or_else(|error| {
            log::error!(error:?; "failed to set up ramdisk filesystems");
            None
        })
    } else {
        None
    };

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());
    let scope = ExecutionScope::new();
    let device_publisher = DevicePublisher::new(scope.clone());
    let extra_matchers = matcher::get_config_matchers(&device_publisher)
        .await
        .context("failed to get configured matchers")?;
    let publisher_block_dir = device_publisher.block_dir();
    let publisher_debug_block_dir = device_publisher.debug_block_dir();
    let mut env = FshostEnvironment::new(
        config.clone(),
        inspector.clone(),
        watcher,
        registered_devices.clone(),
        device_publisher,
        scope.clone(),
        shutdown_tx.clone(),
    );

    // Records inspect metrics. Too expensive to build the tree data in newer fxfs environments.
    register_stats(inspector.root(), env.data_root()?, config.data_filesystem_format != "fxfs");
    let blob_exposed_dir = env.blobfs_exposed_dir()?;
    let data_exposed_dir = env.data_exposed_dir()?;
    let gpt_exposed_dir = env.partition_manager_exposed_dir()?;
    let export = vfs::pseudo_directory! {
        "block" => publisher_block_dir,
        "debug_block" => publisher_debug_block_dir,
        "fs" => vfs::pseudo_directory! {
            "blob" => remote_dir(blob_exposed_dir),
            "data" => remote_dir(data_exposed_dir),
        },
        "gpt" => remote_dir(gpt_exposed_dir),
        "mnt" => vfs::pseudo_directory! {},
    };

    let system_gpt_service_instance = env.system_gpt_volume_service_instance()?;
    let gpt_exposed_dir = env.partition_manager_exposed_dir()?;
    let launcher = env.launcher();
    let env: Arc<Mutex<dyn Environment>> = Arc::new(Mutex::new(env));
    let recovery_ops = Arc::new(recovery::RecoveryOps::new(
        env.clone(),
        registered_devices.clone(),
        config.clone(),
        launcher,
        gpt_exposed_dir,
        scope.clone(),
    ));

    let svc_dir = vfs::pseudo_directory! {
        fshost::AdminMarker::PROTOCOL_NAME =>
            service::fshost_admin(recovery_ops.clone()),
        fshost::RecoveryMarker::PROTOCOL_NAME =>
            service::fshost_recovery(recovery_ops),
        fidl_fuchsia_hardware_block_volume::ServiceMarker::SERVICE_NAME => vfs::pseudo_directory! {
            "system_gpt" => remote_dir(system_gpt_service_instance),
        }
    };

    if config.fxfs_blob {
        export
            .add_entry(
                "user_volumes",
                vfs::pseudo_directory! {
                    "starnix" =>
                        service::fshost_volume_provider(env.clone(), config.clone()),
                },
            )
            .unwrap();
        svc_dir
            .add_entry(
                fidl_fuchsia_update_verify::ComponentOtaHealthCheckMarker::PROTOCOL_NAME,
                fxblob::ota_health_check_service(),
            )
            .unwrap();
    }
    export.add_entry("svc", svc_dir).unwrap();

    // The inspector is global and will maintain strong references to callbacks used to gather
    // inspect data which will include env.data_root() which is a proxy with an async channel that
    // is registered with the executor.  The executor will assert if anything is regsistered with it
    // when its destructor runs, so we make sure to clean up the inspector here.
    scopeguard::defer! { inspector.root().clear_recorded(); }

    let _ = service::handle_lifecycle_requests(shutdown_tx)?;

    vfs::directory::serve_on(
        export,
        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
        scope.clone(),
        fidl::endpoints::ServerEnd::new(directory_request.into()),
    );

    // TODO(https://fxbug.dev/42069366): //src/tests/oom looks for "fshost: lifecycle handler ready"
    // to indicate the watcher is about to start.
    log::info!("fshost: lifecycle handler ready");

    // Run the main loop of fshost, handling devices as they appear according to our filesystem
    // policy.
    let mut fs_manager = manager::Manager::new(&config, env, extra_matchers);
    let shutdown_responder = if config.disable_block_watcher {
        // If the block watcher is disabled, fshost just waits on the shutdown receiver instead of
        // processing devices.
        shutdown_rx
            .next()
            .await
            .ok_or_else(|| format_err!("shutdown signal stream ended unexpectedly"))?
    } else {
        fs_manager
            .device_handler(stream::iter(ramdisk_device).chain(device_stream), shutdown_rx)
            .await?
    };

    log::info!("shutdown signal received");
    let is_crash = match &shutdown_responder {
        service::FshostShutdownResponder::Lifecycle(_) => {
            // TODO(https://fxbug.dev/42069366): //src/tests/oom looks for "received shutdown
            // command over lifecycle interface" to indicate fshost shutdown is starting. Shutdown
            // logs have to go straight to serial because of timing issues
            // (https://fxbug.dev/42179880).
            debug_log("received shutdown command over lifecycle interface");
            false
        }
        service::FshostShutdownResponder::Crash(volume_name) => {
            file_crash_report(format!("{}-volume-crash", volume_name)).await;
            true
        }
    };

    // Shutting down fshost involves sending asynchronous shutdown signals to several different
    // systems in order. If at any point we hit an error, we log loudly, but continue with the
    // shutdown procedure.

    // 0. Before fshost is told to shut down, almost everything that is running out of the
    //    filesystems is shut down by component manager.

    // 1. Shut down the scope for the export directory. This hosts the fshost services. This
    //    prevents additional connections to fshost services from being created.
    scope.shutdown();

    // 2. Shut down all the filesystems we started.
    fs_manager.shutdown().await?;

    // NB There are tests that look for this specific log message.  We write directly to serial
    // because writing via syslog has been found to not reliably make it to serial before shutdown
    // occurs.
    debug_log("fshost shutdown complete");

    // 3. Notify whoever asked for a shutdown that it's complete. After this point, it's possible
    //    the fshost process will be terminated externally.
    shutdown_responder.close()?;

    if is_crash {
        return Err(anyhow::format_err!("fshost shutting down due to volume crash"));
    }

    Ok(())
}
