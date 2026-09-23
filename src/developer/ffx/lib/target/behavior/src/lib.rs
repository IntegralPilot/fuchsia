// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::DiscoverySources;
#[cfg(test)]
use discovery::{DiscoveryBuilder, TargetEvent, TargetHandle, TargetState};
use ffx_command_error::{Result, bug};
use ffx_config::EnvironmentContext;
use ffx_target::{
    DefaultTargetResolver, Resolution, build_discovery, build_discovery_builder_common,
    build_discovery_from_config,
};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::OnceCell;

struct DirectConnectorInner {
    context: EnvironmentContext,
    resolution: futures::lock::Mutex<Option<Arc<Resolution>>>,
    resolver: futures::lock::Mutex<DefaultTargetResolver>,
}

#[derive(Debug, thiserror::Error)]
pub enum TargetResolutionError {
    #[error("Failed to resolve target from environment: {0}")]
    Context(#[from] ffx_command_error::Error),
}

impl TargetResolutionError {
    pub fn into_command_error(self) -> ffx_command_error::Error {
        match self {
            TargetResolutionError::Context(e) => e,
        }
    }
}

#[derive(Clone)]
pub struct DirectConnector(Arc<DirectConnectorInner>);

impl DirectConnector {
    pub fn from_resolution_for_test(resolution: Resolution) -> Self {
        let discovery = build_discovery(DiscoverySources::all(), &EnvironmentContext::default());
        DirectConnector(Arc::new(DirectConnectorInner {
            context: EnvironmentContext::default(),
            resolution: futures::lock::Mutex::new(Some(Arc::new(resolution))),
            resolver: futures::lock::Mutex::new(DefaultTargetResolver::new(discovery)),
        }))
    }

    pub async fn discovery_timeout(&self) -> Option<Duration> {
        self.0.resolver.lock().await.discovery_timeout()
    }

    pub async fn set_discovery_timeout(&self, timeout: Option<Duration>) {
        self.0.resolver.lock().await.set_discovery_timeout(timeout);
    }

    // Return a pinned boxed future (LocalBoxFuture) to prevent deep recursion
    // of compiler-generated future types. When this method is called within
    // other complex async contexts (e.g., the ffx log or update plugins),
    // the nested async blocks can otherwise exceed the compiler's recursion limit.
    pub fn resolution(
        &self,
    ) -> futures::future::LocalBoxFuture<'_, Result<Arc<Resolution>, TargetResolutionError>> {
        Box::pin(async move {
            let mut resolution = self.0.resolution.lock().await;

            let use_cache = if let Some(resolution) = &*resolution {
                if resolution.is_usable().await {
                    return Ok(Arc::clone(resolution));
                }
                false
            } else {
                true
            };

            let resolver = self.0.resolver.lock().await;
            let new = Arc::new(
                Resolution::try_from_env_context_with_resolver(
                    &*resolver,
                    &self.0.context,
                    use_cache,
                )
                .await?,
            );
            *resolution = Some(Arc::clone(&new));

            Ok(new)
        })
    }

    fn get_connection_if_already_established(&self) -> Option<Arc<ffx_target::Connection>> {
        if let Some(guard) = self.0.resolution.try_lock() {
            if let Some(resolution) = &*guard {
                resolution.get_connection_if_already_established()
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub enum ConnectionBehavior {
    Direct(DirectConnector),
}

impl ConnectionBehavior {
    pub fn direct_connector(&self) -> Option<&DirectConnector> {
        let ConnectionBehavior::Direct(dc) = self;
        Some(dc)
    }

    pub fn fake_direct_connector(resolution: Resolution) -> Self {
        ConnectionBehavior::Direct(DirectConnector::from_resolution_for_test(resolution))
    }

    pub async fn fake_with_fdomain_client(client: std::sync::Arc<fdomain_client::Client>) -> Self {
        let socket_addr = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            22,
        );
        let target_handle = discovery::TargetHandle {
            node_name: Some("test-target".to_string()),
            state: discovery::TargetState::Product {
                addrs: vec![addr::TargetAddr::Net(socket_addr)],
                serial: None,
            },
            manual: false,
        };
        let resolution = ffx_target::Resolution::from_target_handle(target_handle).unwrap();
        let conn = ffx_target::Connection::from_fdomain_client(client);
        resolution.set_connection_for_test(Some(conn)).await;
        Self::fake_direct_connector(resolution)
    }
}

pub async fn setup_fake_resolution(host_address: Option<String>) -> Resolution {
    let device_address =
        std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), 22);
    let target_handle = discovery::TargetHandle {
        node_name: Some("test-target".to_string()),
        state: discovery::TargetState::Product {
            addrs: vec![addr::TargetAddr::Net(device_address.clone())],
            serial: None,
        },
        manual: false,
    };
    let ret = Resolution::from_target_handle(target_handle).unwrap();
    let host_ssh_address = host_address.map(|a| ffx_ssh::parse::HostAddr::from(a));
    let fidl_pipe =
        ffx_target::FidlPipe::fake_with_host_ssh_address(Some(device_address), host_ssh_address);
    let conn = ffx_target::Connection::fake(fidl_pipe);
    ret.set_connection_for_test(Some(conn)).await;
    ret
}

// Manually implement Debug here so we can skip implementing
// Debug on the traits of the variant data.
impl fmt::Debug for ConnectionBehavior {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ConnectionBehavior::Direct(_) = self;
        write!(f, "ConnectionBehavior(DirectConnector)")
    }
}

#[derive(Clone, Default)]
pub struct FhoTargetEnvironment(Arc<FhoTargetEnvironmentOuter>);

#[derive(Default)]
pub struct FhoTargetEnvironmentOuter {
    inner: FhoTargetEnvironmentInner,
    want_direct: AtomicBool,
}

impl std::ops::Deref for FhoTargetEnvironment {
    type Target = FhoTargetEnvironmentOuter;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FhoTargetEnvironmentOuter {
    /// This attempts to wrap errors around a potential failure in the underlying connection being
    /// used to facilitate FIDL protocols. This should NOT be used by developers, this is intended
    /// to be used outside of the scope of an ffx subtool (outside of the `main` function).
    fn maybe_wrap_connection_errors(&self, err: fho::Error) -> fho::Error {
        self.inner.maybe_wrap_connection_errors(err)
    }

    /// Specify that we want a direct connection if we ever make that connection. Used when
    /// an FfxTool specifies '#[direct]', but may not actually need a connection at all
    pub fn set_direct(&self) {
        self.want_direct.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn get_direct(&self) -> bool {
        self.want_direct.load(Ordering::Acquire)
    }

    /// Initialize a direct connection behavior.
    pub async fn init_connection_behavior(
        &self,
        context: &EnvironmentContext,
    ) -> Result<Arc<ConnectionBehavior>> {
        self.init_direct_connection_behavior(context).await
    }

    /// Initialize connection behavior with an infinite discovery timeout.
    pub async fn init_connection_behavior_indef(
        &self,
        context: &EnvironmentContext,
    ) -> Result<Arc<ConnectionBehavior>> {
        self.inner.init_direct_connection_behavior_indef(context).await
    }

    /// Explicitly create direct connection behavior.
    pub async fn init_direct_connection_behavior(
        &self,
        context: &EnvironmentContext,
    ) -> Result<Arc<ConnectionBehavior>> {
        self.inner.init_direct_connection_behavior(context).await
    }

    pub fn set_behavior_for_test(&self, new_behavior: ConnectionBehavior) {
        self.inner.set_behavior_for_test(new_behavior)
    }

    pub fn behavior(&self) -> Result<Arc<ConnectionBehavior>> {
        self.inner.behavior()
    }
}

pub struct FhoTargetEnvironmentInner {
    /// Defines how to connect to a Fuchsia device. Multiple tasks can attempt
    /// to initialize it at the same time, so we gate the initialization using
    /// a OnceCell.
    behavior: OnceCell<Arc<ConnectionBehavior>>,
}

impl Default for FhoTargetEnvironmentInner {
    fn default() -> Self {
        Self { behavior: OnceCell::new() }
    }
}

impl FhoTargetEnvironmentInner {
    pub fn set_behavior_for_test(&self, new_behavior: ConnectionBehavior) {
        self.behavior.set(Arc::new(new_behavior)).expect("OnceCell::set(behavior)")
    }

    /// This attempts to wrap errors around a potential failure in the underlying connection being
    /// used to facilitate FIDL protocols. This should NOT be used by developers, this is intended
    /// to be used outside of the scope of an ffx subtool (outside of the `main` function).
    fn maybe_wrap_connection_errors(&self, err: fho::Error) -> fho::Error {
        if let Some(behavior) = self.behavior.get() {
            let ConnectionBehavior::Direct(dc) = &**behavior;
            if let Some(conn) = dc.get_connection_if_already_established() {
                match err {
                    fho::Error::User(e) => {
                        return fho::Error::User(conn.wrap_connection_errors(e).into());
                    }
                    fho::Error::Unexpected(e) => {
                        return fho::Error::Unexpected(conn.wrap_connection_errors(e).into());
                    }
                    _ => (),
                }
            }
        }
        err
    }

    /// Explicitly create direct connection behavior with a specific discovery timeout.
    pub async fn init_direct_connection_behavior_indef(
        &self,
        context: &EnvironmentContext,
    ) -> Result<Arc<ConnectionBehavior>> {
        let behavior = self
            .initialize_behavior_with(|| async {
                log::info!("Initializing indef ConnectionBehavior::DirectConnector");
                let discovery =
                    build_discovery_builder_common(context).with_timeout_msecs(None).build(context);
                let resolver = DefaultTargetResolver::new(discovery);
                let connector = DirectConnector(Arc::new(DirectConnectorInner {
                    context: context.clone(),
                    resolution: futures::lock::Mutex::new(None),
                    resolver: futures::lock::Mutex::new(resolver),
                }));
                Ok(ConnectionBehavior::Direct(connector))
            })
            .await?;
        Ok(behavior)
    }

    /// Explicitly create direct connection behavior. Note that we don't actually
    /// resolve a connection here -- it will only be validated when requested via
    /// `resolution().` Among other things, this allows unit-tests to test connection
    /// logic without requiring a target.
    pub async fn init_direct_connection_behavior(
        &self,
        context: &EnvironmentContext,
    ) -> Result<Arc<ConnectionBehavior>> {
        self.initialize_behavior_with(|| async {
            log::info!("Initializing ConnectionBehavior::DirectConnector");
            let discovery = build_discovery_from_config(context);
            let resolver = DefaultTargetResolver::new(discovery);
            let connector = DirectConnector(Arc::new(DirectConnectorInner {
                context: context.clone(),
                resolution: futures::lock::Mutex::new(None),
                resolver: futures::lock::Mutex::new(resolver),
            }));
            Ok(ConnectionBehavior::Direct(connector))
        })
        .await
    }

    pub fn behavior(&self) -> Result<Arc<ConnectionBehavior>> {
        self.behavior.get().cloned().ok_or(bug!("Connection behavior is not initialized"))
    }

    async fn initialize_behavior_with<F, Fut>(&self, creator: F) -> Result<Arc<ConnectionBehavior>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<ConnectionBehavior>>,
    {
        self.behavior
            .get_or_try_init(move || async move {
                log::debug!("initializing behavior");
                let res = creator().await.map(Arc::new);
                log::debug!("initializing behavior done");
                res
            })
            .await
            .map(Clone::clone)
    }
}

impl fho::EnvironmentInterface for FhoTargetEnvironment {
    fn wrap_main_errors(&self, err: fho::Error) -> fho::Error {
        self.maybe_wrap_connection_errors(err)
    }
}
pub fn target_interface(env: &fho::FhoEnvironment) -> FhoTargetEnvironment {
    if env.get_interface::<FhoTargetEnvironment>().is_none() {
        let target_interface = FhoTargetEnvironment::default();
        env.set_interface(target_interface);
    }
    env.get_interface::<FhoTargetEnvironment>().expect("No target interface in FhoEnvironment??")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::environment::ExecutableKind;
    use ffx_config::test_env;

    #[fuchsia::test]
    async fn test_connection_behavior_correct_in_strict() {
        let runtime_args =
            serde_json::json!({"target": {"default" : "127.0.0.1"}}).as_object().unwrap().clone();
        let ctx = EnvironmentContext::strict(ExecutableKind::Test, runtime_args).unwrap();
        let target_env = FhoTargetEnvironment::default();
        let behavior = target_env.init_connection_behavior(&ctx).await.unwrap();
        let _ = behavior.direct_connector().unwrap().resolution();
    }

    #[fuchsia::test]
    async fn test_connection_behavior_correct_in_non_strict() {
        let env = test_env().build().unwrap();
        let target_env = FhoTargetEnvironment::default();
        let behavior = target_env.init_connection_behavior(&env.context).await.unwrap();
        let _ = behavior.direct_connector().unwrap().resolution();
    }

    #[fuchsia::test]
    async fn test_direct_connection_behavior() {
        let env = test_env().runtime_config("target.default", "127.0.0.1").build().unwrap();
        let target_env = FhoTargetEnvironment::default();
        let behavior = target_env.init_connection_behavior(&env.context).await.unwrap();
        let _ = behavior.direct_connector().unwrap().resolution();
    }

    #[fuchsia::test]
    async fn set_behavior_succeeds_when_called_twice() {
        let env = test_env().build().unwrap();
        let target_env = FhoTargetEnvironment::default();
        let _behavior = target_env.init_connection_behavior(&env.context).await.unwrap();
        let res = target_env.init_connection_behavior(&env.context).await;
        assert!(matches!(res, Ok(_)))
    }

    #[fuchsia::test]
    async fn test_connection_behavior_correct_with_set_direct() {
        let env = test_env().runtime_config("target.default", "127.0.0.1").build().unwrap();
        let target_env = FhoTargetEnvironment::default();
        target_env.set_direct();
        let behavior = target_env.init_connection_behavior(&env.context).await.unwrap();
        let _ = behavior.direct_connector().unwrap().resolution();
    }

    #[fuchsia::test]
    async fn set_behavior_persists() {
        let env = test_env().build().unwrap();
        let fho_env = fho::FhoEnvironment::new_with_args(&env.context, &["some", "test"]);
        let target_env = target_interface(&fho_env);
        assert_eq!(target_env.get_direct(), false);
        target_env.set_direct();
        let target_env = target_interface(&fho_env);
        assert_eq!(target_env.get_direct(), true);
    }

    fn write_test_cache(context: &EnvironmentContext, nodename: &str, ip: &str) {
        let cache_file = ffx_target::get_discovery_cache_file(context).expect("cache file path");
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let json = serde_json::json!({
            "version": 2,
            "expires": "2036-06-03T19:50:40Z",
            "targets": [
                {
                    "nodename": nodename,
                    "addresses": [
                        {"Net": ip}
                    ],
                    "rcs_state": "Unknown",
                    "target_state": "Product",
                    "product_config": null,
                    "board_config": null,
                    "serial_number": null,
                    "is_manual": false,
                    "boot_id": null,
                    "is_default": null
                }
            ]
        });
        let file = std::fs::File::create(cache_file).unwrap();
        serde_json::to_writer(file, &json).unwrap();
    }

    #[fuchsia::test]
    async fn test_direct_connector_initial_resolution_uses_cache() {
        let mut builder = test_env();
        let cache_dir = builder.isolate_root().join("cache");
        let env = builder
            .runtime_config("target.default", "test-target")
            .runtime_config("target.discovery_cache_dir", cache_dir.to_str().unwrap())
            .build()
            .unwrap();

        write_test_cache(&env.context, "test-target", "127.0.0.1:8082");

        let discovery = build_discovery(DiscoverySources::all(), &env.context);
        let resolver = DefaultTargetResolver::new(discovery);
        let connector = DirectConnector(Arc::new(DirectConnectorInner {
            context: env.context.clone(),
            resolution: futures::lock::Mutex::new(None),
            resolver: futures::lock::Mutex::new(resolver),
        }));

        let res = connector.resolution().await;
        assert!(res.is_ok(), "Expected resolution to succeed using cache, got {:?}", res);
        let res = res.unwrap();
        assert_eq!(res.target_spec(), "127.0.0.1:8082");
    }

    #[fuchsia::test]
    async fn test_direct_connector_reconnect_bypasses_cache() {
        let mut builder = test_env();
        let cache_dir = builder.isolate_root().join("cache");
        let env = builder
            .runtime_config("target.default", "test-target")
            .runtime_config("target.discovery_cache_dir", cache_dir.to_str().unwrap())
            .build()
            .unwrap();

        write_test_cache(&env.context, "test-target", "127.0.0.1:8082");

        #[derive(Debug)]
        struct TerminatedConnector;
        impl ffx_target::TargetConnector for TerminatedConnector {
            const CONNECTION_TYPE: &'static str = "terminated";
            async fn connect(
                &mut self,
            ) -> Result<ffx_target::TargetConnection, ffx_target::TargetConnectionError>
            {
                Ok(ffx_target::TargetConnection::FDomain(ffx_target::FDomainConnection::invalid()))
            }
        }

        let conn = ffx_target::Connection::new(TerminatedConnector).await.unwrap();
        fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        assert!(conn.is_terminated());

        let resolution = Resolution::mock(|| Err(anyhow::anyhow!("reconnect failed").into()));

        resolution.set_connection_for_test(Some(conn)).await;

        let discovery = build_discovery(DiscoverySources::all(), &env.context);
        let resolver = DefaultTargetResolver::new(discovery);
        let connector = DirectConnector(Arc::new(DirectConnectorInner {
            context: env.context.clone(),
            resolution: futures::lock::Mutex::new(Some(Arc::new(resolution))),
            resolver: futures::lock::Mutex::new(resolver),
        }));

        let res = connector.resolution().await;
        assert!(
            res.is_err(),
            "Expected resolution to fail (cache bypassed), but got Ok: {:?}",
            res
        );
    }

    #[fuchsia::test]
    async fn test_direct_connector_reresolves_on_terminated_connection() {
        let env = test_env().runtime_config("target.default", "127.0.0.1:8082").build().unwrap();

        #[derive(Debug)]
        struct TerminatedConnector;
        impl ffx_target::TargetConnector for TerminatedConnector {
            const CONNECTION_TYPE: &'static str = "terminated";
            async fn connect(
                &mut self,
            ) -> Result<ffx_target::TargetConnection, ffx_target::TargetConnectionError>
            {
                Ok(ffx_target::TargetConnection::FDomain(ffx_target::FDomainConnection::invalid()))
            }
        }

        let conn = ffx_target::Connection::new(TerminatedConnector).await.unwrap();
        fuchsia_async::Timer::new(std::time::Duration::from_millis(10)).await;
        assert!(conn.is_terminated());

        let reconnect_called = Arc::new(AtomicBool::new(false));
        let reconnect_called_clone = reconnect_called.clone();
        let initial_resolution = Resolution::mock(move || {
            reconnect_called_clone.store(true, Ordering::SeqCst);
            Err(anyhow::anyhow!("reconnect failed").into())
        });

        initial_resolution.set_connection_for_test(Some(conn)).await;

        let discovery = build_discovery(DiscoverySources::all(), &env.context);
        let resolver = DefaultTargetResolver::new(discovery);
        let connector = DirectConnector(Arc::new(DirectConnectorInner {
            context: env.context.clone(),
            resolution: futures::lock::Mutex::new(Some(Arc::new(initial_resolution))),
            resolver: futures::lock::Mutex::new(resolver),
        }));

        let res = connector.resolution().await;
        assert!(res.is_ok(), "Expected re-resolution to succeed for IP target, got {:?}", res);
        let res = res.unwrap();
        assert_eq!(res.target_spec(), "127.0.0.1:8082");

        assert!(
            !reconnect_called.load(Ordering::SeqCst),
            "Expected no reconnection attempt on terminated connection"
        );
    }

    #[fuchsia::test]
    async fn test_direct_connector_failed_connection_bypasses_cache() {
        let mut builder = test_env();
        let cache_dir = builder.isolate_root().join("cache");
        let env = builder
            .runtime_config("target.default", "test-target")
            .runtime_config("target.discovery_cache_dir", cache_dir.to_str().unwrap())
            .build()
            .unwrap();

        // 1. Create a mocked resolution that fails to connect
        let resolution = Resolution::mock(|| Err(anyhow::anyhow!("connect failed").into()));

        // Call get_connection to trigger the failure
        let conn_res = resolution.get_connection(&env.context).await;
        assert!(conn_res.is_err());

        // 2. Put it in the connector
        let discovery = build_discovery(DiscoverySources::all(), &env.context);
        let resolver = DefaultTargetResolver::new(discovery);
        let connector = DirectConnector(Arc::new(DirectConnectorInner {
            context: env.context.clone(),
            resolution: futures::lock::Mutex::new(Some(Arc::new(resolution))),
            resolver: futures::lock::Mutex::new(resolver),
        }));

        // 3. Resolve. Since the connection failed, it should bypass the cache.
        // Bypassing the cache will try to resolve "test-target" via discovery,
        // which fails in the test environment, so we expect an Err.
        // If it did NOT bypass the cache, it would return Ok(cached_resolution).
        let res = connector.resolution().await;
        assert!(
            res.is_err(),
            "Expected resolution to fail (cache bypassed), but got Ok: {:?}",
            res
        );
    }

    #[fuchsia::test]
    async fn test_direct_connector_reconnect_re_resolves_to_new_address() {
        use std::str::FromStr;
        let mut builder = test_env();
        let cache_dir = builder.isolate_root().join("cache");
        let env = builder
            .runtime_config("target.default", "test-target")
            .runtime_config("target.discovery_cache_dir", cache_dir.to_str().unwrap())
            // Set "ssh.priv" to an invalid path to force SshConnector to fail instantly with a
            // Fatal error. This simulates a connection failure without getting stuck in the
            // infinite NonFatal retry loop (e.g. on connection refused/timeout), which would
            // hang the test.
            .runtime_config("ssh.priv", "/invalid/nonexistent/ssh/path")
            .build()
            .unwrap();

        // Define two target handles representing the device before and after reboot (with different ports)
        let addr1 = addr::TargetAddr::from_str("127.0.0.1:8082").unwrap();
        let handle1 = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Product { addrs: vec![addr1], serial: None },
            manual: false,
        };

        let addr2 = addr::TargetAddr::from_str("127.0.0.1:8083").unwrap();
        let handle2 = TargetHandle {
            node_name: Some("test-target".to_string()),
            state: TargetState::Product { addrs: vec![addr2], serial: None },
            manual: false,
        };

        // 1. Target appears at Address 1
        let (tx1, rx1) = futures::channel::mpsc::unbounded::<TargetEvent>();
        tx1.unbounded_send(TargetEvent::Added(handle1.clone())).unwrap();
        drop(tx1); // Close the stream so discovery terminates immediately
        let discovery1 = DiscoveryBuilder::default().build_with_stream(&env.context, rx1);
        let resolver1 = DefaultTargetResolver::new(discovery1);

        let connector = DirectConnector(Arc::new(DirectConnectorInner {
            context: env.context.clone(),
            resolution: futures::lock::Mutex::new(None),
            resolver: futures::lock::Mutex::new(resolver1),
        }));

        // Resolve first time - should find Address 1
        let res1 = connector.resolution().await;
        assert!(res1.is_ok());
        let res1 = res1.unwrap();
        assert_eq!(res1.target_spec(), "127.0.0.1:8082");

        // 2. Mock a successful connection
        #[derive(Debug)]
        struct PlaceholderConnector;
        impl ffx_target::TargetConnector for PlaceholderConnector {
            const CONNECTION_TYPE: &'static str = "placeholder";
            async fn connect(
                &mut self,
            ) -> Result<ffx_target::TargetConnection, ffx_target::TargetConnectionError>
            {
                Ok(ffx_target::TargetConnection::FDomain(ffx_target::FDomainConnection::invalid()))
            }
        }
        let conn = ffx_target::Connection::new(PlaceholderConnector).await.unwrap();
        res1.set_connection_for_test(Some(conn)).await;

        // Verify we can get the connection
        assert!(res1.get_connection(&env.context).await.is_ok());

        // 3. Simulate device going away (connection lost / rebooted)
        // Clear the cached connection
        res1.set_connection_for_test(None).await;

        // Attempt connection, which fails immediately because ssh.priv is invalid
        let conn_res = res1.get_connection(&env.context).await;
        assert!(conn_res.is_err());

        // 4. Target reappears at Address 2
        let (tx2, rx2) = futures::channel::mpsc::unbounded::<TargetEvent>();
        tx2.unbounded_send(TargetEvent::Added(handle2)).unwrap();
        drop(tx2); // Close the stream so discovery terminates immediately
        let discovery2 = DiscoveryBuilder::default().build_with_stream(&env.context, rx2);
        let resolver2 = DefaultTargetResolver::new(discovery2);
        *connector.0.resolver.lock().await = resolver2;

        // Call resolution again - since the connection failed, it should re-resolve
        // and find the new Address 2
        let res2 = connector.resolution().await;
        assert!(res2.is_ok());
        let res2 = res2.unwrap();
        assert_eq!(res2.target_spec(), "127.0.0.1:8083");
    }

    // Tests that `FhoTargetEnvironment` can initialize a `DirectConnector` connection behavior
    // with an explicit indefinite discovery timeout (`timeout: None`), and that `set_discovery_timeout`
    // can be called on the resulting `DirectConnector` to update its resolver's discovery timeout.
    #[fuchsia::test]
    async fn test_init_direct_connection_behavior_with_indefinite_timeout() {
        let env = test_env().build().unwrap();
        let target_env = FhoTargetEnvironment::default();
        let behavior =
            target_env.0.inner.init_direct_connection_behavior_indef(&env.context).await.unwrap();
        match &*behavior {
            ConnectionBehavior::Direct(dc) => {
                assert_eq!(dc.discovery_timeout().await, None);
                dc.set_discovery_timeout(Some(Duration::from_millis(500))).await;
                assert_eq!(dc.discovery_timeout().await, Some(Duration::from_millis(500)));
            }
        }
    }
}
