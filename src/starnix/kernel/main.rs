// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]
#![allow(clippy::too_many_arguments)]
// TODO(https://fxbug.dev/42073005): Remove this allow once the lint is fixed.
#![allow(unknown_lints, clippy::extra_unused_type_parameters)]

// Avoid unused crate warnings on non-test/non-debug builds because this needs to be an
// unconditional dependency for rustdoc generation.
use extended_pstate as _;
use tracing_mutex as _;

use anyhow::{Context as _, Error};
use async_lock::OnceCell;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fidl_fuchsia_starnix_container as fstarcontainer;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health::Reporter;
use fuchsia_runtime as fruntime;
use futures::{StreamExt, TryStreamExt};
use starnix_core::mm::{init_usercopy, zxio_maybe_faultable_copy_impl};
use starnix_kernel_runner::{
    Container, ContainerServiceConfig, create_component_from_stream, serve_component_runner,
    serve_container_controller, serve_memory_attribution_provider_elfkernel,
};
use starnix_kernel_structured_config::Config as KernelStructuredConfig;
use starnix_logging::{
    CATEGORY_STARNIX, NAME_START_KERNEL, log_debug, log_error, log_info, log_warn,
};
use std::rc::Rc;

mod lockup_detector;

/// Overrides the `zxio_maybe_faultable_copy` weak symbol found in zxio.
#[unsafe(no_mangle)]
extern "C" fn zxio_maybe_faultable_copy(
    dest: *mut u8,
    src: *const u8,
    count: usize,
    ret_dest: bool,
) -> bool {
    // SAFETY: we know that we are either copying from or to a buffer that
    // zxio (and thus Starnix) owns per `zxio_maybe_faultable_copy`'s
    // documentation.
    unsafe { zxio_maybe_faultable_copy_impl(dest, src, count, ret_dest) }
}

/// Overrides the `zxio_fault_catching_disabled` weak symbol found in zxio.
#[unsafe(no_mangle)]
extern "C" fn zxio_fault_catching_disabled() -> bool {
    false
}

fn maybe_serve_lifecycle(container: Rc<OnceCell<Container>>) -> Option<fasync::Task<()>> {
    if let Some(lifecycle) =
        fruntime::take_startup_handle(fruntime::HandleInfo::new(fruntime::HandleType::Lifecycle, 0))
    {
        Some(fasync::Task::local(async move {
            let mut stream =
                fidl::endpoints::ServerEnd::<flifecycle::LifecycleMarker>::new(lifecycle.into())
                    .into_stream();
            if let Ok(Some(request)) = stream.try_next().await {
                match request {
                    flifecycle::LifecycleRequest::Stop { .. } => {
                        if let Some(container) = container.get() {
                            container.kernel.shut_down();
                        } else {
                            log_warn!("Stopping kernel process without a running container.");
                            std::process::exit(0);
                        }
                    }
                }
            }
        }))
    } else {
        log_warn!("No lifecycle channel received from ELF runner.");
        None
    }
}

enum KernelServices {
    /// This service lets clients start a single container using this kernel.
    ///
    /// The `starnix_kernel` is capable of running a single container, which can be started using
    /// this protocol. Attempts to use this protocol a second time will fail.
    ///
    /// This service uses the `ComponentRunner` protocol but the service is exposed using the name
    /// `fuchsia.starnix.container.Runner` to reduce confusion with the instance of the
    /// `ComponentRunner` protocol that runs components inside the container.
    ContainerRunner(frunner::ComponentRunnerRequestStream),

    /// This service lets clients run components inside the container being run by this kernel.
    ///
    /// This service will wait to process any requests until the kernel starts a container.
    ///
    /// This service is also exposed via the container itself.
    ComponentRunner(frunner::ComponentRunnerRequestStream),

    /// This service lets clients control the container being run by this kernel.
    ///
    /// This service will wait to process any requests until the kernel starts a container.
    ///
    /// This service is also exposed via the container itself.
    ContainerController(fstarcontainer::ControllerRequestStream),

    /// This service lets clients read which memory resources are used to run the
    /// various starnix programs in the container.
    ///
    /// It provides a finer grained attribution than possible with Zircon process level
    /// tooling, because all starnix processes in a container share the same handle table.
    /// This protocol lets the kernel report exactly which VMOs it used to run a starnix
    /// program, out of VMOs in the shared handle table.
    ///
    /// The starnix runner connects to this protocol to report memory attribution
    /// information for each container it runs.
    MemoryAttributionProvider(fattribution::ProviderRequestStream),
}

#[fuchsia::main(
    // Don't add any statically declared tags to reduce right-ward drift in log output. In practice
    // all logs get tagged with task info that makes it clear from context the log comes from
    // Starnix.
    logging_tags = [],
    // LINT.IfChange(starnix_panic_tefmo)
    logging_panic_prefix="\n\n\n\nSTARNIX KERNEL PANIC\n\n\n\n",
    // LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go:starnix_panic_tefmo)
)]
async fn main() -> Result<(), Error> {
    // Make sure that if this process panics in normal mode that the whole kernel's job is killed.
    fruntime::job_default()
        .set_critical(zx::JobCriticalOptions::RETCODE_NONZERO, &fruntime::process_self())
        .context("ensuring main process panics kill whole kernel")?;

    let kernel_structured_config = KernelStructuredConfig::take_from_startup_handle();
    let KernelStructuredConfig { extra_features: kernel_extra_features, prefetch, .. } =
        kernel_structured_config;

    if prefetch {
        let file_proxy = fuchsia_fs::file::open_in_namespace(
            "/pkg/bin/starnix_kernel",
            fidl_fuchsia_io::PERM_READABLE | fidl_fuchsia_io::PERM_EXECUTABLE,
        )
        .context("open /pkg/bin/starnix_kernel")?;

        let vmo = file_proxy
            .get_backing_memory(
                fidl_fuchsia_io::VmoFlags::READ | fidl_fuchsia_io::VmoFlags::EXECUTE,
            )
            .await
            .context("call get_backing_memory")?
            .map_err(zx::Status::err_from_raw)
            .context("get_backing_memory")?;
        let size = vmo.get_size().context("get executable vmo size")?;
        vmo.op_range(zx::VmoOp::ALWAYS_NEED, 0, size).context("pin executable vmo")?;

        log_info!("Prefetched starnix_kernel binary");
    }

    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::init_inspector_with_size(1_000_000),
        inspect_runtime::PublishOptions::default(),
    );
    fuchsia_inspect::component::serve_inspect_stats();
    let mut health = fuchsia_inspect::component::health();
    health.set_starting_up();

    let scope_dispatcher = libasync_scope_dispatcher::ScopeDispatcher::new();
    let trace_provider =
        fuchsia_trace_provider::TraceProvider::new_with_fdio(&scope_dispatcher, None);
    fuchsia_trace::instant!(CATEGORY_STARNIX, NAME_START_KERNEL, fuchsia_trace::Scope::Thread);

    let _lockup_detector_task = lockup_detector::start_thread_lockup_detector();

    starnix_kernel_runner::initialize();
    let container = Rc::new(OnceCell::<Container>::new());
    let _lifecycle_task = maybe_serve_lifecycle(container.clone());

    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_service_at("fuchsia.starnix.container.Runner", KernelServices::ContainerRunner)
        .add_fidl_service(KernelServices::ComponentRunner)
        .add_fidl_service(KernelServices::ContainerController)
        .add_fidl_service(KernelServices::MemoryAttributionProvider);

    let inspector = fuchsia_inspect::component::inspector();
    #[cfg(target_arch = "x86_64")]
    {
        inspector.root().record_string(
            "x86_64_extended_pstate_strategy",
            format!("{:?}", *extended_pstate::x86_64::PREFERRED_STRATEGY),
        );
    }
    inspector.root().record_lazy_child("not_found", starnix_logging::not_found_lazy_node_callback);
    inspector.root().record_lazy_child("stubs", starnix_logging::track_stub_lazy_node_callback);
    inspector
        .root()
        .record_lazy_child("thread_lockup_detector", lockup_detector::inspect_lazy_node_callback);
    starnix_logging::register_stub_context_callback();

    log_debug!("Serving kernel services on outgoing directory handle.");
    fs.take_and_serve_directory_handle()?;
    health.set_ok();

    // We call this early during Starnix boot to make sure the usercopy utilities
    // are ready for use before any restricted-mode/Linux processes are created.
    init_usercopy();

    while let Some(request) = fs.next().await {
        match request {
            KernelServices::ContainerRunner(stream) => {
                let container = container.clone();
                let kernel_extra_features = kernel_extra_features.clone();
                fuchsia_async::Task::local(async move {
                    let mut config: Option<ContainerServiceConfig> = None;
                    let container = container
                        .get_or_try_init(|| async {
                            create_component_from_stream(stream, kernel_extra_features).await.map(
                                |(container, new_config)| {
                                    config = Some(new_config);
                                    container
                                },
                            )
                        })
                        .await
                        .expect("failed to start container");
                    if let Some(config) = config {
                        container
                            .serve(config)
                            .await
                            .expect("failed to serve the expected services from the container");
                    } else {
                        log_error!("No config provided for container, not running it.");
                    }
                })
                .detach();
            }
            KernelServices::ComponentRunner(stream) => {
                let container = container.clone();
                fuchsia_async::Task::local(async move {
                    serve_component_runner(stream, container.wait().await.system_task())
                        .await
                        .expect("failed to start component runner");
                })
                .detach();
            }
            KernelServices::ContainerController(stream) => {
                let container = container.clone();
                fuchsia_async::Task::local(async move {
                    serve_container_controller(stream, container.wait().await.system_task())
                        .await
                        .expect("failed to start container controller");
                })
                .detach();
            }
            KernelServices::MemoryAttributionProvider(stream) => {
                let container = container.clone();
                fuchsia_async::Task::local(async move {
                    serve_memory_attribution_provider_elfkernel(stream, container.wait().await)
                        .await
                        .expect("failed to start memory attribution provider");
                })
                .detach();
            }
        }
    }

    drop(trace_provider);
    scope_dispatcher.shutdown().await;

    Ok(())
}
