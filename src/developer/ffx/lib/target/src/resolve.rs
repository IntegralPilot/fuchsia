// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::info;
use addr::{TargetAddr, TargetIpAddr};
use anyhow::Result;
use discovery::query::TargetInfoQuery;
use discovery::{Discovery, DiscoveryBuilder, DiscoverySources, TargetEvent, TargetHandle};
use fdomain_fuchsia_developer_remotecontrol::{IdentifyHostResponse, RemoteControlProxy};
use ffx_command_error::{NonFatalError, user_error};
use ffx_config::{EnvironmentContext, TryFromEnvContext, keys};
use ffx_diagnostics_analytics::ResultExt;
use ffx_diagnostics_formatting::TargetInfoQueryExt;
use fidl_fuchsia_developer_ffx::{self as ffx};
use fuchsia_async::TimeoutExt;
use futures::future::{BoxFuture, LocalBoxFuture};
use futures::{FutureExt, Stream, StreamExt, pin_mut};
use netext::IsLocalAddr;
use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use target_errors::{FfxTargetError, TargetSource};
use tokio::sync::Mutex;

use crate::analytics::PointOfFailure;
use crate::connection::Connection;
use crate::ssh_connector::SshConnector;
use crate::usb_connector::{UsbConnector, try_daemon_autostart};
use crate::vsock_connector::VSockConnector;
use crate::{TargetInfo, get_target_specifier_with_source, target_source_for_query};

const CONFIG_TARGET_SSH_TIMEOUT: &str = "target.host_pipe_ssh_timeout";

const TARGET_DEFAULT_PORT: u16 = 22;
const DEFAULT_SSH_TIMEOUT_MS: u64 = 10000;

#[cfg(test)]
use {mockall::mock, mockall::predicate::*};

/// Check if daemon discovery is disabled, resolving locally if so.
pub async fn maybe_locally_resolve_target_spec(
    target_spec: &TargetInfoQuery,
    env_context: &EnvironmentContext,
) -> Result<TargetInfoQuery> {
    // This should be read as "is discovery enabled on the daemon".
    if crate::is_discovery_enabled(env_context) {
        Ok(target_spec.clone())
    } else {
        log::warn!(
            "crate::is_discovery_enabled is false - using local target resolution. is_usb_discovery_disabled is {}, is_mdns_discovery_disabled is {}",
            ffx_config::is_usb_discovery_disabled(env_context),
            ffx_config::is_mdns_discovery_disabled(env_context)
        );

        let discovery = build_discovery_from_config(env_context);
        let resolver = DefaultTargetResolver::new(discovery);
        locally_resolve_target_spec(target_spec, &resolver, env_context).await
    }
}

fn replace_default_port(sa: SocketAddr) -> SocketAddr {
    let mut sa = sa;
    if sa.port() == 0 {
        sa.set_port(TARGET_DEFAULT_PORT);
    }
    sa
}

/// Attempts to resolve the query into an explicit string query that can be
/// passed to the daemon. If already an address or serial number, just return
/// it. Otherwise, perform discovery to find the address or serial #. Returns
/// Some(_) if a target has been found, None otherwise.
async fn locally_resolve_target_spec<T: TargetResolver>(
    target_spec: &TargetInfoQuery,
    resolver: &T,
    env_context: &EnvironmentContext,
) -> Result<TargetInfoQuery> {
    let query = TargetInfoQuery::from(target_spec.clone());
    let explicit_spec = match query {
        // If an address is passed in, make sure that the default port is filled in if it hadn't
        // been explicit, then just pass it on to the daemon as is
        TargetInfoQuery::Addr(addr) => format!("{}", replace_default_port(addr)),
        TargetInfoQuery::Id(sn) => format!("id:{sn}"),
        _ => {
            let source = target_source_for_query(target_spec, env_context);
            let resolution =
                resolver.resolve_single_target(&target_spec, true, env_context, source).await?;
            log::debug!("Locally resolved target '{target_spec:?}' to {:?}", resolution.discovered);
            resolution.target.to_spec()
        }
    };

    Ok(TargetInfoQuery::try_from(explicit_spec)?)
}

fn target_error_to_analytics<'a>(
    err: &'a FfxTargetError,
) -> Option<crate::analytics::PointOfFailure<'a>> {
    match err {
        FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::TargetNotFound,
            target,
            ..
        } => Some(crate::analytics::PointOfFailure::NoMatchingTargets {
            query: target.as_ref().map(|s| {
                TargetInfoQuery::try_from(s.clone())
                    .map(|q| q.to_analytics_tag())
                    .unwrap_or_else(|_| "invalid".to_owned())
            }),
            discovery_sources: DiscoverySources::all(),
        }),
        FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::QueryAmbiguous,
            target,
            ..
        } => Some(crate::analytics::PointOfFailure::TooManyMatchingTargets {
            query: target.as_ref().map(|s| {
                TargetInfoQuery::try_from(s.clone())
                    .map(|q| q.to_analytics_tag())
                    .unwrap_or_else(|_| "invalid".to_owned())
            }),
            discovery_sources: DiscoverySources::all(),
        }),
        _ => None,
    }
}

pub(crate) fn expect_single_target<T>(
    query: &TargetInfoQuery,
    targets: Vec<T>,
    source: Option<TargetSource>,
) -> Result<T, FfxTargetError>
where
    T: Display,
{
    match targets.len() {
        0 => Err(FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::TargetNotFound,
            target: Some(query.into()),
            targets: vec![],
            target_source: source,
        })
        .into(),
        1 => Ok(targets.into_iter().next().unwrap()),
        _ => Err(FfxTargetError::OpenTargetError {
            err: ffx::OpenTargetError::QueryAmbiguous,
            target: Some(query.into()),
            targets: targets.iter().map(|f| format!("{}", f)).collect(),
            target_source: source,
        })
        .into(),
    }
}

/// Discover a target; useful when we don't necessarily want to make a connection,
/// but we _do_ want to get the information available when discovering (TargetState,
/// etc).
pub async fn discover_single_default_target(
    ctx: &EnvironmentContext,
) -> std::result::Result<TargetHandle, crate::FfxTargetCrateError> {
    let (query_s, source) = get_target_specifier_with_source(ctx)?;
    let query = TargetInfoQuery::try_from(query_s)?;

    // Note: this will use the target cache if it exists
    let handles = get_discovered_targets(query.clone(), true, true, ctx).await?;
    let res = expect_single_target(&query, handles, source)
        .or_else_maybe_analytics(|e| target_error_to_analytics(e).map(Into::into))
        .await?;
    Ok(res)
}

/// A trait for resolving target queries into concrete target information.
///
/// This trait provides an abstraction over the process of finding a target
/// based on a user-provided specifier, which could be a nodename, a serial
/// number, an IP address, or other forms of target identification.
///
/// Implementors of this trait are responsible for searching for targets,
/// handling ambiguity, and returning a `Resolution` object that can be used
/// to connect to the target.
pub trait TargetResolver {
    /// Gets a set of handles from the resolver's sources that match the query
    fn discovered_targets(
        &self,
        query: TargetInfoQuery,
    ) -> impl Future<Output = Result<Vec<TargetHandle>>>;

    /// Attempts to resolve a target by name from a manually configured list.
    ///
    /// This method is used to check for targets that have been explicitly
    /// defined by the user, outside of the standard discovery mechanisms.
    fn try_resolve_manual_target(
        &self,
        name: &str,
        ctx: &EnvironmentContext,
    ) -> impl Future<Output = Result<Option<Resolution>>>;

    /// Resolves a target specifier into a `Resolution` containing a connectable address.
    ///
    /// This is a high-level method that should handle various query types and
    /// ensure that a single, unambiguous target is found. It is an error if the query
    /// does not resolve to exactly one target.
    fn resolve_target_address(
        &self,
        target_spec: &TargetInfoQuery,
        use_cache: bool,
        ctx: &EnvironmentContext,
        source: Option<TargetSource>,
    ) -> impl Future<Output = Result<Resolution, FfxTargetError>> {
        async move {
            if let Some(target_addr) = target_spec.get_target_addr() {
                let query_tag = target_spec.to_analytics_tag();
                emit_cache_event("explicit_addr", &query_tag).await;
                return Ok(Resolution::from_target(target_addr.into()));
            }
            let res = self.resolve_single_target(target_spec, use_cache, ctx, source).await?;
            let target_spec_info: String = target_spec.into();
            log::debug!("resolved target spec {target_spec_info} to address {:?}", res.addr());
            Ok(res)
        }
    }

    /// Resolves a target specifier to a single `Resolution`.
    ///
    /// This method orchestrates the discovery process, including checking
    /// manual targets. We return a Resolution because checking manual
    /// targets involves talking to them, so we want to take advantage of
    /// that to preserve the full Resolution state.
    fn discover_matching_targets(
        &self,
        target_spec: &TargetInfoQuery,
        env_context: &EnvironmentContext,
        source: Option<TargetSource>,
    ) -> impl Future<Output = Result<Vec<Resolution>, FfxTargetError>> {
        async move {
            let handles_fut = self.discovered_targets(target_spec.clone()).fuse();
            pin_mut!(handles_fut);
            let mut discovered: Option<Vec<TargetHandle>> = None;

            // If the query is not specified, we won't even bother with trying to resolve a manual target.
            if let TargetInfoQuery::NodenameOrId(s) = &target_spec {
                // We want to query both the manual targets and the discoverable handles concurrently.
                let manual_target_fut = self.try_resolve_manual_target(s, env_context).fuse();
                pin_mut!(manual_target_fut);
                futures::select! {
                    mtr = &mut manual_target_fut => match mtr {
                        Err(e) => {
                            log::debug!("Failed to resolve target {s} as manual target: {e:?}");
                            // Keep going, waiting for the discovery to complete
                        }
                        Ok(Some(res)) => return Ok(vec![res]), // We found a manual target, so we're done
                        _ => (), // No manual target with this name
                    },
                    handles_res = handles_fut => match handles_res {
                        Ok(r) => discovered = Some(r),
                        Err(e) => {
                            log::warn!("Target discovery failed: {e:?}");
                            return Err(FfxTargetError::OpenTargetError {
                                err: ffx::OpenTargetError::FailedDiscovery,
                                target: Some(target_spec.into()),
                                targets: vec![],
                                target_source: source.clone(),
                            })
                        },
                    },
                }
            }

            // If we haven't gotten a result yet (because the query wasn't
            // NodenameOrId, or because the manual-target query completed
            // without finding a target), get it now.
            let discovered = match discovered {
                Some(targets) => targets,
                None => handles_fut.await.map_err(|e| {
                    log::warn!("Target discovery failed: {e:?}");
                    FfxTargetError::OpenTargetError {
                        err: ffx::OpenTargetError::FailedDiscovery,
                        target: Some(target_spec.into()),
                        targets: vec![],
                        target_source: source.clone(),
                    }
                })?,
            };
            discovered
                .into_iter()
                .map(|th| {
                    Resolution::from_target_handle(th).map_err(|e| {
                        log::warn!("Conversion to Resolution failed: {e:?}");
                        FfxTargetError::OpenTargetError {
                            err: ffx::OpenTargetError::FailedDiscovery,
                            target: Some(target_spec.into()),
                            targets: vec![],
                            target_source: source.clone(),
                        }
                    })
                })
                .collect()
        }
    }

    /// Resolves a target specifier to a single `Resolution`.
    ///
    /// This method tries to look up the query in the cache. If it is not
    /// available there, it calls "discover_matching_targets()"
    fn resolve_single_target(
        &self,
        target_spec: &TargetInfoQuery,
        use_cache: bool,
        env_context: &EnvironmentContext,
        source: Option<TargetSource>,
    ) -> impl Future<Output = Result<Resolution, FfxTargetError>> {
        async move {
            let query_tag = target_spec.to_analytics_tag();
            let mut resolutions: Option<Vec<Resolution>> = None;
            if use_cache {
                if let Some(cache_file) = crate::cache::get_discovery_cache_file(env_context) {
                    match crate::cache::Cache::load(&cache_file) {
                        Ok(cache) => {
                            // Given a TargetInfo from the cache, we don't have much information -- just the target we've resolved
                            // it to
                            let matched: Vec<Resolution> = cache
                                .targets
                                .into_iter()
                                .filter(|ti| ti.match_query(target_spec))
                                .map(|ti| {
                                    ResolutionTarget::from_target_info(&ti)
                                        .map(Resolution::from_target)
                                })
                                .flatten()
                                .collect();
                            if matched.is_empty() {
                                // If the cache has no matches, leave `resolutions` as `None`
                                // so that target resolution falls back to active discovery.
                                emit_cache_event("cache_miss", &query_tag).await;
                            } else {
                                emit_cache_event("cache_hit", &query_tag).await;
                                resolutions = Some(matched);
                            }
                        }
                        Err(crate::cache::CacheError::OpenFile { err, .. })
                            if err.kind() == std::io::ErrorKind::NotFound =>
                        {
                            emit_cache_event("no_cache_file", &query_tag).await;
                        }
                        Err(crate::cache::CacheError::Expired(_)) => {
                            emit_cache_event("cache_expired", &query_tag).await;
                        }
                        Err(e) => {
                            emit_cache_event("cache_invalid", &query_tag).await;
                            log::warn!("Cache loading failed: {e:?}");
                        }
                    }
                } else {
                    emit_cache_event("no_cache_file", &query_tag).await;
                }
            } else {
                emit_cache_event("cache_disabled", &query_tag).await;
            }

            let resolutions = match resolutions {
                Some(rs) => rs,
                None => {
                    self.discover_matching_targets(target_spec, env_context, source.clone()).await?
                }
            };

            expect_single_target(target_spec, resolutions, source)
                .or_else_maybe_analytics(|e| target_error_to_analytics(e).map(Into::into))
                .await
        }
    }
}

#[cfg(test)]
mock! {
    TargetResolver{}
    impl TargetResolver for TargetResolver {
        async fn discovered_targets(
            &self,
            query: TargetInfoQuery,
        ) -> Result<Vec<TargetHandle>>;

        async fn try_resolve_manual_target(
            &self,
            name: &str,
            ctx: &EnvironmentContext,
        ) -> Result<Option<Resolution>>;
    }
}

pub(crate) fn build_discovery_builder(
    sources: DiscoverySources,
    ctx: &EnvironmentContext,
) -> DiscoveryBuilder {
    let mut builder = DiscoveryBuilder::default()
        .set_source(sources)
        .with_timeout_msecs(ctx.get(ffx_config::keys::DISCOVERY_TIMEOUT_MS).ok());

    if sources.contains(DiscoverySources::EMULATOR) {
        // If there is an error getting the config option, it is ignored to prevent
        // ffx-strict from failing when default configuration is not overridden.
        let emu_instance_root = ctx.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR).ok();
        builder = builder.with_emulator_instance_root(emu_instance_root);
    }
    if sources.contains(DiscoverySources::FASTBOOT_FILE) {
        // If there is an error getting the config option, it is ignored to prevent
        // ffx-strict from failing when default configuration is not overridden.
        let fastboot_file_path = ctx.get(ffx_config::keys::FASTBOOT_FILE_PATH).ok();
        builder = builder.with_fastboot_devices_file_path(fastboot_file_path);
    }

    if sources.contains(DiscoverySources::USB_VSOCK) {
        let usb_driver_socket = ctx.get(usb_driver_api::CONFIG_USB_SOCKET_PATH).ok();
        if let Some(path) = &usb_driver_socket {
            try_daemon_autostart(path, ctx);
        }
        builder = builder.with_usb_vsock_driver_socket_path(usb_driver_socket);
    }

    if sources.contains(DiscoverySources::GCE) {
        let gce_instance_root = ctx.get(ffx_config::keys::GCE_INSTANCE_ROOT_DIR).ok();
        builder = builder.with_gce_instance_root(gce_instance_root);
    }

    builder
}

pub fn build_discovery(sources: DiscoverySources, ctx: &EnvironmentContext) -> Discovery {
    let builder = build_discovery_builder(sources, ctx);
    builder.build(ctx)
}

pub fn build_discovery_from_config(ctx: &EnvironmentContext) -> Discovery {
    build_discovery_builder_common(ctx).build(ctx)
}

pub fn build_discovery_builder_common(ctx: &EnvironmentContext) -> DiscoveryBuilder {
    let mut sources = DiscoverySources::all();
    // Fastboot targets (bootloaders) are not ssh-able and do not support target
    // address resolution, so we exclude them from configuration-based discovery.
    sources.remove(DiscoverySources::USB_FASTBOOT | DiscoverySources::FASTBOOT_FILE);
    if !ctx.get(ffx_config::keys::USB_ENABLED).unwrap_or(false) {
        sources.remove(DiscoverySources::USB_VSOCK);
    }
    if !ctx.get(ffx_config::keys::NETWORK_ENABLED).unwrap_or(true) {
        sources.remove(DiscoverySources::MDNS);
    }
    build_discovery_builder(sources, ctx)
}

// Return a stream of TargetHandles that come from the specified sources, and
// match the query. The discovery will return early if a result "perfectly"
// matches the query (e.g. an mDNS response with the exact name requested).
fn get_discovery_stream_with_sources(
    query: TargetInfoQuery,
    sources: DiscoverySources,
    ctx: &EnvironmentContext,
) -> Result<impl Stream<Item = TargetHandle>> {
    let discovery = build_discovery(sources, ctx);
    // Just get the new handles
    let stream = discovery.discovery_stream(query).map_err(anyhow::Error::from)?;
    Ok(stream.filter_map(|ev| async move {
        if let TargetEvent::Added(th) = ev { Some(th) } else { None }
    }))
}

// Return a set of TargetHandles that come from the specified sources, and match
// the query. The discovery will return early if a result "perfectly" matches the
// query (e.g. an mDNS response with the exact name requested).
async fn get_discovered_targets_with_sources(
    query: TargetInfoQuery,
    sources: DiscoverySources,
    ctx: &EnvironmentContext,
) -> Result<Vec<TargetHandle>> {
    let discovery = build_discovery(sources, ctx);
    discovery.discover_devices(query.clone()).await.map_err(|e| anyhow::anyhow!(e))
}

/// Attempts to resolve the query into a target's ssh-able address. It is an error
/// if it the query doesn't match exactly one target.
// Perhaps refactor as connect_to_target() -> Result<Connection>, since that seems
// to be the only way this function is used?
pub async fn resolve_target_address(
    target_spec: &TargetInfoQuery,
    use_cache: bool,
    ctx: &EnvironmentContext,
) -> Result<Resolution, FfxTargetError> {
    let source = target_source_for_query(target_spec, ctx);
    let discovery = build_discovery_from_config(ctx);
    let resolver = DefaultTargetResolver::new(discovery);
    resolver.resolve_target_address(target_spec, use_cache, ctx, source).await
}

pub struct DefaultTargetResolver {
    discovery: Discovery,
}

impl DefaultTargetResolver {
    pub fn new(discovery: Discovery) -> Self {
        Self { discovery }
    }

    pub fn discovery_timeout(&self) -> Option<Duration> {
        self.discovery.timeout()
    }

    pub fn set_discovery_timeout(&mut self, timeout: Option<Duration>) {
        self.discovery.set_timeout(timeout);
    }
}

/// Return a stream of handles matching the query. If a target matches the query
/// exactly (i.e. the query is the full name of the target, not a substring),
/// return the handle immediately. Otherwise, run until the timeout (default 2
/// seconds, configurable via "discovery.timeout"). The contents of the stream
/// can be filtered by USB and mDNS.
pub fn get_discovery_stream(
    query: TargetInfoQuery,
    usb: bool,
    mdns: bool,
    ctx: &EnvironmentContext,
) -> std::result::Result<impl Stream<Item = TargetHandle>, crate::FfxTargetCrateError> {
    let mut sources = DiscoverySources::MANUAL
        | DiscoverySources::EMULATOR
        | DiscoverySources::FASTBOOT_FILE
        | DiscoverySources::GCE;
    if usb {
        sources = sources | DiscoverySources::USB_FASTBOOT;

        if ctx.get(keys::USB_ENABLED).unwrap_or(false) {
            sources = sources | DiscoverySources::USB_VSOCK;
        }
    }
    if mdns && ctx.get(keys::NETWORK_ENABLED).unwrap_or(true) {
        sources = sources | DiscoverySources::MDNS;
    }
    Ok(get_discovery_stream_with_sources(query, sources, ctx)?)
}

/// Return a list of handles matching the query. If a target matches the query
/// exactly (i.e. the query is the full name of the target, not a substring),
/// return the handle immediately. Otherwise, run until the timeout (default 2
/// seconds, configurable via "discovery.timeout"). The contents of the set can
/// be filtered by USB and mDNS.
pub async fn get_discovered_targets(
    query: TargetInfoQuery,
    usb: bool,
    mdns: bool,
    ctx: &EnvironmentContext,
) -> std::result::Result<Vec<TargetHandle>, crate::FfxTargetCrateError> {
    let mut sources = DiscoverySources::MANUAL
        | DiscoverySources::EMULATOR
        | DiscoverySources::FASTBOOT_FILE
        | DiscoverySources::GCE;
    if usb {
        sources = sources | DiscoverySources::USB_FASTBOOT;

        if ctx.get(keys::USB_ENABLED).unwrap_or(false) {
            sources = sources | DiscoverySources::USB_VSOCK;
        }
    }
    if mdns && ctx.get(keys::NETWORK_ENABLED).unwrap_or(true) {
        sources = sources | DiscoverySources::MDNS;
    }
    // Get nodename, in case we're trying to find an exact match
    Ok(get_discovered_targets_with_sources(query, sources, ctx).await?)
}

impl TargetResolver for DefaultTargetResolver {
    async fn discovered_targets(&self, query: TargetInfoQuery) -> Result<Vec<TargetHandle>> {
        self.discovery
            .discover_devices(query.clone())
            .await
            .map_err(|e| anyhow::anyhow!(e))
            .or_analytics(PointOfFailure::DiscoveryFailure {
                query: Some(query.to_analytics_tag()),
                discovery_sources: self.discovery.sources(),
            })
            .await
    }

    async fn try_resolve_manual_target(
        &self,
        name: &str,
        ctx: &EnvironmentContext,
    ) -> Result<Option<Resolution>> {
        // This is something that is often mocked for testing. An improvement here would be to use the
        // environment context for locating manual targets.
        let finder = manual_targets::Config::new_from_context(ctx);
        let ssh_timeout: u64 = ctx.get(CONFIG_TARGET_SSH_TIMEOUT).unwrap_or(DEFAULT_SSH_TIMEOUT_MS);
        let ssh_timeout = Duration::from_millis(ssh_timeout);
        let mut res = None;
        for t in manual_targets::watcher::parse_manual_targets(&finder).await.into_iter() {
            let addr = t.addr();
            let resolution = Resolution::from_addr(addr);
            let identify = resolution
                .identify(ctx)
                .on_timeout(ssh_timeout, || {
                    Err(crate::error::TargetResolutionError::ManualTargetTimeout {
                        addr,
                        timeout: ssh_timeout,
                    }
                    .into())
                })
                .await?;

            if identify.nodename == Some(String::from(name)) {
                res = Some(resolution);
            }
        }
        Ok(res)
    }
}

enum ResolutionTarget {
    Addr(SocketAddr),
    Usb(u32),
    Vsock(u32),
    TestMock(Box<dyn Fn() -> Result<Connection> + Send + Sync>),
    TestMockAsync(Box<dyn Fn() -> BoxFuture<'static, Result<Connection>> + Send + Sync>),
}

impl Debug for ResolutionTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Addr(arg0) => f.debug_tuple("Addr").field(arg0).finish(),
            Self::Usb(arg0) => f.debug_tuple("Usb").field(arg0).finish(),
            Self::Vsock(arg0) => f.debug_tuple("Vsock").field(arg0).finish(),
            Self::TestMock(_) => f.debug_tuple("TestMock").field(&"..").finish(),
            Self::TestMockAsync(_) => f.debug_tuple("TestMockAsync").field(&"..").finish(),
        }
    }
}

fn sort_socket_addrs(a1: &SocketAddr, a2: &SocketAddr) -> Ordering {
    match (a1.ip().is_link_local_addr(), a2.ip().is_link_local_addr()) {
        (true, true) | (false, false) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}

fn sort_addrs(&a1: &TargetAddr, a2: &TargetAddr) -> Ordering {
    match (a1, a2) {
        (TargetAddr::VSockCtx(_), TargetAddr::VSockCtx(_)) => Ordering::Equal,
        (TargetAddr::UsbCtx(_), TargetAddr::UsbCtx(_)) => Ordering::Equal,
        (TargetAddr::VSockCtx(_), _) => Ordering::Less,
        (_, TargetAddr::VSockCtx(_)) => Ordering::Greater,
        (TargetAddr::UsbCtx(_), _) => Ordering::Less,
        (_, TargetAddr::UsbCtx(_)) => Ordering::Greater,
        (TargetAddr::Net(a), TargetAddr::Net(b)) => sort_socket_addrs(&a, &b),
    }
}

// When choosing an address for resolution, we choose in the following order:
// - any VSock address
// - any USB address
// - any link-local V6 address
// - any other network address
fn choose_address_from_addresses(
    nodename: &Option<String>,
    addresses: &[TargetAddr],
) -> Option<TargetAddr> {
    if addresses.is_empty() {
        log::warn!("Target discovered but does not contain addresses: {nodename:?}");
        return None;
    }
    Some(
        addresses
            .iter()
            .cloned()
            .min_by(sort_addrs)
            .expect("Address list mysteriously became empty!"),
    )
}

impl From<TargetAddr> for ResolutionTarget {
    fn from(addr: TargetAddr) -> Self {
        match addr {
            TargetAddr::Net(sock) => {
                let addr: SocketAddr = replace_default_port(sock);
                Self::Addr(addr)
            }
            TargetAddr::VSockCtx(cid) => Self::Vsock(cid),
            TargetAddr::UsbCtx(cid) => Self::Usb(cid),
        }
    }
}

impl ResolutionTarget {
    fn from_addrs(name: &Option<String>, addresses: &[TargetAddr]) -> Option<ResolutionTarget> {
        // We'll ignore products where we can't choose an address, e.g. because
        // they don't seem to have one.
        let addr = choose_address_from_addresses(name, addresses)?;
        Some(addr.into())
    }
    fn from_target_handle(target: &TargetHandle) -> Option<ResolutionTarget> {
        match &target.state {
            // If the target is a product, use the "best" network address from the target.
            discovery::TargetState::Product { addrs: addresses, .. } => {
                Self::from_addrs(&target.node_name, addresses)
            }
            // Resolutions are only supported for devices in Product mode
            _ => None,
        }
    }

    fn from_target_info(info: &TargetInfo) -> Option<ResolutionTarget> {
        match info.target_state {
            // If the target is a product, use the "best" network address from the target.
            info::TargetState::Product => Self::from_addrs(&info.nodename, &info.addresses),
            // Resolutions are only supported for devices in Product mode
            _ => {
                log::debug!(
                    "Skipping target {:?} in unsupported state {:?}",
                    info.nodename,
                    info.target_state
                );
                None
            }
        }
    }

    fn to_spec(&self) -> String {
        match &self {
            ResolutionTarget::Addr(ssh_addr) => {
                format!("{ssh_addr}")
            }
            ResolutionTarget::Usb(cid) => {
                format!("usb:cid:{cid}")
            }
            ResolutionTarget::Vsock(cid) => {
                format!("vsock:cid:{cid}")
            }
            ResolutionTarget::TestMock(_) | ResolutionTarget::TestMockAsync(_) => {
                format!("mock_target")
            }
        }
    }
}

#[derive(Debug)]
enum ConnectionState {
    Uninitialized,
    Connected(Arc<Connection>),
    Failed,
}

// Group the information collected when resolving the address. (This is
// particularly important for the rcs_proxy, which we may need when resolving
// a manual target -- we don't want make an RCS connection just to resolve the
// name, drop it, then re-establish it later.)
#[derive(Debug)]
pub struct Resolution {
    target: ResolutionTarget,
    discovered: Option<TargetHandle>,
    connection: Mutex<ConnectionState>,
    rcs_proxy: Mutex<Option<RemoteControlProxy>>,
    identify_host_response: Mutex<Option<IdentifyHostResponse>>,
}

impl Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "Target: {:?}", self.target)?;
        if let Some(discovered) = &self.discovered {
            write!(f, " Discovered: {}", discovered)?;
        }
        Ok(())
    }
}

impl Resolution {
    fn from_target(target: ResolutionTarget) -> Self {
        Self {
            target,
            discovered: None,
            connection: Mutex::new(ConnectionState::Uninitialized),
            rcs_proxy: Mutex::new(None),
            identify_host_response: Mutex::new(None),
        }
    }
    fn from_addr(sa: SocketAddr) -> Self {
        let scope_id = if let SocketAddr::V6(addr) = sa { addr.scope_id() } else { 0 };
        let port = match sa.port() {
            0 => TARGET_DEFAULT_PORT,
            p => p,
        };
        let addr = TargetIpAddr::new(sa.ip(), scope_id, port).into();
        Self::from_target(ResolutionTarget::Addr(addr))
    }

    pub fn from_target_handle(
        th: TargetHandle,
    ) -> std::result::Result<Self, crate::FfxTargetCrateError> {
        let target = ResolutionTarget::from_target_handle(&th).ok_or_else(|| {
            crate::error::TargetResolutionError::MissingProductAddress {
                node_name: th.node_name.clone(),
            }
        })?;
        Ok(Self { discovered: Some(th), ..Self::from_target(target) })
    }

    pub fn addr(&self) -> std::result::Result<SocketAddr, crate::FfxTargetCrateError> {
        match self.target {
            ResolutionTarget::Addr(addr) => Ok(addr),
            _ => Err(crate::error::TargetResolutionError::NonNetworkTarget.into()),
        }
    }

    pub fn mock(f: impl Fn() -> Result<Connection> + Send + Sync + 'static) -> Self {
        Self::from_target(ResolutionTarget::TestMock(Box::new(f)))
    }

    pub fn mock_async<F, Fut>(f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Connection>> + Send + 'static,
    {
        Self::from_target(ResolutionTarget::TestMockAsync(Box::new(move || Box::pin(f()))))
    }

    pub async fn set_connection_for_test(&self, connection: Option<Connection>) {
        *self.connection.lock().await = match connection {
            Some(conn) => ConnectionState::Connected(Arc::new(conn)),
            None => ConnectionState::Uninitialized,
        };
    }

    pub fn usb_cid(&self) -> Option<u32> {
        if let ResolutionTarget::Usb(cid) = &self.target { Some(*cid) } else { None }
    }

    pub fn vsock_cid(&self) -> Option<u32> {
        if let ResolutionTarget::Vsock(cid) = &self.target { Some(*cid) } else { None }
    }

    pub fn target_spec(&self) -> String {
        self.target.to_spec()
    }

    pub async fn is_usable(&self) -> bool {
        // Acquire the lock to read the connection state. Note that this may suspend/block if a
        // connection attempt is currently in progress (which holds the lock). This blocking is
        // correct and desirable: it prevents duplicate, parallel connection attempts by forcing
        // us to wait and see if the in-progress attempt succeeds.
        match &*self.connection.lock().await {
            // We lazily connect to the target (some callers like SshAddrHolder only query the
            // address without connecting). An Uninitialized state is still considered healthy/usable
            // because we haven't failed any connection attempts yet.
            ConnectionState::Uninitialized => {
                log::debug!(
                    "Resolution::is_usable(): connection is uninitialized for target {:?}",
                    self.target
                );
                true
            }
            ConnectionState::Connected(conn) => {
                if conn.is_terminated() {
                    log::debug!(
                        "Resolution::is_usable(): connection is terminated for target {:?}",
                        self.target
                    );
                    false
                } else {
                    log::debug!(
                        "Resolution::is_usable(): connection is active for target {:?}",
                        self.target
                    );
                    true
                }
            }
            // If connection failed, we treat the cache entry as unusable to force eviction and
            // re-resolution. Even if this results in a false positive (e.g. the target recovered
            // at the same address by the time we check), re-resolution is safe and necessary in
            // cases where the target address has changed.
            ConnectionState::Failed => {
                log::debug!(
                    "Resolution::is_usable(): previous connection attempt failed for target {:?}",
                    self.target
                );
                false
            }
        }
    }

    pub async fn get_connection(
        &self,
        context: &EnvironmentContext,
    ) -> std::result::Result<Arc<Connection>, crate::FfxTargetCrateError> {
        // Hold a lock to make sure only one connection is being initialized at a time.
        // Note that this is a tokio Mutex, not a std Mutex, so it's safe to hold across
        // await points.
        let mut conn_guard = self.connection.lock().await;

        if let ConnectionState::Connected(conn) = &*conn_guard
            && !conn.is_terminated()
        {
            return Ok(conn.clone());
        }

        let conn_res = self.connect_new(context).await;
        match &conn_res {
            Ok(conn) => {
                *conn_guard = ConnectionState::Connected(conn.clone());
            }
            Err(_) => {
                *conn_guard = ConnectionState::Failed;
                // If a connection attempt fails, invalidate the discovery cache file so subsequent
                // attempts re-discover rather than pinning to a stale or unreachable cached entry.
                if let Err(e) = crate::cache::remove_target_cache(context) {
                    log::debug!("Failed to remove target cache on connection failure: {e:?}");
                }
            }
        }
        conn_res
    }

    async fn connect_new(
        &self,
        context: &EnvironmentContext,
    ) -> std::result::Result<Arc<Connection>, crate::FfxTargetCrateError> {
        let conn =
            match &self.target {
                ResolutionTarget::Addr(socket_addr) => {
                    if !context.get(keys::NETWORK_ENABLED).unwrap_or(true) {
                        return Err(crate::error::TargetResolutionError::NetworkDisabled.into());
                    }
                    let connector = SshConnector::new(
                        netext::ScopedSocketAddr::from_socket_addr(*socket_addr)?,
                        context,
                    )?;
                    emit_target_connection_event("SSH").await;
                    Connection::new(connector).await.map_err(|e| {
                        crate::KnockError::Critical(crate::KnockCriticalError::TargetError(
                            format!("{:?}", e),
                        ))
                    })?
                }
                ResolutionTarget::Usb(cid) => {
                    if !context.get(keys::USB_ENABLED).unwrap_or(false) {
                        return Err(crate::error::TargetResolutionError::UsbDisabled.into());
                    }
                    let connector = UsbConnector::new(*cid, context).await?;
                    emit_target_connection_event("USB").await;
                    Connection::new(connector).await.map_err(|e| {
                        crate::KnockError::Critical(crate::KnockCriticalError::TargetError(
                            format!("{:?}", e),
                        ))
                    })?
                }
                ResolutionTarget::Vsock(cid) => {
                    if !context.get(keys::VSOCK_ENABLED).unwrap_or(false) {
                        return Err(crate::error::TargetResolutionError::VsockDisabled.into());
                    }
                    let connector = VSockConnector::new(*cid);
                    emit_target_connection_event("VSOCK").await;
                    Connection::new(connector).await.map_err(|e| {
                        crate::KnockError::Critical(crate::KnockCriticalError::TargetError(
                            format!("{:?}", e),
                        ))
                    })?
                }
                ResolutionTarget::TestMock(f) => f()?,
                ResolutionTarget::TestMockAsync(f) => f().await?,
            };

        Ok(Arc::new(conn))
    }

    pub fn get_connection_if_already_established(&self) -> Option<Arc<Connection>> {
        if let Ok(guard) = self.connection.try_lock() {
            match &*guard {
                ConnectionState::Connected(conn) => Some(conn.clone()),
                _ => None,
            }
        } else {
            None
        }
    }

    async fn get_rcs_proxy(
        &self,
        context: &EnvironmentContext,
    ) -> std::result::Result<RemoteControlProxy, crate::FfxTargetCrateError> {
        // Hold a lock to make sure only one RCS proxy is being initialized at a time.
        // Note that this is a tokio Mutex, not a std Mutex, so it's safe to hold across
        // await points.
        let mut rcs_proxy_guard = self.rcs_proxy.lock().await;
        if rcs_proxy_guard.is_none() {
            let conn = self.get_connection(context).await?;
            *rcs_proxy_guard = Some(conn.rcs_proxy_fdomain().await?);
        }
        // Unwrap safety: either the guard was already Some(), or we just initialized it with Some()
        Ok(rcs_proxy_guard.as_ref().unwrap().clone())
    }

    pub async fn identify(
        &self,
        context: &EnvironmentContext,
    ) -> std::result::Result<IdentifyHostResponse, crate::FfxTargetCrateError> {
        // Hold a lock to make sure only one IdentifyHost is being called at a time.
        // Note that this is a tokio Mutex, not a std Mutex, so it's safe to hold across
        // await points.
        let mut identify_guard = self.identify_host_response.lock().await;
        if identify_guard.is_none() {
            let rcs_proxy = self.get_rcs_proxy(context).await?;
            *identify_guard = Some(
                rcs_proxy
                    .identify_host()
                    .await?
                    .map_err(crate::FfxTargetCrateError::IdentifyHost)?,
            );
        }
        // Unwrap safety: either the guard was already Some(), or we just initialized it with Some()
        Ok(identify_guard.as_ref().unwrap().clone())
    }

    pub async fn get_target_info(
        &self,
        addr: TargetAddr,
        context: &EnvironmentContext,
    ) -> Result<TargetInfo> {
        let identify = self.identify(context).await?;
        // This is only called in a situation where we started from an address, so we're
        // going to only return that address. The problem is that we can't really trust
        // the result of IdentifyHost: its "addresses" don't include the port.
        Ok(TargetInfo {
            nodename: identify.nodename,
            addresses: vec![addr],
            // If we could Identify, then clearly RCS is up
            rcs_state: info::RemoteControlState::Up,
            target_state: info::TargetState::Product,
            product_config: identify.product_config,
            board_config: identify.board_config,
            serial_number: identify.serial_number,
            // "is_manual" is not available without discovery, but manual targets are going away, so this is reasonable
            is_manual: false,
            boot_id: identify.boot_id,
            is_default: None,
        })
    }
}

async fn emit_target_connection_event(ty: &str) {
    let _ = analytics::add_custom_event(
        Some("ffx_target_connection"),
        Some(ty),
        None,
        [].into_iter().collect(),
    )
    .await;
}

async fn emit_cache_event(status: &'static str, query_type: &str) {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("status", analytics::GA4Value::from(status));
    fields.insert("query_type", analytics::GA4Value::from(query_type));
    let _ =
        analytics::add_custom_event(Some("ffx_target_resolution_cache"), None, None, fields).await;
}

impl Resolution {
    pub async fn try_from_env_context_with_resolver<R: TargetResolver>(
        resolver: &R,
        env: &EnvironmentContext,
        use_cache: bool,
    ) -> ffx_command_error::Result<Self> {
        let (target_spec, source) = get_target_specifier_with_source(env)?;
        if env.is_strict() && target_spec.is_none() {
            return Err(user_error!(
                "You must specify a target via `-t <target_name>` before any command arguments"
            )
            .into());
        }
        let spec: TargetInfoQuery = TargetInfoQuery::try_from(target_spec)
            .map_err(|e| user_error!("Invalid target specifier: {}", e))?;

        let resolution = resolver
            .resolve_target_address(&spec, use_cache, env, source)
            .await
            .map_err(|e| ffx_command_error::Error::User(NonFatalError(e.into()).into()))?;
        Ok(resolution)
    }

    pub async fn try_from_env_context_with_cache(
        env: &EnvironmentContext,
        use_cache: bool,
    ) -> ffx_command_error::Result<Self> {
        let discovery = build_discovery_from_config(env);
        let resolver = DefaultTargetResolver::new(discovery);
        Self::try_from_env_context_with_resolver(&resolver, env, use_cache).await
    }
}

impl TryFromEnvContext for Resolution {
    fn try_from_env_context<'a>(
        env: &'a EnvironmentContext,
    ) -> LocalBoxFuture<'a, ffx_command_error::Result<Self>> {
        Box::pin(async { Self::try_from_env_context_with_cache(env, true).await })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

    #[fuchsia::test]
    async fn test_sort_socket_addrs() {
        struct TestCase {
            a1: SocketAddr,
            a2: SocketAddr,
            want_res: Ordering,
            name: String,
        }

        let cases: Vec<TestCase> = vec![
            TestCase {
                a1: "127.0.0.1:8082".parse().expect("Valid SocketAddr"),
                a2: "[::1]:8082".parse().expect("Valid SocketAddr"),
                want_res: Ordering::Equal,
                name: "Loopback v4 and v6 equal".to_string(),
            },
            TestCase {
                a1: "192.168.1.1:8082".parse().expect("Valid SocketAddr"),
                a2: "[2002:2::%2]:8082".parse().expect("Valid SocketAddr"),
                want_res: Ordering::Equal,
                name: "Non local v4 and non local v6 local equal".to_string(),
            },
            TestCase {
                // Local
                a1: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 6, 7, 8),
                    8080,
                    0,
                    1,
                )),
                // Non Local
                a2: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0x2607, 0xf8b0, 0x4005, 0x805, 0, 0, 0, 0x200e),
                    8080,
                    0,
                    1,
                )),
                want_res: Ordering::Less,
                name: "local v6 and non v6 local less".to_string(),
            },
            TestCase {
                // Local
                a1: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xfe80, 0, 0, 0, 1, 6, 7, 8),
                    8080,
                    0,
                    1,
                )),
                // Local
                a2: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xfe80, 0, 0, 0, 2, 6, 7, 8),
                    8080,
                    0,
                    1,
                )),
                want_res: Ordering::Equal,
                name: "local v6 and local v6 local equal".to_string(),
            },
        ];

        for case in cases {
            let got = sort_socket_addrs(&case.a1, &case.a2);
            assert_eq!(
                got, case.want_res,
                "TestCase: {0}. Results differ: {1:?}, {2:?}",
                case.name, got, case.want_res
            );
        }
    }

    fn get_addr_and_spec() -> (SocketAddr, TargetInfoQuery) {
        let addr = "127.0.0.1:123".to_string();
        let addr_spec =
            TargetInfoQuery::Addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 123));
        let sa = addr.parse::<SocketAddr>().unwrap();
        (sa, addr_spec)
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_addr() {
        let test_env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();
        // A network address will resolve to itself
        let (_, addr_spec) = get_addr_and_spec();
        // Note that this will fail if we try to call resolve_target_spec()
        // since we haven't mocked a return value. So it's also checking that no
        // resolution is done.
        let target_spec =
            locally_resolve_target_spec(&addr_spec.clone(), &resolver, &test_env.context)
                .await
                .unwrap();
        assert_eq!(target_spec, addr_spec.clone().into());
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_id() {
        let test_env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();
        // An ID spec will resolve to itself
        let id = "abcdef".to_string();
        let id_spec = TargetInfoQuery::Id(id.clone());
        // Note that this will fail if we try to call resolve_target_spec()
        // since we still haven't mocked a return value. So it's also checking that no
        // resolution is done.
        let target_spec =
            locally_resolve_target_spec(&id_spec.clone(), &resolver, &test_env.context)
                .await
                .unwrap();
        assert_eq!(target_spec, id_spec.clone());
    }

    fn make_target_handle_for_product(name: &str, sa: SocketAddr) -> TargetHandle {
        let state = discovery::TargetState::Product { addrs: vec![sa.into()], serial: None };
        let th = TargetHandle { node_name: Some(String::from(name)), state, manual: false };
        th
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_dns() {
        let test_env = ffx_config::test_init().unwrap();
        let mut resolver = MockTargetResolver::new();
        // A DNS name will satisfy the resolution request
        let name = "foobar".to_string();
        let name_spec = TargetInfoQuery::NodenameOrId(name.clone());
        let (sa, addr_spec) = get_addr_and_spec();
        let th = make_target_handle_for_product(&name, sa);
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th]));
        let target_spec =
            locally_resolve_target_spec(&name_spec.clone(), &resolver, &test_env.context)
                .await
                .unwrap();
        assert_eq!(target_spec, addr_spec);
    }

    #[fuchsia::test]
    async fn test_cannot_resolve_target_locally_id_name() {
        let test_env = ffx_config::test_init().unwrap();
        let mut resolver = MockTargetResolver::new();
        // Test with "<id>", _not_ "id:<id>"
        let id = "abcdef".to_string();
        let th = TargetHandle {
            node_name: None,
            state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                serial_number: id.clone(),
                connection_state: discovery::FastbootConnectionState::Usb,
            }),
            manual: false,
        };
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th]));
        let target_spec = locally_resolve_target_spec(
            &(TargetInfoQuery::try_from(id.clone()).unwrap()),
            &resolver,
            &test_env.context,
        )
        .await;
        assert!(target_spec.is_err())
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_name_ambiguous() {
        let test_env = ffx_config::test_init().unwrap();
        let mut resolver = MockTargetResolver::new();
        // An ambiguous name will result in an error
        let name = "foobar".to_string();
        let (sa, _) = get_addr_and_spec();
        let th1 = make_target_handle_for_product(&name, sa);
        let th2 = make_target_handle_for_product(&name, sa);
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th1, th2]));
        let target_spec_res = locally_resolve_target_spec(
            &(TargetInfoQuery::try_from("foo".to_string()).unwrap()),
            &resolver,
            &test_env.context,
        )
        .await;
        assert!(target_spec_res.is_err());
        assert!(dbg!(target_spec_res.unwrap_err().to_string()).contains("multiple targets"));
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_first() {
        let test_env = ffx_config::test_init().unwrap();
        let mut resolver = MockTargetResolver::new();
        // A "first" query will satisfy the resolution request
        let first_spec = TargetInfoQuery::First;
        let (sa, addr_spec) = get_addr_and_spec();
        let th = make_target_handle_for_product("foo", sa);
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th]));
        let target_spec =
            locally_resolve_target_spec(&first_spec, &resolver, &test_env.context).await.unwrap();
        assert_eq!(target_spec, addr_spec);
    }

    #[fuchsia::test]
    async fn test_can_resolve_target_locally_first_ambiguous() {
        let test_env = ffx_config::test_init().unwrap();
        let mut resolver = MockTargetResolver::new();
        // A "first" query will fail if there are multiple matches
        let name = "foobar".to_string();
        let (sa, _) = get_addr_and_spec();
        let th1 = make_target_handle_for_product(&name, sa);
        let th2 = make_target_handle_for_product(&name, sa);
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th1, th2]));
        let first_spec = TargetInfoQuery::First;
        let target_spec_res =
            locally_resolve_target_spec(&first_spec, &resolver, &test_env.context).await;
        assert!(target_spec_res.is_err());
        assert!(dbg!(target_spec_res.unwrap_err().to_string()).contains("More than one device"));
    }

    #[fuchsia::test]
    async fn test_expect_single_target_empty() {
        let query = TargetInfoQuery::NodenameOrId("foo".to_string());
        let handles: Vec<TargetHandle> = vec![];
        let res = expect_single_target(&query, handles, None);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(
            err,
            FfxTargetError::OpenTargetError { err: ffx::OpenTargetError::TargetNotFound, .. }
        ));
    }

    #[fuchsia::test]
    async fn test_expect_single_target_single() {
        let query = TargetInfoQuery::NodenameOrId("foo".to_string());
        let handle = make_target_handle_for_product("foo", "127.0.0.1:8080".parse().unwrap());
        let handles = vec![handle.clone()];
        let res = expect_single_target(&query, handles, None);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), handle);
    }

    #[fuchsia::test]
    async fn test_expect_single_target_multiple() {
        let query = TargetInfoQuery::NodenameOrId("foo".to_string());
        let handle1 = make_target_handle_for_product("foo", "127.0.0.1:8080".parse().unwrap());
        let handle2 = make_target_handle_for_product("bar", "127.0.0.1:8081".parse().unwrap());
        let handles = vec![handle1, handle2];
        let res = expect_single_target(&query, handles, None);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(
            err,
            FfxTargetError::OpenTargetError { err: ffx::OpenTargetError::QueryAmbiguous, .. }
        ));
    }

    #[fuchsia::test]
    async fn test_get_target_info_serial() {
        let test_env = ffx_config::test_init().unwrap();
        let sa: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let addr = TargetAddr::Net(sa.clone());
        let resolution = Resolution::from_addr(sa);
        let mut identify = IdentifyHostResponse::default();
        identify.nodename = Some("test-node".to_string());
        identify.serial_number = Some("test_serial_123".to_string());
        *resolution.identify_host_response.lock().await = Some(identify);

        let info = resolution.get_target_info(addr, &test_env.context).await.unwrap();

        assert_eq!(info.serial_number, Some("test_serial_123".to_string()));
    }

    #[fuchsia::test]
    async fn test_resolution_tracks_failed_connection() {
        let test_env = ffx_config::test_init().unwrap();
        let resolution = Resolution::from_target(ResolutionTarget::TestMock(Box::new(|| {
            Err(anyhow::anyhow!("Mock connection failure"))
        })));

        // Initially it is usable
        assert!(resolution.is_usable().await);

        // Attempt to connect, which fails
        let res = resolution.get_connection(&test_env.context).await;
        assert!(res.is_err());

        // Now it should not be usable
        assert!(!resolution.is_usable().await);
    }

    #[fuchsia::test]
    async fn test_cache_miss_falls_back_to_discovery() {
        use crate::cache::{Cache, get_discovery_cache_file};

        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir_path = temp_dir.path().to_path_buf();

        let test_env = ffx_config::test_env()
            .user_config(
                ffx_config::keys::DISCOVERY_CACHE_DIR_CONFIG,
                serde_json::json!(cache_dir_path.to_str().unwrap().to_string()),
            )
            .build()
            .unwrap();

        // Write an empty cache file (contains no targets)
        let cache_file = get_discovery_cache_file(&test_env.context).unwrap();
        let cache = Cache::new(vec![]);
        cache.save(&cache_file).unwrap();

        let mut resolver = MockTargetResolver::new();
        let name = "foobar".to_string();
        let name_spec = TargetInfoQuery::NodenameOrId(name.clone());
        let (sa, addr_spec) = get_addr_and_spec();
        let th = make_target_handle_for_product(&name, sa);

        // Even though cache exists (empty), because of cache miss, it must fall back to manual and discovery queries:
        resolver.expect_try_resolve_manual_target().return_once(move |_, _| Ok(None));
        resolver.expect_discovered_targets().return_once(move |_| Ok(vec![th]));

        let target_spec =
            locally_resolve_target_spec(&name_spec.clone(), &resolver, &test_env.context)
                .await
                .unwrap();
        assert_eq!(target_spec, addr_spec);
    }

    #[fuchsia::test]
    async fn test_cache_hit_bypasses_discovery() {
        use crate::cache::{Cache, get_discovery_cache_file};

        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir_path = temp_dir.path().to_path_buf();

        let test_env = ffx_config::test_env()
            .user_config(
                ffx_config::keys::DISCOVERY_CACHE_DIR_CONFIG,
                serde_json::json!(cache_dir_path.to_str().unwrap().to_string()),
            )
            .build()
            .unwrap();

        // Write a cache file containing our target info!
        let cache_file = get_discovery_cache_file(&test_env.context).unwrap();
        let (sa, addr_spec) = get_addr_and_spec();

        let mut target_info = crate::TargetInfo::default();
        target_info.nodename = Some("foobar".to_string());
        target_info.target_state = info::TargetState::Product;
        target_info.addresses = vec![sa.into()];
        let cache = Cache::new(vec![target_info]);
        cache.save(&cache_file).unwrap();

        let resolver = MockTargetResolver::new();
        let name_spec = TargetInfoQuery::NodenameOrId("foobar".to_string());

        // Note: we do NOT mock expect_try_resolve_manual_target or expect_discovered_targets.
        // If the resolver attempts to call them, MockTargetResolver will panic,
        // validating that discovery was indeed bypassed on cache hit.

        let target_spec =
            locally_resolve_target_spec(&name_spec.clone(), &resolver, &test_env.context)
                .await
                .unwrap();
        assert_eq!(target_spec, addr_spec);
    }

    // Tests that `build_discovery_from_config` reads and applies the discovery timeout from the
    // configuration key (`discovery.timeout`), in addition to filtering out fastboot sources.
    #[fuchsia::test]
    async fn test_build_discovery_from_config_reads_timeout_from_config() {
        let test_env = ffx_config::test_env()
            .user_config(ffx_config::keys::DISCOVERY_TIMEOUT_MS, 2500)
            .build()
            .unwrap();
        let discovery = build_discovery_from_config(&test_env.context);
        let sources = discovery.sources();
        assert!(!sources.contains(DiscoverySources::USB_FASTBOOT));
        assert!(!sources.contains(DiscoverySources::FASTBOOT_FILE));
        assert_eq!(discovery.timeout(), Some(Duration::from_millis(2500)));
    }

    // Tests that `build_discovery_from_config_with_timeout` overrides any timeout configured
    // in the environment context with the explicitly requested timeout (e.g. `None` for indefinite discovery),
    // and that `DefaultTargetResolver::set_discovery_timeout` can dynamically update the timeout afterwards.
    #[fuchsia::test]
    async fn test_build_discovery_from_config_maybe_indef() {
        let test_env = ffx_config::test_env()
            .user_config(ffx_config::keys::DISCOVERY_TIMEOUT_MS, 2500)
            .build()
            .unwrap();
        let discovery = build_discovery_builder_common(&test_env.context)
            .with_timeout_msecs(None)
            .build(&test_env.context);
        let sources = discovery.sources();
        assert!(!sources.contains(DiscoverySources::USB_FASTBOOT));
        assert!(!sources.contains(DiscoverySources::FASTBOOT_FILE));
        // Overrides configured 2500ms with None (indefinite)
        assert_eq!(discovery.timeout(), None);

        let mut resolver = DefaultTargetResolver::new(discovery);
        assert_eq!(resolver.discovery_timeout(), None);

        resolver.set_discovery_timeout(Some(Duration::from_millis(100)));
        assert_eq!(resolver.discovery_timeout(), Some(Duration::from_millis(100)));
    }

    #[fuchsia::test]
    async fn test_connection_failure_removes_discovery_cache() {
        use crate::cache::{Cache, get_discovery_cache_file};

        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir_path = temp_dir.path().to_path_buf();

        let test_env = ffx_config::test_env()
            .user_config(
                ffx_config::keys::DISCOVERY_CACHE_DIR_CONFIG,
                serde_json::json!(cache_dir_path.to_str().unwrap()),
            )
            .build()
            .unwrap();

        // Write a cache file containing target info
        let cache_file = get_discovery_cache_file(&test_env.context).unwrap();
        let sa: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let mut target_info = crate::TargetInfo::default();
        target_info.nodename = Some("test-target".to_string());
        target_info.target_state = info::TargetState::Product;
        target_info.addresses = vec![sa.into()];
        let cache = Cache::new(vec![target_info]);
        cache.save(&cache_file).unwrap();

        assert!(cache_file.exists());

        // Create a Resolution that fails to connect
        let resolution = Resolution::from_target(ResolutionTarget::TestMock(Box::new(|| {
            Err(anyhow::anyhow!("Connection error"))
        })));

        let res = resolution.get_connection(&test_env.context).await;
        assert!(res.is_err());

        // Cache file should now have been removed
        assert!(!cache_file.exists());
    }

    #[fuchsia::test]
    async fn test_resolve_target_address_explicit_usb() {
        let test_env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();
        let usb_spec = TargetInfoQuery::Usb(42);
        // Note: MockTargetResolver has no expectations configured.
        // If resolution falls back to discovery, this test will panic,
        // verifying that explicit USB resolution returns immediately.
        let res = resolver
            .resolve_target_address(&usb_spec, false, &test_env.context, None)
            .await
            .unwrap();
        assert_eq!(res.usb_cid(), Some(42));
        assert_eq!(res.vsock_cid(), None);
        assert_eq!(res.target_spec(), "usb:cid:42");
        assert!(matches!(
            res.addr().unwrap_err(),
            crate::FfxTargetCrateError::Resolution(
                crate::error::TargetResolutionError::NonNetworkTarget
            )
        ));
    }

    #[fuchsia::test]
    async fn test_resolve_target_address_explicit_vsock() {
        let test_env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();
        let vsock_spec = TargetInfoQuery::VSock(12345);
        // Note: MockTargetResolver has no expectations configured.
        // If resolution falls back to discovery, this test will panic,
        // verifying that explicit VSOCK resolution returns immediately.
        let res = resolver
            .resolve_target_address(&vsock_spec, false, &test_env.context, None)
            .await
            .unwrap();
        assert_eq!(res.vsock_cid(), Some(12345));
        assert_eq!(res.usb_cid(), None);
        assert_eq!(res.target_spec(), "vsock:cid:12345");
        assert!(matches!(
            res.addr().unwrap_err(),
            crate::FfxTargetCrateError::Resolution(
                crate::error::TargetResolutionError::NonNetworkTarget
            )
        ));
    }

    #[fuchsia::test]
    async fn test_resolve_target_address_explicit_addr() {
        let test_env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();
        let (sa, addr_spec) = get_addr_and_spec();
        // Note: MockTargetResolver has no expectations configured.
        let res = resolver
            .resolve_target_address(&addr_spec, false, &test_env.context, None)
            .await
            .unwrap();
        assert_eq!(res.addr().unwrap(), sa);
        assert_eq!(res.usb_cid(), None);
        assert_eq!(res.vsock_cid(), None);
        assert_eq!(res.target_spec(), format!("{sa}"));
    }

    #[fuchsia::test]
    async fn test_resolution_try_from_env_context_usb_and_vsock() {
        let env = ffx_config::test_init().unwrap();
        let resolver = MockTargetResolver::new();

        let mut usb_ctx = env.context.clone();
        usb_ctx.override_target_specifier(&Some("usb:cid:42".to_string()));
        let res_usb = Resolution::try_from_env_context_with_resolver(&resolver, &usb_ctx, false)
            .await
            .unwrap();
        assert_eq!(res_usb.usb_cid(), Some(42));
        assert_eq!(res_usb.vsock_cid(), None);

        let mut vsock_ctx = env.context.clone();
        vsock_ctx.override_target_specifier(&Some("vsock:cid:12345".to_string()));
        let res_vsock =
            Resolution::try_from_env_context_with_resolver(&resolver, &vsock_ctx, false)
                .await
                .unwrap();
        assert_eq!(res_vsock.vsock_cid(), Some(12345));
        assert_eq!(res_vsock.usb_cid(), None);
    }
}
