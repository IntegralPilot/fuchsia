// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use discovery::{DiscoverySources, TargetHandle};
use ffx_config::keys::TARGET_DEFAULT_KEY;

use ffx_config::{ConfigLevel, EnvironmentContext};
use fidl_fuchsia_developer_ffx::{self as ffx, DaemonError};
use fuchsia_async::Timer;
use futures::Future;
use futures::future::{Either, pending};
use log::{debug, info};
use std::time::Duration;
use target_errors::{FfxTargetError, TargetSource};
use thiserror::Error;

#[cfg(test)]
use mockall::predicate::*;

pub mod analytics;
pub mod connection;
pub mod info;
pub mod list;
pub mod ssh_connector;
pub mod usb_connector;
pub mod vsock_connector;

mod cache;
mod error;
mod fdomain_transport;
mod fidl_pipe;
mod resolve;
mod target_connector;

pub use cache::{
    create_target_cache, get_discovery_cache_dir, get_discovery_cache_file,
    get_discovery_cache_recheck_time, remove_target_cache,
};
pub use connection::{Connection, ConnectionError};
pub use discovery::desc::{Description, FastbootInterface};
pub use discovery::query::TargetInfoQuery;
pub use error::FfxTargetCrateError;
pub use fidl_pipe::{FidlPipe, create_overnet_socket};
pub use info::TargetInfo;
pub use list::list_targets;
pub use resolve::{
    DefaultTargetResolver, Resolution, TargetResolver, build_discovery,
    build_discovery_builder_common, build_discovery_from_config, discover_single_default_target,
    get_discovered_targets, get_discovery_stream, maybe_locally_resolve_target_spec,
    resolve_target_address,
};
pub use target_connector::{
    FDomainConnection, OvernetConnection, TargetConnection, TargetConnectionError, TargetConnector,
};

/// Re-export of [`fidl_fuchsia_developer_ffx::TargetProxy`] for ease of use
pub use fidl_fuchsia_developer_ffx::TargetProxy;

pub use target_errors::{UNKNOWN_TARGET_NAME, UNSPECIFIED_TARGET_NAME};

/// Emit an analytics event indicating an RCS proxy was created via the daemon.
pub async fn emit_daemon_rcs_proxy_event(ty: &str) {
    connection::emit_rcs_proxy_event(ty, Some(true), true).await
}

/// Attempt to connect to RemoteControl on a target device using a connection to a daemon.
///
/// The optional |target| is a string matcher as defined in fuchsia.developer.ffx.TargetQuery

pub fn is_discovery_enabled(ctx: &EnvironmentContext) -> bool {
    // TODO (b/355292969): put back the discovery check after we've addressed the flakes associated
    // with client-side discovery. (Currently re-enabled, but I want to validate the flake before resolving
    // this bug -slgrady 8/7/24)
    // true
    !ffx_config::is_usb_discovery_disabled(ctx) || !ffx_config::is_mdns_discovery_disabled(ctx)
}

#[derive(Debug, Error)]
pub enum KnockError {
    #[error("critical error: {0}")]
    Critical(KnockCriticalError),
    #[error("non-critical error: {0}")]
    NonCritical(KnockNonCriticalError),
}

#[derive(Debug, Error)]
pub enum KnockCriticalError {
    #[error("Timeout opening target {target}")]
    TimeoutOpeningTarget { target: String },
    #[error("Lost connection to the Daemon: {detail}")]
    LostDaemonConnection { detail: String },
    #[error("FIDL error: {0}")]
    Fidl(String),
    #[error("Target error: {0}")]
    TargetError(String),
    #[error("Other critical error: {0}")]
    Custom(String),
}

#[derive(Debug, Error)]
pub enum KnockNonCriticalError {
    #[error("Target not found: {target}")]
    TargetNotFound { target: String },
    #[error("RCS knock failed: {detail}")]
    RcsKnockFailed { detail: String },
    #[error("Timeout: {detail}")]
    Timeout { detail: String },
    #[error("Other non-critical error: {0}")]
    Custom(String),
}

// Derive from rcs knock timeout as this is the minimum amount of time to knock.
// Uses nanos to ensure that if RCS_KNOCK_TIMEOUT changes it is using the smallest unit possible.
//
// This is written as such due to some inconsistencies with Duration::from_nanos where `as_nanos()`
// returns a u128 but `from_nanos()` takes a u64.
pub const DEFAULT_RCS_KNOCK_TIMEOUT: Duration =
    Duration::new(rcs::RCS_KNOCK_TIMEOUT.as_secs() * 3, rcs::RCS_KNOCK_TIMEOUT.subsec_nanos() * 3);

impl From<ConnectionError> for KnockError {
    fn from(e: ConnectionError) -> Self {
        match e {
            ConnectionError::KnockError(ke) => ke,
            other => KnockError::Critical(KnockCriticalError::Custom(format!("{:?}", other))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitFor {
    DeviceOnline,
    DeviceOffline,
    Fastboot,
    Product,
}

const DOWN_REPOLL_DELAY_MS: u64 = 500;

pub async fn wait_for_device(
    wait_timeout: Option<Duration>,
    env: &EnvironmentContext,
    target_spec: &Option<String>,
    behavior: WaitFor,
) -> Result<(), ffx_command_error::Error> {
    match behavior {
        WaitFor::DeviceOnline | WaitFor::DeviceOffline => {
            let ever_found = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let use_cache = behavior == WaitFor::DeviceOffline;
            let knocker = RcsKnockerImpl { ever_found: ever_found.clone(), use_cache };
            wait_for_device_inner(knocker, wait_timeout, env, target_spec, behavior, ever_found)
                .await
        }
        WaitFor::Fastboot | WaitFor::Product => {
            wait_for_discovered_state(
                DefaultTargetStateDiscoverer,
                wait_timeout,
                env,
                target_spec,
                behavior,
            )
            .await
        }
    }
}

#[cfg_attr(test, mockall::automock)]
pub trait TargetStateDiscoverer {
    fn discover(
        &self,
        query: TargetInfoQuery,
        env: &EnvironmentContext,
    ) -> impl Future<Output = std::result::Result<Vec<TargetHandle>, crate::FfxTargetCrateError>>;
}

pub struct DefaultTargetStateDiscoverer;

impl TargetStateDiscoverer for DefaultTargetStateDiscoverer {
    async fn discover(
        &self,
        query: TargetInfoQuery,
        env: &EnvironmentContext,
    ) -> std::result::Result<Vec<TargetHandle>, crate::FfxTargetCrateError> {
        get_discovered_targets(query, true, true, env).await
    }
}

async fn wait_for_discovered_state(
    discoverer: impl TargetStateDiscoverer,
    wait_timeout: Option<Duration>,
    env: &EnvironmentContext,
    target_spec: &Option<String>,
    behavior: WaitFor,
) -> Result<(), ffx_command_error::Error> {
    let query = TargetInfoQuery::try_from(target_spec.clone())
        .map_err(|e| ffx_command_error::Error::User(e.into()))?;
    let source = target_source_for_query(&query, env);
    let discover_fut = async {
        loop {
            futures_lite::future::yield_now().await;

            match discoverer.discover(query.clone(), env).await {
                Ok(handles) => match resolve::expect_single_target(&query, handles, source.clone())
                {
                    Ok(handle) => {
                        let matches_state = match (behavior, &handle.state) {
                            (WaitFor::Fastboot, discovery::TargetState::Fastboot(_)) => true,
                            (WaitFor::Product, discovery::TargetState::Product { .. }) => true,
                            _ => false,
                        };
                        if matches_state {
                            return Ok(());
                        }
                        log::debug!(
                            "Target discovered with state {:?}, waiting for {:?}",
                            handle.state,
                            behavior
                        );
                    }
                    Err(FfxTargetError::OpenTargetError {
                        err: ffx::OpenTargetError::TargetNotFound,
                        ..
                    }) => {
                        log::debug!("Target not found yet, continuing to wait...");
                    }
                    Err(e) => {
                        return Err(ffx_command_error::Error::User(e.into()));
                    }
                },
                Err(e) => {
                    log::debug!("Discovery error while waiting for target state: {e:?}");
                }
            }

            Timer::new(Duration::from_millis(DOWN_REPOLL_DELAY_MS)).await;
        }
    };

    let timer = if wait_timeout.is_some() {
        Either::Left(fuchsia_async::Timer::new(wait_timeout.unwrap()))
    } else {
        Either::Right(pending())
    };

    futures_lite::FutureExt::or(discover_fut, async move {
        timer.await;
        Err(ffx_command_error::Error::User(
            FfxTargetError::DaemonError {
                err: DaemonError::Timeout,
                target: target_spec.clone().into(),
            }
            .into(),
        ))
    })
    .await
}

async fn wait_for_device_inner(
    knocker: impl RcsKnocker,
    wait_timeout: Option<Duration>,
    env: &EnvironmentContext,
    target_spec: &Option<String>,
    behavior: WaitFor,
    ever_found: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), ffx_command_error::Error> {
    let ever_knocked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ever_knocked_clone = ever_knocked.clone();
    let target_spec_clone = target_spec.clone();
    let knock_fut = async {
        loop {
            futures_lite::future::yield_now().await;
            // Note that we transform the target_spec into a query every time
            // through the loop, because while we are waiting for the device, we
            // may have to wait for a valid scope-id to appear.
            let query = match TargetInfoQuery::try_from(target_spec_clone.clone()) {
                Ok(q) => q,
                Err(e) => {
                    log::debug!("Waiting for valid target specifier: {e}");
                    Timer::new(Duration::from_millis(DOWN_REPOLL_DELAY_MS)).await;
                    continue;
                }
            };

            break match knocker.knock_rcs(&query, &env).await {
                Err(e) => {
                    log::debug!("unable to knock target: {e:?}");
                    if let WaitFor::DeviceOffline = behavior {
                        Ok(())
                    } else {
                        match e {
                            KnockError::Critical(e) => {
                                Err(ffx_command_error::Error::Unexpected(e.into()))
                            }
                            KnockError::NonCritical(e) => {
                                log::debug!("received non-critical error. retrying. error: {e}");
                                Timer::new(Duration::from_millis(DOWN_REPOLL_DELAY_MS)).await;
                                continue;
                            }
                        }
                    }
                }
                Ok(()) => {
                    ever_knocked.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let WaitFor::DeviceOffline = behavior {
                        Timer::new(Duration::from_millis(DOWN_REPOLL_DELAY_MS)).await;
                        continue;
                    } else {
                        Ok(())
                    }
                }
            };
        }
    };
    let timer = if wait_timeout.is_some() {
        Either::Left(fuchsia_async::Timer::new(wait_timeout.unwrap()))
    } else {
        Either::Right(pending())
    };
    futures_lite::FutureExt::or(knock_fut, async move {
        timer.await;
        let was_knocked = ever_knocked_clone.load(std::sync::atomic::Ordering::Relaxed);
        Err(ffx_command_error::Error::User(match behavior {
            WaitFor::DeviceOnline | WaitFor::Fastboot | WaitFor::Product => {
                FfxTargetError::DaemonError {
                    err: DaemonError::Timeout,
                    target: target_spec.clone().into(),
                }
                .into()
            }
            WaitFor::DeviceOffline => {
                if was_knocked {
                    FfxTargetError::DaemonError {
                        err: DaemonError::ShutdownTimeout,
                        target: target_spec.clone().into(),
                    }
                    .into()
                } else if ever_found.load(std::sync::atomic::Ordering::Relaxed) {
                    let msg = match target_spec {
                        Some(spec) if !spec.is_empty() => format!("Timeout waiting for device to shut down. Device \"{spec}\" was found but never responsive."),
                        _ => "Timeout waiting for device to shut down. The device was found but never responsive.".to_string(),
                    };
                    anyhow::anyhow!(msg).into()
                } else {
                    let discovery_timeout_ms =
                        env.get::<u64, _>(ffx_config::keys::DISCOVERY_TIMEOUT_MS).unwrap_or(2000);
                    let wait_timeout_ms = wait_timeout.map(|d| d.as_millis() as u64).unwrap_or(u64::MAX);

                    if wait_timeout_ms < discovery_timeout_ms {
                        anyhow::anyhow!("Timeout waiting for device to shut down. The specified timeout ({}ms) was too short to allow discovery to complete (discovery timeout is {}ms).", wait_timeout_ms, discovery_timeout_ms).into()
                    } else {
                        let msg = match target_spec {
                            Some(spec) if !spec.is_empty() => format!("Timeout waiting for device to shut down. Device \"{spec}\" was never found."),
                            _ => "Timeout waiting for device to shut down. The device was never found.".to_string(),
                        };
                        anyhow::anyhow!(msg).into()
                    }
                }
            }
        }))
    })
    .await
}

/// Represents the ability to knock RCS on a specified Target.
#[cfg_attr(test, mockall::automock)]
pub trait RcsKnocker {
    fn knock_rcs(
        &self,
        target_spec: &TargetInfoQuery,
        env: &EnvironmentContext,
    ) -> impl Future<Output = Result<(), KnockError>>;
}

///  Knocks RCS.
pub struct RcsKnockerImpl {
    pub ever_found: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub use_cache: bool,
}

impl<T: RcsKnocker + ?Sized> RcsKnocker for Box<T> {
    fn knock_rcs(
        &self,
        target_spec: &TargetInfoQuery,
        env: &EnvironmentContext,
    ) -> impl Future<Output = Result<(), KnockError>> {
        (**self).knock_rcs(target_spec, env)
    }
}

impl RcsKnocker for RcsKnockerImpl {
    async fn knock_rcs(
        &self,
        target_spec: &TargetInfoQuery,
        env: &EnvironmentContext,
    ) -> Result<(), KnockError> {
        knock_target_impl(target_spec, env, None, self.use_cache, Some(self.ever_found.clone()))
            .await
            .map(|()| {
                log::debug!("Knocked target.");
            })
    }
}

/// Attempts to "knock" a target to determine if it is up and connectable via RCS.
///
/// If `knock_timeout` is set to `None`, the default timeout will be set to 2 times
/// `DEFAULT_RCS_KNOCK_TIMEOUT`.
pub async fn knock_target(
    target_spec: &TargetInfoQuery,
    context: &EnvironmentContext,
    knock_timeout: Option<Duration>,
) -> Result<(), KnockError> {
    knock_target_impl(target_spec, context, knock_timeout, false, None).await
}

pub(crate) async fn knock_target_impl(
    target_spec: &TargetInfoQuery,
    context: &EnvironmentContext,
    knock_timeout: Option<Duration>,
    use_cache: bool,
    ever_found: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(), KnockError> {
    let knock_timeout = knock_timeout.unwrap_or(DEFAULT_RCS_KNOCK_TIMEOUT * 2);
    let res_future = async {
        log::debug!("resolving target spec address from {target_spec:?}");
        let source = target_source_for_query(target_spec, context);
        let discovery = build_discovery_from_config(context);
        let resolver = resolve::DefaultTargetResolver::new(discovery);
        let res = resolver
            .resolve_target_address(target_spec, use_cache, context, source)
            .await
            .map_err(|e| match e {
                // When knocking, it's not critical if we have not yet found the target. The caller should just retry
                FfxTargetError::OpenTargetError {
                    err: ffx::OpenTargetError::TargetNotFound,
                    ..
                } => KnockError::NonCritical(KnockNonCriticalError::TargetNotFound {
                    target: format!("{:?}", target_spec),
                }),
                _ => KnockError::Critical(KnockCriticalError::TargetError(format!("{:?}", e))),
            })?;

        if let Some(ever_found) = ever_found {
            ever_found.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        log::debug!("daemonless knock connecting to resolved target {:?}", res);
        if res.get_connection_if_already_established().is_none() {
            let conn = res.get_connection(context).await.map_err(|e| {
                KnockError::Critical(KnockCriticalError::TargetError(format!("{:?}", e)))
            })?;
            log::debug!("daemonless knock connection established");
            let _ = conn.rcs_proxy_fdomain().await.map_err(|e| {
                KnockError::NonCritical(KnockNonCriticalError::Custom(format!("{:?}", e)))
            })?;
        }
        Ok(())
    };

    futures_lite::pin!(res_future);
    timeout::timeout(knock_timeout, res_future).await.map_err(|_| {
        KnockError::NonCritical(KnockNonCriticalError::Timeout {
            detail: "daemonless knock timeout".to_string(),
        })
    })?
}

/// Get the target specifier.
///
/// ## What this function does
///
/// The target specifier is a string taken from the `EnvironmentContext` one of
/// two ways:
///
/// 1. From the top-level `ffx` `-t/--target` command line argument.
/// 2. From an environment variable. The environment will be searched, in order,
///    for `$FUCHSIA_DEVICE_ADDR`, and then `$FUCHSIA_NODENAME`. If neither are
///    set, then this function will return `Ok(None)`.
///
/// `ffx` validates that `-t/--target` must be set whenever using `--strict`,
/// erroring out if the target is not explicitly specified via command line.
///
///
/// ## Underlying config specifics
///
/// For more detail: The target specifier always comes from the config key
/// `target.default`. In step (1) the top level command line argument `--target`
/// or `-t` sets the `target.default` value for the _runtime config_, which
/// supersedes all other config levels. For more info see the `ffx_config`
/// crate.
///
/// In step (2) the `target.default` value (at the _default config_) is set to
/// an array of environment variables. The first environment variable found is
/// returned. If none are found, this function returns `Ok(None)`.
/// See the `target.default` field in `//src/developer/ffx/data/config.json`.
///
/// Note: Stateful config sources for `target.default` are always bypassed and
/// ignored here (i.e. ConfigLevel::{User, Build, Global}).
/// Only stateless config sources (i.e. `ConfigLevel::{Runtime, Default}`) for
/// `target.default` are used to determine the target specifier.
/// See https://fxbug.dev/394619603 for the rationale around this decision.
///
///
/// ## How the return value is intended to be used
///
/// The result is a string which can be turned into a `TargetInfoQuery` to match
/// against the available targets (by name, address, etc). We don't return the
/// query itself because some callers assume the specifier is the name of the
/// target. This is used for the purposes of error messages or other forms of
/// presentation. The repo server, for example, only works if an explicit
/// device name (exact match) is provided.  In other contexts, it is valid for
/// the specifier to be a substring of the nodename, a network address, serial
/// number, or vsock identifier.
pub fn get_target_specifier(context: &EnvironmentContext) -> Result<Option<String>> {
    get_target_specifier_with_source(context).map(|(spec, _)| spec)
}

/// Get the target specifier along with the source where it was configured.
///
/// See [`get_target_specifier`] for full details on how the target is determined.
pub fn get_target_specifier_with_source(
    context: &EnvironmentContext,
) -> Result<(Option<String>, Option<TargetSource>)> {
    if let Some(ts) = context.get_overridden_target_specifier() {
        return Ok((ts, Some(TargetSource::Overridden)));
    }
    let (runtime_spec, runtime_source) = context
        .query(TARGET_DEFAULT_KEY)
        .level(Some(ConfigLevel::Runtime))
        .build()
        .get_optional_with_source::<Option<String>>(context)?;

    if let Some(spec) = runtime_spec {
        let source = runtime_source.map(Into::into).unwrap_or(TargetSource::CommandLine);
        info!("Target specifier: ['{spec:?}'] (source: command-line/runtime)");
        return Ok((Some(spec), Some(source)));
    }

    let (default_spec, default_source) = context
        .query(TARGET_DEFAULT_KEY)
        .level(Some(ConfigLevel::Default))
        .build()
        .get_optional_with_source::<Option<String>>(context)?;

    if let Some(spec) = default_spec {
        let source = default_source.map(Into::into).unwrap_or(TargetSource::Default);
        info!("Target specifier: ['{spec:?}'] (source: {source:?})");
        return Ok((Some(spec), Some(source)));
    }

    debug!("No target specified");
    Ok((None, None))
}

/// Returns the `TargetSource` if the given `query` matches the target configured in the `context`.
pub fn target_source_for_query(
    query: &TargetInfoQuery,
    context: &EnvironmentContext,
) -> Option<TargetSource> {
    match get_target_specifier_with_source(context) {
        Ok((spec, source)) => match TargetInfoQuery::try_from(spec) {
            Ok(default_query) if &default_query == query => source,
            _ => None,
        },
        Err(_) => None,
    }
}

/// Discover fastboot targets only. Useful for fastboot-related plugins (flash/bootloader/fastboot).
pub async fn discover_fastboot_target(
    ctx: &EnvironmentContext,
    query: TargetInfoQuery,
    timeout: Option<u64>,
) -> Result<TargetHandle> {
    let mut builder = crate::resolve::build_discovery_builder(DiscoverySources::all(), ctx)
        .with_state_filter(discovery::TargetStateFilter::FASTBOOT)
        .with_short_circuit_on_first(true);
    if let Some(ms) = timeout {
        builder = builder.with_timeout_msecs(Some(ms));
    };
    let disco = builder.build(&ctx);

    let discovered_devices = disco.discover_devices(query.clone()).await?;

    let source = target_source_for_query(&query, ctx);
    resolve::expect_single_target(&query, discovered_devices, source).map_err(|e| e.into())
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_command_error::bug;
    use ffx_config::{test_env, test_init};
    use futures_lite::future::{pending, ready};
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_get_target_specifier_unset() {
        // Explicitly initialize the test with no env vars.
        // That way, $FUCHSIA_NODENAME and $FUCHSIA_DEVICE_ADDR are both unset.
        let env = test_env().build().unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, None);
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_from_nodename_env() {
        let env = test_env().env_var("FUCHSIA_NODENAME", "nodename-default").build().unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, Some("nodename-default".into()));
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_from_device_addr_env() {
        let env = test_env().env_var("FUCHSIA_DEVICE_ADDR", "device-addr-default").build().unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, Some("device-addr-default".into()));
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_from_both_envs() {
        let env = test_env()
            .env_var("FUCHSIA_NODENAME", "nodename-default")
            .env_var("FUCHSIA_DEVICE_ADDR", "device-addr-default")
            .build()
            .unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, Some("device-addr-default".into()));
    }

    #[fuchsia::test]
    async fn test_discover_fastboot_target_not_found() {
        let env = test_init().unwrap();
        let res = discover_fastboot_target(&env.context, TargetInfoQuery::First, Some(1)).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_bypasses_state() {
        let build_dir = tempdir().expect("temp dir");
        let env = test_env()
            .in_tree(build_dir.path())
            .user_config(TARGET_DEFAULT_KEY, "stateful-user-default")
            .build_config(TARGET_DEFAULT_KEY, "stateful-build-default")
            .global_config(TARGET_DEFAULT_KEY, "stateful-global-default")
            .build()
            .unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, None);
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_from_nodename_env_bypasses_state() {
        let build_dir = tempdir().expect("temp dir");
        let env = test_env()
            .env_var("FUCHSIA_NODENAME", "nodename-default")
            .in_tree(build_dir.path())
            .user_config(TARGET_DEFAULT_KEY, "stateful-user-default")
            .build_config(TARGET_DEFAULT_KEY, "stateful-build-default")
            .global_config(TARGET_DEFAULT_KEY, "stateful-global-default")
            .build()
            .unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, Some("nodename-default".into()));
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_from_all_sources() {
        let build_dir = tempdir().expect("temp dir");
        let env = test_env()
            .env_var("FUCHSIA_NODENAME", "nodename-default")
            .env_var("FUCHSIA_DEVICE_ADDR", "device-addr-default")
            .runtime_config(TARGET_DEFAULT_KEY, "runtime-default")
            .in_tree(build_dir.path())
            .user_config(TARGET_DEFAULT_KEY, "stateful-user-default")
            .build_config(TARGET_DEFAULT_KEY, "stateful-build-default")
            .global_config(TARGET_DEFAULT_KEY, "stateful-global-default")
            .build()
            .unwrap();

        let target_spec = get_target_specifier(&env.context).unwrap();
        assert_eq!(target_spec, Some("runtime-default".into()));
    }

    #[fuchsia::test]
    async fn test_get_override_target_spec() {
        let env = test_init().unwrap();
        let mut context = env.context.clone();
        context.override_target_specifier(&Some("foo".to_string()));
        let target = get_target_specifier(&context).expect("get_target_specifier");
        assert_eq!(target, Some("foo".to_string()));
    }

    #[fuchsia::test]
    async fn test_get_target_specifier_with_source() {
        let env = test_env().env_var("FUCHSIA_NODENAME", "nodename-default").build().unwrap();
        let (spec, source) = get_target_specifier_with_source(&env.context).unwrap();
        assert_eq!(spec, Some("nodename-default".into()));
        assert_eq!(source, Some(TargetSource::Environment("FUCHSIA_NODENAME".into())));

        let env_addr = test_env().env_var("FUCHSIA_DEVICE_ADDR", "addr-default").build().unwrap();
        let (spec, source) = get_target_specifier_with_source(&env_addr.context).unwrap();
        assert_eq!(spec, Some("addr-default".into()));
        assert_eq!(source, Some(TargetSource::Environment("FUCHSIA_DEVICE_ADDR".into())));

        let env_runtime = test_env()
            .runtime_config(TARGET_DEFAULT_KEY, "runtime-target")
            .env_var("FUCHSIA_NODENAME", "nodename-default")
            .build()
            .unwrap();
        let (spec, source) = get_target_specifier_with_source(&env_runtime.context).unwrap();
        assert_eq!(spec, Some("runtime-target".into()));
        assert_eq!(source, Some(TargetSource::CommandLine));

        let env_unset = test_env().build().unwrap();
        let (spec, source) = get_target_specifier_with_source(&env_unset.context).unwrap();
        assert_eq!(spec, None);
        assert_eq!(source, None);

        let mut override_context = env_unset.context.clone();
        override_context.override_target_specifier(&Some("overridden-target".to_string()));
        let (spec, source) = get_target_specifier_with_source(&override_context).unwrap();
        assert_eq!(spec, Some("overridden-target".into()));
        assert_eq!(source, Some(TargetSource::Overridden));

        // Test with empty string in runtime config (does not fall back to default)
        let env_empty_runtime = test_env()
            .runtime_config(TARGET_DEFAULT_KEY, "")
            .env_var("FUCHSIA_NODENAME", "nodename-default")
            .build()
            .unwrap();
        let (spec, source) = get_target_specifier_with_source(&env_empty_runtime.context).unwrap();
        assert_eq!(spec, Some("".into()));
        assert_eq!(source, Some(TargetSource::CommandLine));
    }

    #[fuchsia::test]
    async fn test_target_source_for_query() {
        let env = test_env().env_var("FUCHSIA_NODENAME", "my-target").build().unwrap();

        // Query matching the default target from environment
        let matching_query = TargetInfoQuery::NodenameOrId("my-target".to_string());
        assert_eq!(
            target_source_for_query(&matching_query, &env.context),
            Some(TargetSource::Environment("FUCHSIA_NODENAME".to_string()))
        );

        // Query for a different custom/explicit target
        let different_query = TargetInfoQuery::NodenameOrId("other-target".to_string());
        assert_eq!(target_source_for_query(&different_query, &env.context), None);

        // Environment with no default target set
        let unset_env = test_env().build().unwrap();
        assert_eq!(target_source_for_query(&matching_query, &unset_env.context), None);
    }

    #[fuchsia::test]
    async fn test_bad_timeout() {
        let env = test_init().unwrap();
        assert!(
            knock_target(
                &TargetInfoQuery::NodenameOrId("foo".to_string()),
                &env.context,
                Some(rcs::RCS_KNOCK_TIMEOUT)
            )
            .await
            .is_err()
        );
    }

    #[fuchsia::test]
    async fn wait_for_device_knock_works() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(async { Ok(()) }));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3000)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOnline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn wait_for_device_timeout_on_shutdown() {
        let mut mock = MockRcsKnocker::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_knock_rcs()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Box::pin(ready(Ok(()))));
        mock.expect_knock_rcs().returning(|_, _| Box::pin(pending()));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        // This step is essential for converting the error properly. Otherwise converting it to top
        // level anyhow error will lost context and turn the error into a string, making
        // downcasting infeasible.
        let anyhow_err: anyhow::Error =
            res.unwrap_err().source().expect("should have an anyhow error source");
        let FfxTargetError::DaemonError { err, .. } =
            anyhow_err.downcast_ref::<FfxTargetError>().expect("expected target error")
        else {
            panic!("Received unexpected error: {anyhow_err:?}");
        };
        assert!(matches!(err, DaemonError::ShutdownTimeout));
    }

    #[fuchsia::test]
    async fn wait_for_device_timeout_on_shutdown_never_knocked() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(pending()));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        let err = res.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains(
                "Timeout waiting for device to shut down. Device \"foo\" was never found."
            ),
            "expected new error message, got: {err_msg}"
        );
    }

    #[fuchsia::test]
    async fn wait_for_device_timeout_on_shutdown_found_but_never_responsive() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(pending()));
        let env = ffx_config::test_init().unwrap();
        let ever_found = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(1)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            ever_found.clone(),
        )
        .await;
        let err = res.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains(
                "Timeout waiting for device to shut down. Device \"foo\" was found but never responsive."
            ),
            "expected new error message, got: {err_msg}"
        );
    }

    #[fuchsia::test]
    async fn wait_for_device_timeout_on_shutdown_short_timeout() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(pending()));

        let env = ffx_config::test_env()
            .user_config(ffx_config::keys::DISCOVERY_TIMEOUT_MS, 2000)
            .build()
            .unwrap();

        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(1)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        let err = res.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("was too short to allow discovery to complete"),
            "expected short timeout message, got: {err_msg}"
        );
    }

    #[fuchsia::test]
    async fn wait_for_device_hangs_indefinitely() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(pending()));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOnline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn wait_for_device_critical_error_causes_failure() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().times(1).returning(|_, _| {
            Box::pin(async {
                Err(KnockError::Critical(KnockCriticalError::Custom(format!("{}", bug!("Oh no!")))))
            })
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOnline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn wait_for_device_critical_error_does_not_cause_failure_waiting_for_down() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().times(1).returning(|_, _| {
            Box::pin(async {
                Err(KnockError::Critical(KnockCriticalError::Custom(format!("{}", bug!("Oh no!")))))
            })
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn non_critical_error_causes_eventual_timeout() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| {
            Box::pin(async {
                Err(KnockError::NonCritical(KnockNonCriticalError::Custom(format!(
                    "{}",
                    bug!("Oh no!")
                ))))
            })
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOnline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn non_critical_error_returns_ok_for_down_target() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| {
            Box::pin(async {
                Err(KnockError::NonCritical(KnockNonCriticalError::Custom(format!(
                    "{}",
                    bug!("Oh no!")
                ))))
            })
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn knock_error_reattempt_successful() {
        let mut mock = MockRcsKnocker::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_knock_rcs().times(1).in_sequence(&mut seq).returning(|_, _| {
            Box::pin(ready(Err(KnockError::NonCritical(KnockNonCriticalError::Timeout {
                detail: "timeout".to_string(),
            }))))
        });
        mock.expect_knock_rcs()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Box::pin(ready(Ok(()))));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(10)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOnline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn wait_for_offline_after_online() {
        let mut mock = MockRcsKnocker::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_knock_rcs()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Box::pin(ready(Ok(()))));
        mock.expect_knock_rcs()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Box::pin(ready(Ok(()))));
        mock.expect_knock_rcs().times(1).in_sequence(&mut seq).returning(|_, _| {
            Box::pin(ready(Err(KnockError::NonCritical(KnockNonCriticalError::Custom(format!(
                "{}",
                bug!("Oh no it's not connected")
            ))))))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn wait_for_down_when_able_to_connect_to_device() {
        let mut mock = MockRcsKnocker::new();
        mock.expect_knock_rcs().returning(|_, _| Box::pin(ready(Ok(()))));
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_device_inner(
            mock,
            Some(Duration::from_secs(3)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::DeviceOffline,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn test_wait_for_fastboot_success() {
        let mut mock = MockTargetStateDiscoverer::new();
        mock.expect_discover().times(1).returning(|_, _| {
            Box::pin(ready(Ok(vec![discovery::TargetHandle {
                node_name: Some("foo".to_string()),
                state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                    serial_number: "12345".to_string(),
                    connection_state: discovery::FastbootConnectionState::Usb,
                }),
                manual: false,
            }])))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Fastboot,
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn test_wait_for_product_success() {
        let mut mock = MockTargetStateDiscoverer::new();
        mock.expect_discover().times(1).returning(|_, _| {
            Box::pin(ready(Ok(vec![discovery::TargetHandle {
                node_name: Some("foo".to_string()),
                state: discovery::TargetState::Product {
                    addrs: vec![],
                    serial: Some("12345".to_string()),
                },
                manual: false,
            }])))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Product,
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn test_wait_for_fastboot_timeout() {
        let mut mock = MockTargetStateDiscoverer::new();
        mock.expect_discover().returning(|_, _| {
            Box::pin(ready(Ok(vec![discovery::TargetHandle {
                node_name: Some("foo".to_string()),
                state: discovery::TargetState::Product {
                    addrs: vec![],
                    serial: Some("12345".to_string()),
                },
                manual: false,
            }])))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_millis(50)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Fastboot,
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn test_wait_for_product_timeout() {
        let mut mock = MockTargetStateDiscoverer::new();
        mock.expect_discover().returning(|_, _| {
            Box::pin(ready(Ok(vec![discovery::TargetHandle {
                node_name: Some("foo".to_string()),
                state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                    serial_number: "12345".to_string(),
                    connection_state: discovery::FastbootConnectionState::Usb,
                }),
                manual: false,
            }])))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_millis(50)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Product,
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }

    #[fuchsia::test]
    async fn test_wait_for_discovered_state_retry_then_success() {
        let mut mock = MockTargetStateDiscoverer::new();
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();
        mock.expect_discover().returning(move |_, _| {
            let attempt = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt == 0 {
                Box::pin(ready(Ok(vec![])))
            } else {
                Box::pin(ready(Ok(vec![discovery::TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                        serial_number: "12345".to_string(),
                        connection_state: discovery::FastbootConnectionState::Usb,
                    }),
                    manual: false,
                }])))
            }
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Fastboot,
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
        assert!(call_count.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    }

    #[fuchsia::test]
    async fn test_wait_for_discovered_state_discovery_error_then_success() {
        let mut mock = MockTargetStateDiscoverer::new();
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();
        mock.expect_discover().returning(move |_, _| {
            let attempt = count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if attempt == 0 {
                Box::pin(ready(Err(crate::FfxTargetCrateError::Resolution(
                    crate::error::TargetResolutionError::NonNetworkTarget,
                ))))
            } else {
                Box::pin(ready(Ok(vec![discovery::TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: discovery::TargetState::Product {
                        addrs: vec![],
                        serial: Some("12345".to_string()),
                    },
                    manual: false,
                }])))
            }
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &Some("foo".to_string()),
            WaitFor::Product,
        )
        .await;
        assert!(res.is_ok(), "{:?}", res);
        assert!(call_count.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    }

    #[fuchsia::test]
    async fn test_wait_for_discovered_state_ambiguous_error() {
        let mut mock = MockTargetStateDiscoverer::new();
        mock.expect_discover().times(1).returning(|_, _| {
            Box::pin(ready(Ok(vec![
                discovery::TargetHandle {
                    node_name: Some("foo1".to_string()),
                    state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                        serial_number: "12345".to_string(),
                        connection_state: discovery::FastbootConnectionState::Usb,
                    }),
                    manual: false,
                },
                discovery::TargetHandle {
                    node_name: Some("foo2".to_string()),
                    state: discovery::TargetState::Fastboot(discovery::FastbootTargetState {
                        serial_number: "67890".to_string(),
                        connection_state: discovery::FastbootConnectionState::Usb,
                    }),
                    manual: false,
                },
            ])))
        });
        let env = ffx_config::test_init().unwrap();
        let res = wait_for_discovered_state(
            mock,
            Some(Duration::from_secs(5)),
            &env.context,
            &None,
            WaitFor::Fastboot,
        )
        .await;
        assert!(res.is_err(), "{:?}", res);
    }
}
