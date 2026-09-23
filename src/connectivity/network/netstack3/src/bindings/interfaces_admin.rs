// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL Worker for the `fuchsia.net.interfaces.admin` API, including the
//! `DeviceControl`, `Control` and `AddressStateProvider` Protocols.
//!
//! Each instance of these protocols is tied to the lifetime of a particular
//! entity in the Netstack:
//!    `DeviceControl`        => device
//!    `Control`              => interface
//!    `AddressStateProvider` => address
//! meaning the entity is removed if the protocol is closed, and the protocol is
//! closed if the entity is removed. Some protocols expose a `detach` method
//! that allows the lifetimes to be decoupled.
//!
//! These protocols (and their corresponding entities) are nested:
//! `DeviceControl` allows a new `Control` protocol to be connected (creating a
//! new interface on the device), while `Control` allows a new
//! `AddressStateProvider` protocol to be connected (creating a new address on
//! the interface).
//!
//! In general, each protocol is served by a "worker", either a
//! [`fuchsia_async::Task`] or a bare [`futures::future::Future`], that handles
//! incoming requests, spawns the workers for the nested protocols, and tears
//! down its associated entity if canceled.
//!
//! The `fuchsia.net.debug/Interfaces.GetAdmin` method exposes a backdoor that
//! allows clients to deviate from the behavior described above. Clients can
//! attach a new `Control` protocol to an existing interface with one-way
//! ownership semantics (removing the interface closes the protocol; closing
//! protocol does not remove the interface).

use std::collections::hash_map;
use std::convert::Infallible as Never;
use std::fmt::Debug;
use std::ops::DerefMut as _;
use std::pin::pin;

use assert_matches::assert_matches;
use fidl::endpoints::{ProtocolMarker, ServerEnd};
use fidl_fuchsia_net_interfaces_ext::{NotPositiveMonotonicInstantError, PositiveMonotonicInstant};
use fnet_resources::GrantForInterfaceAuthorization;
use futures::future::{FusedFuture as _, OptionFuture};
use futures::stream::FusedStream as _;
use futures::{FutureExt as _, SinkExt as _, StreamExt as _, TryStreamExt as _};
use log::{debug, error, info, warn};
use net_types::ip::{
    AddrSubnetEither, GenericOverIp, Ip, IpAddr, IpInvariant, IpVersion, Ipv4, Ipv6,
};
use net_types::{SpecifiedAddr, Witness, map_ip_twice};
use netstack3_core::device::{BlackholeDevice, DeviceConfigurationUpdateError, DeviceId};
use netstack3_core::error::NotFoundError;
use netstack3_core::ip::{
    AddIpAddrSubnetError, AddrSubnetAndManualConfigEither, CommonAddressConfig,
    CommonAddressProperties, IpDeviceConfigurationAndFlags, IpDeviceConfigurationUpdate,
    IpDeviceIpExt, Ipv4AddrConfig, Ipv6AddrManualConfig, Lifetime,
    PendingIpDeviceConfigurationUpdate, PreferredLifetime, SetIpAddressPropertiesError,
    UpdateIpConfigurationError,
};
use thiserror::Error;
use zx::Rights;

use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_resources as fnet_resources;
use fuchsia_async as fasync;

use crate::bindings::devices::{
    self, BlackholeDeviceInfo, DynamicCommonInfo, EthernetInfo, LoopbackInfo,
    OwnedDeviceSpecificInfo, PureIpDeviceInfo, StaticCommonInfo, TxTask, TxTaskError,
};
use crate::bindings::interface_config::{
    BindingsConfig, BindingsConfigUpdate, FidlInterfaceConfig, InterfaceConfig,
    InterfaceConfigUpdate,
};
use crate::bindings::interfaces_admin::enabled::InterfaceEnabledController;
use crate::bindings::netdevice_worker::LinkMulticastWorker;
use crate::bindings::routes::TableIdOverflowsError;
use crate::bindings::routes::admin::RouteSet;
use crate::bindings::routes::interface_local::LocalRouteTables;
use crate::bindings::time::StackTime;
use crate::bindings::util::{
    ErrorLogExt, IntoCore as _, RemoveResourceResultExt as _, ResultExt as _, ScopeExt as _,
    TryIntoCore,
};
use crate::bindings::{
    BindingId, BindingsCtx, CoreRwLock, Ctx, DeviceIdExt as _, InterfaceProperties,
    LifetimeExt as _, Netstack, netdevice_worker, routes,
};

pub(crate) async fn serve(
    ns: Netstack,
    mut req: fnet_interfaces_admin::InstallerRequestStream,
) -> Result<(), fidl::Error> {
    while let Some(req) = req.try_next().await? {
        match req {
            fnet_interfaces_admin::InstallerRequest::InstallBlackholeInterface {
                interface,
                options,
                control_handle: _,
            } => {
                let _: fasync::JoinHandle<()> = fasync::Scope::current()
                    .spawn(run_blackhole_interface(ns.clone(), interface, options));
            }
            fnet_interfaces_admin::InstallerRequest::InstallDevice {
                device,
                device_control,
                control_handle: _,
            } => fasync::Scope::current()
                .spawn_request_stream_handler(device_control.into_stream(), |rs| {
                    run_device_control(ns.clone(), device, rs)
                }),
        }
    }

    Ok(())
}

async fn run_blackhole_interface(
    mut ns: Netstack,
    control_server_end: ServerEnd<fnet_interfaces_admin::ControlMarker>,
    options: fnet_interfaces_admin::Options,
) {
    let scope = fasync::Scope::current();
    // Acquire a guard, we need to see run_interface_control running to
    // completion.
    let Some(guard) = scope.active_guard() else {
        warn!("not creating blackhole interface, could not acquire scope guard");
        return;
    };

    let InterfaceOptions { name, metric, netstack_managed_routes_designation } = match options
        .try_into()
    {
        Ok(options) => options,
        Err(err @ fnet_interfaces_ext::admin::UnknownNetstackManagedRoutesDesignation(_)) => {
            error!("invalid option: {err}");
            let (_stream, control_handle) = control_server_end.into_stream_and_control_handle();
            control_handle
                .send_on_interface_removed(
                    fnet_interfaces_admin::InterfaceRemovedReason::InvalidNetstackManagedRoutesDesignation,
                )
                .unwrap_or_log("failed to send OnInterfaceRemoved");
            return;
        }
    };

    let (binding_id, binding_id_alloc, name) = match name {
        Some(name) => {
            match ns.ctx.bindings_ctx().devices.try_reserve_name_and_alloc_new_id(name.clone()) {
                Ok((id, alloc)) => (id, alloc, name),
                Err(devices::NameNotAvailableError) => {
                    let (_stream, control_handle) =
                        control_server_end.into_stream_and_control_handle();
                    control_handle
                        .send_on_interface_removed(
                            fnet_interfaces_admin::InterfaceRemovedReason::DuplicateName,
                        )
                        .unwrap_or_else(|e| {
                            warn!("failed to send removed reason: {:?}", e);
                        });
                    return;
                }
            }
        }
        None => {
            ns.ctx.bindings_ctx().devices.generate_and_reserve_name_and_alloc_new_id("blackhole")
        }
    };

    let events = ns.create_interface_event_producer(
        binding_id,
        InterfaceProperties {
            name: name.clone(),
            port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Blackhole,
            // Blackhole interfaces do not have a backing port.
            port_identity_koid: None,
        },
    );
    let local_route_tables =
        match maybe_create_local_route_tables(&ns.ctx, &name, netstack_managed_routes_designation)
            .await
        {
            Ok(tables) => tables,
            Err(err @ TableIdOverflowsError) => {
                error!("cannot create local route tables for {name}: {err:?}");
                let (_stream, control_handle) = control_server_end.into_stream_and_control_handle();
                control_handle
                    .send_on_interface_removed(
                        fnet_interfaces_admin::InterfaceRemovedReason::LocalRouteTableUnavailable,
                    )
                    .unwrap_or_log("failed to send OnInterfaceRemoved");
                return;
            }
        };

    let (control_sender, control_receiver) =
        OwnedControlHandle::new_channel_with_owned_handle(control_server_end).await;
    let (interface_control_stop_sender, interface_control_stop_receiver) =
        futures::channel::oneshot::channel();

    let info = BlackholeDeviceInfo {
        common_info: StaticCommonInfo {
            authorization_token: zx::Event::create(),
            local_route_tables,
        },
        dynamic_common_info: CoreRwLock::new(DynamicCommonInfo::new(
            net_types::ip::Mtu::no_limit(),
            events,
            control_sender,
        )),
    };
    let core_id = ns.ctx.api().device::<BlackholeDevice>().add_device(
        crate::bindings::DeviceIdAndName { id: binding_id, name },
        (),
        netstack3_core::routes::RawMetric(
            metric.unwrap_or(crate::bindings::DEFAULT_INTERFACE_METRIC),
        ),
        info,
        (),
    );
    let core_id = DeviceId::Blackhole(core_id);
    ns.ctx.apply_interface_defaults(&core_id);

    // LINT.IfChange(netstack_created_interface_tefmo)
    info!("created interface {:?}", core_id);
    // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netstack_created_interface_tefmo)
    ns.ctx.bindings_ctx().devices.add_device_and_start_publishing(binding_id_alloc, core_id);

    let interface_control_fut = pin!(run_interface_control(
        ns.ctx.clone(),
        scope.new_child_with_name("blackhole_inner"),
        binding_id,
        interface_control_stop_receiver,
        control_receiver,
        true, /* removable */
        futures::stream::pending(),
    ));
    let stop_fut = guard.on_cancel().then(|()| {
        interface_control_stop_sender
            .send(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed)
            .unwrap_or_else(|reason| {
                // Already stopped, we should be going down.
                let _: fnet_interfaces_admin::InterfaceRemovedReason = reason;
            });
        futures::future::pending::<Never>()
    });
    {
        let stop_fut = pin!(stop_fut);
        match futures::future::select(interface_control_fut, stop_fut).await {
            futures::future::Either::Left(((), _fut)) => (),
        }
    }
    std::mem::drop(guard);
}

#[derive(Error, Debug)]
enum DeviceControlError {
    #[error("worker error: {0}")]
    Worker(#[from] netdevice_worker::Error),
    #[error("fidl error: {0}")]
    Fidl(#[from] fidl::Error),
}

impl ErrorLogExt for DeviceControlError {
    fn log_level(&self) -> log::Level {
        match self {
            DeviceControlError::Worker(_) => log::Level::Warn,
            DeviceControlError::Fidl(fidl) => fidl.log_level(),
        }
    }
}

async fn run_device_control(
    mut ns: Netstack,
    device: fidl::endpoints::ClientEnd<fhardware_network::DeviceMarker>,
    mut req_stream: fnet_interfaces_admin::DeviceControlRequestStream,
) -> Result<(), DeviceControlError> {
    let scope = fasync::Scope::new_with_name("device_control");
    let worker = netdevice_worker::NetdeviceWorker::new(ns.ctx.clone(), device).await?;
    let handler = worker.new_handler();
    let mut worker_fut = scope
        .spawn(async move {
            let result = worker.run().await;
            match result {
                Err(e) => log::warn!("netdevice worker exited: {e:?}"),
            }
            // If we lose the netdevice worker, that means we can't run anything
            // for this device so we should drop the scope in case the device
            // control has detached.
            let _: fasync::scope::Join<_> = fasync::Scope::current().cancel();
        })
        .fuse();

    enum Outcome {
        WorkerFinished,
        StreamClosed,
        StreamError(fidl::Error),
    }

    let mut detached = false;
    let outcome = loop {
        let result = futures::select! {
            req = req_stream.try_next() => req,
            () = worker_fut => break Outcome::WorkerFinished,
        };
        match result {
            Ok(None) => {
                break Outcome::StreamClosed;
            }
            Ok(Some(req)) => match req {
                fnet_interfaces_admin::DeviceControlRequest::CreateInterface {
                    port,
                    control,
                    options,
                    control_handle: _,
                } => {
                    create_interface(port, control, options, &mut ns, scope.as_handle(), &handler)
                        .await;
                }
                fnet_interfaces_admin::DeviceControlRequest::Detach { control_handle: _ } => {
                    detached = true;
                }
            },
            Err(e) => break Outcome::StreamError(e),
        }
    };

    // Check if we should cancel everything under this scope.
    let (result, cancel) = match outcome {
        Outcome::WorkerFinished => (Ok(()), true),
        Outcome::StreamClosed => (Ok(()), !detached),
        Outcome::StreamError(error) => (Err(error.into()), !detached),
    };

    if cancel {
        scope.cancel().await;
    } else {
        // Parent scope will take care of this now.
        scope.detach();
    }

    result
}

const INTERFACES_ADMIN_CHANNEL_SIZE: usize = 16;
/// A wrapper over `fuchsia.net.interfaces.admin/Control` handles to express ownership semantics.
///
/// If `owns_interface` is true, this handle 'owns' the interfaces, and when the handle closes the
/// interface should be removed.
pub(crate) struct OwnedControlHandle {
    request_stream: fnet_interfaces_admin::ControlRequestStream,
    control_handle: fnet_interfaces_admin::ControlControlHandle,
    owns_interface: bool,
}

impl OwnedControlHandle {
    pub(crate) fn new_unowned(
        handle: fidl::endpoints::ServerEnd<fnet_interfaces_admin::ControlMarker>,
    ) -> OwnedControlHandle {
        let (stream, control) = handle.into_stream_and_control_handle();
        OwnedControlHandle {
            request_stream: stream,
            control_handle: control,
            owns_interface: false,
        }
    }

    // Constructs a new channel of `OwnedControlHandle` with no owner.
    pub(crate) fn new_channel() -> (
        futures::channel::mpsc::Sender<OwnedControlHandle>,
        futures::channel::mpsc::Receiver<OwnedControlHandle>,
    ) {
        futures::channel::mpsc::channel(INTERFACES_ADMIN_CHANNEL_SIZE)
    }

    // Constructs a new channel of `OwnedControlHandle` with the given handle as the owner.
    // TODO(https://fxbug.dev/42169142): This currently enforces that there is only ever one owner,
    // which will need to be revisited to implement `Clone`.
    pub(crate) async fn new_channel_with_owned_handle(
        handle: fidl::endpoints::ServerEnd<fnet_interfaces_admin::ControlMarker>,
    ) -> (
        futures::channel::mpsc::Sender<OwnedControlHandle>,
        futures::channel::mpsc::Receiver<OwnedControlHandle>,
    ) {
        let (mut sender, receiver) = Self::new_channel();
        let (stream, control) = handle.into_stream_and_control_handle();
        sender
            .send(OwnedControlHandle {
                request_stream: stream,
                control_handle: control,
                owns_interface: true,
            })
            .await
            .expect("failed to attach initial control handle");
        (sender, receiver)
    }

    // Consumes the OwnedControlHandle and returns its `control_handle`.
    pub(crate) fn into_control_handle(self) -> fnet_interfaces_admin::ControlControlHandle {
        self.control_handle
    }
}

pub(crate) struct InterfaceOptions {
    pub(crate) name: Option<String>,
    pub(crate) metric: Option<u32>,
    pub(crate) netstack_managed_routes_designation:
        fnet_interfaces_ext::admin::NetstackManagedRoutesDesignation,
}

impl TryFrom<fnet_interfaces_admin::Options> for InterfaceOptions {
    type Error = fnet_interfaces_ext::admin::UnknownNetstackManagedRoutesDesignation;

    fn try_from(value: fnet_interfaces_admin::Options) -> Result<Self, Self::Error> {
        let fnet_interfaces_admin::Options {
            name,
            metric,
            netstack_managed_routes_designation,
            __source_breaking,
        } = value;
        let netstack_managed_routes_designation = netstack_managed_routes_designation
            .map_or_else(|| Ok(Default::default()), |designation| designation.try_into())?;
        Ok(Self { name, metric, netstack_managed_routes_designation })
    }
}

pub(crate) async fn maybe_create_local_route_tables(
    ctx: &Ctx,
    if_name: &str,
    designation: fnet_interfaces_ext::admin::NetstackManagedRoutesDesignation,
) -> Result<Option<LocalRouteTables>, TableIdOverflowsError> {
    match designation {
        fnet_interfaces_ext::admin::NetstackManagedRoutesDesignation::InterfaceLocal => {
            let tables = LocalRouteTables::new(&ctx, if_name).await?;
            Ok(Some(tables))
        }
        fnet_interfaces_ext::admin::NetstackManagedRoutesDesignation::Main => Ok(None),
    }
}

/// Operates a fuchsia.net.interfaces.admin/DeviceControl.CreateInterface
/// request.
async fn create_interface(
    port: fhardware_network::PortId,
    control: fidl::endpoints::ServerEnd<fnet_interfaces_admin::ControlMarker>,
    options: fnet_interfaces_admin::Options,
    ns: &mut Netstack,
    scope: &fasync::ScopeHandle,
    handler: &netdevice_worker::DeviceHandler,
) {
    debug!("creating interface from {:?} with {:?}", port, options);
    let interface_options = match options.try_into() {
        Ok(options) => options,
        Err(err @ fnet_interfaces_ext::admin::UnknownNetstackManagedRoutesDesignation(_)) => {
            error!("unknown option: {err}");
            let (_stream, control_handle) = control.into_stream_and_control_handle();
            control_handle
                .send_on_interface_removed(
                    fnet_interfaces_admin::InterfaceRemovedReason::InvalidNetstackManagedRoutesDesignation,
                )
                .unwrap_or_log("failed to send OnInterfaceRemoved");
            return;
        }
    };
    let (control_sender, mut control_receiver) =
        OwnedControlHandle::new_channel_with_owned_handle(control).await;
    match handler.add_port(ns, scope, interface_options, port, control_sender).await {
        Ok((binding_id, status_stream, guard, tx_task, link_multicast_task)) => {
            let _: fasync::JoinHandle<()> =
                guard.as_handle().spawn(run_netdevice_interface_control(
                    ns.ctx.clone(),
                    binding_id,
                    status_stream,
                    guard.clone(),
                    control_receiver,
                    tx_task,
                    link_multicast_task,
                ));
        }
        Err(e) => {
            warn!("failed to add port {:?} to device: {:?}", port, e);
            let removed_reason = match e {
                netdevice_worker::Error::Client(e) => match e {
                    // Assume any fidl errors are port closed
                    // errors.
                    netdevice_client::Error::Fidl(_) => {
                        Some(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed)
                    }
                    netdevice_client::Error::RxFlags(_)
                    | netdevice_client::Error::FrameType(_)
                    | netdevice_client::Error::Config(_)
                    | netdevice_client::Error::LargeChain(_)
                    | netdevice_client::Error::Index(_, _)
                    | netdevice_client::Error::Pad(_, _)
                    | netdevice_client::Error::TxLength
                    | netdevice_client::Error::Open(_, _)
                    | netdevice_client::Error::Vmo(_, _)
                    | netdevice_client::Error::Fifo(_, _, _)
                    | netdevice_client::Error::VmoSize(_, _)
                    | netdevice_client::Error::Map(_, _)
                    | netdevice_client::Error::DeviceInfo(_)
                    | netdevice_client::Error::PortStatus(_)
                    | netdevice_client::Error::InvalidPortId(_)
                    | netdevice_client::Error::Attach(_, _)
                    | netdevice_client::Error::Detach(_, _)
                    | netdevice_client::Error::TooSmall { size: _, offset: _, length: _ }
                    | netdevice_client::Error::InvalidLease
                    | netdevice_client::Error::RegisterForTx(_)
                    | netdevice_client::Error::UnregisterForTx(_) => None,
                },
                netdevice_worker::Error::AlreadyInstalled(_) => {
                    Some(fnet_interfaces_admin::InterfaceRemovedReason::PortAlreadyBound)
                }
                netdevice_worker::Error::Fidl(_)
                | netdevice_worker::Error::PortClosed
                | netdevice_worker::Error::ScopeFinished => {
                    Some(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed)
                }
                netdevice_worker::Error::ConfigurationNotSupported
                | netdevice_worker::Error::UnsupportedFrameType(_)
                | netdevice_worker::Error::MacNotUnicast { .. }
                | netdevice_worker::Error::MismatchedRxFrameType { .. }
                | netdevice_worker::Error::InvalidPortClass(_)
                | netdevice_worker::Error::MacAddressing(_) => {
                    Some(fnet_interfaces_admin::InterfaceRemovedReason::BadPort)
                }
                netdevice_worker::Error::DuplicateName(_) => {
                    Some(fnet_interfaces_admin::InterfaceRemovedReason::DuplicateName)
                }
                netdevice_worker::Error::InvalidPortInfo(_)
                | netdevice_worker::Error::CantCreateLocalRouteTables => None,
            };
            if let Some(removed_reason) = removed_reason {
                // Retrieve the original control handle from the receiver.
                let OwnedControlHandle { request_stream: _, control_handle, owns_interface: _ } =
                    control_receiver
                        .try_next()
                        .expect("expected control handle to be ready in the receiver")
                        .expect("expected receiver to not be closed/empty");
                control_handle.send_on_interface_removed(removed_reason).unwrap_or_else(|e| {
                    warn!("failed to send removed reason: {:?}", e);
                });
            }
        }
    }
}

/// Manages the lifetime of a newly created Netdevice interface, including spawning an
/// interface control worker, spawning a link state worker, and cleaning up the interface on
/// deletion.
async fn run_netdevice_interface_control<
    S: futures::Stream<Item = netdevice_client::Result<netdevice_client::client::PortStatus>>,
>(
    ctx: Ctx,
    id: BindingId,
    status_stream: S,
    scope_guard: fasync::scope::ScopeActiveGuard,
    control_receiver: futures::channel::mpsc::Receiver<OwnedControlHandle>,
    tx_task: TxTask,
    mut link_multicast_worker: Option<LinkMulticastWorker>,
) {
    let status_stream = status_stream.scan((), |(), status| {
        futures::future::ready(match status {
            Ok(netdevice_client::client::PortStatus { flags, mtu: _ }) => {
                Some(DeviceState { online: flags.contains(fhardware_network::StatusFlags::ONLINE) })
            }
            Err(e) => {
                match e {
                    netdevice_client::Error::Fidl(e) if e.is_closed() => {
                        warn!("error operating port state stream {:?} for interface {}", e, id)
                    }
                    e => {
                        error!("error operating port state stream {:?} for interface {}", e, id)
                    }
                }
                // Terminate the stream in case of any errors.
                None
            }
        })
    });

    let (interface_control_stop_sender, interface_control_stop_receiver) =
        futures::channel::oneshot::channel();
    // Create a scope where we'll spawn all the inner tasks for this interface,
    // and we'll give it to the interface control loop to ensure all tasks are
    // stopped before returning.
    let inner_scope = scope_guard.as_handle().new_child_with_name("netdevice_inner");

    let mut tx_task = inner_scope
        .compute(tx_task.run().map(move |r| match r {
            Err(TxTaskError::Netdevice(e)) => {
                // We expect all netdevice errors that can happen when driving the
                // tx task are problems fulfilling buffer layouts, so we can't
                // continue driving the interface. Log a loud message and we''ll
                // close the channel with PortClosed.
                error!("netdevice error operating tx task: {e:?} for {id}");
                fnet_interfaces_admin::InterfaceRemovedReason::BadPort
            }
        }))
        .fuse();

    let link_multicast_fut: OptionFuture<_> = link_multicast_worker
        .as_mut()
        .map(|worker| {
            worker
                .run()
                .then(async |r| match r {
                    Ok(()) => {
                        // The device has been destroyed, pend so we don't
                        // interrupt the cleanup process.
                        futures::future::pending().await
                    }
                    Err(e) => {
                        // Tear down the device because we've either observed the
                        // port closing or something has gone wrong.
                        match e {
                            netdevice_worker::Error::Fidl(e) if e.is_closed() => {}
                            e => error!(
                                "netdevice error operating link multicast task: {e:?} for {id}"
                            ),
                        }
                        fnet_interfaces_admin::InterfaceRemovedReason::PortClosed
                    }
                })
                .fuse()
        })
        .into();

    // Device-backed interfaces are always removable.
    let removable = true;
    let interface_control_fut = run_interface_control(
        ctx.clone(),
        inner_scope,
        id,
        interface_control_stop_receiver,
        control_receiver,
        removable,
        status_stream,
    )
    .fuse();

    {
        let mut interface_control_fut = pin!(interface_control_fut);
        let mut canceled_fut = pin!(scope_guard.on_cancel().fuse());
        let mut link_multicast_fut = pin!(link_multicast_fut);
        let remove_reason = futures::select! {
            () = canceled_fut => Some(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed),
            // Remove the interface if a task servicing the port exits with a reason.
            r = tx_task => Some(r),
            r = link_multicast_fut => r,
            () = interface_control_fut => None,
        };

        if let Some(remove_reason) = remove_reason {
            if !interface_control_fut.is_terminated() {
                // Cancel interface control and drive it to completion,
                // allowing it to terminate each control handle.
                interface_control_stop_sender
                    .send(remove_reason)
                    .expect("failed to cancel interface control");

                interface_control_fut.await
            }
        }
    }

    // This holds the link multicast receiver open until the device is fully gone.
    drop(link_multicast_worker);
}

pub(crate) struct DeviceState {
    online: bool,
}

/// Runs a worker to serve incoming `fuchsia.net.interfaces.admin/Control`
/// handles.
pub(crate) async fn run_interface_control<S: futures::Stream<Item = DeviceState>>(
    ctx: Ctx,
    scope: fasync::Scope,
    id: BindingId,
    mut stop_receiver: futures::channel::oneshot::Receiver<
        fnet_interfaces_admin::InterfaceRemovedReason,
    >,
    control_receiver: futures::channel::mpsc::Receiver<OwnedControlHandle>,
    removable: bool,
    device_state: S,
) {
    // An event indicating that the individual control request streams should stop.
    let cancel_request_streams = async_utils::event::Event::new();

    let enabled_controller = InterfaceEnabledController::new(ctx.clone(), id);
    let enabled_controller = &enabled_controller;
    // A struct to retain per-handle state of the individual request streams in `control_receiver`.
    struct ReqStreamState {
        owns_interface: bool,
        control_handle: fnet_interfaces_admin::ControlControlHandle,
        ctx: Ctx,
        id: BindingId,
    }

    // Convert `control_receiver` (a stream-of-streams) into a stream of futures, where each future
    // represents the termination of an inner `ControlRequestStream`.

    let scope_handle = scope.as_handle();
    let stream_of_fut = control_receiver.map(
        |OwnedControlHandle { request_stream, control_handle, owns_interface }| {
            let initial_state =
                ReqStreamState { owns_interface, control_handle, ctx: ctx.clone(), id };
            // Attach `cancel_request_streams` as a short-circuit mechanism to stop handling new
            // `ControlRequest`.
            let request_stream =
                request_stream.take_until(cancel_request_streams.wait_or_dropped());

            // Convert the request stream into a future, dispatching each incoming
            // `ControlRequest` and retaining the `ReqStreamState` along the way.
            async_utils::fold::fold_while(
                request_stream,
                initial_state,
                |mut state, request| async move {
                    let ReqStreamState { ctx, id, owns_interface, control_handle: _ } = &mut state;
                    match request {
                        Err(e) => {
                            error!(
                                "error operating {} stream for interface {}: {:?}",
                                fnet_interfaces_admin::ControlMarker::DEBUG_NAME,
                                id,
                                e
                            );
                            async_utils::fold::FoldWhile::Continue(state)
                        }
                        Ok(req) => {
                            match dispatch_control_request(
                                req,
                                ctx,
                                scope_handle,
                                *id,
                                removable,
                                owns_interface,
                                enabled_controller,
                            )
                            .await
                            {
                                Err(e) => {
                                    error!(
                                        "failed to handle request for interface {}: {:?}",
                                        id, e
                                    );
                                    async_utils::fold::FoldWhile::Continue(state)
                                }
                                Ok(ControlRequestResult::Continue) => {
                                    async_utils::fold::FoldWhile::Continue(state)
                                }
                                Ok(ControlRequestResult::Remove) => {
                                    // Short-circuit the stream, user called
                                    // remove.
                                    async_utils::fold::FoldWhile::Done(state.control_handle)
                                }
                            }
                        }
                    }
                },
            )
        },
    );
    // Enable the stream of futures to be polled concurrently.
    let mut stream_of_fut = stream_of_fut.buffer_unordered(usize::MAX);

    let device_state = {
        device_state
            .then(|DeviceState { online }| async move {
                enabled_controller.set_phy_up(online).await;
            })
            .fuse()
            .collect::<()>()
    };

    let (remove_reason, removal_requester) = {
        // Drive the `ControlRequestStreams` to completion, short-circuiting if
        // an owner terminates or if interface removal is requested.
        let mut interface_control_stream = stream_of_fut.by_ref().filter_map(|stream_result| {
            futures::future::ready(match stream_result {
                async_utils::fold::FoldResult::StreamEnded(ReqStreamState {
                    owns_interface,
                    control_handle: _,
                    ctx: _,
                    id: _,
                }) => {
                    // If we own the interface, return `Some(None)` to stop
                    // operating on the request streams.
                    owns_interface.then(|| None)
                }
                async_utils::fold::FoldResult::ShortCircuited(control_handle) => {
                    // If it was short-circuited by a call to remove, return
                    // `Some(Some(control_handle))` so we stop operating on
                    // the request streams and inform the requester of
                    // removal when done.
                    Some(Some(control_handle))
                }
            })
        });

        let mut device_state = pin!(device_state);
        futures::select! {
            // One of the interface's owning channels hung up or `Remove` was
            // called; inform the other channels.
            removal_requester = interface_control_stream.next() => {
                (fnet_interfaces_admin::InterfaceRemovedReason::User, removal_requester.flatten())
            }
            // Cancelation event was received with a specified remove reason.
            reason = stop_receiver => (reason.expect("failed to receive stop"), None),
            // Device state stream ended, assume device was removed.
            () = device_state => (fnet_interfaces_admin::InterfaceRemovedReason::PortClosed, None),
        }
    };

    // Close the control_receiver, preventing new RequestStreams from attaching.
    stream_of_fut.get_mut().get_mut().close();
    // Cancel the active request streams, and drive the remaining `ControlRequestStreams` to
    // completion, allowing each handle to send termination events.
    assert!(cancel_request_streams.signal(), "expected the event to be unsignaled");
    let control_handles = if !stream_of_fut.is_terminated() {
        // Accumulate all the control handles first so we stop operating on
        // them.
        stream_of_fut
            .map(|fold_result| match fold_result {
                async_utils::fold::FoldResult::StreamEnded(ReqStreamState {
                    owns_interface: _,
                    control_handle,
                    ctx: _,
                    id: _,
                }) => control_handle,
                async_utils::fold::FoldResult::ShortCircuited(control_handle) => control_handle,
            })
            // Append the client that requested that the interface be removed
            // (if there is one) so it receives the terminal event too.
            .chain(futures::stream::iter(removal_requester))
            .collect::<Vec<_>>()
            .await
    } else {
        // Drop the terminated stream of fut to get rid of context borrows.
        std::mem::drop(stream_of_fut);
        Vec::new()
    };
    // Cancel the `AddressStateProvider` workers with the right cancellation
    // reason. They should all join in the scope later.
    {
        let core_id =
            ctx.bindings_ctx().devices.get_core_id(id).expect("missing device info for interface");
        core_id.external_state().with_common_info_mut(|i| {
            for addr_info in i.addresses.values_mut() {
                let devices::AddressInfo {
                    address_state_provider:
                        devices::FidlWorkerInfo { worker: _, cancelation_sender },
                    assignment_state_sender: _,
                } = addr_info;
                if let Some(cancelation_sender) = cancelation_sender.take() {
                    cancelation_sender
                        .send(AddressStateProviderCancellationReason::InterfaceRemoved)
                        .expect("failed to stop AddressStateProvider");
                }
            }
        })
    }

    // Cancel our inner scope to make sure nothing else is referencing this
    // interface.
    scope.cancel().await;

    // Nothing else should be borrowing ctx by now. Moving to a mutable bind
    // proves this.
    let mut ctx = ctx;
    remove_interface(&mut ctx, id).await;

    // Send the termination reason for all handles we had on removal.
    control_handles.into_iter().for_each(|control_handle| {
        control_handle.send_on_interface_removed(remove_reason).unwrap_or_else(|e| {
            error!("failed to send terminal event: {:?} for interface {}", e, id)
        });
    });
}

enum ControlRequestResult {
    Continue,
    Remove,
}

/// Serves a `fuchsia.net.interfaces.admin/Control` Request.
async fn dispatch_control_request(
    req: fnet_interfaces_admin::ControlRequest,
    ctx: &mut Ctx,
    scope: &fasync::ScopeHandle,
    id: BindingId,
    removable: bool,
    owns_interface: &mut bool,
    enabled_controller: &InterfaceEnabledController,
) -> Result<ControlRequestResult, fidl::Error> {
    match req {
        fnet_interfaces_admin::ControlRequest::AddAddress {
            address,
            parameters,
            address_state_provider,
            control_handle: _,
        } => Ok(add_address(ctx, scope, id, address, parameters, address_state_provider)),
        fnet_interfaces_admin::ControlRequest::RemoveAddress { address, responder } => {
            responder.send(Ok(remove_address(ctx, id, address).await))
        }
        fnet_interfaces_admin::ControlRequest::GetId { responder } => responder.send(id.get()),
        fnet_interfaces_admin::ControlRequest::SetConfiguration { config, responder } => {
            let result = set_configuration(enabled_controller, config).await;
            if let Err(e) = &result {
                warn!("failed to set interface {id} configuration: {e:?}");
            }
            responder.send(result.as_ref().map_err(|e| *e))
        }
        fnet_interfaces_admin::ControlRequest::GetConfiguration { responder } => {
            responder.send(Ok(&get_configuration(ctx, id)))
        }
        fnet_interfaces_admin::ControlRequest::Enable { responder } => {
            responder.send(Ok(enabled_controller.set_admin_enabled(true).await))
        }
        fnet_interfaces_admin::ControlRequest::Disable { responder } => {
            responder.send(Ok(enabled_controller.set_admin_enabled(false).await))
        }
        fnet_interfaces_admin::ControlRequest::Detach { control_handle: _ } => {
            *owns_interface = false;
            Ok(())
        }
        fnet_interfaces_admin::ControlRequest::Remove { responder } => {
            if removable {
                return responder.send(Ok(())).map(|()| ControlRequestResult::Remove);
            }
            responder.send(Err(fnet_interfaces_admin::ControlRemoveError::NotAllowed))
        }
        fnet_interfaces_admin::ControlRequest::GetAuthorizationForInterface { responder } => {
            responder.send(grant_for_interface(ctx, id))
        }
    }
    .map(|()| ControlRequestResult::Continue)
}

/// Cleans up and removes the specified interface.
async fn remove_interface(ctx: &mut Ctx, id: BindingId) {
    let (devices::StaticNetdeviceInfo { handler, tx_notifier: _ }, weak_id) = {
        let core_id = ctx
            .bindings_ctx()
            .devices
            .remove_device(id)
            .expect("device was not removed since retrieval");
        // Keep a weak ID around to debug pending destruction.
        let weak_id = core_id.downgrade();

        let info = netstack3_core::for_any_device_id!(
            DeviceId,
            core_id,
            core_id => {
                // We want to remove the routes on the device _after_ we mark
                // the device for deletion (by calling `remove_device`) so
                // that we don't race with any new routes being added through
                // that device.
                let result = ctx.api().device().remove_device(core_id);
                ctx.bindings_ctx().remove_routes_on_device(&weak_id).await;
                ctx.bindings_ctx()
                    .multicast_admin
                    .remove_multicast_routes_on_device(&weak_id).await;
                ctx.bindings_ctx().packet_captures.notify_interface_removed(id);
                result.map_deferred(|d| d.into_future("device", &id, ctx)).into_future().await.into()
            }
        );

        match info {
            OwnedDeviceSpecificInfo::Loopback(LoopbackInfo {
                static_common_info: _,
                dynamic_common_info: _,
                rx_notifier: _,
            }) => {
                // Allow the loopback interface to be removed as part of clean
                // shutdown, but emit a warning about it.
                warn!("loopback interface was removed");
                return;
            }
            OwnedDeviceSpecificInfo::Blackhole(BlackholeDeviceInfo {
                common_info: _,
                dynamic_common_info: _,
            }) => {
                return;
            }
            OwnedDeviceSpecificInfo::Ethernet(EthernetInfo { netdevice, .. })
            | OwnedDeviceSpecificInfo::PureIp(PureIpDeviceInfo { netdevice, .. }) => {
                (netdevice, weak_id)
            }
        }
    };

    let id = weak_id.bindings_id();
    handler.uninstall().await.unwrap_or_else(|e| {
        warn!("error uninstalling netdevice handler for interface {:?}: {:?}", id, e)
    });
    info!("device {id:?} removal complete");
}

/// Removes the given `address` from the interface with the given `id`.
///
/// Returns `true` if the address existed and was removed; otherwise `false`.
async fn remove_address(ctx: &mut Ctx, id: BindingId, address: fnet::Subnet) -> bool {
    let specified_addr = match address.addr.try_into_core() {
        Ok(addr) => addr,
        Err(e) => {
            warn!("not removing unspecified address {:?}: {:?}", address.addr, e);
            return false;
        }
    };
    let core_id =
        ctx.bindings_ctx().devices.get_core_id(id).expect("missing device info for interface");
    let Some((worker, cancelation_sender)) = ({
        core_id.external_state().with_common_info_mut(|i| {
            i.addresses.get_mut(&specified_addr).map(
                |devices::AddressInfo {
                     address_state_provider: devices::FidlWorkerInfo { worker, cancelation_sender },
                     assignment_state_sender: _,
                 }| (worker.clone(), cancelation_sender.take()),
            )
        })
    }) else {
        // Even if the address is not in the bindings HashMap of addresses,
        // it could still be in core (e.g. if it's a loopback address or a
        // SLAAC address).
        // TODO(https://fxbug.dev/42084781): Rather than falling back in this way,
        // make it impossible to make this mistake by centralizing the
        // "source of truth" for addresses on a device.
        return match ctx.api().device_ip_any().del_ip_addr(&core_id, specified_addr) {
            Ok(result) => {
                let _: AddrSubnetEither = result
                    .map_deferred(|d| {
                        d.map_left(|l| {
                            l.into_future("device addr", &specified_addr, ctx).map(Into::into)
                        })
                        .map_right(|r| {
                            r.into_future("device addr", &specified_addr, ctx).map(Into::into)
                        })
                    })
                    .into_future()
                    .await;
                true
            }
            Err(netstack3_core::error::NotFoundError) => false,
        };
    };
    let did_cancel_worker = match cancelation_sender {
        Some(cancelation_sender) => {
            cancelation_sender
                .send(AddressStateProviderCancellationReason::UserRemoved)
                .expect("failed to stop AddressStateProvider");
            true
        }
        // The worker was already canceled by some other task.
        None => false,
    };
    // Wait for the worker to finish regardless of if we were the task to cancel
    // it. Doing so prevents us from prematurely returning while the address is
    // pending cleanup.
    worker.await;
    // Because the worker removes the address on teardown, `did_cancel_worker`
    // is a suitable proxy for `did_remove_addr`.
    return did_cancel_worker;
}

/// Sets the provided `config` on the interface with the given `core_id`.
///
/// Returns the previously set configuration on the interface.
async fn set_configuration(
    enabled_controller: &InterfaceEnabledController,
    config: fnet_interfaces_admin::Configuration,
) -> fnet_interfaces_admin::ControlSetConfigurationResult {
    let InterfaceConfigUpdate {
        bindings: bindings_update,
        ipv4: ipv4_update,
        ipv6: ipv6_update,
        device: device_update,
    } = FidlInterfaceConfig::from(config).try_into_update()?;

    let mut config_updater = enabled_controller.config_updater().await;

    let log_forwarding_change =
        |core_id: &DeviceId<BindingsCtx>,
         version: IpVersion,
         ip_config: Option<&IpDeviceConfigurationUpdate>| {
            if let Some(forwarding) = ip_config.and_then(|i| i.unicast_forwarding_enabled.as_ref())
            {
                info!(
                    "updating {version:?} unicast forwarding on \
                    {core_id:?} to enabled={forwarding}"
                );
            }
            if let Some(forwarding) =
                ip_config.and_then(|i| i.multicast_forwarding_enabled.as_ref())
            {
                info!(
                    "updating {version:?} multicast forwarding on \
                    {core_id:?} to enabled={forwarding}"
                );
            }
        };

    log_forwarding_change(
        config_updater.device_id(),
        IpVersion::V4,
        ipv4_update.as_ref().map(|i| &i.ip_config),
    );
    log_forwarding_change(
        config_updater.device_id(),
        IpVersion::V6,
        ipv6_update.as_ref().map(|i| &i.ip_config),
    );

    let bindings @ BindingsConfigUpdate { ipv4_enabled, ipv6_enabled } =
        bindings_update.apply_to_device(config_updater.device_id());

    // If either the ip_enabled or ip_update is Some, we need to update the ip configuration.
    // Note we don't care the value of ip_enabled, as the truth will be found in the
    // `new_ip_configuration_update` later.
    fn merge_ip_update<I: IpDeviceIpExt>(
        ip_update: Option<I::ConfigurationUpdate>,
        ip_enabled: Option<bool>,
    ) -> Option<I::ConfigurationUpdate>
    where
        I::ConfigurationUpdate: Default,
    {
        if ip_enabled.is_some() { Some(ip_update.unwrap_or_default()) } else { ip_update }
    }
    let ipv4_update = merge_ip_update::<Ipv4>(ipv4_update, ipv4_enabled);
    let ipv6_update = merge_ip_update::<Ipv6>(ipv6_update, ipv6_enabled);

    let mut api_provider = config_updater.api_provider();

    let ipv4_update = ipv4_update
        .map(|ipv4_update| {
            api_provider.new_ip_configuration_update::<Ipv4>(ipv4_update)
        })
        .transpose()
        .map_err(|e| match e {
            UpdateIpConfigurationError::UnicastForwardingNotSupported => {
                fnet_interfaces_admin::ControlSetConfigurationError::Ipv4ForwardingUnsupported
            }
            UpdateIpConfigurationError::MulticastForwardingNotSupported => {
                fnet_interfaces_admin::ControlSetConfigurationError::Ipv4MulticastForwardingUnsupported
            }
        })?;
    let ipv6_update = ipv6_update
        .map(|ipv6_update| {
            api_provider.new_ip_configuration_update::<Ipv6>(ipv6_update)
        })
        .transpose()
        .map_err(|e| match e {
            UpdateIpConfigurationError::UnicastForwardingNotSupported => {
                fnet_interfaces_admin::ControlSetConfigurationError::Ipv6ForwardingUnsupported
            }
            UpdateIpConfigurationError::MulticastForwardingNotSupported => {
                fnet_interfaces_admin::ControlSetConfigurationError::Ipv6MulticastForwardingUnsupported
            }
        })?;

    let device = {
        let device_id = api_provider.device_id().clone();
        let device_update = device_update
            .map(|device_update| {
                api_provider.api().device_any().new_configuration_update(&device_id, device_update)
            })
            .transpose()
            .map_err(|e| match e {
                DeviceConfigurationUpdateError::ArpNotSupported => {
                    fnet_interfaces_admin::ControlSetConfigurationError::ArpNotSupported
                }
                DeviceConfigurationUpdateError::NdpNotSupported => {
                    fnet_interfaces_admin::ControlSetConfigurationError::NdpNotSupported
                }
            })?;

        device_update.map(|u| api_provider.api().device_any().apply_configuration(u))
    };

    fn update_ip_configuration_and_log_enabled<'a, I: IpDeviceIpExt>(
        api: netstack3_core::CoreApi<'a, &'a mut BindingsCtx>,
        pending: PendingIpDeviceConfigurationUpdate<'a, I, DeviceId<BindingsCtx>>,
    ) -> I::ConfigurationUpdate {
        let new_ip_enabled = pending.config_update().as_ref().ip_enabled;
        let core_id = pending.device_id();
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Output<I: IpDeviceIpExt>(I::ConfigurationUpdate);
        let Output(update) =
            map_ip_twice!(I, (IpInvariant(api), pending), |(IpInvariant(api), pending)| Output(
                api.device_ip::<I>().apply_configuration(pending)
            ));
        if let Some(new_ip_enabled) = new_ip_enabled {
            let old_ip_eneabled = update.as_ref().ip_enabled.expect("ip_enabled should be set");
            if new_ip_enabled != old_ip_eneabled {
                info!(
                    "updated {:?} ip_enabled on {core_id:?} to {new_ip_enabled}, was {}",
                    I::VERSION,
                    old_ip_eneabled
                );
            }
        }
        update
    }
    // Apply both updates now that we have checked for errors and get the deltas
    // back.
    let ipv4 =
        ipv4_update.map(|u| update_ip_configuration_and_log_enabled::<Ipv4>(api_provider.api(), u));
    let ipv6 =
        ipv6_update.map(|u| update_ip_configuration_and_log_enabled::<Ipv6>(api_provider.api(), u));

    Ok(FidlInterfaceConfig::new_update(InterfaceConfigUpdate { bindings, ipv4, ipv6, device })
        .into())
}

/// Returns the configuration used for the interface with the given `id`.
fn get_configuration(ctx: &mut Ctx, id: BindingId) -> fnet_interfaces_admin::Configuration {
    let core_id = ctx
        .bindings_ctx()
        .devices
        .get_core_id(id)
        .expect("device lifetime should be tied to channel lifetime");

    let (ipv4_enabled, ipv6_enabled) = core_id
        .external_state()
        .with_common_info(|common_info| (common_info.ipv4_enabled, common_info.ipv6_enabled));

    let device = ctx.api().device_any().get_configuration(&core_id);

    let IpDeviceConfigurationAndFlags { config: ipv4, flags: _, gmp_mode: igmp } =
        ctx.api().device_ip::<Ipv4>().get_configuration(&core_id);

    let IpDeviceConfigurationAndFlags { config: ipv6, flags: _, gmp_mode: mld } =
        ctx.api().device_ip::<Ipv6>().get_configuration(&core_id);

    FidlInterfaceConfig::new_complete(InterfaceConfig {
        device,
        bindings_config: BindingsConfig { ipv4_enabled, ipv6_enabled },
        ipv4,
        ipv6,
        igmp,
        mld,
    })
    .into()
}

/// Adds the given `address` to the interface with the given `id`.
///
/// If the address cannot be added, the appropriate removal reason will be sent
/// to the address_state_provider.
fn add_address(
    ctx: &mut Ctx,
    scope: &fasync::ScopeHandle,
    id: BindingId,
    address: fnet::Subnet,
    params: fnet_interfaces_admin::AddressParameters,
    address_state_provider: ServerEnd<fnet_interfaces_admin::AddressStateProviderMarker>,
) {
    let (req_stream, control_handle) = address_state_provider.into_stream_and_control_handle();
    let core_addr = address.addr.into_core();

    let Some(guard) = scope.active_guard() else {
        warn!(
            "not adding address {} to interface {}: failed to acquire scope guard",
            core_addr, id
        );
        send_address_removal_event(
            core_addr,
            id,
            control_handle,
            fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved,
        );
        return;
    };

    let addr_subnet_either: AddrSubnetEither = match address.try_into_core() {
        Ok(addr) => addr,
        Err(e) => {
            warn!("not adding invalid address {} to interface {}: {:?}", core_addr, id, e);
            send_address_removal_event(
                core_addr,
                id,
                control_handle,
                fnet_interfaces_admin::AddressRemovalReason::Invalid,
            );
            return;
        }
    };
    let specified_addr = addr_subnet_either.addr();
    const INFINITE_NANOS: i64 = zx::MonotonicInstant::INFINITE.into_nanos();

    let fnet_interfaces_admin::AddressParameters {
        initial_properties,
        temporary,
        add_subnet_route,
        perform_dad,
        __source_breaking: fidl::marker::SourceBreaking,
    } = params;

    let fnet_interfaces_admin::AddressProperties {
        valid_lifetime_end,
        preferred_lifetime_info,
        __source_breaking: fidl::marker::SourceBreaking,
    } = initial_properties.unwrap_or_default();

    let valid_lifetime_end = valid_lifetime_end.unwrap_or(INFINITE_NANOS);
    // Disallow non-positive lifetimes.
    let valid_until = match PositiveMonotonicInstant::try_from(valid_lifetime_end) {
        Ok(i) => Lifetime::from_zx_time(i.into()),
        Err(e) => {
            warn!("not adding address {core_addr} to interface {id}: invalid lifetime {e}");
            send_address_removal_event(
                core_addr,
                id,
                control_handle,
                fnet_interfaces_admin::AddressRemovalReason::InvalidProperties,
            );
            return;
        }
    };

    let preferred_lifetime = preferred_lifetime_info
        .unwrap_or(fnet_interfaces::PreferredLifetimeInfo::PreferredUntil(INFINITE_NANOS));
    let preferred_lifetime =
        match fnet_interfaces_ext::PreferredLifetimeInfo::try_from(preferred_lifetime) {
            Ok(p) => {
                let p: PreferredLifetime<zx::MonotonicInstant> = p.into_core();
                p.map_instant(StackTime::from_zx)
            }
            Err(e) => {
                warn!(
                    "not adding address {} to interface {}: invalid preferred lifetime: {:?}",
                    core_addr, id, e
                );
                send_address_removal_event(
                    core_addr,
                    id,
                    control_handle,
                    fnet_interfaces_admin::AddressRemovalReason::InvalidProperties,
                );
                return;
            }
        };

    let temporary = temporary.unwrap_or(false);

    let common_properties = CommonAddressProperties { valid_until, preferred_lifetime };
    let common_config = CommonAddressConfig { should_perform_dad: perform_dad };

    let addr_subnet_either = match addr_subnet_either {
        AddrSubnetEither::V4(addr_subnet) => {
            // Core doesn't take temporary addresses into account for source
            // address selection over IPv4 and we don't expose it over the
            // watcher API at all, so we shouldn't need to keep that around.
            if temporary {
                warn!("v4 address {core_addr} requested temporary, ignoring for IPv4");
            }
            AddrSubnetAndManualConfigEither::V4(
                addr_subnet,
                Ipv4AddrConfig { config: common_config, properties: common_properties },
            )
        }
        AddrSubnetEither::V6(addr_subnet) => AddrSubnetAndManualConfigEither::V6(
            addr_subnet,
            Ipv6AddrManualConfig {
                config: common_config,
                properties: common_properties,
                temporary,
            },
        ),
    };

    let core_id =
        ctx.bindings_ctx().devices.get_core_id(id).expect("missing device info for interface");
    core_id.external_state().with_common_info_mut(|i| {
        let vacant_address_entry = match i.addresses.entry(specified_addr) {
            hash_map::Entry::Occupied(_occupied) => {
                send_address_removal_event(
                    address.addr.into_core(),
                    id,
                    control_handle,
                    fnet_interfaces_admin::AddressRemovalReason::AlreadyAssigned,
                );
                return;
            }
            hash_map::Entry::Vacant(vacant) => vacant,
        };
        // Cancelation mechanism for the `AddressStateProvider` worker.
        let (cancelation_sender, cancelation_receiver) = futures::channel::oneshot::channel();
        // Sender/receiver for updates in `AddressAssignmentState`, as
        // published by Core.
        let (assignment_state_sender, assignment_state_receiver) =
            futures::channel::mpsc::unbounded();
        // Spawn the `AddressStateProvider` worker, which during
        // initialization, will add the address to Core.
        let worker = scope
            .spawn(run_address_state_provider(
                ctx.clone(),
                guard,
                addr_subnet_either,
                add_subnet_route.unwrap_or(false),
                id,
                control_handle,
                req_stream,
                assignment_state_receiver,
                cancelation_receiver,
            ))
            .shared();
        let _: &mut devices::AddressInfo = vacant_address_entry.insert(devices::AddressInfo {
            address_state_provider: devices::FidlWorkerInfo {
                worker,
                cancelation_sender: Some(cancelation_sender),
            },
            assignment_state_sender,
        });
    })
}

fn grant_for_interface(ctx: &mut Ctx, id: BindingId) -> GrantForInterfaceAuthorization {
    let core_id = ctx
        .bindings_ctx()
        .devices
        .get_core_id(id)
        .expect("device lifetime should be tied to channel lifetime");

    let external_state = core_id.external_state();
    let StaticCommonInfo { authorization_token, local_route_tables: _ } =
        external_state.static_common_info();

    GrantForInterfaceAuthorization {
        interface_id: id.get(),
        token: authorization_token
            .duplicate_handle(Rights::TRANSFER | Rights::DUPLICATE)
            .expect("failed to duplicate handle"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AddressStateProviderCancellationReason {
    InvalidProperties,
    UserRemoved,
    DadFailed,
    Forfeited,
    InterfaceRemoved,
}

impl From<AddressStateProviderCancellationReason> for fnet_interfaces_admin::AddressRemovalReason {
    fn from(value: AddressStateProviderCancellationReason) -> Self {
        match value {
            AddressStateProviderCancellationReason::UserRemoved => {
                fnet_interfaces_admin::AddressRemovalReason::UserRemoved
            }
            AddressStateProviderCancellationReason::DadFailed => {
                fnet_interfaces_admin::AddressRemovalReason::DadFailed
            }
            AddressStateProviderCancellationReason::Forfeited => {
                fnet_interfaces_admin::AddressRemovalReason::Forfeited
            }
            AddressStateProviderCancellationReason::InterfaceRemoved => {
                fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved
            }
            AddressStateProviderCancellationReason::InvalidProperties => {
                fnet_interfaces_admin::AddressRemovalReason::InvalidProperties
            }
        }
    }
}

/// A worker for `fuchsia.net.interfaces.admin/AddressStateProvider`.
async fn run_address_state_provider(
    mut ctx: Ctx,
    guard: fasync::scope::ScopeActiveGuard,
    addr_subnet_and_config: AddrSubnetAndManualConfigEither<StackTime>,
    add_subnet_route: bool,
    id: BindingId,
    control_handle: fnet_interfaces_admin::AddressStateProviderControlHandle,
    req_stream: fnet_interfaces_admin::AddressStateProviderRequestStream,
    mut assignment_state_receiver: futures::channel::mpsc::UnboundedReceiver<
        fnet_interfaces::AddressAssignmentState,
    >,
    mut stop_receiver: futures::channel::oneshot::Receiver<AddressStateProviderCancellationReason>,
) {
    let addr_subnet_either = addr_subnet_and_config.addr_subnet_either();
    let (address, subnet) = addr_subnet_either.addr_subnet();
    struct StateInCore {
        address: bool,
        subnet_route:
            Option<(routes::admin::UserRouteSet<Ipv4>, routes::admin::UserRouteSet<Ipv6>)>,
    }

    // Add the address to Core. Note that even though we verified the address
    // was absent from `ctx` before spawning this worker, it's still possible
    // for the address to exist in core (e.g. auto-configured addresses such as
    // loopback or SLAAC).
    let add_to_core_result = {
        let device_id = ctx.bindings_ctx().devices.get_core_id(id).expect("interface not found");
        ctx.api().device_ip_any().add_ip_addr_subnet(&device_id, addr_subnet_and_config)
    };
    let (state_to_remove_from_core, mut removal_reason) = match add_to_core_result {
        Err(e) => {
            let removal_reason = match e {
                AddIpAddrSubnetError::Exists => {
                    fnet_interfaces_admin::AddressRemovalReason::AlreadyAssigned
                }
                AddIpAddrSubnetError::InvalidAddr => {
                    fnet_interfaces_admin::AddressRemovalReason::Invalid
                }
            };
            send_address_removal_event(*address, id, control_handle.clone(), removal_reason);
            // The address wasn't added, so don't attempt to remove it. In the
            // `AlreadyAssigned` case, this ensures we don't accidentally remove
            // an address we didn't add.
            (StateInCore { address: false, subnet_route: None }, None)
        }
        Ok(()) => {
            let state_to_remove = if add_subnet_route {
                let bindings_ctx = ctx.bindings_ctx();
                let core_id = bindings_ctx
                    .devices
                    .get_core_id(id)
                    .expect("missing device info for interface")
                    .downgrade();

                let route_set_v4 = routes::admin::UserRouteSet::from_main_table(&ctx);
                let route_set_v6 = routes::admin::UserRouteSet::from_main_table(&ctx);

                let add_route_result = match subnet {
                    net_types::ip::SubnetEither::V4(subnet) => {
                        let entry = netstack3_core::routes::AddableEntry::without_gateway(
                            subnet,
                            core_id,
                            netstack3_core::routes::AddableMetric::MetricTracksInterface,
                        );
                        route_set_v4.apply_route_op(routes::RouteOp::Add(entry)).await
                    }
                    net_types::ip::SubnetEither::V6(subnet) => {
                        let entry = netstack3_core::routes::AddableEntry::without_gateway(
                            subnet,
                            core_id,
                            netstack3_core::routes::AddableMetric::MetricTracksInterface,
                        );
                        route_set_v6.apply_route_op(routes::RouteOp::Add(entry)).await
                    }
                };

                match add_route_result {
                    Err(e) => {
                        warn!("failed to add subnet route {:?}: {:?}", subnet, e);
                        route_set_v4.close().await;
                        route_set_v6.close().await;
                        StateInCore { address: true, subnet_route: None }
                    }
                    Ok(outcome) => {
                        assert_matches!(
                            outcome,
                            routes::ChangeOutcome::Changed,
                            "subnet route for {subnet:?} should be new to \
                            route set, since route set was created especially \
                            for this"
                        );

                        StateInCore {
                            address: true,
                            subnet_route: Some((route_set_v4, route_set_v6)),
                        }
                    }
                }
            } else {
                StateInCore { address: true, subnet_route: None }
            };

            control_handle.send_on_address_added().unwrap_or_else(|e| {
                error!(
                    "failed to send address added event for addr {:?} on interface {}: {:?}",
                    address, id, e
                );
            });

            // Receive the initial assignment state from Core. The message
            // must already be in the channel, so don't await.
            let initial_assignment_state = assignment_state_receiver
                .next()
                .now_or_never()
                .expect("receiver unexpectedly empty")
                .expect("sender unexpectedly closed");

            // Run the `AddressStateProvider` main loop.
            // Pass in the `assignment_state_receiver` and `stop_receiver` by
            // ref so that they don't get dropped after the main loop exits
            // (before the senders are removed from `ctx`).
            let (needs_removal, removal_reason) = address_state_provider_main_loop(
                &mut ctx,
                address,
                id,
                req_stream,
                &mut assignment_state_receiver,
                initial_assignment_state,
                &mut stop_receiver,
            )
            .await;

            (
                match needs_removal {
                    AddressNeedsExplicitRemovalFromCore::Yes => state_to_remove,
                    AddressNeedsExplicitRemovalFromCore::No => {
                        StateInCore { address: false, ..state_to_remove }
                    }
                },
                removal_reason,
            )
        }
    };

    // Remove the address.
    let bindings_ctx = ctx.bindings_ctx();
    let core_id = bindings_ctx.devices.get_core_id(id).expect("missing device info for interface");

    let StateInCore { address: remove_address, subnet_route } = state_to_remove_from_core;

    // The presence of this `route_set` indicates that we added a subnet route
    // previously due to the `add_subnet_route` AddressParameters option.
    // Closing `route_set` will correctly remove this route.
    if let Some((route_set_v4, route_set_v6)) = subnet_route {
        route_set_v4.close().await;
        route_set_v6.close().await;
    }

    if remove_address {
        let result = ctx.api().device_ip_any().del_ip_addr(&core_id, address);
        match result {
            Ok(resource_removal) => {
                let _: AddrSubnetEither = resource_removal
                    .map_deferred(|d| {
                        d.map_left(|l| l.into_future("device addr", &address, &ctx).map(Into::into))
                            .map_right(|r| {
                                r.into_future("device addr", &address, &ctx).map(Into::into)
                            })
                    })
                    .into_future()
                    .await;
            }
            Err(NotFoundError) => {
                // We *must* have raced here with a core-initiated removal.
                //
                // If the address state provider was cancelled with a reason
                // (which may have come from core or bindings) then we use that
                // one for the removal event. Otherwise we wait for the reason
                // from core.
                removal_reason = Some(if stop_receiver.is_terminated() {
                    removal_reason.expect("missing a removal reason")
                } else {
                    stop_receiver.await.expect("address disappeared without removal reason")
                });
            }
        }
    }

    // After the address is removed from core we can remove it from the
    // bindings' book-keeping attached to the device. Note that this must happen
    // _after_ we've resolved the race with core removal above, otherwise core
    // doesn't have access to the removal reason sender.

    // We can drop the join handle safely, since it does not encode task
    // lifetime (this task, specifically).
    let _: futures::future::Shared<fasync::JoinHandle<()>> =
        match core_id.external_state().with_common_info_mut(|i| i.addresses.remove(&address)) {
            Some(devices::AddressInfo {
                address_state_provider: devices::FidlWorkerInfo { worker, cancelation_sender: _ },
                assignment_state_sender: _,
            }) => worker,
            None => {
                panic!("`AddressInfo` unexpectedly missing for {:?}", address)
            }
        };
    // After having removed ASP from the external state, the bindings ID may not
    // be valid anymore if this is racing with device removal. Shadow the
    // variable name to prevent misuse.
    let id = ();
    let () = id;

    if let Some(removal_reason) = removal_reason {
        send_address_removal_event(address.get(), &core_id, control_handle, removal_reason.into());
    }

    // Ensure our scope guard is only dropped after the address is fully
    // removed.
    std::mem::drop(guard);
}

enum AddressNeedsExplicitRemovalFromCore {
    /// The caller is expected to delete the address from Core.
    Yes,
    /// The caller need not delete the address from Core (e.g. interface removal,
    /// which implicitly removes all addresses.)
    No,
}

async fn address_state_provider_main_loop(
    ctx: &mut Ctx,
    address: SpecifiedAddr<IpAddr>,
    id: BindingId,
    req_stream: fnet_interfaces_admin::AddressStateProviderRequestStream,
    assignment_state_receiver: &mut futures::channel::mpsc::UnboundedReceiver<
        fnet_interfaces::AddressAssignmentState,
    >,
    initial_assignment_state: fnet_interfaces::AddressAssignmentState,
    stop_receiver: &mut futures::channel::oneshot::Receiver<AddressStateProviderCancellationReason>,
) -> (AddressNeedsExplicitRemovalFromCore, Option<AddressStateProviderCancellationReason>) {
    // When detached, the lifetime of `req_stream` should not be tied to the
    // lifetime of `address`.
    let mut detached = false;
    let mut watch_state = AddressAssignmentWatcherState {
        fsm: AddressAssignmentWatcherStateMachine::UnreportedUpdate(initial_assignment_state),
        last_response: None,
    };
    enum AddressStateProviderEvent {
        Request(Result<Option<fnet_interfaces_admin::AddressStateProviderRequest>, fidl::Error>),
        AssignmentStateChange(fnet_interfaces::AddressAssignmentState),
        Canceled(AddressStateProviderCancellationReason),
    }
    let mut req_stream = pin!(req_stream);
    let mut stop_receiver = pin!(stop_receiver);
    let mut assignment_state_receiver = pin!(assignment_state_receiver);
    let cancelation_reason = loop {
        let next_event = futures::select! {
            req = req_stream.try_next() => AddressStateProviderEvent::Request(req),
            state = assignment_state_receiver.next() => {
                    AddressStateProviderEvent::AssignmentStateChange(
                        // It's safe to `expect` here, because the
                        // AddressStateProvider worker is responsible for
                        // cleaning up the sender, and only does so after this
                        // function exits.
                        state.expect("sender unexpectedly closed"))
                },
            reason = stop_receiver => AddressStateProviderEvent::Canceled(reason.expect("failed to receive stop")),
        };

        match next_event {
            AddressStateProviderEvent::Request(req) => match req {
                // The client hung up, stop serving.
                Ok(None) => {
                    // If detached, wait to be canceled before exiting.
                    if detached {
                        // N.B. The `Canceled` arm of this match exits the loop,
                        // meaning we can't already be canceled here.
                        debug_assert!(!stop_receiver.is_terminated());
                        break Some(stop_receiver.await.expect("failed to receive stop"));
                    }
                    break None;
                }
                Ok(Some(request)) => match dispatch_address_state_provider_request(
                    ctx,
                    request,
                    address,
                    id,
                    &mut detached,
                    &mut watch_state,
                ) {
                    Ok(Some(UserRemove)) => {
                        break Some(AddressStateProviderCancellationReason::UserRemoved);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(
                            "failed to handle request for address {:?} on interface {}: {}",
                            address, id, err
                        );
                        match err {
                            AddressStateProviderError::PreviousPendingWatchRequest => {
                                break None;
                            }
                            AddressStateProviderError::InvalidPropertiesUpdate { .. } => {
                                break Some(
                                    AddressStateProviderCancellationReason::InvalidProperties,
                                );
                            }
                            AddressStateProviderError::AddressNotFound => {
                                // We raced with the address being removed. Get
                                // the reason from Core.
                                debug_assert!(!stop_receiver.is_terminated());
                                break Some(
                                    stop_receiver
                                        .await
                                        .expect("address disappeared without removal reason"),
                                );
                            }
                            AddressStateProviderError::Fidl(_) => {
                                break None;
                            }
                        }
                    }
                },
                Err(e) => {
                    error!(
                        "error operating {} stream for address {:?} on interface {}: {:?}",
                        fnet_interfaces_admin::AddressStateProviderMarker::DEBUG_NAME,
                        address,
                        id,
                        e
                    );
                    break None;
                }
            },
            AddressStateProviderEvent::AssignmentStateChange(state) => {
                watch_state.on_new_assignment_state(state).unwrap_or_else(|e|{
                        error!(
                            "failed to respond to pending watch request for address {:?} on interface {}: {:?}",
                            address,
                            id,
                            e
                        )
                    });
            }
            AddressStateProviderEvent::Canceled(reason) => {
                break Some(reason);
            }
        }
    };

    (
        match cancelation_reason {
            Some(AddressStateProviderCancellationReason::DadFailed)
            | Some(AddressStateProviderCancellationReason::Forfeited)
            | Some(AddressStateProviderCancellationReason::InterfaceRemoved) => {
                AddressNeedsExplicitRemovalFromCore::No
            }
            Some(AddressStateProviderCancellationReason::UserRemoved)
            | Some(AddressStateProviderCancellationReason::InvalidProperties)
            | None => AddressNeedsExplicitRemovalFromCore::Yes,
        },
        cancelation_reason,
    )
}

#[derive(Error, Debug)]
pub(crate) enum AddressStateProviderError {
    #[error("received a `WatchAddressAssignmentState` request while a previous request is pending")]
    PreviousPendingWatchRequest,
    #[error("FIDL error: {0}")]
    Fidl(fidl::Error),
    #[error("invalid address properties update for {field}: {err}")]
    InvalidPropertiesUpdate { field: &'static str, err: NotPositiveMonotonicInstantError },
    #[error("address not found in core")]
    AddressNotFound,
}

// State Machine for the `WatchAddressAssignmentState` "Hanging-Get" FIDL API.
#[derive(Debug)]
enum AddressAssignmentWatcherStateMachine {
    // Holds the new assignment state waiting to be sent.
    UnreportedUpdate(fnet_interfaces::AddressAssignmentState),
    // Holds the hanging responder waiting for a new assignment state to send.
    HangingRequest(fnet_interfaces_admin::AddressStateProviderWatchAddressAssignmentStateResponder),
    Idle,
}
struct AddressAssignmentWatcherState {
    // The state of the `WatchAddressAssignmentState` "Hanging-Get" FIDL API.
    fsm: AddressAssignmentWatcherStateMachine,
    // The last response to a `WatchAddressAssignmentState` FIDL request.
    // `None` until the first request, after which it will always be `Some`.
    last_response: Option<fnet_interfaces::AddressAssignmentState>,
}

impl AddressAssignmentWatcherState {
    // Handle a change in `AddressAssignmentState` as published by Core.
    fn on_new_assignment_state(
        &mut self,
        new_state: fnet_interfaces::AddressAssignmentState,
    ) -> Result<(), fidl::Error> {
        use AddressAssignmentWatcherStateMachine::*;
        let Self { fsm, last_response } = self;
        // Use `Idle` as a placeholder value to take ownership of `fsm`.
        let old_fsm = std::mem::replace(fsm, Idle);
        let (new_fsm, result) = match old_fsm {
            UnreportedUpdate(old_state) => {
                if old_state == new_state {
                    warn!("received duplicate AddressAssignmentState event from Core.");
                }
                if self.last_response == Some(new_state) {
                    // Return to `Idle` because we've coalesced
                    // multiple updates and no-longer have new state to send.
                    (Idle, Ok(()))
                } else {
                    (UnreportedUpdate(new_state), Ok(()))
                }
            }
            HangingRequest(responder) => {
                *last_response = Some(new_state);
                (Idle, responder.send(new_state))
            }
            Idle => (UnreportedUpdate(new_state), Ok(())),
        };
        assert_matches!(std::mem::replace(fsm, new_fsm), Idle);
        result
    }

    // Handle a new `WatchAddressAssignmentState` FIDL request.
    fn on_new_watch_req(
        &mut self,
        responder: fnet_interfaces_admin::AddressStateProviderWatchAddressAssignmentStateResponder,
    ) -> Result<(), AddressStateProviderError> {
        use AddressAssignmentWatcherStateMachine::*;
        let Self { fsm, last_response } = self;
        // Use `Idle` as a placeholder value to take ownership of `fsm`.
        let old_fsm = std::mem::replace(fsm, Idle);
        let (new_fsm, result) = match old_fsm {
            UnreportedUpdate(state) => {
                *last_response = Some(state);
                (Idle, responder.send(state).map_err(AddressStateProviderError::Fidl))
            }
            HangingRequest(_existing_responder) => (
                HangingRequest(responder),
                Err(AddressStateProviderError::PreviousPendingWatchRequest),
            ),
            Idle => (HangingRequest(responder), Ok(())),
        };
        assert_matches!(std::mem::replace(fsm, new_fsm), Idle);
        result
    }
}

struct UserRemove;

/// Serves a `fuchsia.net.interfaces.admin/AddressStateProvider` request.
///
/// If `Ok(Some(UserRemove))` is returned, the client has explicitly
/// requested removal of the address, but the address has not been removed yet.
fn dispatch_address_state_provider_request(
    ctx: &mut Ctx,
    req: fnet_interfaces_admin::AddressStateProviderRequest,
    address: SpecifiedAddr<IpAddr>,
    id: BindingId,
    detached: &mut bool,
    watch_state: &mut AddressAssignmentWatcherState,
) -> Result<Option<UserRemove>, AddressStateProviderError> {
    debug!("serving {:?}", req);
    match req {
        fnet_interfaces_admin::AddressStateProviderRequest::UpdateAddressProperties {
            address_properties:
                fnet_interfaces_admin::AddressProperties {
                    valid_lifetime_end: valid_lifetime_end_nanos,
                    preferred_lifetime_info,
                    __source_breaking: fidl::marker::SourceBreaking,
                },
            responder,
        } => {
            let device_id =
                ctx.bindings_ctx().devices.get_core_id(id).expect("interface not found");
            let valid_until = valid_lifetime_end_nanos
                .map(|nanos| {
                    PositiveMonotonicInstant::try_from(nanos)
                        .map(|i| Lifetime::from_zx_time(i.into()))
                        .map_err(|err| AddressStateProviderError::InvalidPropertiesUpdate {
                            field: "valid_until",
                            err,
                        })
                })
                .unwrap_or(Ok(Lifetime::Infinite))?;
            let preferred_lifetime = preferred_lifetime_info
                .map(|p| {
                    fnet_interfaces_ext::PreferredLifetimeInfo::try_from(p).map(|p| p.into_core())
                })
                .unwrap_or(Ok(PreferredLifetime::preferred_forever()))
                .map_err(|err| AddressStateProviderError::InvalidPropertiesUpdate {
                    field: "preferred_lifetime_info",
                    err,
                })?
                .map_instant(StackTime::from_zx);

            let common_props = CommonAddressProperties { valid_until, preferred_lifetime };
            let result = match address.into() {
                IpAddr::V4(address) => ctx.api().device_ip::<Ipv4>().set_addr_properties(
                    &device_id,
                    address,
                    common_props,
                ),
                IpAddr::V6(address) => ctx.api().device_ip::<Ipv6>().set_addr_properties(
                    &device_id,
                    address,
                    common_props,
                ),
            };
            match result {
                Ok(()) => {
                    responder.send().map_err(AddressStateProviderError::Fidl)?;
                    Ok(None)
                }
                Err(SetIpAddressPropertiesError::NotFound(
                    netstack3_core::error::NotFoundError,
                )) => Err(AddressStateProviderError::AddressNotFound),
                Err(SetIpAddressPropertiesError::NotManual) => {
                    panic!("address not manual")
                }
            }
        }
        fnet_interfaces_admin::AddressStateProviderRequest::WatchAddressAssignmentState {
            responder,
        } => {
            watch_state.on_new_watch_req(responder)?;
            Ok(None)
        }
        fnet_interfaces_admin::AddressStateProviderRequest::Detach { control_handle: _ } => {
            *detached = true;
            Ok(None)
        }
        fnet_interfaces_admin::AddressStateProviderRequest::Remove { control_handle: _ } => {
            Ok(Some(UserRemove))
        }
    }
}

fn send_address_removal_event(
    addr: IpAddr,
    id: impl Debug,
    control_handle: fnet_interfaces_admin::AddressStateProviderControlHandle,
    reason: fnet_interfaces_admin::AddressRemovalReason,
) {
    control_handle.send_on_address_removed(reason).unwrap_or_else(|e| {
        error!(
            "failed to send address removal reason for addr {:?} on interface {:?}: {:?}",
            addr, id, e
        );
    })
}

mod enabled {
    use super::*;
    use crate::bindings::{
        BindingsCtx, DeviceSpecificInfo, DynamicEthernetInfo, DynamicNetdeviceInfo,
    };
    use futures::lock::{self, Mutex as AsyncMutex};
    use netstack3_core::ip::{Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfigurationUpdate};

    /// A helper that provides interface enabling and disabling under an
    /// interface-scoped lock.
    ///
    /// All interface enabling and disabling must go through this controller to
    /// guarantee that futures are not racing to act on the same status that is
    /// protected by locks owned by device structures in core.
    pub(super) struct InterfaceEnabledController {
        id: BindingId,
        ctx: AsyncMutex<Ctx>,
    }

    /// Configuration updater that takes IP enablement into account.
    ///
    /// This is needed because all core IP enablement state is protected by the
    /// lock on [`InterfaceEnabledController`].
    ///
    /// This struct encapsulates the lifetime of the lock by holding the lock
    /// guard.
    pub(super) struct InterfaceConfigurationUpdater<'ctx> {
        ctx: lock::MutexGuard<'ctx, Ctx>,
        device_id: DeviceId<BindingsCtx>,
    }

    impl InterfaceEnabledController {
        pub(super) fn new(ctx: Ctx, id: BindingId) -> Self {
            Self { id, ctx: AsyncMutex::new(ctx) }
        }

        /// Sets this controller's interface to `admin_enabled = enabled`.
        ///
        /// Returns `true` if the value of `admin_enabled` changed in response
        /// to this call.
        pub(super) async fn set_admin_enabled(&self, enabled: bool) -> bool {
            let Self { id, ctx } = self;
            let mut ctx = ctx.lock().await;
            enum Info<A, B, C, D> {
                Loopback(A),
                Ethernet(B),
                PureIp(C),
                Blackhole(D),
            }

            let core_id = ctx.bindings_ctx().devices.get_core_id(*id).expect("device not present");
            let (port_handler, status_sampler) = {
                let (mut info, port_handler, status_sampler) = match core_id.external_state() {
                    devices::DeviceSpecificInfo::Loopback(devices::LoopbackInfo {
                        static_common_info: _,
                        dynamic_common_info,
                        rx_notifier: _,
                    }) => (Info::Loopback(dynamic_common_info.write()), None, None),
                    devices::DeviceSpecificInfo::Ethernet(devices::EthernetInfo {
                        netdevice,
                        common_info: _,
                        dynamic_info,
                        mac: _,
                        multicast_event_sink: _,
                        status_sampler,
                    }) => (
                        Info::Ethernet(dynamic_info.write()),
                        Some(&netdevice.handler),
                        Some(status_sampler),
                    ),
                    devices::DeviceSpecificInfo::PureIp(devices::PureIpDeviceInfo {
                        netdevice,
                        common_info: _,
                        dynamic_info,
                        status_sampler,
                    }) => (
                        Info::PureIp(dynamic_info.write()),
                        Some(&netdevice.handler),
                        Some(status_sampler),
                    ),
                    devices::DeviceSpecificInfo::Blackhole(devices::BlackholeDeviceInfo {
                        common_info: _,
                        dynamic_common_info,
                    }) => (Info::Blackhole(dynamic_common_info.write()), None, None),
                };
                let common_info = match info {
                    Info::Loopback(ref mut common_info) => common_info.deref_mut(),
                    Info::Blackhole(ref mut common_info) => common_info.deref_mut(),
                    Info::Ethernet(ref mut dynamic) => &mut dynamic.netdevice.common_info,
                    Info::PureIp(ref mut dynamic) => &mut dynamic.common_info,
                };

                // Already set to expected value.
                if common_info.admin_enabled == enabled {
                    return false;
                }
                common_info.admin_enabled = enabled;

                (port_handler, status_sampler)
            };

            if let Some(handler) = port_handler {
                let r = if enabled { handler.attach().await } else { handler.detach().await };
                match r {
                    Ok(()) => (),
                    Err(e) => {
                        warn!("failed to set port {:?} to {}: {:?}", handler, enabled, e);
                        // NB: There might be other errors here to consider in the
                        // future, we start with a very strict set of known errors to
                        // allow and panic on anything that is unexpected.
                        match e {
                            // We can race with the port being removed or the device
                            // being destroyed.
                            netdevice_client::Error::Attach(_, zx::Status::NOT_FOUND)
                            | netdevice_client::Error::Detach(_, zx::Status::NOT_FOUND) => (),
                            netdevice_client::Error::Fidl(e) if e.is_closed() => (),
                            e => panic!(
                                "unexpected error setting enabled={} on port {:?}: {:?}",
                                enabled, handler, e
                            ),
                        }
                    }
                }
            }
            Self::update_enabled_state(&mut ctx, &core_id);

            if let Some(status_sampler) = status_sampler {
                status_sampler.report_admin_state(enabled).await;
            }

            true
        }

        /// Sets this controller's interface to `phy_up = online`.
        pub(super) async fn set_phy_up(&self, online: bool) {
            let Self { id, ctx } = self;
            let mut ctx = ctx.lock().await;
            let core_id = ctx.bindings_ctx().devices.get_core_id(*id).expect("device not present");
            let (original_state, status_sampler) = match core_id.external_state() {
                devices::DeviceSpecificInfo::Ethernet(i) => {
                    let prev = i.with_dynamic_info_mut(|i| {
                        std::mem::replace(&mut i.netdevice.phy_up, online)
                    });
                    (prev, &i.status_sampler)
                }
                i @ (devices::DeviceSpecificInfo::Loopback(_)
                | devices::DeviceSpecificInfo::Blackhole(_)) => {
                    unreachable!("unexpected device info {:?} for interface {}", i, *id)
                }
                devices::DeviceSpecificInfo::PureIp(i) => {
                    let prev =
                        i.with_dynamic_info_mut(|i| std::mem::replace(&mut i.phy_up, online));
                    (prev, &i.status_sampler)
                }
            };

            match (original_state, online) {
                (true, true) | (false, false) => {
                    debug!(
                        "observed no-op port status update on interface {:?}. online = {}",
                        core_id, online
                    );
                }
                (true, false) => {
                    warn!("observed port status change to offline on interface {:?}", core_id)
                }
                (false, true) => {
                    info!("observed port status change to online on interface {:?}", core_id)
                }
            }

            if original_state != online {
                status_sampler.report_link_state(online).await;
            }

            // Enable or disable interface with context depending on new
            // online status. The helper functions take care of checking if
            // admin enable is the expected value.
            Self::update_enabled_state(&mut ctx, &core_id);
        }

        pub(super) async fn config_updater(&self) -> InterfaceConfigurationUpdater<'_> {
            let Self { ctx, id } = self;
            let ctx = ctx.lock().await;
            let device_id =
                ctx.bindings_ctx().devices.get_core_id(*id).expect("device not present");
            InterfaceConfigurationUpdater { ctx, device_id }
        }

        /// Commits interface enabled state to core.
        ///
        /// # Panics
        ///
        /// Panics if `id` is not a valid installed interface identifier.
        ///
        /// Panics if `should_enable` is `false` but the device state reflects
        /// that it should be enabled.
        fn update_enabled_state(ctx: &mut Ctx, core_id: &DeviceId<BindingsCtx>) {
            let EffectiveIpEnabledFlags { ipv4_enabled, ipv6_enabled } =
                EffectiveIpEnabledFlags::from_device_info(core_id.external_state());

            // The update functions from core are already capable of identifying
            // deltas and return the previous values for us. Log the deltas for
            // info.
            let was_v4_previously_enabled = ctx
                .api()
                .device_ip::<Ipv4>()
                .update_configuration(
                    core_id,
                    Ipv4DeviceConfigurationUpdate {
                        ip_config: IpDeviceConfigurationUpdate {
                            ip_enabled: Some(ipv4_enabled),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .expect("changing ip_enabled should never fail")
                .ip_config
                .ip_enabled
                .expect("ip enabled must be informed");

            let was_v6_previously_enabled = ctx
                .api()
                .device_ip::<Ipv6>()
                .update_configuration(
                    core_id,
                    Ipv6DeviceConfigurationUpdate {
                        ip_config: IpDeviceConfigurationUpdate {
                            ip_enabled: Some(ipv6_enabled),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .expect("changing ip_enabled should never fail")
                .ip_config
                .ip_enabled
                .expect("ip enabled must be informed");

            // LINT.IfChange(netstack_ip_enabled_tefmo)
            info!(
                "updated core state to ipv4_enabled={ipv4_enabled}, ipv6_enabled={ipv6_enabled} on {core_id:?}, \
                prev v4={was_v4_previously_enabled},v6={was_v6_previously_enabled}"
            );
            // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netstack_ip_enabled_tefmo)
        }
    }

    impl<'ctx> InterfaceConfigurationUpdater<'ctx> {
        pub(super) fn api_provider(&mut self) -> ApiProvider<'_> {
            ApiProvider { ctx: &mut *self.ctx, device_id: &self.device_id }
        }

        pub(super) fn device_id(&self) -> &DeviceId<BindingsCtx> {
            &self.device_id
        }
    }

    /// Provides access to the APIs to Core.
    pub(super) struct ApiProvider<'a> {
        ctx: &'a mut Ctx,
        device_id: &'a DeviceId<BindingsCtx>,
    }

    impl<'a> ApiProvider<'a> {
        /// Provides the core API struct.
        pub(super) fn api(&mut self) -> netstack3_core::CoreApi<'_, &mut BindingsCtx> {
            self.ctx.api()
        }

        /// Overrides the `new_ip_configuration_update` from the device_ip API with the
        /// state kept in the `device_id`.
        pub(super) fn new_ip_configuration_update<I: IpDeviceIpExt>(
            &mut self,
            update: I::ConfigurationUpdate,
        ) -> Result<
            PendingIpDeviceConfigurationUpdate<'a, I, DeviceId<BindingsCtx>>,
            UpdateIpConfigurationError,
        > {
            let Self { ctx, device_id } = self;
            let ip_enabled = EffectiveIpEnabledFlags::from_device_info(device_id.external_state())
                .ip_enabled::<I>();

            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Input<'a, I: IpDeviceIpExt>(&'a mut Ctx, I::ConfigurationUpdate);

            #[derive(GenericOverIp)]
            #[generic_over_ip(I, Ip)]
            struct Output<'a, I: IpDeviceIpExt>(
                Result<
                    PendingIpDeviceConfigurationUpdate<'a, I, DeviceId<BindingsCtx>>,
                    UpdateIpConfigurationError,
                >,
            );

            let Output(result) = I::map_ip(
                Input(ctx, update),
                |Input(ctx, mut ipv4_update)| {
                    ipv4_update.ip_config.ip_enabled = Some(ip_enabled);
                    Output(
                        ctx.api()
                            .device_ip::<Ipv4>()
                            .new_configuration_update(device_id, ipv4_update),
                    )
                },
                |Input(ctx, mut ipv6_update)| {
                    ipv6_update.ip_config.ip_enabled = Some(ip_enabled);
                    Output(
                        ctx.api()
                            .device_ip::<Ipv6>()
                            .new_configuration_update(device_id, ipv6_update),
                    )
                },
            );
            result
        }

        pub(super) fn device_id(&self) -> &DeviceId<BindingsCtx> {
            self.device_id
        }
    }

    struct EffectiveIpEnabledFlags {
        ipv4_enabled: bool,
        ipv6_enabled: bool,
    }

    impl EffectiveIpEnabledFlags {
        fn from_device_info(device_info: DeviceSpecificInfo<'_>) -> Self {
            let (dev_enabled, ipv4_enabled, ipv6_enabled) = match device_info {
                DeviceSpecificInfo::Ethernet(i) => i.with_dynamic_info(
                    |DynamicEthernetInfo {
                         netdevice: DynamicNetdeviceInfo { phy_up, common_info },
                         neighbor_event_sink: _,
                     }| {
                        (
                            *phy_up && common_info.admin_enabled,
                            common_info.ipv4_enabled,
                            common_info.ipv6_enabled,
                        )
                    },
                ),
                DeviceSpecificInfo::Loopback(i) => i.with_dynamic_info(|common_info| {
                    (common_info.admin_enabled, common_info.ipv4_enabled, common_info.ipv6_enabled)
                }),
                DeviceSpecificInfo::PureIp(i) => {
                    i.with_dynamic_info(|DynamicNetdeviceInfo { phy_up, common_info }| {
                        (
                            *phy_up && common_info.admin_enabled,
                            common_info.ipv4_enabled,
                            common_info.ipv6_enabled,
                        )
                    })
                }
                DeviceSpecificInfo::Blackhole(i) => i.with_dynamic_info(|common_info| {
                    (common_info.admin_enabled, common_info.ipv4_enabled, common_info.ipv6_enabled)
                }),
            };
            Self {
                ipv4_enabled: dev_enabled && ipv4_enabled,
                ipv6_enabled: dev_enabled && ipv6_enabled,
            }
        }

        /// Returns the enablement state of the given IP version.
        fn ip_enabled<I: Ip>(&self) -> bool {
            I::map_ip_in((), |()| self.ipv4_enabled, |()| self.ipv6_enabled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use net_declare::fidl_subnet;

    use crate::bindings::integration_tests::{StackSetupBuilder, TestSetup, TestSetupBuilder};
    use crate::bindings::interfaces_watcher::{InterfaceEvent, InterfaceUpdate};
    use crate::bindings::util::IntoFidl as _;

    // Verifies that when an an interface is removed, its addresses are
    // implicitly removed, rather then explicitly removed one-by-one. Explicit
    // removal would be redundant and is unnecessary.
    #[fixture::teardown(TestSetup::shutdown)]
    #[fuchsia::test(logging = false)]
    async fn implicit_address_removal_on_interface_removal() {
        const ENDPOINT: &'static str = "endpoint";

        let (spy_sink, event_receiver) = futures::channel::mpsc::unbounded();
        let mut t = TestSetupBuilder::new()
            .add_named_endpoint(ENDPOINT)
            .add_stack(StackSetupBuilder::new().spy_interface_events(spy_sink))
            .build()
            .await;

        // We just want to add and remove an interface from the stack. Since loopback interfaces
        // are never removed (except for during stack teardown), it's better to use a netdevice
        // interface instead.
        let (endpoint, port_id) = t.get_endpoint(ENDPOINT).await;

        let test_stack = t.get(0);

        let (device_control_proxy, device_control_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fnet_interfaces_admin::DeviceControlMarker>(
            );

        let device_control_scope = fasync::Scope::new_with_name("test_scope");
        let _: fasync::JoinHandle<()> = device_control_scope.spawn(
            run_device_control(test_stack.netstack(), endpoint, device_control_request_stream)
                .map(|e| e.expect("terminated with error")),
        );

        let (interface_control, control_server_end) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::ControlMarker>();

        device_control_proxy
            .create_interface(
                &port_id,
                control_server_end,
                fnet_interfaces_admin::Options::default(),
            )
            .expect("create interface");

        let binding_id = interface_control.get_id().await.expect("get ID");

        // Filter for only events on this interface.
        let event_receiver = event_receiver
            .filter(|event| match event {
                InterfaceEvent::Added { id }
                | InterfaceEvent::Reserved { id, properties: _ }
                | InterfaceEvent::Changed { id, event: _ }
                | InterfaceEvent::Removed(id) => futures::future::ready(id.get() == binding_id),
            })
            .fuse();
        let mut event_receiver = pin!(event_receiver);

        // We should see the interface get reserved and added.
        let event = event_receiver.next().await;
        assert_matches!(event, Some(InterfaceEvent::Reserved {
            id,
            properties: _,
         }) if id.get() == binding_id);
        assert_matches!(event_receiver.next().await, Some(InterfaceEvent::Added {
            id,
        }) if id.get() == binding_id);

        // Add an address.
        let (asp_client_end, asp_server_end) =
            fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
        let addr = fidl_subnet!("1.1.1.1/32");
        interface_control
            .add_address(
                &addr,
                &fnet_interfaces_admin::AddressParameters::default(),
                asp_server_end,
            )
            .expect("failed to add address");

        // Observe the `AddressAdded` event.
        let event = event_receiver.next().await;
        assert_matches!(event, Some(InterfaceEvent::Changed {
            id,
            event: InterfaceUpdate::AddressAdded {
                addr: address,
                assignment_state: _,
                valid_until: _,
                preferred_lifetime: _,
            }
        }) if (id.get() == binding_id && address.into_fidl() == addr ));
        let mut asp_event_stream = asp_client_end.take_event_stream();
        let event = asp_event_stream
            .try_next()
            .await
            .expect("read AddressStateProvider event")
            .expect("AddressStateProvider event stream unexpectedly empty");
        assert_matches!(event, fnet_interfaces_admin::AddressStateProviderEvent::OnAddressAdded {});

        // Drop the device control handle and expect the device task to exit.
        // This will cause the interface to be removed as well.
        drop(device_control_proxy);
        // Ensure all spawned tasks exit.
        device_control_scope.join().await;

        // Expect that the event receiver observes the interface being removed
        // without ever seeing an `AddressRemoved` event, which would indicate
        // the address was explicitly removed.
        assert_matches!(event_receiver.next().await,
        Some(InterfaceEvent::Removed(id)) if id.get() == binding_id);

        // Verify the ASP closed for the correct reason.
        let event = asp_event_stream
            .try_next()
            .await
            .expect("read AddressStateProvider event")
            .expect("AddressStateProvider event stream unexpectedly empty");
        assert_matches!(
            event,
            fnet_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved { error } => {
                assert_eq!(error, fnet_interfaces_admin::AddressRemovalReason::InterfaceRemoved)
            }
        );

        t
    }
}
