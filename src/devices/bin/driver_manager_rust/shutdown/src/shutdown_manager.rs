// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node_remover::NodeRemover;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_kernel as fkernel;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fidl_fuchsia_system_state as fsystem_state;
use fuchsia_async as fasync;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_sync};
use fuchsia_component::server::{FidlService, ServiceFs, ServiceObjLocal};
use futures::channel::oneshot;
use futures::prelude::*;
use log::{error, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use zx::sys::{
    ZX_SYSTEM_POWERCTL_ACK_KERNEL_INITIATED_REBOOT, ZX_SYSTEM_POWERCTL_REBOOT,
    ZX_SYSTEM_POWERCTL_REBOOT_BOOTLOADER, ZX_SYSTEM_POWERCTL_REBOOT_RECOVERY,
    ZX_SYSTEM_POWERCTL_SHUTDOWN,
};

#[derive(Copy, Clone, PartialEq, Debug)]
enum State {
    Running,
    PackageStopping,
    PackageStopped,
    BootStopping,
    Stopped,
}

type ShutdownSender = oneshot::Sender<Result<(), zx::Status>>;

struct LifecycleServer {
    on_stop: RefCell<Option<oneshot::Sender<ShutdownSender>>>,
}

impl LifecycleServer {
    fn new(on_stop: oneshot::Sender<ShutdownSender>) -> Self {
        Self { on_stop: RefCell::new(Some(on_stop)) }
    }

    async fn serve(
        self: Rc<Self>,
        mut stream: flifecycle::LifecycleRequestStream,
    ) -> Result<(), fidl::Error> {
        if let Some(request) = stream.try_next().await? {
            match request {
                flifecycle::LifecycleRequest::Stop { control_handle } => {
                    let (tx, rx) = oneshot::channel();
                    let on_stop = self.on_stop.borrow_mut().take();
                    if let Some(on_stop) = on_stop {
                        let _ = on_stop.send(tx);
                        if let Ok(result) = rx.await {
                            control_handle.shutdown_with_epitaph(result);
                        } else {
                            control_handle.shutdown_with_epitaph(zx::Status::INTERNAL);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

struct ShutdownManagerState {
    state: State,
    received_boot_shutdown_signal: bool,
    package_shutdown_complete_callbacks: Vec<ShutdownSender>,
    boot_shutdown_complete_callbacks: Vec<ShutdownSender>,
    lifecycle_stop: bool,
}

pub struct ShutdownManager {
    node_remover: Rc<dyn NodeRemover>,
    power_resource: Option<zx::Resource>,
    mexec_resource: Option<zx::Resource>,
    log_flush: Option<fdiagnostics::LogFlusherProxy>,
    internal_state: RefCell<ShutdownManagerState>,
    scope: fasync::Scope,
}

fn get_power_resource() -> Result<zx::Resource, anyhow::Error> {
    let client = connect_to_protocol_sync::<fkernel::PowerResourceMarker>()?;
    let resource = client.get(zx::MonotonicInstant::INFINITE)?;
    Ok(resource)
}

fn get_mexec_resource() -> Result<zx::Resource, anyhow::Error> {
    let client = connect_to_protocol_sync::<fkernel::MexecResourceMarker>()?;
    let resource = client.get(zx::MonotonicInstant::INFINITE)?;
    Ok(resource)
}

async fn get_system_power_state() -> fsystem_state::SystemPowerState {
    let client = match connect_to_protocol::<fsystem_state::SystemStateTransitionMarker>() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to StateStateTransition: {}, falling back to default", e);
            return fsystem_state::SystemPowerState::Reboot;
        }
    };

    match client.get_termination_system_state().await {
        Ok(state) => state,
        Err(e) => {
            error!("Failed to get termination system state: {}, falling back to default", e);
            fsystem_state::SystemPowerState::Reboot
        }
    }
}

impl ShutdownManager {
    pub fn new(node_remover: Rc<dyn NodeRemover>) -> Rc<Self> {
        let power_resource = get_power_resource()
            .inspect_err(|e| {
                info!("Failed to get power resource, assuming test environment: {}", e)
            })
            .ok();
        let mexec_resource = get_mexec_resource()
            .inspect_err(|e| {
                info!("Failed to get mexec resource, assuming test environment: {}", e)
            })
            .ok();
        let log_flush = connect_to_protocol::<fdiagnostics::LogFlusherMarker>()
            .inspect_err(|e| error!("Failed to connect to LogFlusher: {}", e))
            .ok();

        let shutdown_manager = Rc::new(Self {
            node_remover: node_remover.clone(),
            internal_state: RefCell::new(ShutdownManagerState {
                state: State::Running,
                received_boot_shutdown_signal: false,
                package_shutdown_complete_callbacks: Vec::new(),
                boot_shutdown_complete_callbacks: Vec::new(),
                lifecycle_stop: false,
            }),
            power_resource,
            mexec_resource,
            log_flush,
            scope: fasync::Scope::new_with_name("shutdown_manager"),
        });

        let weak_manager = Rc::downgrade(&shutdown_manager);
        node_remover.set_on_removal_timeout_callback(Box::new(move || {
            if let Some(strong_manager) = weak_manager.upgrade() {
                info!("Driver timed out during shutdown, issuing syscall to reboot/shutdown");
                let strong_manager_clone = strong_manager.clone();
                strong_manager.scope.spawn_local(async move {
                    strong_manager_clone.system_execute().await;
                });
            }
        }));

        shutdown_manager
    }

    pub fn publish<'a>(self: &Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'a, ()>>) {
        let self_clone = self.clone();
        let (tx, rx) = oneshot::channel::<ShutdownSender>();
        self.scope.spawn_local(async move {
            if let Ok(sender) = rx.await {
                let status = self_clone.signal_package_shutdown().await;
                let _ = sender.send(status);
            }
        });
        let devfs_with_pkg_lifecycle = Rc::new(LifecycleServer::new(tx));

        let scope = self.scope.as_handle().clone();
        fs.dir("svc").add_service_at(
            "fuchsia.device.fs.with.pkg.lifecycle.Lifecycle",
            FidlService::from(move |stream: flifecycle::LifecycleRequestStream| {
                let devfs_with_pkg_lifecycle = devfs_with_pkg_lifecycle.clone();
                scope.spawn_local(async move {
                    devfs_with_pkg_lifecycle.serve(stream).await.unwrap_or_else(|e| {
                        error!("Failed to serve devfs with pkg lifecycle: {}", e)
                    });
                });
            }),
        );

        let self_clone = self.clone();
        let (tx, rx) = oneshot::channel::<ShutdownSender>();
        self.scope.spawn_local(async move {
            if let Ok(sender) = rx.await {
                let status = self_clone.signal_boot_shutdown().await;
                let _ = sender.send(status);
            }
        });
        let devfs_lifecycle = Rc::new(LifecycleServer::new(tx));

        let scope = self.scope.as_handle().clone();
        fs.dir("svc").add_service_at(
            "fuchsia.device.fs.lifecycle.Lifecycle",
            FidlService::from(move |stream: flifecycle::LifecycleRequestStream| {
                let devfs_lifecycle = devfs_lifecycle.clone();
                scope.spawn_local(async move {
                    devfs_lifecycle
                        .serve(stream)
                        .await
                        .unwrap_or_else(|e| error!("Failed to serve devfs lifecycle: {}", e));
                });
            }),
        );

        // Bind to process lifecycle
        let self_clone = self.clone();
        let (tx, rx) = oneshot::channel::<ShutdownSender>();
        self.scope.spawn_local(async move {
            if let Ok(sender) = rx.await {
                self_clone.internal_state.borrow_mut().lifecycle_stop = true;
                let status = self_clone.signal_boot_shutdown().await;
                let _ = sender.send(status);
            }
        });
        let lifecycle_server = Rc::new(LifecycleServer::new(tx));

        if let Some(handle) =
            fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::Lifecycle.into())
        {
            let channel = zx::Channel::from(handle);
            let server_end =
                fidl::endpoints::ServerEnd::<flifecycle::LifecycleMarker>::new(channel);
            let stream = server_end.into_stream();

            let self_clone = self.clone();
            self.scope.spawn_local(async move {
                if let Err(e) = lifecycle_server.serve(stream).await {
                    error!("Lifecycle connection got unbound: {}", e);
                    // Per C++ implementation, we should shut down if this happens.
                    let _ = self_clone.signal_boot_shutdown().await;
                }
            });
        } else {
            info!(concat!(
                "No valid handle found for lifecycle events, assuming test environment ",
                "and continuing"
            ));
        }
    }

    async fn on_package_shutdown_complete(&self) {
        info!("Package shutdown complete");
        let received_boot_shutdown_signal = {
            let mut internal_state = self.internal_state.borrow_mut();
            assert_eq!(internal_state.state, State::PackageStopping);
            internal_state.state = State::PackageStopped;

            for sender in internal_state.package_shutdown_complete_callbacks.drain(..) {
                let _ = sender.send(Ok(()));
            }

            if internal_state.received_boot_shutdown_signal {
                internal_state.state = State::BootStopping;
                true
            } else {
                false
            }
        };

        if received_boot_shutdown_signal {
            self.node_remover.shutdown_all_drivers().await;
            self.on_boot_shutdown_complete().await;
        }
    }

    async fn on_boot_shutdown_complete(&self) {
        {
            let mut internal_state = self.internal_state.borrow_mut();
            assert_eq!(internal_state.state, State::BootStopping);
            internal_state.state = State::Stopped;
        }
        self.system_execute().await;
        let mut internal_state = self.internal_state.borrow_mut();
        for sender in internal_state.boot_shutdown_complete_callbacks.drain(..) {
            let _ = sender.send(Ok(()));
        }
    }

    async fn signal_package_shutdown(&self) -> Result<(), zx::Status> {
        // TODO: switch logs to debuglog

        // We explicitly drop this before going into the await.
        #![allow(clippy::await_holding_refcell_ref)]
        let mut internal_state = self.internal_state.borrow_mut();

        match internal_state.state {
            State::Running | State::PackageStopping => {
                let (tx, rx) = oneshot::channel();
                internal_state.package_shutdown_complete_callbacks.push(tx);
                if internal_state.state == State::Running {
                    internal_state.state = State::PackageStopping;
                    drop(internal_state);
                    self.node_remover.shutdown_pkg_drivers().await;
                    self.on_package_shutdown_complete().await;
                } else {
                    drop(internal_state);
                }
                rx.await.unwrap_or(Err(zx::Status::INTERNAL))
            }
            _ => Ok(()),
        }
    }

    async fn signal_boot_shutdown(&self) -> Result<(), zx::Status> {
        // We explicitly drop this before going into the await.
        #![allow(clippy::await_holding_refcell_ref)]
        let mut internal_state = self.internal_state.borrow_mut();

        if internal_state.state == State::Stopped {
            return Ok(());
        }

        let (tx, rx) = oneshot::channel();
        internal_state.boot_shutdown_complete_callbacks.push(tx);

        internal_state.received_boot_shutdown_signal = true;
        let state = internal_state.state;
        match state {
            State::Running | State::PackageStopped => {
                internal_state.state = State::BootStopping;
                drop(internal_state);

                self.node_remover.shutdown_all_drivers().await;
                self.on_boot_shutdown_complete().await;
            }
            State::BootStopping => {
                error!("SignalBootShutdown() called during shutdown.");
            }
            _ => {}
        }
        rx.await.unwrap_or(Err(zx::Status::INTERNAL))
    }

    async fn system_execute(&self) {
        let shutdown_system_state = get_system_power_state().await;
        info!("Suspend fallback with flags {:?}", shutdown_system_state);
        let mut what = "zx_system_powerctl";

        let (Some(mexec_resource), Some(power_resource)) =
            (&self.mexec_resource, &self.power_resource)
        else {
            warn!("Invalid Power/mexec resources. Assuming test.");
            let internal_state = self.internal_state.borrow();
            if internal_state.lifecycle_stop {
                info!("Exiting driver manager gracefully");
                std::process::exit(0);
            }
            return;
        };

        info!("Flushing logs.");
        if let Some(log_flush) = &self.log_flush
            && let Err(e) = log_flush.wait_until_flushed().await
        {
            warn!("Failed to flush logs: {}", e);
        }

        info!("Executing powerctl.");
        let status = match shutdown_system_state {
            fsystem_state::SystemPowerState::Reboot => zx::Status::ok(unsafe {
                zx::sys::zx_system_powerctl(
                    power_resource.raw_handle(),
                    ZX_SYSTEM_POWERCTL_REBOOT,
                    std::ptr::null(),
                )
            }),
            fsystem_state::SystemPowerState::RebootBootloader => zx::Status::ok(unsafe {
                zx::sys::zx_system_powerctl(
                    power_resource.raw_handle(),
                    ZX_SYSTEM_POWERCTL_REBOOT_BOOTLOADER,
                    std::ptr::null(),
                )
            }),
            fsystem_state::SystemPowerState::RebootRecovery => zx::Status::ok(unsafe {
                zx::sys::zx_system_powerctl(
                    power_resource.raw_handle(),
                    ZX_SYSTEM_POWERCTL_REBOOT_RECOVERY,
                    std::ptr::null(),
                )
            }),
            fsystem_state::SystemPowerState::RebootKernelInitiated => {
                let status = zx::Status::ok(unsafe {
                    zx::sys::zx_system_powerctl(
                        power_resource.raw_handle(),
                        ZX_SYSTEM_POWERCTL_ACK_KERNEL_INITIATED_REBOOT,
                        std::ptr::null(),
                    )
                });
                if status.is_ok() {
                    // sleep indefinitely
                    loop {
                        fasync::Timer::new(std::time::Duration::from_secs(5 * 60)).await;
                        println!(
                            "driver_manager: unexpectedly still running after successful reboot syscall"
                        );
                    }
                }
                status
            }
            fsystem_state::SystemPowerState::Poweroff => zx::Status::ok(unsafe {
                zx::sys::zx_system_powerctl(
                    power_resource.raw_handle(),
                    ZX_SYSTEM_POWERCTL_SHUTDOWN,
                    std::ptr::null(),
                )
            }),

            fsystem_state::SystemPowerState::Mexec => {
                info!("About to mexec...");
                match mexec_boot::mexec_boot(zx::Unowned::new(mexec_resource)) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        error!("mexec_boot failed: {}", e);
                        what = "zx_system_mexec";
                        Err(zx::Status::INTERNAL)
                    }
                }
            }
            fsystem_state::SystemPowerState::FullyOn
            | fsystem_state::SystemPowerState::SuspendRam => {
                error!("Unexpected shutdown state requested: {:?}", shutdown_system_state);
                Err(zx::Status::INVALID_ARGS)
            }
        };

        let internal_state = self.internal_state.borrow();
        if internal_state.lifecycle_stop {
            info!("Exiting driver manager gracefully");
            std::process::exit(0);
        }

        warn!("{}: {status:?}", what);
    }
}
