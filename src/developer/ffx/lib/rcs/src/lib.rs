// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol::{
    ConnectCapabilityError, RemoteControlMarker, RemoteControlProxy,
};
use fdomain_fuchsia_kernel as proto_fuchsia_kernel;
pub use fdomain_fuchsia_sys2::OpenDirType;
use fdomain_fuchsia_sys2::{
    ConfigOverrideMarker, ConfigOverrideProxy, LifecycleControllerMarker, LifecycleControllerProxy,
    RealmQueryMarker, RealmQueryProxy, RouteValidatorMarker, RouteValidatorProxy,
};
use fidl_fuchsia_developer_ffx as ffx;
use futures::StreamExt;
use std::convert::Infallible;
use std::time::{Duration, Instant};
use timeout::timeout;

pub mod toolbox;

/// Note that this is only used for backwards compatibility. All new usages should prefer using the
/// toolbox moniker instead.
const REMOTE_CONTROL_MONIKER: &str = "core/remote-control";

pub const RCS_KNOCK_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum RcsError {
    #[error("FIDL error: {0}")]
    Fidl(#[from] fidl::Error),

    #[error("Timeout error: {0}")]
    Timeout(#[from] timeout::TimeoutError),

    #[error(
        "The plugin service did not match any capabilities on the target for moniker '{moniker}' and capability '{capability}'.\n\nIt is possible that the expected component is either not built into the system image, or that the package server has not been setup.\n\nFor users, ensure your Fuchsia device is registered with ffx. To do this you can run:\n\n$ ffx target repository register -r $IMAGE_TYPE --alias fuchsia.com\n\nFor plugin developers, it may be possible that the moniker you're attempting to connect to is incorrect.\nYou can use `ffx component explore '{moniker}'` to explore the component topology of your target device to fix this moniker if this is the case.\n\nIf you believe you have encountered a bug after walking through the above please report it at https://fxbug.dev/new/ffx+User+Bug"
    )]
    NoMatchingCapabilities { moniker: String, capability: String },

    #[error(
        "Service dependency exists but connecting to it failed with error {error:?}. Moniker: {moniker}. Capability name: {capability}"
    )]
    ConnectionFailed { moniker: String, capability: String, error: ConnectCapabilityError },

    #[error(
        "Timed out connecting to capability: '{capability}' with moniker: '{moniker}'.\nThis is likely due to a sudden shutdown or disconnect of the target.\nIf you have encountered what you think is a bug, Please report it at https://fxbug.dev/new/ffx+User+Bug\n\nTo diagnose the issue, use `ffx doctor`."
    )]
    TimedOutConnecting { moniker: String, capability: String },
}

#[derive(thiserror::Error, Debug)]
pub enum KnockRcsError {
    #[error("FIDL error {0:?}")]
    FidlError(#[from] fidl::Error),
    #[error("FDomain error {0:?}")]
    FDomainError(#[from] fdomain_client::Error),
    #[error("Creating FIDL channel: {0:?}")]
    ChannelError(#[from] fidl::handle::Status),
    #[error("Connecting to RCS {0:?}")]
    RcsConnectCapabilityError(ConnectCapabilityError),
    #[error("Could not knock service from RCS")]
    FailedToKnock,
}

impl From<Infallible> for KnockRcsError {
    fn from(_value: Infallible) -> Self {
        unreachable!()
    }
}

/// Attempts to "knock" RCS.
///
/// This can be used to verify whether it is up and running, or as a control flow to ensure that
/// RCS is up and running before continuing time-sensitive operations.
// TODO(b/339266778): Use non-FIDL error type.
pub async fn knock_rcs(rcs_proxy: &RemoteControlProxy) -> Result<(), ffx::TargetConnectionError> {
    knock_rcs_impl(rcs_proxy).await.map_err(|e| match e {
        KnockRcsError::FidlError(e) => {
            log::warn!("FIDL error: {:?}", e);
            ffx::TargetConnectionError::FidlCommunicationError
        }
        KnockRcsError::FDomainError(e) => {
            log::warn!("FDomain error: {:?}", e);
            ffx::TargetConnectionError::FidlCommunicationError
        }
        KnockRcsError::ChannelError(e) => {
            log::warn!("RCS connect channel err: {:?}", e);
            ffx::TargetConnectionError::FidlCommunicationError
        }
        KnockRcsError::RcsConnectCapabilityError(c) => {
            log::warn!("RCS failed connecting to itself for knocking: {:?}", c);
            ffx::TargetConnectionError::RcsConnectionError
        }
        KnockRcsError::FailedToKnock => ffx::TargetConnectionError::FailedToKnockService,
    })
}

type KnockClientType = fidl::client::Client<fdomain_client::fidl::FDomainResourceDialect>;

async fn connect_to_rcs(
    rcs_proxy: &RemoteControlProxy,
    moniker: &str,
    capability_set: OpenDirType,
    capability_name: &str,
) -> Result<KnockClientType, KnockRcsError> {
    let rcs_client = rcs_proxy.domain();
    // Try to connect via fuchsia.developer.remotecontrol/RemoteControl.ConnectCapability.
    let (client, server) = rcs_client.create_channel();

    rcs_proxy
        .connect_capability(moniker, capability_set, capability_name, server)
        .await?
        .map_err(|e| KnockRcsError::RcsConnectCapabilityError(e))?;
    Ok(KnockClientType::new(client, "knock_client"))
}

async fn knock_rcs_impl(rcs_proxy: &RemoteControlProxy) -> Result<(), KnockRcsError> {
    let knock_client = match connect_to_rcs(
        rcs_proxy,
        toolbox::MONIKER,
        OpenDirType::NamespaceDir,
        &format!("svc/{}", RemoteControlMarker::PROTOCOL_NAME),
    )
    .await
    {
        Ok(client) => client,
        Err(KnockRcsError::RcsConnectCapabilityError(_)) => {
            // Fallback to the legacy moniker if toolbox doesn't contain the capability.
            connect_to_rcs(
                rcs_proxy,
                REMOTE_CONTROL_MONIKER,
                OpenDirType::ExposedDir,
                RemoteControlMarker::PROTOCOL_NAME,
            )
            .await?
        }
        Err(e) => return Err(e),
    };

    let mut event_receiver = knock_client.take_event_receiver();
    let res = timeout(RCS_KNOCK_TIMEOUT, event_receiver.next()).await;
    match res {
        // no events are expected -- the only reason we'll get an event is if
        // channel closes. So the only valid response here is a timeout.
        Err(_) => Ok(()),
        Ok(r) => r.ok_or(KnockRcsError::FailedToKnock).map(drop),
    }
}

pub trait ProtocolMarker: fdomain_client::fidl::ProtocolMarker {}

impl<T> ProtocolMarker for T where T: fdomain_client::fidl::ProtocolMarker {}

pub async fn open_with_timeout_at<T: ProtocolMarker>(
    dur: Duration,
    moniker: &str,
    capability_set: OpenDirType,
    capability_name: &str,
    rcs_proxy: &RemoteControlProxy,
) -> std::result::Result<T::Proxy, RcsError> {
    let connect_capability_fut = async move {
        // Try to connect via fuchsia.developer.remotecontrol/RemoteControl.ConnectCapability.
        let (proxy, server) = rcs_proxy.domain().create_proxy::<T>();
        log::info!("RCS: Connecting to capability '{}' at moniker '{}'", capability_name, moniker);
        let res = rcs_proxy
            .connect_capability(moniker, capability_set, capability_name, server.into_channel())
            .await;
        log::info!("RCS: ConnectCapability response received for '{}'", capability_name);
        res.map(|result| result.map(|_| proxy))
    };
    let result = timeout::timeout(dur, connect_capability_fut).await;

    let result = result.map_err(|_| RcsError::TimedOutConnecting {
        moniker: moniker.to_string(),
        capability: capability_name.to_string(),
    })?;

    let result = result.map_err(RcsError::Fidl)?;

    let proxy = result.map_err(|e| match e {
        ConnectCapabilityError::NoMatchingCapabilities => RcsError::NoMatchingCapabilities {
            moniker: moniker.to_string(),
            capability: capability_name.to_string(),
        },
        _ => RcsError::ConnectionFailed {
            moniker: moniker.to_string(),
            capability: capability_name.to_string(),
            error: e,
        },
    })?;

    Ok(proxy)
}

pub async fn connect_with_timeout_at<T: ProtocolMarker>(
    dur: Duration,
    moniker: &str,
    capability_name: &str,
    rcs_proxy: &RemoteControlProxy,
) -> std::result::Result<T::Proxy, RcsError> {
    open_with_timeout_at::<T>(dur, moniker, OpenDirType::ExposedDir, capability_name, rcs_proxy)
        .await
}

pub async fn connect_with_timeout<P: ProtocolMarker + DiscoverableProtocolMarker>(
    dur: Duration,
    moniker: &str,
    rcs_proxy: &RemoteControlProxy,
) -> std::result::Result<P::Proxy, RcsError> {
    open_with_timeout_at::<P>(dur, moniker, OpenDirType::ExposedDir, P::PROTOCOL_NAME, rcs_proxy)
        .await
}

pub async fn connect_to_protocol<P: DiscoverableProtocolMarker>(
    dur: Duration,
    moniker: &str,
    rcs_proxy: &RemoteControlProxy,
) -> std::result::Result<P::Proxy, RcsError> {
    connect_with_timeout::<P>(dur, moniker, rcs_proxy).await
}

pub async fn open_with_timeout<P: DiscoverableProtocolMarker>(
    dur: Duration,
    moniker: &str,
    capability_set: OpenDirType,
    rcs_proxy: &RemoteControlProxy,
) -> std::result::Result<P::Proxy, RcsError> {
    open_with_timeout_at::<P>(dur, moniker, capability_set, P::PROTOCOL_NAME, rcs_proxy).await
}

async fn get_cf_root_from_namespace<M: DiscoverableProtocolMarker>(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<M::Proxy, RcsError> {
    let start_time = Instant::now();
    let res = open_with_timeout_at::<M>(
        timeout,
        toolbox::MONIKER,
        OpenDirType::NamespaceDir,
        &format!("svc/{}.root", M::PROTOCOL_NAME),
        rcs_proxy,
    )
    .await;
    // Fallback to the legacy remote control moniker if toolbox doesn't contain the capability.
    match res {
        Ok(proxy) => Ok(proxy),
        Err(_) => {
            let timeout = timeout.saturating_sub(Instant::now() - start_time);
            open_with_timeout_at::<M>(
                timeout,
                REMOTE_CONTROL_MONIKER,
                OpenDirType::NamespaceDir,
                &format!("svc/{}.root", M::PROTOCOL_NAME),
                rcs_proxy,
            )
            .await
        }
    }
}

pub async fn kernel_stats(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<proto_fuchsia_kernel::StatsProxy, RcsError> {
    let start_time = Instant::now();
    let res = open_with_timeout_at::<proto_fuchsia_kernel::StatsMarker>(
        timeout,
        toolbox::MONIKER,
        OpenDirType::NamespaceDir,
        &format!("svc/{}", proto_fuchsia_kernel::StatsMarker::PROTOCOL_NAME),
        rcs_proxy,
    )
    .await;
    // Fallback to the legacy remote control moniker if toolbox doesn't contain the capability.
    match res {
        Ok(proxy) => Ok(proxy),
        Err(_) => {
            let timeout = timeout.saturating_sub(Instant::now() - start_time);
            open_with_timeout_at::<proto_fuchsia_kernel::StatsMarker>(
                timeout,
                REMOTE_CONTROL_MONIKER,
                OpenDirType::NamespaceDir,
                &format!("svc/{}", proto_fuchsia_kernel::StatsMarker::PROTOCOL_NAME),
                rcs_proxy,
            )
            .await
        }
    }
}

pub async fn root_config_override(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<ConfigOverrideProxy, RcsError> {
    get_cf_root_from_namespace::<ConfigOverrideMarker>(rcs_proxy, timeout).await
}

pub async fn root_realm_query(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<RealmQueryProxy, RcsError> {
    get_cf_root_from_namespace::<RealmQueryMarker>(rcs_proxy, timeout).await
}

pub async fn root_lifecycle_controller(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<LifecycleControllerProxy, RcsError> {
    get_cf_root_from_namespace::<LifecycleControllerMarker>(rcs_proxy, timeout).await
}

pub async fn root_route_validator(
    rcs_proxy: &RemoteControlProxy,
    timeout: Duration,
) -> std::result::Result<RouteValidatorProxy, RcsError> {
    get_cf_root_from_namespace::<RouteValidatorMarker>(rcs_proxy, timeout).await
}
