// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO Follow 2018 idioms
#![allow(elided_lifetimes_in_paths)]
// This is needed for the pseudo_directory nesting in crate::model::tests
#![recursion_limit = "256"]
// Printing to stdout and stderr directly is discouraged for component_manager.
// Instead, the tracing library, e.g. through macros like `info!`, and `error!`,
// should be used.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr,))]

use crate::bootfs::BootfsSvc;
use crate::builtin::builtin_runner::BuiltinRunner;
use crate::builtin_environment::{BuiltinEnvironment, BuiltinEnvironmentBuilder};
use ::cm_logger::klog;
use anyhow::Error;
use cm_config::RuntimeConfig;
use fidl_fuchsia_component_internal as finternal;
use fuchsia_async as fasync;
use fuchsia_runtime::{job_default, process_self};
use log::{error, info};
use std::path::PathBuf;
use std::{panic, process};
use zx::JobCriticalOptions;

#[cfg(feature = "heapdump")]
use ::routing::component_instance::ComponentInstanceInterface;

#[cfg(feature = "heapdump")]
use runtime_capabilities::Routable;

#[cfg(feature = "tracing")]
use cm_config::TraceProvider;

mod bootfs;
mod builtin;
mod builtin_environment;
mod capability;
mod framework;
mod model;
mod root_input_builder;
mod root_stop_notifier;
mod runner;
mod sandbox_util;
mod startup;

unsafe extern "C" {
    fn dl_set_loader_service(handle: zx::sys::zx_handle_t) -> zx::sys::zx_handle_t;
}

fn main() {
    // Set ourselves as critical to our job. If we do not fail gracefully, our
    // job will be killed.
    if let Err(err) =
        job_default().set_critical(JobCriticalOptions::RETCODE_NONZERO, &process_self())
    {
        panic!("Component manager failed to set itself as critical: {}", err);
    }

    // Close any loader service passed to component manager so that the service session can be
    // freed, as component manager won't make use of a loader service such as by calling dlopen.
    // If userboot invoked component manager directly, this service was the only reason userboot
    // continued to run and closing it will let userboot terminate.
    let ldsvc =
        unsafe { zx::NullableHandle::from_raw(dl_set_loader_service(zx::sys::ZX_HANDLE_INVALID)) };
    drop(ldsvc);

    let args = startup::Arguments::from_args()
        .unwrap_or_else(|err| panic!("{}\n{}", err, startup::Arguments::usage()));
    let (runtime_config, bootfs_svc) = build_runtime_config(&args);
    let mut executor =
        fasync::SendExecutorBuilder::new().num_threads(runtime_config.num_threads).build();

    match runtime_config.log_destination {
        finternal::LogDestination::Syslog => {
            diagnostics_log::initialize_buffered(diagnostics_log::PublishOptions::default())
                .unwrap();
        }
        finternal::LogDestination::Klog => {
            klog::KernelLogger::init();
        }
    };

    info!("Component manager is starting up...");
    if args.boot {
        info!("Component manager was started with boot defaults");
    }

    #[cfg(feature = "tracing")]
    if runtime_config.trace_provider == TraceProvider::Namespace {
        fuchsia_trace_provider::trace_provider_create_with_fdio();
    }

    let run_root_fut = async move {
        let mut builtin_environment = match build_environment(runtime_config, bootfs_svc).await {
            Ok(environment) => environment,
            Err(error) => {
                error!(error:%; "Component manager setup failed");
                process::exit(1);
            }
        };

        #[cfg(feature = "heapdump")]
        connect_to_heapdump(&builtin_environment);

        if let Err(error) = builtin_environment.run_root().await {
            error!(error:%; "Failed to start root component");
            process::exit(1);
        }
    };

    executor.run(run_root_fut);
}

#[cfg(feature = "heapdump")]
fn connect_to_heapdump(builtin_environment: &BuiltinEnvironment) {
    let model = builtin_environment.model.clone();
    builtin_environment.model.top_instance().execution_scope().spawn(async move {
        let heapdump_router = model.top_instance().get_root_exposed_capability_router(
            cm_types::Name::new("fuchsia.memory.heapdump.process.Registry").unwrap(),
        );
        let heapdump_connector = match heapdump_router
            .route(
                fidl_fuchsia_component_runtime::RouteRequest::default(),
                model.root().clone().as_weak().into(),
            )
            .await
        {
            Ok(Some(connector)) => connector,
            other_value => {
                error!("Failed to connect to heapdump collector: {:?}", other_value);
                return;
            }
        };
        let (client_end, server_end) = zx::Channel::create();
        match heapdump_connector.send(server_end) {
            Ok(_) => heapdump::bind_with_channel(client_end),
            Err(e) => error!("Failed to send handle to heapdump collector: {:?}", e),
        };
    });
}

/// Loads component_manager's config.
///
/// This function panics on failure because the logger is not initialized yet.
fn build_runtime_config(args: &startup::Arguments) -> (RuntimeConfig, Option<BootfsSvc>) {
    let bootfs_svc =
        args.host_bootfs.then(|| BootfsSvc::new().expect("Failed to create Rust bootfs"));
    let config_bytes = if let Some(ref bootfs_svc) = bootfs_svc {
        // The Rust bootfs VFS has not been brought up yet, so to find the component manager's
        // config we must find the config's offset and size in the bootfs VMO, and read from it
        // directly.
        let canonicalized =
            if args.config.starts_with("/boot/") { &args.config[6..] } else { &args.config };
        bootfs_svc.read_config_from_uninitialized_vfs(canonicalized).unwrap_or_else(|err| {
            panic!("Failed to read config from uninitialized vfs with error {}.", err)
        })
    } else {
        let path = PathBuf::from(&args.config);
        std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read config file {path:?}: {e}"))
    };

    let mut config = RuntimeConfig::new_from_bytes(&config_bytes)
        .unwrap_or_else(|err| panic!("Failed to load runtime config: {}", err));

    match (config.root_component_url.as_ref(), args.root_component_url.as_ref()) {
        (Some(_url), None) => (config, bootfs_svc),
        (None, Some(url)) => {
            config.root_component_url = Some(url.clone());
            (config, bootfs_svc)
        }
        (None, None) => {
            panic!(
                "`root_component_url` not provided. This field must be provided either as a \
                command line argument or config file parameter."
            );
        }
        (Some(_), Some(_)) => {
            panic!(
                "`root_component_url` set in two places: as a command line argument \
                and a config file parameter. This field can only be set in one of those places."
            );
        }
    }
}

async fn build_environment(
    config: RuntimeConfig,
    bootfs_svc: Option<BootfsSvc>,
) -> Result<BuiltinEnvironment, Error> {
    let service_broker = BuiltinRunner::get_service_broker_program();
    let dispatcher = BuiltinRunner::get_dispatcher_program();
    let devfs = BuiltinRunner::get_devfs_program();
    let shutdown_shim = BuiltinRunner::get_shutdown_shim_program();
    let add_to_env = true;
    let mut builder = BuiltinEnvironmentBuilder::new()
        .set_runtime_config(config)
        .create_utc_clock(&bootfs_svc)
        .await?
        .add_builtin_elf_runner(add_to_env)?
        .add_builtin_runner("builtin_service_broker", service_broker, add_to_env)?
        .add_builtin_runner("builtin_dispatcher", dispatcher, add_to_env)?
        .add_builtin_runner("builtin_devfs", devfs, !add_to_env)?
        .add_builtin_runner("builtin_shutdown_shim", shutdown_shim, !add_to_env)?
        .include_namespace_resolvers();

    if let Some(bootfs_svc) = bootfs_svc {
        builder = builder.set_bootfs_svc(bootfs_svc);
    }

    builder.build().await
}
