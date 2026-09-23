// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy as _};
use fdomain_fuchsia_pkg::RepositoryManagerMarker;
use fdomain_fuchsia_pkg_rewrite::{EngineMarker, EngineProxy};
use fdomain_fuchsia_sys2::OpenDirType;
use ffx_config::EnvironmentContext;
use ffx_writer::VerifiedMachineWriter;
use fho::{Deferred, Result, bug, return_bug, return_user_error};
use fidl_fuchsia_pkg_ext::RepositoryRegistrationAliasConflictMode;
use fuchsia_async::{Task, Timer};
use std::io::{Error, ErrorKind};
use std::net::Ipv6Addr;
use std::process;
use std::time::Duration;
use target_connector::Connector;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder};
use timeout::timeout;
use zx_status::Status;

const REPOSITORY_MANAGER_MONIKER: &str = "/core/pkg-resolver";

struct LogWriter {
    prefix: String,
}

impl LogWriter {
    pub fn new(prefix: &str) -> Self {
        Self { prefix: prefix.to_string() }
    }
}

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let strings = std::str::from_utf8(buf).map_err(|e| {
            Error::new(ErrorKind::InvalidData, format!("Could not convert to UTF8: {e}"))
        })?;
        for s in strings.lines() {
            log::info!("{}{}", self.prefix, s);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) struct PackageServerTask {
    pub(crate) repo_name: String,
    pub(crate) repo_url_rx: futures::channel::mpsc::UnboundedReceiver<String>,
    pub(crate) task: Task<Result<()>>,
}

pub(crate) async fn package_server_task(
    target_spec: Deferred<TargetInfoQueryHolder>,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    host_address: Deferred<HostAddrHolder>,
    context: EnvironmentContext,
    product_bundle: camino::Utf8PathBuf,
    repo_port: u16,
    should_register_repo: bool,
) -> Result<PackageServerTask> {
    log::info!("starting package server for {product_bundle:?}");

    // Make the name mostly unique, that way it is easier to remove this update source.
    let repo_name_prefix = "pb-update-source-";
    let repo_name = format!("{repo_name_prefix}{}", process::id());

    let cmd = ffx_repository_server_start_args::StartCommand {
        // Start a server on the given port.
        address: Some((Ipv6Addr::UNSPECIFIED, repo_port).into()),
        foreground: true,
        // Give it a name. This is actually a prefix of the name when running a product bundle.
        repository: Some(repo_name.clone()),

        product_bundle: Some(product_bundle),

        // Replace all other alias rules so the update uses this server.
        alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,

        // These are defaults, nothing special needed.
        background: false,
        disconnected: false,
        trusted_root: None,
        repo_path: None,
        alias: vec![],
        storage_type: None,
        port_path: None,
        no_device: !should_register_repo,
        refresh_metadata: false,
        auto_publish: None,
        tunnel_addr: None,
    };

    // Check that there is not an update source that has the same exact name (which includes the process ID).
    if should_register_repo
        && is_server_registered(&repo_name, rcs_proxy_connector.clone(), Duration::from_secs(60))
            .await?
    {
        return_user_error!(
            "Product bundle repository server name collision detected (unlikely host PID reuse suspected). \
         Please deregister the repository with `ffx target repository deregister -r {repo_name}` and stop the server with `ffx repository server stop {repo_name}` if it is still running."
        )
    }

    let (repo_url_tx, repo_url_rx) = futures::channel::mpsc::unbounded();

    let task = fuchsia_async::Task::local(async move {
        let stdout = LogWriter::new("repo_server stdout");
        let stderr = LogWriter::new("repo_server stderr");

        let server_writer = VerifiedMachineWriter::new_buffers(None, stdout, stderr);

        let server_result = Box::pin(ffx_repository_server_start::server::run_foreground_server(
            cmd,
            context,
            target_spec,
            rcs_proxy_connector,
            host_address,
            server_writer,
            pkg::ServerMode::Foreground,
            Some(repo_url_tx),
        ))
        .await;

        log::info!("product bundle server exited: {server_result:?}");
        server_result.map_err(Into::into)
    });

    Ok(PackageServerTask { repo_name, repo_url_rx, task })
}

pub(crate) async fn wait_for_device_task(
    repo_name: &str,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
) -> Result<()> {
    // Once the server task is running, wait until the registration appears on the device.
    let registered = timeout::<_, fho::Result<()>>(Duration::from_secs(30), async {
        loop {
            fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
            if is_server_registered(repo_name, rcs_proxy_connector.clone(), Duration::from_secs(30))
                .await?
            {
                return Ok(());
            }
        }
    })
    .await
    .map_err(|e| bug!("waiting for server registration on device: {e:?}"))?;

    if let Err(e) = registered {
        return_user_error!("Product bundle server was not registered on the device: {e}")
    }
    Ok(())
}

/// unregisters all servers that have the given prefix.
/// This uses the Connector for the rcs_proxy to potentially
/// reconnect to the device since post-update the device may
/// be rebooting.
pub(crate) async fn unregister_pb_repo_server(
    repo_name_prefix: &str,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
) -> Result<()> {
    let mut retry = true;

    let mut repo_manager_proxy: fdomain_fuchsia_pkg::RepositoryManagerProxy =
        match connect_to_capability::<RepositoryManagerMarker>(
            rcs_proxy_connector.clone(),
            Duration::from_secs(500),
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(err) => {
                log::info!("repo manager proxy closed, retrying");
                if retry {
                    fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
                    connect_to_capability::<RepositoryManagerMarker>(
                        rcs_proxy_connector.clone(),
                        Duration::from_secs(500),
                    )
                    .await?
                } else {
                    return_bug!("Could not list servers on device: {err}")
                }
            }
        };

    let mut names: Vec<String> = vec![];

    retry = true;

    loop {
        let (repo_iterator, repo_iterator_server): (
            fdomain_fuchsia_pkg::RepositoryIteratorProxy,
            _,
        ) = repo_manager_proxy.domain().create_proxy();
        match repo_manager_proxy.list(repo_iterator_server) {
            Ok(_) => {
                loop {
                    let repos = repo_iterator.next().await.map_err(|e| bug!(e))?;
                    if repos.is_empty() {
                        break;
                    }
                    names.extend(repos.iter().filter_map(|r| {
                        if let Some(repo_url) = &r.repo_url {
                            if repo_url.starts_with(&format!("fuchsia-pkg://{repo_name_prefix}")) {
                                Some(repo_url.to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }));
                }
                break;
            }
            Err(err) => {
                if err.is_closed() {
                    log::info!("repo manager proxy closed, retrying");
                    if retry {
                        retry = false;
                        repo_manager_proxy = connect_to_capability::<RepositoryManagerMarker>(
                            rcs_proxy_connector.clone(),
                            Duration::from_secs(500),
                        )
                        .await?;
                    } else {
                        return_bug!("Could not list servers on device: {err}")
                    }
                }
            }
        };
    }
    for name in names {
        deregister_standalone(&name, rcs_proxy_connector.clone(), Duration::from_secs(500)).await?
    }
    Ok(())
}

/// Inspects the on-device registrations and looks for a
/// server that has the given prefix.
async fn is_server_registered(
    repo_name: &str,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    time_to_wait: Duration,
) -> Result<bool> {
    let repo_manager_proxy: fdomain_fuchsia_pkg::RepositoryManagerProxy =
        connect_to_capability::<RepositoryManagerMarker>(rcs_proxy_connector, time_to_wait).await?;

    let (repo_iterator, repo_iterator_server): (fdomain_fuchsia_pkg::RepositoryIteratorProxy, _) =
        repo_manager_proxy.domain().create_proxy();
    repo_manager_proxy.list(repo_iterator_server).map_err(|e| bug!(e))?;
    loop {
        let repos = repo_iterator.next().await.map_err(|e| bug!(e))?;
        if repos.is_empty() {
            break;
        }
        if repos.iter().any(|r| {
            if let Some(repo_url) = &r.repo_url {
                repo_url.starts_with(&format!("fuchsia-pkg://{repo_name}"))
            } else {
                false
            }
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn deregister_standalone(
    repo_name: &str,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    time_to_wait: Duration,
) -> Result<()> {
    let repo_url = if repo_name.starts_with("fuchsia-pkg://") {
        repo_name
    } else {
        &format!("fuchsia-pkg://{repo_name}")
    };
    log::info!("Removing server {repo_url}");

    let repo_proxy: fdomain_fuchsia_pkg::RepositoryManagerProxy =
        match connect_to_capability::<RepositoryManagerMarker>(
            rcs_proxy_connector.clone(),
            time_to_wait,
        )
        .await
        {
            Ok(proxy) => proxy,
            Err(e) => {
                log::warn!("Got error getting repo_manager_proxy: {e}, retrying");
                Timer::new(Duration::from_secs(1)).await;
                connect_to_capability::<RepositoryManagerMarker>(
                    rcs_proxy_connector.clone(),
                    time_to_wait,
                )
                .await?
            }
        };

    match repo_proxy.remove(repo_url).await {
        Ok(Ok(())) => (),
        Ok(Err(err)) => {
            let status = Status::err_from_raw(err);
            if status != Status::NOT_FOUND {
                let message = format!(
                    "failed to remove registration for {repo_url}: {:#?}",
                    Status::err_from_raw(err)
                );
                log::error!("{message}");
                return_bug!("{message}");
            } else {
                log::info!("registration for {repo_url} was not found. Ignoring.")
            }
        }
        Err(err) => {
            let message =
                format!("failed to remove registrtation  due to communication error: {:#?}", err);
            log::error!("{message}");
            return_bug!("{message}");
        }
    };
    // Remove any alias rules.
    let rewrite_proxy = match connect_to_capability::<EngineMarker>(
        rcs_proxy_connector.clone(),
        time_to_wait,
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(e) => {
            log::warn!("Got error getting rewrite_proxy: {e}, retrying");
            Timer::new(Duration::from_secs(1)).await;
            connect_to_capability::<EngineMarker>(rcs_proxy_connector.clone(), time_to_wait).await?
        }
    };

    remove_aliases(repo_name, rewrite_proxy).await
}

use fdomain_fuchsia_pkg_rewrite_ext::{EditTransaction, Rule, do_transaction};
async fn remove_aliases(repo_url: &str, rewrite_proxy: EngineProxy) -> Result<()> {
    log::info!("Removing aliases for {repo_url}");
    // Check flag here for "overwrite" style
    do_transaction(&rewrite_proxy, |transaction: EditTransaction| async {
        // Prepend the alias rules to the front so they take priority.
        let mut rules: Vec<Rule> = vec![];

        // These are rules to re-evaluate...
        let repo_rules_state = transaction.list_dynamic().await?;
        rules.extend(repo_rules_state);

        // Clear the list, since we'll be adding it back later.
        transaction.reset_all()?;

        // Keep rules that do not match the repo being removed.
        rules.retain(|r: &Rule| r.host_replacement() != repo_url);

        // Add the rules back into the transaction. We do it in reverse, because `.add()`
        // always inserts rules into the front of the list.
        for rule in rules.into_iter().rev() {
            transaction.add(rule).await?
        }

        Ok(transaction)
    })
    .await
    .map_err(|err| {
        log::warn!("failed to create transactions: {:#?}", err);
        bug!("Failed to create transaction for aliases: {err}")
    })?;
    Ok(())
}

// Returns a boxed future to avoid `clippy::large_futures` at call sites.
fn connect_to_capability<T: DiscoverableProtocolMarker>(
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    time_to_wait: Duration,
) -> futures::future::LocalBoxFuture<'static, Result<T::Proxy>> {
    Box::pin(async move {
        let rcs_proxy = try_rcs_proxy_connection(rcs_proxy_connector, time_to_wait).await?;
        // Try to connect via fuchsia.developer.remotecontrol/RemoteControl.ConnectCapability.
        let (proxy, server) = rcs_proxy.domain().create_proxy::<T>();
        rcs_proxy
            .connect_capability(
                &REPOSITORY_MANAGER_MONIKER,
                OpenDirType::ExposedDir,
                T::PROTOCOL_NAME,
                server.into_channel(),
            )
            .await
            .map_err(|e| bug!(e))?
            .map_err(|err| {
                bug!(
                    "Attempting to connect to moniker {REPOSITORY_MANAGER_MONIKER} failed with {err:?}",
                )
            })?;
        Ok(proxy)
    })
}

async fn try_rcs_proxy_connection(
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    time_to_wait: Duration,
) -> Result<RemoteControlProxyHolder> {
    let rcs_proxy = timeout(
        time_to_wait,
        rcs_proxy_connector.try_connect(|target, _err| {
            log::info!(
                "RCS proxy: Waiting for target '{}' to return",
                match target {
                    Some(s) => s,
                    _ => "None",
                }
            );
            Ok(())
        }),
    )
    .await;
    match rcs_proxy {
        Ok(r) => r,
        Err(e) => fho::return_user_error!("Timeout connecting to rcs: {}", e),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ffx_config::TestEnv;
    use ffx_target::TargetInfoQuery;
    use fho::{FhoEnvironment, TryFromEnv as _};
    use fidl::endpoints::DiscoverableProtocolMarker;
    use fidl_fuchsia_developer_remotecontrol::{ConnectCapabilityError, RemoteControlRequest};
    use fidl_fuchsia_pkg::{
        RepositoryConfig, RepositoryIteratorRequest, RepositoryManagerMarker,
        RepositoryManagerRequest, RepositoryManagerRequestStream,
    };
    use fidl_fuchsia_pkg_rewrite::{
        EditTransactionRequest, EngineMarker, EngineRequest, EngineRequestStream,
        RuleIteratorRequest,
    };
    use futures::channel::mpsc;
    use futures::{SinkExt as _, StreamExt as _, TryStreamExt as _};
    use std::assert_matches;
    use std::sync::{Arc, Mutex};
    use target_behavior::ConnectionBehavior;
    use target_holders::{HostAddrHolder, RemoteControlProxyHolder};

    fn setup_fake_client() -> Arc<fdomain_client::Client> {
        fdomain_local::local_client(move || {
            let (client_end, mut stream) =
                fidl::endpoints::create_request_stream::<fidl_fuchsia_io::DirectoryMarker>();
            fuchsia_async::Task::local(async move {
                use futures::StreamExt;
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        fidl_fuchsia_io::DirectoryRequest::Open { path, object, .. } => {
                            if path == fidl_fuchsia_developer_remotecontrol::RemoteControlMarker::PROTOCOL_NAME {
                                let mut rcs_stream = fidl::endpoints::ServerEnd::<
                                    fidl_fuchsia_developer_remotecontrol::RemoteControlMarker,
                                >::new(object)
                                .into_stream();
                                fuchsia_async::Task::local(async move {
                                    use futures::TryStreamExt;
                                    while let Ok(Some(req)) = rcs_stream.try_next().await {
                                        handle_rcs_proxy_request(req);
                                    }
                                }).detach();
                            }
                        }
                        _ => {}
                    }
                }
            }).detach();
            Ok(client_end)
        })
    }

    pub(crate) struct FakeTestEnv {
        pub context: EnvironmentContext,
        pub rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
        pub host_address: Deferred<HostAddrHolder>,
        pub target_spec: Deferred<TargetInfoQueryHolder>,
        pub _client: Arc<fdomain_client::Client>,
    }

    impl FakeTestEnv {
        pub(crate) async fn new(test_env: &TestEnv) -> Self {
            let fdomain_client = setup_fake_client();
            let fho_env = FhoEnvironment::new_with_args(&test_env.context, &["some", "test"]);
            let target_env = target_behavior::target_interface(&fho_env);
            let behavior =
                ConnectionBehavior::fake_with_fdomain_client(fdomain_client.clone()).await;
            target_env.set_behavior_for_test(behavior);

            let rcs_proxy_connector =
                Connector::try_from_env(&fho_env).await.expect("Could not make RCS test connector");
            let host_address =
                Deferred::from_output(Ok(HostAddrHolder::from("127.0.0.1".to_string())));
            let target_spec = Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                TargetInfoQuery::try_from("1.1.1.1".to_string()).unwrap(),
            )));
            Self {
                context: test_env.context.clone(),
                rcs_proxy_connector,
                host_address,
                target_spec,
                _client: fdomain_client,
            }
        }
    }

    #[derive(Clone)]
    struct FakeRepositoryManager {
        sender: mpsc::Sender<()>,
    }

    impl FakeRepositoryManager {
        fn new() -> (Self, mpsc::Receiver<()>) {
            let (sender, rx) = futures::channel::mpsc::channel::<()>(1);

            (Self { sender }, rx)
        }

        fn spawn(&self, mut stream: RepositoryManagerRequestStream) {
            let sender = self.sender.clone();

            Task::local(async move {
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        RepositoryManagerRequest::Add { responder, .. } => {
                            let mut sender = sender.clone();

                            Task::local(async move {
                                responder.send(Ok(())).unwrap();
                                let _send = sender.send(()).await.unwrap();
                            })
                            .detach();
                        }
                        RepositoryManagerRequest::Remove { responder, .. } => {
                            responder.send(Ok(())).unwrap();
                        }
                        RepositoryManagerRequest::List {
                            iterator,
                            control_handle: _control_handle,
                        } => {
                            let mut stream = iterator.into_stream();
                            let mut sent = false;
                            while let Some(RepositoryIteratorRequest::Next { responder }) =
                                stream.try_next().await.expect("next try_next")
                            {
                                if !sent {
                                    responder
                                        .send(&[RepositoryConfig {
                                            repo_url: Some(
                                                "fuchsia-pkg://registered_test_repo".into(),
                                            ),
                                            ..Default::default()
                                        }])
                                        .expect("next send");
                                    sent = true;
                                } else {
                                    responder.send(&[]).expect("next send");
                                }
                            }
                        }
                        _ => panic!("unexpected request: {:?}", req),
                    }
                }
            })
            .detach();
        }
    }

    #[derive(Clone)]
    struct FakeEngine {
        sender: mpsc::Sender<()>,
    }

    impl FakeEngine {
        fn new() -> (Self, mpsc::Receiver<()>) {
            let (sender, rx) = futures::channel::mpsc::channel::<()>(1);
            (Self { sender }, rx)
        }

        fn spawn(&self, mut stream: EngineRequestStream) {
            let rules: Arc<Mutex<Vec<Rule>>> = Arc::new(Mutex::new(Vec::<Rule>::new()));
            let sender = self.sender.clone();

            Task::local(async move {
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        EngineRequest::StartEditTransaction { transaction, control_handle: _ } => {
                            let mut sender = sender.clone();
                            let rules = Arc::clone(&rules);

                            Task::local(async move {
                                let mut stream = transaction.into_stream();
                                while let Some(request) = stream.next().await {
                                    let request = request.unwrap();
                                    match request {
                                        EditTransactionRequest::ResetAll { control_handle: _ } => {}
                                        EditTransactionRequest::ListDynamic {
                                            iterator,
                                            control_handle: _,
                                        } => {
                                            let mut stream = iterator.into_stream();

                                            let mut rules =
                                                rules.lock().unwrap().clone().into_iter();

                                            while let Some(req) = stream.try_next().await.unwrap() {
                                                let RuleIteratorRequest::Next { responder } = req;

                                                if let Some(rule) = rules.next() {
                                                    responder.send(&[rule.into()]).unwrap();
                                                } else {
                                                    responder.send(&[]).unwrap();
                                                }
                                            }
                                        }
                                        EditTransactionRequest::Add { rule: _, responder } => {
                                            responder.send(Ok(())).unwrap()
                                        }
                                        EditTransactionRequest::Commit { responder } => {
                                            let res = responder.send(Ok(())).unwrap();
                                            let _send = sender.send(()).await.unwrap();
                                            res
                                        }
                                    }
                                }
                            })
                            .detach();
                        }
                        _ => panic!("unexpected request: {:?}", req),
                    }
                }
            })
            .detach();
        }
    }

    fn handle_rcs_proxy_request(req: RemoteControlRequest) {
        let (repo_manager, _) = FakeRepositoryManager::new();
        let (engine, _) = FakeEngine::new();
        match req {
            RemoteControlRequest::ConnectCapability {
                capability_name,
                server_channel,
                responder,
                ..
            } => {
                let capability_name =
                    capability_name.strip_prefix("svc/").unwrap_or(capability_name.as_str());
                match capability_name {
                    RepositoryManagerMarker::PROTOCOL_NAME => {
                        repo_manager.spawn(
                            fidl::endpoints::ServerEnd::<RepositoryManagerMarker>::new(
                                server_channel,
                            )
                            .into_stream(),
                        );
                        responder.send(Ok(())).expect("Could not send response")
                    }
                    EngineMarker::PROTOCOL_NAME => {
                        engine.spawn(
                            fidl::endpoints::ServerEnd::<EngineMarker>::new(server_channel)
                                .into_stream(),
                        );
                        responder.send(Ok(())).expect("Could not send response")
                    }
                    "fuchsia.posix.socket.Provider" => {
                        responder.send(Ok(())).unwrap();
                    }
                    _ => {
                        responder.send(Err(ConnectCapabilityError::NoMatchingCapabilities)).unwrap()
                    }
                }
            }
            _ => panic!("Unexpected request: {:?}", req),
        }
    }

    #[fuchsia::test]
    async fn test_package_server_task() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        package_server_task(
            fake_env.target_spec,
            fake_env.rcs_proxy_connector,
            fake_env.host_address,
            fake_env.context,
            "/path/to/product_bundle".into(),
            0,
            true,
        )
        .await
        .unwrap();
    }

    #[fuchsia::test]
    async fn test_package_server_task_packageless() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        package_server_task(
            fake_env.target_spec,
            fake_env.rcs_proxy_connector,
            fake_env.host_address,
            fake_env.context,
            "/path/to/product_bundle".into(),
            0,
            false,
        )
        .await
        .unwrap();
    }
    #[fuchsia::test]
    async fn test_wait_for_device_task() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        let repo_name = "registered_test_repo";

        wait_for_device_task(repo_name, fake_env.rcs_proxy_connector).await.unwrap();
    }
    #[fuchsia::test]
    async fn test_unregister_pb_repo_server() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        unregister_pb_repo_server("repo_name_prefix", fake_env.rcs_proxy_connector).await.unwrap();
    }
    #[fuchsia::test]
    async fn test_is_server_registered() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        let repo_name = "registered_test_repo";
        let time_to_wait = Duration::from_secs(5);

        assert_matches!(
            is_server_registered(repo_name, fake_env.rcs_proxy_connector.clone(), time_to_wait)
                .await,
            Ok(true)
        );

        assert_matches!(
            is_server_registered("unregistered_repo", fake_env.rcs_proxy_connector, time_to_wait)
                .await,
            Ok(false)
        );
    }

    #[fuchsia::test]
    async fn test_deregister_standalone() {
        let test_env = ffx_config::test_init().expect("test env");
        let fake_env = FakeTestEnv::new(&test_env).await;

        deregister_standalone("repo_name", fake_env.rcs_proxy_connector, Duration::from_secs(30))
            .await
            .unwrap();
    }
}
