// Copyright 2023 The Frights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::target;
use anyhow::{Context, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use errors::FfxError;
use ffx_command_error::Result;
use ffx_config::EnvironmentContext;
use ffx_config::environment::EnvironmentKind;
use ffx_repository_server_start_args::{StartCommand, default_address, default_tunnel_addr};
use ffx_ssh::parse::HostAddr;
use ffx_target::RcsKnockerImpl;
use fho::{Deferred, FfxError};
use fuchsia_async as fasync;
use fuchsia_repo::manager::RepositoryManager;
use fuchsia_repo::repo_client::RepoClient;
use fuchsia_repo::repository::{PmRepository, RepoProvider};
use fuchsia_repo::server::RepositoryServer;
use futures::channel::mpsc;
use futures::executor::block_on;
use futures::{SinkExt, StreamExt};
use package_tool::{RepoPublishCommand, cmd_repo_publish};
use pkg::config::DEFAULT_REPO_NAME;
use pkg::{
    PkgServerInfo, PkgServerInstanceInfo as _, PkgServerInstances, ServerMode, write_instance_info,
};
use thiserror::Error;

use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::fs;
use std::sync::Arc;
use target_connector::Connector;
use target_errors::FfxTargetError;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder};
use tuf::metadata::RawSignedMetadata;

const REPO_CONNECT_TIMEOUT_CONFIG: &str = "repository.connect_timeout_secs";
const DEFAULT_CONNECTION_TIMEOUT_SECS: u64 = 120;
const REPO_PATH_RELATIVE_TO_BUILD_DIR: &str = "amber-files";
const CONFIG_KEY_DEFAULT_REPOSITORY: &str = "repository.default";

fn start_signal_monitoring(
    mut conn_quit_tx: futures::channel::mpsc::Sender<()>,
    mut server_quit_tx: futures::channel::mpsc::Sender<()>,
) {
    log::debug!("Starting monitoring for SIGHUP, SIGINT, SIGTERM");
    let mut signals = Signals::new(&[SIGHUP, SIGINT, SIGTERM]).unwrap();
    // Can't use async here, as signals.forever() is blocking.
    std::thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            match signal {
                SIGINT | SIGHUP | SIGTERM => {
                    log::info!("Received signal {signal}, quitting");
                    let _ = block_on(conn_quit_tx.send(())).ok();
                    let _ = block_on(server_quit_tx.send(())).ok();
                }
                _ => unreachable!(),
            }
        }
    });
}

// Constructs a repo client with an explicitly passed trusted
// root, or defaults to the trusted root of the repository if
// none is provided.
async fn repo_client_from_optional_trusted_root(
    trusted_root: Option<Utf8PathBuf>,
    repository: impl RepoProvider + 'static,
) -> Result<RepoClient<Box<dyn RepoProvider>>, anyhow::Error> {
    let repo_client = if let Some(ref trusted_root_path) = trusted_root {
        let buf = async_fs::read(&trusted_root_path)
            .await
            .with_context(|| format!("reading trusted root {trusted_root_path}"))?;

        let trusted_root = RawSignedMetadata::new(buf);

        RepoClient::from_trusted_root(&trusted_root, Box::new(repository) as Box<_>)
            .await
            .with_context(|| {
                format!("Creating repo client using trusted root {trusted_root_path}")
            })?
    } else {
        RepoClient::from_trusted_remote(Box::new(repository) as Box<_>)
            .await
            .with_context(|| format!("Creating repo client using default trusted root"))?
    };
    Ok(repo_client)
}

/// Refreshes repository metadata, in the same way running a shell command
/// `ffx repository publish /path/to/repository` would
async fn refresh_repository_metadata(path: &Utf8PathBuf) -> std::result::Result<(), ServeError> {
    let rf = RepoPublishCommand {
        signing_keys: None,
        trusted_keys: None,
        trusted_root: None,
        package_manifests: vec![],
        package_list_manifests: vec![],
        package_archives: vec![],
        product_bundle: vec![],
        time_versioning: false,
        metadata_current_time: chrono::Utc::now(),
        refresh_root: false,
        clean: false,
        depfile: None,
        copy_mode: fuchsia_repo::repository::CopyMode::Copy,
        delivery_blob_type: 1,
        watch: false,
        ignore_missing_packages: false,
        blob_manifest: None,
        blob_repo_dir: None,
        repo_path: path.clone(),
    };
    cmd_repo_publish(rf)
        .await
        .map_err(|e| ServeError::User(anyhow!("failed publishing to repo {}: {}", path, e)))
}

pub fn get_repo_base_name(
    cmd_line: &Option<String>,
    context: &EnvironmentContext,
) -> std::result::Result<String, ffx_config::api::ConfigError> {
    if let Some(repo_name) = cmd_line.as_ref() {
        return Ok(repo_name.to_string());
    } else if let Some(repo_name) =
        context.get::<Option<String>, _>(CONFIG_KEY_DEFAULT_REPOSITORY)?
    {
        return Ok(repo_name);
    }
    Ok(DEFAULT_REPO_NAME.to_string())
}

#[derive(Error, Debug)]
#[error(transparent)]
pub struct ServerValidationError {
    kind: Box<ServerValidationErrorKind>,
}

impl From<ServerValidationErrorKind> for ServerValidationError {
    fn from(kind: ServerValidationErrorKind) -> Self {
        Self { kind: Box::new(kind) }
    }
}

impl From<ServerValidationError> for ffx_command_error::Error {
    fn from(err: ServerValidationError) -> Self {
        (*err.kind).into()
    }
}

impl From<ffx_config::api::ConfigError> for ServerValidationError {
    fn from(err: ffx_config::api::ConfigError) -> Self {
        ServerValidationErrorKind::from(err).into()
    }
}

#[derive(FfxError, Error, Debug)]
pub enum ServerValidationErrorKind {
    #[user]
    #[error("{0}")]
    TargetAmbiguous(String),

    #[user]
    #[error("{0}")]
    TargetNotFound(String),

    #[unexpected]
    #[error("Target connection failed: {0}")]
    TargetConnectionFailed(#[source] anyhow::Error),

    #[unexpected]
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[user]
    #[error("Cannot specify both --repo-path and --product-bundle")]
    PathConflict,

    #[user]
    #[error("product bundle {0:?} does not exist")]
    ProductBundleDoesNotExist(Utf8PathBuf),

    #[user]
    #[error("product bundle {0:?} does not contain any repositories")]
    ProductBundleHasNoRepositories(Utf8PathBuf),

    #[user]
    #[error("repo-path {0:?} does not exist")]
    RepositoryDoesNotExist(Utf8PathBuf),

    #[user]
    #[error("package manifest {0:?} does not exist")]
    PackageManifestDoesNotExist(Utf8PathBuf),

    #[user]
    #[error("Either --repo-path or --product-bundle need to be specified")]
    MissingPaths,

    #[user]
    #[error(
        "Repository address conflict. \
            Cannot start a server named {repo_name} serving {repo_path:?}. \
            Repository server \"{duplicate_name}\" is already running on {addr} serving a different path: {duplicate_path}\n\
            Use `ffx repository server list` to list running servers"
    )]
    AddressConflict {
        repo_name: String,
        repo_path: Utf8PathBuf,
        duplicate_name: String,
        duplicate_path: String,
        addr: std::net::SocketAddr,
    },

    #[user]
    #[error(
        "Repository name conflict. \
            Cannot start a server named {repo_name} serving {repo_path:?}. \
            Repository server \"{duplicate_name}\" is already running on {addr} serving a different path: {duplicate_path}\n\
            Use `ffx repository server list` to list running servers"
    )]
    NameConflict {
        repo_name: String,
        repo_path: Utf8PathBuf,
        duplicate_name: String,
        duplicate_path: String,
        addr: std::net::SocketAddr,
    },

    #[unexpected]
    #[error("Unexpected error: {0}")]
    Unexpected(#[source] anyhow::Error),
}

#[derive(FfxError, Error, Debug)]
pub enum ServeError {
    #[transparent]
    #[error(transparent)]
    Validation(#[from] ServerValidationError),

    #[exit_with_code(1)]
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[user]
    #[error("{0}")]
    User(#[source] anyhow::Error),

    #[unexpected]
    #[error("Unexpected error: {0}")]
    Unexpected(#[source] anyhow::Error),
}

impl From<ffx_command_error::Error> for ServeError {
    fn from(e: ffx_command_error::Error) -> Self {
        match e {
            ffx_command_error::Error::User(err) => Self::User(err),
            ffx_command_error::Error::Unexpected(err) => Self::Unexpected(err),
            ffx_command_error::Error::Config(err) => Self::Unexpected(err),
            e => Self::Unexpected(anyhow!(e)),
        }
    }
}

impl From<anyhow::Error> for ServeError {
    fn from(e: anyhow::Error) -> Self {
        Self::Unexpected(e)
    }
}

pub async fn serve_impl_validate_args(
    cmd: &StartCommand,
    rcs_proxy_connector: &Connector<RemoteControlProxyHolder>,
    context: &EnvironmentContext,
) -> std::result::Result<Option<PkgServerInfo>, ServerValidationError> {
    // Check that there is a target device identified, it is OK if it is not online.
    if !cmd.no_device {
        let res = rcs_proxy_connector
            .try_connect(|target, err| {
                log::info!(
                    "Validating RCS proxy: Waiting for target '{target:?}' to return error: {err:?}"
                );
                if target.is_none() {
                    return Err(Into::<errors::FfxError>::into(FfxTargetError::OpenTargetError {
                        err: fidl_fuchsia_developer_ffx::OpenTargetError::TargetNotFound,
                        target: None,
                        targets: vec![],
                        target_source: None,
                    })
                    .into());
                } else {
                    Ok(())
                }
            })
            .await;
        if let Err(fho::Error::User(ref user_err)) = res {
            if let Some(FfxError::OpenTargetError { .. }) = user_err.downcast_ref::<FfxError>() {
                if let Some(FfxTargetError::OpenTargetError { err, .. }) =
                    user_err.source().and_then(|s| s.downcast_ref::<FfxTargetError>())
                {
                    if err == &fidl_fuchsia_developer_ffx::OpenTargetError::QueryAmbiguous {
                        return Err(ServerValidationErrorKind::TargetAmbiguous(
                            user_err.to_string(),
                        )
                        .into());
                    } else if err == &fidl_fuchsia_developer_ffx::OpenTargetError::TargetNotFound {
                        return Err(ServerValidationErrorKind::TargetNotFound(
                            user_err.to_string(),
                        )
                        .into());
                    }
                }
            }
        }
    }

    let repo_base_name = get_repo_base_name(&cmd.repository, context)?;
    // Validate the repo-path vs. product bundle.
    // Product bundles may contain multiple repositories, So return a list.
    let repo_name_paths: Vec<(String, Utf8PathBuf)> = match (
        cmd.repo_path.clone(),
        cmd.product_bundle.clone(),
    ) {
        (Some(_), Some(_)) => {
            return Err(ServerValidationErrorKind::PathConflict.into());
        }
        (None, Some(product_bundle)) => {
            if !product_bundle.exists() {
                return Err(
                    ServerValidationErrorKind::ProductBundleDoesNotExist(product_bundle).into()
                );
            }
            let repositories = product_bundle::get_repositories(product_bundle.clone())
                .with_context(|| {
                    format!("getting repositories from product bundle {product_bundle}")
                })
                .map_err(ServerValidationErrorKind::Unexpected)?;
            if repositories.is_empty() {
                return Err(ServerValidationErrorKind::ProductBundleHasNoRepositories(
                    product_bundle,
                )
                .into());
            }
            let mut pb_repo_name_paths = vec![];
            for r in repositories {
                if let Some(first_alias) = r.aliases().first() {
                    pb_repo_name_paths
                        .push((format!("{repo_base_name}.{first_alias}"), product_bundle.clone()));
                } else {
                    return Err(ServerValidationErrorKind::Unexpected(anyhow!(
                        "Invalid repository configuration in the product bundle {product_bundle}. No aliases defined for a repository"
                    )).into());
                }
            }
            pb_repo_name_paths
        }
        (repo_path, None) => {
            if let Some(path) = repo_path {
                if !path.exists() {
                    return Err(ServerValidationErrorKind::RepositoryDoesNotExist(path).into());
                }
                vec![(repo_base_name, path)]
            // TODO(b/359927881): Use the configuration to read repo-path
            // vs. constructing it from the build dir. This way it works with other EnvironmentKinds.
            } else if let EnvironmentKind::InTree { build_dir: Some(build_dir), .. } =
                context.env_kind()
            {
                let path = build_dir.join(REPO_PATH_RELATIVE_TO_BUILD_DIR);
                let path_utf8 = Utf8Path::from_path(&path)
                    .with_context(|| format!("converting repo path to UTF-8 {:?}", path))
                    .map_err(ServerValidationErrorKind::Unexpected)?
                    .to_path_buf();
                if !path_utf8.exists() {
                    return Err(ServerValidationErrorKind::RepositoryDoesNotExist(path_utf8).into());
                }
                vec![(repo_base_name, path_utf8)]
            } else {
                log::warn!("repo-path not found in env: {:?}", context.env_kind());
                return Err(ServerValidationErrorKind::MissingPaths.into());
            }
        }
    };

    if let Some(package_manifest) = &cmd.auto_publish {
        if !package_manifest.exists() {
            let msg = format!("package manifest {package_manifest:?} does not exist");
            log::error!("{msg}");
            return Err(ServerValidationErrorKind::PackageManifestDoesNotExist(
                package_manifest.clone(),
            )
            .into());
        }
    }

    // Compare against running instances.
    let instance_root =
        context.get("repository.process_dir").map_err(ServerValidationErrorKind::Config)?;
    let mgr = PkgServerInstances::new(instance_root);
    let running_instances = mgr
        .list_instances()
        .map_err(|e| ServerValidationErrorKind::Unexpected(anyhow::Error::from(e)))?;

    // Check all the name/path pairs for conflicts. If there is an exact match, return it as
    // an indicator that the server is already running and does not need to be started again.
    let mut already_running_instance: Option<PkgServerInfo> = None;
    let cmd_address = if let Some(addr) = &cmd.address { addr.clone() } else { default_address() };
    let repo_name_paths = repo_name_paths.into_iter();
    for (repo_name, repo_path) in repo_name_paths {
        let addr = cmd_address.clone();
        let duplicate = running_instances.iter().find(|instance| instance.address == addr);
        if let Some(duplicate) = duplicate {
            // if we're starting using a product bundle, the name will be different so compare the repo_path
            // which is the path to the product bundle
            if let Some(pb_path) = &cmd.product_bundle {
                if *pb_path != duplicate.repo_path_display() {
                    return Err(ServerValidationErrorKind::AddressConflict {
                        repo_name,
                        repo_path,
                        duplicate_name: duplicate.name.clone(),
                        duplicate_path: duplicate.repo_path_display(),
                        addr,
                    }
                    .into());
                }
            } else {
                if repo_name != duplicate.name {
                    return Err(ServerValidationErrorKind::AddressConflict {
                        repo_name,
                        repo_path,
                        duplicate_name: duplicate.name.clone(),
                        duplicate_path: duplicate.repo_path_display(),
                        addr,
                    }
                    .into());
                }
            }
            if already_running_instance.is_none() {
                already_running_instance = Some(duplicate.clone());
            }
        }
        let duplicate = running_instances.iter().find(|instance| instance.name == repo_name);
        if let Some(duplicate) = duplicate {
            if addr != duplicate.address {
                return Err(ServerValidationErrorKind::NameConflict {
                    repo_name,
                    repo_path,
                    duplicate_name: duplicate.name.clone(),
                    duplicate_path: duplicate.repo_path_display(),
                    addr: duplicate.address.clone(),
                }
                .into());
            }
            if already_running_instance.is_none() {
                already_running_instance = Some(duplicate.clone());
            }
        }
    }
    Ok(already_running_instance)
}

pub(crate) enum ServeStarted {
    Started { address: std::net::SocketAddr, repo_path: Utf8PathBuf },
    AlreadyRunning { name: String, address: std::net::SocketAddr, repo_path: String, pid: u32 },
}

pub async fn serve_impl(
    target_spec: Deferred<TargetInfoQueryHolder>,
    rcs_proxy: Connector<RemoteControlProxyHolder>,
    host_address: Deferred<HostAddrHolder>,
    cmd: StartCommand,
    context: EnvironmentContext,
    mode: ServerMode,
    tx: &mut mpsc::UnboundedSender<crate::target::ConnectEvent>,
    repo_url_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
) -> std::result::Result<ServeStarted, ServeError> {
    // Validate the cmd args before processing. This allows good error messages to be presented
    // to the user when running in Background mode. If the server is already running, this returns
    // Ok.
    if let Some(running) = serve_impl_validate_args(&cmd, &rcs_proxy, &context).await? {
        // The server that matches the cmd is already running.
        let repo_path = running.repo_path_display();
        return Ok(ServeStarted::AlreadyRunning {
            name: running.name,
            address: running.address,
            repo_path,
            pid: running.pid,
        });
    }

    let repo_base_name = get_repo_base_name(&cmd.repository, &context)?;

    let connect_timeout =
        context.get(REPO_CONNECT_TIMEOUT_CONFIG).unwrap_or(DEFAULT_CONNECTION_TIMEOUT_SECS);

    let connect_timeout = std::time::Duration::from_secs(connect_timeout);
    let repo_manager: Arc<RepositoryManager> = RepositoryManager::new();

    let repo_path = match (cmd.repo_path.clone(), cmd.product_bundle.clone()) {
        (Some(_), Some(_)) => {
            return Err(ServeError::User(anyhow!(
                "Cannot specify both --repo-path and --product-bundle"
            )));
        }
        (None, Some(product_bundle)) => {
            let repositories = product_bundle::get_repositories(product_bundle.clone())
                .with_context(|| {
                    format!("getting repositories from product bundle {product_bundle}")
                })?;
            for repository in repositories {
                let repo_name = format!(
                    "{repo_base_name}.{first_alias}",
                    first_alias = repository.aliases().first().unwrap()
                );

                let repo_client = RepoClient::from_trusted_remote(Box::new(repository) as Box<_>)
                    .await
                    .with_context(|| format!("Creating a repo client for {repo_name}"))?;
                repo_manager.add(&repo_name, repo_client);
            }

            if cmd.refresh_metadata {
                log::warn!("--refresh-metadata is not supported with product bundles, ignoring");
            }

            product_bundle
        }
        (repo_path, None) => {
            let repo_path = if let Some(repo_path) = repo_path {
                repo_path
            } else if let EnvironmentKind::InTree { build_dir: Some(build_dir), .. } =
                context.env_kind()
            {
                // TODO(b/359927881): Use the configuration to read repo-path
                // vs. constructing it from the build dir. This way it works with other EnvironmentKinds.
                let build_dir = Utf8Path::from_path(build_dir)
                    .with_context(|| format!("converting repo path to UTF-8 {:?}", repo_path))?;

                build_dir.join(REPO_PATH_RELATIVE_TO_BUILD_DIR)
            } else {
                return Err(ServeError::User(anyhow!(
                    "Either --repo-path or --product-bundle need to be specified"
                )));
            };

            // Create PmRepository and RepoClient
            let repo_path = repo_path
                .canonicalize_utf8()
                .with_context(|| format!("canonicalizing repo path {:?}", repo_path))?;
            let repository = PmRepository::new(repo_path.clone());

            let mut repo_client =
                repo_client_from_optional_trusted_root(cmd.trusted_root.clone(), repository)
                    .await?;

            let res = repo_client.update().await;
            if let Err(e) = &res {
                let is_expired = match e {
                    fuchsia_repo::repository::Error::Tuf(tuf::Error::ExpiredMetadata {
                        ..
                    }) => true,
                    _ => e.to_string().contains("expired") || e.to_string().contains("Expired"),
                };

                if is_expired {
                    eprintln!("Error: TUF metadata is expired.");
                    eprintln!(
                        "This usually happens if the Fuchsia tree hasn't been built or updated in a long time."
                    );
                    eprintln!("To fix this:");
                    eprintln!(
                        "  If you are using incremental publishing: run `fx serve -C` to clean the repository."
                    );
                    eprintln!(
                        "  Otherwise: run `fx cleanrepo` followed by `fx build` to recreate the repository."
                    );
                }
            }
            res.context("updating the repository metadata")?;

            repo_manager.add(&repo_base_name, repo_client);

            if cmd.refresh_metadata {
                refresh_repository_metadata(&repo_path).await?;
            }
            repo_path
        }
    };

    // Serve RepositoryManager over a RepositoryServer
    let server_addr = if let Some(addr) = cmd.address.clone() { addr } else { default_address() };
    let (server_fut, connection_sink, server) =
        RepositoryServer::builder(server_addr, Arc::clone(&repo_manager))
            .start()
            .await
            .with_context(|| format!("starting repository server"))?;

    // Write port file if needed
    if let Some(port_path) = cmd.port_path.clone() {
        let port = server.local_addr().port().to_string();

        fs::write(port_path, &port)
            .with_context(|| format!("creating port file for port {}", port))?;
    };

    let server_addr = server.local_addr();

    // Write out the instance data
    for (repo_name, repo_client) in repo_manager.repositories() {
        let repo_url = fuchsia_url::RepositoryUrl::parse_host(repo_name.clone())
            .map_err(|e| anyhow!("{e}"))?;
        let url = format!("http://{server_addr}/{repo_name}");
        let mirror_url = url
            .parse()
            .map_err(|e: http::uri::InvalidUri| anyhow!("{e}"))
            // Lower layers do not print the URL, so expose it here to clarify
            // what failed parsing, if something failed parsing.
            .with_context(|| format!("while parsing {url}"))?;
        let repo_config = repo_client
            .read()
            .await
            .get_config(repo_url, mirror_url, cmd.storage_type.clone())
            .map_err(|e| anyhow!("{e}"))?;
        let repo_spec = repo_client.read().await.spec();
        if let Err(e) = write_instance_info(
            &context,
            mode.clone(),
            &repo_name,
            &server_addr,
            repo_spec,
            cmd.storage_type
                .clone()
                .unwrap_or(fidl_fuchsia_pkg_ext::RepositoryStorageType::Ephemeral),
            cmd.alias_conflict_mode.clone(),
            repo_config,
        )
        .await
        {
            log::error!("failed to write repo server instance information for {repo_name}: {e:?}");
        }
    }

    let server_task = fasync::Task::local(server_fut);
    let (mut server_stop_tx, mut server_stop_rx) = futures::channel::mpsc::channel::<()>(1);
    let (loop_stop_tx, loop_stop_rx) = futures::channel::mpsc::channel::<()>(1);

    // Register signal handler and monitor for server requests.
    let _server_stop_task = fasync::Task::local(async move {
        if let Some(_) = server_stop_rx.next().await {
            server.stop();
        }
    });
    start_signal_monitoring(loop_stop_tx.clone(), server_stop_tx.clone());

    // If auto-publishing, start the task in the background.
    if let Some(package_manifest) = &cmd.auto_publish {
        let publish_cmd = RepoPublishCommand {
            signing_keys: None,
            trusted_keys: None,
            trusted_root: cmd.trusted_root.clone(),
            package_manifests: vec![],
            package_list_manifests: vec![package_manifest.clone()],
            package_archives: vec![],
            product_bundle: vec![],
            time_versioning: true,
            metadata_current_time: chrono::Utc::now(),
            refresh_root: false,
            clean: false,
            depfile: None,
            copy_mode: fuchsia_repo::repository::CopyMode::Copy,
            delivery_blob_type: 1,
            watch: true,
            ignore_missing_packages: true,
            blob_manifest: None,
            blob_repo_dir: None,
            repo_path: repo_path.clone(),
        };

        let auto_publisher = fasync::Task::local(async move {
            let publish_result = cmd_repo_publish(publish_cmd).await;
            log::warn!("Auto-publishing exited: {publish_result:?}");
        });

        auto_publisher.detach();
    }

    let result = if cmd.no_device && repo_url_tx.is_none() {
        if let Err(e) = tx
            .send(crate::target::ConnectEvent::StartServe {
                repo_path: repo_path.to_string(),
                addr: server_addr,
            })
            .await
        {
            log::warn!("Error sending start serve message: {e}");
        }
        Ok(ServeStarted::Started { address: server_addr, repo_path })
    } else {
        let tunnel_addr = cmd.tunnel_addr.clone().unwrap_or_else(|| default_tunnel_addr());
        let host_address: Option<HostAddr> = host_address.await?.into();
        let host_address = host_address.map(|t| t.0);
        let knocker = RcsKnockerImpl {
            ever_found: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            use_cache: false,
        };
        let target_spec = target_spec.await?;
        let r = Box::pin(target::main_connect_loop(
            &context,
            &cmd,
            &repo_path,
            server_addr,
            connect_timeout,
            repo_manager,
            loop_stop_rx,
            &target_spec,
            rcs_proxy,
            &knocker,
            tx,
            host_address,
            tunnel_addr,
            connection_sink,
            repo_url_tx,
        ))
        .await;
        if r.is_err() {
            let _ = server_stop_tx.send(()).await;
            Err(r.unwrap_err())
        } else {
            Ok(ServeStarted::Started { address: server_addr, repo_path })
        }
    };

    // Wait for the server to shut down.
    server_task.await;
    let _ = tx.close().await;
    result.map_err(Into::into)
}

///////////////////////////////////////////////////////////////////////////////
// tests
#[cfg(test)]
mod test {
    use super::*;
    use crate::{CommandStatus, ServerStartTool};
    use assert_matches::assert_matches;
    use discovery::query::TargetInfoQuery;
    use fdomain_client::fidl::DiscoverableProtocolMarker;
    use fdomain_fuchsia_developer_remotecontrol as frcs;
    use fdomain_fuchsia_io as fio;
    use fdomain_fuchsia_pkg::{
        MirrorConfig, RepositoryConfig, RepositoryManagerMarker, RepositoryManagerRequest,
        RepositoryManagerRequestStream,
    };
    use fdomain_fuchsia_pkg_rewrite::{
        EditTransactionRequest, EngineMarker, EngineRequest, EngineRequestStream,
        RuleIteratorRequest,
    };
    use fdomain_fuchsia_pkg_rewrite_ext::Rule;
    use fdomain_fuchsia_posix_socket as fsock;
    use ffx_command_error::bug;
    use ffx_config::TestEnv;
    use ffx_config::keys::TARGET_DEFAULT_KEY;
    use ffx_target_net_testutil::FakeNetstack;
    use ffx_writer::{Format, TestBuffers, VerifiedMachineWriter};
    use fho::{FfxMain, FhoEnvironment, TryFromEnv, user_error};
    use fidl_fuchsia_pkg_ext::{
        RepositoryConfigBuilder, RepositoryRegistrationAliasConflictMode, RepositoryStorageType,
    };
    use fuchsia_repo::repo_builder::RepoBuilder;
    use fuchsia_repo::repo_keys::RepoKeys;
    use fuchsia_repo::repository::HttpRepository;
    use fuchsia_repo::test_utils;
    use futures::TryStreamExt;
    use futures::channel::mpsc;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time;
    use target_behavior::{ConnectionBehavior, target_interface};
    use target_connector::Connector;
    use test_case::test_case;
    use timeout::timeout;
    use tuf::crypto::Ed25519PrivateKey;
    use tuf::metadata::Metadata;
    use url::Url;

    const REPO_NAME: &str = "some-repo";
    const REPO_UNSPECIFIED_IPV4_ADDR: [u8; 4] = [0, 0, 0, 0];
    const REPO_LOCALHOST_IPV4_ADDR: [u8; 4] = [127, 0, 0, 1];
    const LOCALHOST: &str = "127.0.0.1";
    const REPO_PORT: u16 = 0;
    const DEVICE_PORT: u16 = 5;
    const HOST_ADDR: &str = "1.2.3.4";
    const TARGET_NODENAME: &str = "some-target";

    macro_rules! rule {
        ($host_match:expr => $host_replacement:expr,
         $path_prefix_match:expr => $path_prefix_replacement:expr) => {
            Rule::new($host_match, $host_replacement, $path_prefix_match, $path_prefix_replacement)
                .unwrap()
        };
    }

    struct FakeRcs;

    impl FakeRcs {
        fn spawn(
            repo_manager: FakeRepositoryManager,
            engine: FakeEngine,
            netstack: Arc<FakeNetstack>,
            stream: frcs::RemoteControlRequestStream,
        ) {
            fasync::Task::spawn(async move {
                let mut stream = stream;
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        frcs::RemoteControlRequest::ConnectCapability {
                            moniker: _,
                            capability_set: _,
                            capability_name,
                            server_channel,
                            responder,
                        } => {
                            let capability_name = capability_name
                                .strip_prefix("svc/")
                                .unwrap_or(capability_name.as_str());
                            match capability_name {
                                RepositoryManagerMarker::PROTOCOL_NAME => repo_manager.spawn(
                                    fdomain_client::fidl::ServerEnd::<RepositoryManagerMarker>::new(
                                        server_channel,
                                    )
                                    .into_stream(),
                                ),
                                EngineMarker::PROTOCOL_NAME => engine.spawn(
                                    fdomain_client::fidl::ServerEnd::<EngineMarker>::new(
                                        server_channel,
                                    )
                                    .into_stream(),
                                ),
                                fsock::ProviderMarker::PROTOCOL_NAME => {
                                    netstack.connect_socket_provider(
                                        fdomain_client::fidl::ServerEnd::<
                                            fsock::ProviderMarker,
                                        >::new(server_channel),
                                    )
                                }
                                p => {
                                    unimplemented!("unimplemented protocol {p}");
                                }
                            }
                            responder.send(Ok(())).unwrap();
                        }
                        _ => panic!("unexpected request: {:?}", req),
                    }
                }
            })
            .detach();
        }
    }

    fn make_fake_directory(
        repo_manager: FakeRepositoryManager,
        engine: FakeEngine,
        netstack: Arc<FakeNetstack>,
        channel: fdomain_client::Channel,
    ) {
        fasync::Task::spawn(async move {
            let mut stream =
                fdomain_client::fidl::ServerEnd::<fio::DirectoryMarker>::new(channel).into_stream();
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fio::DirectoryRequest::Open { path, object, .. } => {
                        let path = path.strip_prefix("./").unwrap_or(&path);
                        let path = path.strip_prefix("svc/").unwrap_or(path);
                        if path == frcs::RemoteControlMarker::PROTOCOL_NAME {
                            let server_end = fdomain_client::fidl::ServerEnd::<
                                frcs::RemoteControlMarker,
                            >::new(object);
                            FakeRcs::spawn(
                                repo_manager.clone(),
                                engine.clone(),
                                netstack.clone(),
                                server_end.into_stream(),
                            );
                        }
                    }
                    _ => {}
                }
            }
        })
        .detach();
    }

    fn setup_fake_target(
        fake_repo: FakeRepositoryManager,
        fake_engine: FakeEngine,
    ) -> (Arc<fdomain_client::Client>, Arc<FakeNetstack>) {
        let netstack_holder = Arc::new(OnceLock::<Arc<FakeNetstack>>::new());
        let netstack_holder_clone = netstack_holder.clone();

        let fdomain_client = fdomain_local::local_client_fdomain(move |channel| {
            let fake_repo = fake_repo.clone();
            let fake_engine = fake_engine.clone();
            let fake_netstack = netstack_holder_clone.get().unwrap().clone();
            make_fake_directory(fake_repo, fake_engine, fake_netstack, channel);
        });

        let fake_netstack = Arc::new(FakeNetstack::new(fdomain_client.clone()));
        let _ = netstack_holder.set(fake_netstack.clone());
        (fdomain_client, fake_netstack)
    }

    #[derive(Debug, PartialEq)]
    enum RepositoryManagerEvent {
        Add { repo: RepositoryConfig },
    }

    #[derive(Clone)]
    struct FakeRepositoryManager {
        events: Arc<Mutex<Vec<RepositoryManagerEvent>>>,
        sender: mpsc::Sender<()>,
    }

    impl FakeRepositoryManager {
        fn new() -> (Self, mpsc::Receiver<()>) {
            let (sender, rx) = futures::channel::mpsc::channel::<()>(1);
            let events = Arc::new(Mutex::new(Vec::new()));

            (Self { events, sender }, rx)
        }

        fn spawn(&self, mut stream: RepositoryManagerRequestStream) {
            let sender = self.sender.clone();
            let events_closure = Arc::clone(&self.events);

            fasync::Task::spawn(async move {
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        RepositoryManagerRequest::Add { repo, responder } => {
                            let mut sender = sender.clone();
                            let events_closure = events_closure.clone();

                            fasync::Task::spawn(async move {
                                events_closure
                                    .lock()
                                    .unwrap()
                                    .push(RepositoryManagerEvent::Add { repo });
                                responder.send(Ok(())).unwrap();
                                let _send = sender.send(()).await.unwrap();
                            })
                            .detach();
                        }
                        _ => panic!("unexpected request: {:?}", req),
                    }
                }
            })
            .detach();
        }

        fn take_events(&self) -> Vec<RepositoryManagerEvent> {
            self.events.lock().unwrap().drain(..).collect::<Vec<_>>()
        }
    }

    #[derive(Debug, PartialEq)]
    enum RewriteEngineEvent {
        ResetAll,
        ListDynamic,
        IteratorNext,
        EditTransactionAdd { rule: Rule },
        EditTransactionCommit,
    }

    #[derive(Clone)]
    struct FakeEngine {
        events: Arc<Mutex<Vec<RewriteEngineEvent>>>,
        sender: mpsc::Sender<()>,
    }

    impl FakeEngine {
        fn new() -> (Self, mpsc::Receiver<()>) {
            let (sender, rx) = futures::channel::mpsc::channel::<()>(1);
            let events = Arc::new(Mutex::new(Vec::new()));

            (Self { events, sender }, rx)
        }

        fn spawn(&self, mut stream: EngineRequestStream) {
            let rules: Arc<Mutex<Vec<Rule>>> = Arc::new(Mutex::new(Vec::<Rule>::new()));
            let sender = self.sender.clone();
            let events_closure = Arc::clone(&self.events);

            fasync::Task::spawn(async move {
                while let Some(Ok(req)) = stream.next().await {
                    match req {
                        EngineRequest::StartEditTransaction { transaction, control_handle: _ } => {
                            let mut sender = sender.clone();
                            let rules = Arc::clone(&rules);
                            let events_closure = Arc::clone(&events_closure);

                            fasync::Task::spawn(async move {
                                let mut stream = transaction.into_stream();
                                while let Some(request) = stream.next().await {
                                    let request = request.unwrap();
                                    match request {
                                        EditTransactionRequest::ResetAll { control_handle: _ } => {
                                            events_closure
                                                .lock()
                                                .unwrap()
                                                .push(RewriteEngineEvent::ResetAll);
                                        }
                                        EditTransactionRequest::ListDynamic {
                                            iterator,
                                            control_handle: _,
                                        } => {
                                            events_closure
                                                .lock()
                                                .unwrap()
                                                .push(RewriteEngineEvent::ListDynamic);
                                            let mut stream = iterator.into_stream();

                                            let mut rules =
                                                rules.lock().unwrap().clone().into_iter();

                                            while let Some(req) = stream.try_next().await.unwrap() {
                                                let RuleIteratorRequest::Next { responder } = req;
                                                events_closure
                                                    .lock()
                                                    .unwrap()
                                                    .push(RewriteEngineEvent::IteratorNext);

                                                if let Some(rule) = rules.next() {
                                                    responder.send(&[rule.into()]).unwrap();
                                                } else {
                                                    responder.send(&[]).unwrap();
                                                }
                                            }
                                        }
                                        EditTransactionRequest::Add { rule, responder } => {
                                            events_closure.lock().unwrap().push(
                                                RewriteEngineEvent::EditTransactionAdd {
                                                    rule: rule.try_into().unwrap(),
                                                },
                                            );
                                            responder.send(Ok(())).unwrap()
                                        }
                                        EditTransactionRequest::Commit { responder } => {
                                            events_closure
                                                .lock()
                                                .unwrap()
                                                .push(RewriteEngineEvent::EditTransactionCommit);
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

        fn take_events(&self) -> Vec<RewriteEngineEvent> {
            self.events.lock().unwrap().drain(..).collect::<Vec<_>>()
        }
    }

    fn get_test_env_builder() -> ffx_config::environment::TestEnvBuilder {
        ffx_config::test_env()
            .runtime_config("connectivity.direct", false)
            .runtime_config("connectivity.enable_network", false)
    }

    fn make_direct_connector_behavior(client: Arc<fdomain_client::Client>) -> ConnectionBehavior {
        let resolution = ffx_target::Resolution::mock(move || {
            Ok(ffx_target::Connection::from_fdomain_client(client.clone()))
        });
        ConnectionBehavior::fake_direct_connector(resolution)
    }

    async fn make_fake_rcs_proxy_connector(
        test_env: &TestEnv,
    ) -> Connector<RemoteControlProxyHolder> {
        let (fake_repo, _) = FakeRepositoryManager::new();
        let (fake_engine, _content) = FakeEngine::new();
        let (fdomain_client, _) = setup_fake_target(fake_repo, fake_engine);

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        Connector::try_from_env(&env).await.expect("Could not make RCS test connector")
    }

    async fn make_no_target_connector(test_env: &TestEnv) -> Connector<RemoteControlProxyHolder> {
        let err = FfxTargetError::OpenTargetError {
            err: fidl_fuchsia_developer_ffx::OpenTargetError::TargetNotFound,
            target: None,
            targets: vec![],
            target_source: None,
        };
        let resolution = ffx_target::Resolution::mock(move || Err(anyhow::anyhow!(err.clone())));
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        Connector::try_from_env(&env).await.expect("Could not make RCS test connector")
    }

    async fn make_ambiguous_connector(test_env: &TestEnv) -> Connector<RemoteControlProxyHolder> {
        let err = FfxTargetError::OpenTargetError {
            err: fidl_fuchsia_developer_ffx::OpenTargetError::QueryAmbiguous,
            target: None,
            targets: vec!["foo".to_string(), "bar".to_string()],
            target_source: None,
        };
        let resolution = ffx_target::Resolution::mock(move || Err(anyhow::anyhow!(err.clone())));
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        Connector::try_from_env(&env).await.expect("Could not make RCS test connector")
    }

    #[fuchsia::test]
    async fn test_serve_impl_validate_args() {
        let env = get_test_env_builder().build().unwrap();

        let pb_path = env.isolate_root.path().join("some-pb");
        fs::create_dir_all(&pb_path).expect("pb temp dir");
        let bundle_path = Utf8PathBuf::from_path_buf(pb_path).expect("utf8 path");
        write_product_bundle(&bundle_path).await;

        let test_cases: Vec<(StartCommand, Result<Option<PkgServerInfo>>)> = vec![
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some("/some/repo/path".into()),
                    product_bundle: Some(bundle_path.clone()),
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("Cannot specify both --repo-path and --product-bundle")),
            ),
            (
                StartCommand {
                    repository: Some("repo-with-name".into()),
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: None,
                    tunnel_addr: None,
                    product_bundle: Some(bundle_path.clone()),
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Ok(None),
            ),
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: None,
                    product_bundle: Some("/missing/product/bundle".into()),
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("product bundle \"/missing/product/bundle\" does not exist")),
            ),
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some("/missing/repo/path".into()),
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("repo-path \"/missing/repo/path\" does not exist")),
            ),
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: None,
                    tunnel_addr: None,
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("Either --repo-path or --product-bundle need to be specified")),
            ),
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: None,
                    tunnel_addr: None,
                    product_bundle: Some(bundle_path.clone()),
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: Some(Utf8PathBuf::from("/missing/package-list")),
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("package manifest \"/missing/package-list\" does not exist")),
            ),
            (
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some(
                        Utf8PathBuf::from_path_buf(env.isolate_root.path().to_path_buf())
                            .expect("repo path"),
                    ),
                    tunnel_addr: None,
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: Some(Utf8PathBuf::from("/missing/package-list")),
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!("package manifest \"/missing/package-list\" does not exist")),
            ),
        ];

        let rcs_proxy_connector = make_fake_rcs_proxy_connector(&env).await;

        for (cmd, expected) in test_cases {
            let result = serve_impl_validate_args(&cmd, &rcs_proxy_connector, &env.context)
                .await
                .map_err(ffx_command_error::Error::from);
            match expected {
                Ok(Some(pkg_server_info)) => {
                    if let Some(actual_info) = result.ok().expect("Ok result") {
                        assert_eq!(actual_info, pkg_server_info)
                    } else {
                        assert!(false, "Expected {pkg_server_info:?}, got None");
                    }
                }
                Ok(None) => {
                    if let Some(actual_info) = result.ok().expect("Ok result") {
                        assert!(false, "Expected None, got {actual_info:?}");
                    }
                }
                Err(e) => {
                    if let Some(actual_err) = result.as_ref().err() {
                        assert_eq!(actual_err.to_string(), e.to_string())
                    } else {
                        assert!(false, "Expected {e}, got no error: {result:?}")
                    }
                }
            };
        }
    }

    #[fuchsia::test]
    async fn test_serve_impl_validate_args_in_tree() {
        let build_dir = tempfile::tempdir().expect("temp dir");
        let env =
            ffx_config::test_env().in_tree(build_dir.path()).build().expect("in-tree test env");
        let cmd = StartCommand {
            repository: None,
            trusted_root: None,
            address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
            repo_path: None,
            tunnel_addr: None,
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            port_path: None,
            no_device: false,
            refresh_metadata: false,
            auto_publish: None,
            background: false,
            foreground: true,
            disconnected: false,
        };
        let expected: Result<Option<PkgServerInfo>> =
            Err(user_error!("repo-path {:?} does not exist", build_dir.path().join("amber-files")));

        let fake_rcs_proxy_connector = make_fake_rcs_proxy_connector(&env).await;

        let result = serve_impl_validate_args(&cmd, &fake_rcs_proxy_connector, &env.context)
            .await
            .map_err(ffx_command_error::Error::from);
        match expected {
            Ok(Some(pkg_server_info)) => {
                if let Some(actual_info) = result.ok().expect("Ok result") {
                    assert_eq!(actual_info, pkg_server_info)
                } else {
                    assert!(false, "Expected {pkg_server_info:?}, got None");
                }
            }
            Ok(None) => {
                if let Some(actual_info) = result.ok().expect("Ok result") {
                    assert!(false, "Expected None, got {actual_info:?}");
                }
            }
            Err(e) => {
                if let Some(actual_err) = result.as_ref().err() {
                    assert_eq!(actual_err.to_string(), e.to_string())
                } else {
                    assert!(false, "Expected {e}, got no error: {result:?}")
                }
            }
        };
    }

    #[fuchsia::test]
    async fn test_serve_impl_validate_args_running_servers() {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let instance_root = isolate_root.join("repo_instances");
        fs::create_dir_all(&instance_root).expect("instance root dir");

        let repo_path = isolate_root.join("repo_path");
        fs::create_dir_all(&repo_path).expect("repo path dir");

        let env = builder
            .user_config("repository.process_dir", instance_root.to_string_lossy())
            .build()
            .unwrap();

        let instance_name = "devhost";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let server_info = PkgServerInfo {
            name: instance_name.into(),
            address: (REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into(),
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::new(),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: fidl_fuchsia_pkg_ext::RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode:
                fidl_fuchsia_pkg_ext::RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Background,
            pid: std::process::id(),
            repo_config,
        };

        let mgr = PkgServerInstances::new(instance_root);
        mgr.write_instance(&server_info).expect("test instance written");

        let test_cases: Vec<(StartCommand, Result<Option<PkgServerInfo>>)> = vec![
            (
                StartCommand {
                    repository: Some("another-name".into()),
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some(
                        Utf8PathBuf::from_path_buf(repo_path.clone()).expect("utf8 repo_path"),
                    ),
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!(
                    "Repository address conflict. \
            Cannot start a server named another-name serving {repo_path:?}. \
            Repository server \"{name}\" is already running on {addr} serving a different path: {dupe_path}\n\
            Use `ffx repository server list` to list running servers",
                    addr = server_info.address,
                    name = server_info.name,
                    dupe_path = server_info.repo_path_display()
                )),
            ),
            (
                StartCommand {
                    repository: Some(instance_name.into()),
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some(
                        Utf8PathBuf::from_path_buf(repo_path.clone()).expect("utf8 repo_path"),
                    ),
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Ok(Some(server_info.clone())),
            ),
            (
                StartCommand {
                    repository: Some(instance_name.into()),
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, 8888).into()),
                    repo_path: Some(
                        Utf8PathBuf::from_path_buf(repo_path.clone()).expect("utf8 repo_path"),
                    ),
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
                    port_path: None,
                    tunnel_addr: None,
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                Err(user_error!(
                    "Repository name conflict. \
    Cannot start a server named {name} serving {repo_path:?}. \
    Repository server \"{dupe_name}\" is already running on {addr} serving a different path: {dupe_path}\n\
    Use `ffx repository server list` to list running servers",
                    name = instance_name,
                    dupe_name = instance_name,
                    addr = server_info.address,
                    dupe_path = server_info.repo_path_display()
                )),
            ),
        ];

        let rcs_proxy_connector = make_fake_rcs_proxy_connector(&env).await;

        for (cmd, expected) in test_cases {
            let result = serve_impl_validate_args(&cmd, &rcs_proxy_connector, &env.context).await;
            match expected {
                Ok(Some(pkg_server_info)) => {
                    if let Some(actual_info) = match result {
                        Ok(info) => info,
                        Err(e) => {
                            assert!(false, " unexpected error {e}");
                            None
                        }
                    } {
                        assert_eq!(actual_info, pkg_server_info)
                    } else {
                        assert!(false, "Expected {pkg_server_info:?}, got None");
                    }
                }
                Ok(None) => {
                    if let Some(actual_info) = result.ok().expect("Ok result") {
                        assert!(false, "Expected None, got {actual_info:?}");
                    }
                }
                Err(e) => {
                    if let Some(actual_err) = result.as_ref().err() {
                        assert_eq!(actual_err.to_string(), e.to_string())
                    } else {
                        assert!(false, "Expected {e}, got no error: {result:?}")
                    }
                }
            };
        }
    }

    #[fuchsia::test]
    async fn test_serve_impl_validate_args_no_device() {
        let env = get_test_env_builder().build().unwrap();

        let instance_root = env.isolate_root.path().join("repo_instances");
        fs::create_dir_all(&instance_root).expect("instance root dir");

        let repo_path = env.isolate_root.path().join("repo_path");
        fs::create_dir_all(&repo_path).expect("repo path dir");

        let cmd = StartCommand {
            repository: Some("some_repo".into()),
            trusted_root: None,
            address: Some((REPO_LOCALHOST_IPV4_ADDR, 8888).into()),
            repo_path: Some(Utf8PathBuf::from_path_buf(repo_path.clone()).expect("utf8 repo_path")),
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            port_path: None,
            tunnel_addr: None,
            no_device: false,
            refresh_metadata: false,
            auto_publish: None,
            background: false,
            foreground: true,
            disconnected: false,
        };

        let rcs_proxy_connector = make_no_target_connector(&env).await;

        let result = serve_impl_validate_args(&cmd, &rcs_proxy_connector, &env.context)
            .await
            .map_err(ffx_command_error::Error::from);

        let err = result.expect_err("Expected an error but did not get one");

        let expected: String = "No devices/emulators found. Please ensure the device you want to use is connected and reachable, or an emulator is started.".into();
        assert_eq!(err.to_string(), expected);
    }

    #[fuchsia::test]
    async fn test_serve_impl_validate_args_too_many_devices() {
        let env = get_test_env_builder().build().unwrap();

        let instance_root = env.isolate_root.path().join("repo_instances");
        fs::create_dir_all(&instance_root).expect("instance root dir");

        let repo_path = env.isolate_root.path().join("repo_path");
        fs::create_dir_all(&repo_path).expect("repo path dir");

        let cmd = StartCommand {
            repository: Some("some_repo".into()),
            trusted_root: None,
            address: Some((REPO_LOCALHOST_IPV4_ADDR, 8888).into()),
            repo_path: Some(Utf8PathBuf::from_path_buf(repo_path.clone()).expect("utf8 repo_path")),
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            port_path: None,
            tunnel_addr: None,
            no_device: false,
            refresh_metadata: false,
            auto_publish: None,
            background: false,
            foreground: true,
            disconnected: false,
        };

        let rcs_proxy_connector = make_ambiguous_connector(&env).await;

        let result = serve_impl_validate_args(&cmd, &rcs_proxy_connector, &env.context)
            .await
            .map_err(ffx_command_error::Error::from);

        let err = result.expect_err("Expected an error but did not get one");

        let expected: String = "More than one device/emulator found. Use `ffx target list` to list known targets and specify one with the `-t` or `--target` flag.\nCurrently found: \n\tfoo\n\tbar".into();
        assert_eq!(err.to_string(), expected);
    }

    #[test_case(true, true; "refresh_direct")]
    #[test_case(true, false; "refresh_tunelled")]
    #[test_case(false, true; "norefresh_direct")]
    #[test_case(false, false; "norefresh_tunelled")]
    #[fuchsia::test]
    async fn test_start_register(refresh_metadata: bool, direct_target_connection: bool) {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .user_config(TARGET_DEFAULT_KEY, TARGET_NODENAME)
            .build()
            .unwrap();

        let (fake_repo, mut fake_repo_rx) = FakeRepositoryManager::new();
        let (fake_engine, mut fake_engine_rx) = FakeEngine::new();

        let (fdomain_client, _) = setup_fake_target(fake_repo.clone(), fake_engine.clone());

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();

        // Use a tmp repo to allow metadata updates
        let tmp_repo = tempfile::tempdir().unwrap();
        let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();
        test_utils::make_empty_pm_repo_dir(tmp_repo_path);

        let (listen_addr, repo_host) = if direct_target_connection {
            (REPO_UNSPECIFIED_IPV4_ADDR, HOST_ADDR)
        } else {
            (REPO_LOCALHOST_IPV4_ADDR, LOCALHOST)
        };

        let serve_tool = ServerStartTool {
            cmd: StartCommand {
                repository: Some(REPO_NAME.to_string()),
                trusted_root: None,
                address: Some((listen_addr, REPO_PORT).into()),
                repo_path: Some(tmp_repo_path.into()),
                product_bundle: None,
                alias: vec!["example.com".into(), "fuchsia.com".into()],
                storage_type: Some(RepositoryStorageType::Ephemeral),
                alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                port_path: Some(tmp_port_file.path().to_owned()),
                tunnel_addr: Some((REPO_LOCALHOST_IPV4_ADDR, DEVICE_PORT).into()),
                no_device: false,
                refresh_metadata,
                auto_publish: None,
                background: false,
                foreground: true,
                disconnected: false,
            },
            context: env.environment_context().clone(),
            rcs_proxy_connector: Connector::try_from_env(&env)
                .await
                .expect("Could not make RCS test connector"),
            host_address: Deferred::from_output(Ok(HostAddrHolder::from("1.2.3.4".to_string()))),
            target_spec: Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                TargetInfoQuery::try_from("".to_string()).unwrap(),
            ))),
        };

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &buffers);

        // Run main in background
        let _task = fasync::Task::local(async move { serve_tool.main(writer).await.unwrap() });

        // Future resolves once repo server communicates with them.
        let _timeout = timeout(time::Duration::from_secs(10), async {
            let _ = fake_repo_rx.next().await.unwrap();
            let _ = fake_engine_rx.next().await.unwrap();
        })
        .await
        .unwrap();

        // Get dynamic port
        let dynamic_repo_port =
            fs::read_to_string(tmp_port_file.path()).unwrap().parse::<u16>().unwrap();
        tmp_port_file.close().unwrap();

        let target_repo_port =
            if direct_target_connection { dynamic_repo_port } else { DEVICE_PORT };

        let target_repo_url = format!("http://{repo_host}:{target_repo_port}/{REPO_NAME}");

        assert_eq!(
            fake_repo.take_events(),
            vec![RepositoryManagerEvent::Add {
                repo: RepositoryConfig {
                    mirrors: Some(vec![MirrorConfig {
                        mirror_url: Some(target_repo_url),
                        subscribe: Some(true),
                        ..Default::default()
                    }]),
                    repo_url: Some(format!("fuchsia-pkg://{}", REPO_NAME)),
                    root_keys: Some(vec![fuchsia_repo::test_utils::repo_key().into()]),
                    root_version: Some(1),
                    root_threshold: Some(1),
                    use_local_mirror: Some(false),
                    storage_type: Some(fdomain_fuchsia_pkg::RepositoryStorageType::Ephemeral),
                    ..Default::default()
                }
            }],
        );

        assert_eq!(
            fake_engine.take_events(),
            vec![
                RewriteEngineEvent::ListDynamic,
                RewriteEngineEvent::IteratorNext,
                RewriteEngineEvent::ResetAll,
                RewriteEngineEvent::EditTransactionAdd {
                    rule: rule!("example.com" => REPO_NAME, "/" => "/"),
                },
                RewriteEngineEvent::EditTransactionAdd {
                    rule: rule!("fuchsia.com" => REPO_NAME, "/" => "/"),
                },
                RewriteEngineEvent::EditTransactionCommit,
            ],
        );

        let host_repo_url = format!("http://{LOCALHOST}:{dynamic_repo_port}/{REPO_NAME}");
        // Check repository state.
        let http_repo = HttpRepository::new(
            fuchsia_hyper::new_client(),
            Url::parse(&host_repo_url).unwrap(),
            Url::parse(&format!("{host_repo_url}/blobs")).unwrap(),
            BTreeSet::new(),
        );
        let mut repo_client = RepoClient::from_trusted_remote(http_repo).await.unwrap();

        assert_matches!(repo_client.update().await, Ok(true));
    }

    #[test_case(true; "direct")]
    #[test_case(false; "tunnelled")]
    #[fuchsia::test]
    async fn test_auto_reconnect(direct_target_connection: bool) {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config(TARGET_DEFAULT_KEY, TARGET_NODENAME)
            .user_config("ffx.isolated", true)
            .user_config(ffx_config::keys::NETWORK_ENABLED, false)
            .user_config(ffx_config::keys::USB_ENABLED, false)
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .unwrap();

        let (fake_repo, mut fake_repo_rx) = FakeRepositoryManager::new();
        let (fake_engine, mut fake_engine_rx) = FakeEngine::new();
        // In this test, target discovery is disabled (NETWORK_ENABLED and USB_ENABLED are false).
        // Consequently, the daemonless knocker (which resolves the target spec via discovery) will
        // fail on every knock and report `TargetNotFound`, simulating a lost connection.
        // Meanwhile, the mocked DirectConnector behavior bypasses resolution and successfully returns a
        // connection to the target, allowing the loop to reconnect and testing the auto-reconnect logic.

        let (fdomain_client, _) = setup_fake_target(fake_repo.clone(), fake_engine.clone());

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();
        let tmp_port_file_path = tmp_port_file.path().to_owned();

        let (listen_addr, repo_host) = if direct_target_connection {
            (REPO_UNSPECIFIED_IPV4_ADDR, HOST_ADDR)
        } else {
            (REPO_LOCALHOST_IPV4_ADDR, LOCALHOST)
        };

        let (mut tx, mut rx) = futures::channel::mpsc::unbounded();

        // Run main in background
        let _task = fasync::Task::local(async move {
            // Use a tmp repo to allow metadata updates
            let tmp_repo = tempfile::tempdir().unwrap();
            let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();
            test_utils::make_empty_pm_repo_dir(tmp_repo_path);

            Box::pin(serve_impl(
                Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                    TargetInfoQuery::try_from("".to_string()).unwrap(),
                ))),
                Connector::try_from_env(&env).await.expect("Could not make RCS test connector"),
                Deferred::from_output(Ok(HostAddrHolder::from("1.2.3.4".to_string()))),
                StartCommand {
                    repository: Some(REPO_NAME.to_string()),
                    trusted_root: None,
                    address: Some((listen_addr, REPO_PORT).into()),
                    repo_path: Some(tmp_repo_path.into()),
                    product_bundle: None,
                    alias: vec!["example.com".into(), "fuchsia.com".into()],
                    storage_type: Some(RepositoryStorageType::Ephemeral),
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                    port_path: Some(tmp_port_file_path),
                    tunnel_addr: Some((REPO_LOCALHOST_IPV4_ADDR, DEVICE_PORT).into()),
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                env.environment_context().clone(),
                ServerMode::Foreground,
                &mut tx,
                None,
            ))
            .await
            .unwrap()
        });

        // Future resolves once repo server communicates with them.
        let _timeout = timeout(time::Duration::from_secs(10), async {
            let _ = fake_repo_rx.next().await.unwrap();
            let _ = fake_engine_rx.next().await.unwrap();
        })
        .await
        .unwrap();

        // Get dynamic port
        let dynamic_repo_port =
            fs::read_to_string(tmp_port_file.path()).unwrap().parse::<u16>().unwrap();
        tmp_port_file.close().unwrap();

        let target_repo_port =
            if direct_target_connection { dynamic_repo_port } else { DEVICE_PORT };

        let target_repo_url = format!("http://{repo_host}:{target_repo_port}/{REPO_NAME}");

        assert_eq!(
            fake_repo.take_events(),
            vec![RepositoryManagerEvent::Add {
                repo: RepositoryConfig {
                    mirrors: Some(vec![MirrorConfig {
                        mirror_url: Some(target_repo_url),
                        subscribe: Some(true),
                        ..Default::default()
                    }]),
                    repo_url: Some(format!("fuchsia-pkg://{}", REPO_NAME)),
                    root_keys: Some(vec![fuchsia_repo::test_utils::repo_key().into()]),
                    root_version: Some(1),
                    root_threshold: Some(1),
                    use_local_mirror: Some(false),
                    storage_type: Some(fdomain_fuchsia_pkg::RepositoryStorageType::Ephemeral),
                    ..Default::default()
                }
            }],
        );

        assert_eq!(
            fake_engine.take_events(),
            vec![
                RewriteEngineEvent::ListDynamic,
                RewriteEngineEvent::IteratorNext,
                RewriteEngineEvent::ResetAll,
                RewriteEngineEvent::EditTransactionAdd {
                    rule: rule!("example.com" => REPO_NAME, "/" => "/"),
                },
                RewriteEngineEvent::EditTransactionAdd {
                    rule: rule!("fuchsia.com" => REPO_NAME, "/" => "/"),
                },
                RewriteEngineEvent::EditTransactionCommit,
            ],
        );

        let host_repo_url = format!("http://{LOCALHOST}:{dynamic_repo_port}/{REPO_NAME}");
        // Check repository state.
        let http_repo = HttpRepository::new(
            fuchsia_hyper::new_client(),
            Url::parse(&host_repo_url).unwrap(),
            Url::parse(&format!("{host_repo_url}/blobs")).unwrap(),
            BTreeSet::new(),
        );
        let mut repo_client = RepoClient::from_trusted_remote(http_repo).await.unwrap();

        assert_matches!(repo_client.update().await, Ok(true));

        // Look for the sequence of outputs on stdout to indicate
        // serving a repo, then reconnecting, then serving again.
        let mut state = 0;
        while state < 3 {
            if let Some(e) = rx.next().await {
                match state {
                    0 => {
                        assert_matches!(
                            e,
                            crate::target::ConnectEvent::StartServe { repo_path: _, addr: _ }
                        );
                    }
                    1 => {
                        assert_matches!(
                            e,
                            crate::target::ConnectEvent::LostConnection { knock_error: _ }
                        );
                    }
                    2 => {
                        assert_matches!(
                            e,
                            crate::target::ConnectEvent::StartServe { repo_path: _, addr: _ }
                        );
                    }
                    _ => {
                        unreachable!();
                    }
                }
            } else {
                assert!(false, "Got a none");
            }
            state += 1;
        }
        assert_eq!(state, 3);
    }

    #[fuchsia::test]
    async fn test_no_device() {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .unwrap();

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();
        let tmp_port_file_path = tmp_port_file.path().to_owned();

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);

        let (mut tx, mut rx) = futures::channel::mpsc::unbounded();
        // Run main in background
        let _task = fasync::Task::local(async move {
            // Use a tmp repo to allow metadata updates
            let tmp_repo = tempfile::tempdir().unwrap();
            let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();
            test_utils::make_empty_pm_repo_dir(tmp_repo_path);
            Box::pin(serve_impl(
                Deferred::from_output(Err(bug!("no target_spec"))),
                Connector::try_from_env(&env).await.expect("Could not make RCS test connector"),
                Deferred::from_output(Err(bug!("no host address"))),
                StartCommand {
                    repository: Some(REPO_NAME.to_string()),
                    trusted_root: None,
                    address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
                    repo_path: Some(tmp_repo_path.into()),
                    product_bundle: None,
                    alias: vec![],
                    storage_type: None,
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                    port_path: Some(tmp_port_file_path),
                    tunnel_addr: None,
                    no_device: true,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                env.environment_context().clone(),
                ServerMode::Foreground,
                &mut tx,
                None,
            ))
            .await
            .unwrap()
        });

        assert!(rx.next().await.is_some(), "Should pull serving repo event");

        // Get dynamic port
        let dynamic_repo_port =
            fs::read_to_string(tmp_port_file.path()).unwrap().parse::<u16>().unwrap();
        tmp_port_file.close().unwrap();

        let repo_url = format!("http://{LOCALHOST}:{dynamic_repo_port}/{REPO_NAME}");

        // Check repository state.
        let http_repo = HttpRepository::new(
            fuchsia_hyper::new_client(),
            Url::parse(&repo_url).unwrap(),
            Url::parse(&format!("{repo_url}/blobs")).unwrap(),
            BTreeSet::new(),
        );
        let mut repo_client = RepoClient::from_trusted_remote(http_repo).await.unwrap();

        assert_matches!(repo_client.update().await, Ok(true));
    }

    async fn write_product_bundle(pb_dir: &Utf8Path) {
        let blobs_dir = pb_dir.join("blobs");

        let mut repositories = vec![];
        for repo_name in ["fuchsia.com", "example.com"] {
            let metadata_path = pb_dir.join(repo_name);
            fuchsia_repo::test_utils::make_repo_dir(
                metadata_path.as_ref(),
                blobs_dir.as_ref(),
                None,
            )
            .await;
            repositories.push(product_bundle::Repository {
                name: repo_name.into(),
                metadata_path,
                blobs_path: blobs_dir.clone(),
                delivery_blob_type: 1,
                root_private_key_path: None,
                targets_private_key_path: None,
                snapshot_private_key_path: None,
                timestamp_private_key_path: None,
                ota_manifest_signature_path: None,
                ota_manifest_path: None,
            });
        }

        let pb = product_bundle::ProductBundle::V2(product_bundle::ProductBundleV2 {
            product_name: "test".into(),
            product_version: "test-product-version".into(),
            partitions: assembly_partitions_config::PartitionsConfig::default(),
            sdk_version: "test-sdk-version".into(),
            system_a: None,
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories,
            update_package_hash: None,
            virtual_devices_path: None,
            release_info: None,
        });
        pb.write(&pb_dir).unwrap();
    }

    #[test_case(true; "direct")]
    #[test_case(false; "tunnelled")]
    #[fuchsia::test]
    async fn test_serve_product_bundle(direct_target_connection: bool) {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config(TARGET_DEFAULT_KEY, TARGET_NODENAME)
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .unwrap();

        let tmp_pb_dir = tempfile::tempdir().unwrap();
        let pb_dir = Utf8Path::from_path(tmp_pb_dir.path()).unwrap().canonicalize_utf8().unwrap();
        write_product_bundle(&pb_dir).await;

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();
        let tmp_port_file_path = tmp_port_file.path().to_owned();

        let (fake_repo, mut fake_repo_rx) = FakeRepositoryManager::new();
        let (fake_engine, _fake_engine_rx) = FakeEngine::new();
        let (fdomain_client, fake_netstack) =
            setup_fake_target(fake_repo.clone(), fake_engine.clone());
        let socket_provider = fake_netstack.new_socket_provider();

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        let (listen_addr, repo_host) = if direct_target_connection {
            (REPO_UNSPECIFIED_IPV4_ADDR, HOST_ADDR)
        } else {
            (REPO_LOCALHOST_IPV4_ADDR, LOCALHOST)
        };

        let (mut tx, _rx) = futures::channel::mpsc::unbounded();
        let (repo_url_tx, mut repo_url_rx) = futures::channel::mpsc::unbounded();

        // Run main in background
        let _task = fasync::Task::local(async move {
            Box::pin(serve_impl(
                Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                    TargetInfoQuery::try_from("".to_string()).unwrap(),
                ))),
                Connector::try_from_env(&env).await.expect("Could not make RCS test connector"),
                Deferred::from_output(Ok(HostAddrHolder::from("1.2.3.4".to_string()))),
                StartCommand {
                    repository: None,
                    trusted_root: None,
                    address: Some((listen_addr, REPO_PORT).into()),
                    repo_path: None,
                    product_bundle: Some(pb_dir),
                    alias: vec![],
                    storage_type: Some(RepositoryStorageType::Ephemeral),
                    alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
                    port_path: Some(tmp_port_file_path),
                    tunnel_addr: Some((REPO_LOCALHOST_IPV4_ADDR, DEVICE_PORT).into()),
                    no_device: false,
                    refresh_metadata: false,
                    auto_publish: None,
                    background: false,
                    foreground: true,
                    disconnected: false,
                },
                test_env.context.clone(),
                ServerMode::Foreground,
                &mut tx,
                Some(repo_url_tx),
            ))
            .await
            .unwrap()
        });

        // Future resolves once repo server communicates with them.
        let _timeout = timeout(time::Duration::from_secs(10), async {
            let _ = fake_repo_rx.next().await.unwrap();
            let _ = fake_repo_rx.next().await.unwrap();
        })
        .await
        .unwrap();

        // Get dynamic port
        let dynamic_repo_port =
            fs::read_to_string(tmp_port_file.path()).unwrap().parse::<u16>().unwrap();
        tmp_port_file.close().unwrap();
        let repo_base_name = "devhost";
        let target_repo_port =
            if direct_target_connection { dynamic_repo_port } else { DEVICE_PORT };

        assert_eq!(
            repo_url_rx.next().await.unwrap(),
            format!("http://{repo_host}:{target_repo_port}/{repo_base_name}.example.com")
        );
        assert_eq!(
            fake_repo.take_events(),
            ["example.com", "fuchsia.com"].map(|repo_name| RepositoryManagerEvent::Add {
                repo: RepositoryConfig {
                    mirrors: Some(vec![MirrorConfig {
                        mirror_url: Some(format!(
                            "http://{repo_host}:{target_repo_port}/{repo_base_name}.{repo_name}"
                        )),
                        subscribe: Some(true),
                        ..Default::default()
                    }]),
                    repo_url: Some(format!("fuchsia-pkg://{repo_base_name}.{repo_name}")),
                    root_keys: Some(vec![fuchsia_repo::test_utils::repo_key().into()]),
                    root_version: Some(1),
                    root_threshold: Some(1),
                    use_local_mirror: Some(false),
                    storage_type: Some(fdomain_fuchsia_pkg::RepositoryStorageType::Ephemeral),
                    ..Default::default()
                }
            },)
        );

        // Check repository state.
        for repo_name in ["example.com", "fuchsia.com"] {
            let repo_url =
                format!("http://{LOCALHOST}:{dynamic_repo_port}/{repo_base_name}.{repo_name}");
            let http_repo = HttpRepository::new(
                fuchsia_hyper::new_client(),
                Url::parse(&repo_url).unwrap(),
                Url::parse(&format!("{repo_url}/blobs")).unwrap(),
                BTreeSet::new(),
            );
            let mut repo_client = RepoClient::from_trusted_remote(http_repo).await.unwrap();

            assert_matches!(repo_client.update().await, Ok(true));
        }

        // Check repository state from the tunnel as well.
        if !direct_target_connection {
            for repo_name in ["example.com", "fuchsia.com"] {
                let repo_url =
                    format!("http://{repo_host}:{target_repo_port}/{repo_base_name}.{repo_name}");
                let http_repo = HttpRepository::new(
                    ffx_target_net_testutil::TargetHyperConnector::new(socket_provider.clone())
                        .into_client(),
                    Url::parse(&repo_url).unwrap(),
                    Url::parse(&format!("{repo_url}/blobs")).unwrap(),
                    BTreeSet::new(),
                );
                let mut repo_client = RepoClient::from_trusted_remote(http_repo).await.unwrap();

                assert_matches!(repo_client.update().await, Ok(true));
            }
        }
    }

    fn generate_ed25519_private_key() -> Ed25519PrivateKey {
        Ed25519PrivateKey::from_pkcs8(&Ed25519PrivateKey::pkcs8().unwrap()).unwrap()
    }

    async fn setup_trusted_root() -> (tempfile::TempDir, tempfile::TempDir, Utf8PathBuf) {
        // Set up a simple test repository
        let tmp_repo = tempfile::tempdir().unwrap();
        let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();
        let tmp_pm_repo = test_utils::make_pm_repository(tmp_repo_path).await;
        let mut tmp_repo_client = RepoClient::from_trusted_remote(&tmp_pm_repo).await.unwrap();
        tmp_repo_client.update().await.unwrap();

        // Generate a newer set of keys.
        let repo_keys_new = RepoKeys::builder()
            .add_root_key(Box::new(generate_ed25519_private_key()))
            .add_targets_key(Box::new(generate_ed25519_private_key()))
            .add_snapshot_key(Box::new(generate_ed25519_private_key()))
            .add_timestamp_key(Box::new(generate_ed25519_private_key()))
            .build();

        // Generate new metadata that trusts the new keys, but signs it with the old keys.
        let repo_signing_keys = tmp_pm_repo.repo_keys().unwrap();
        RepoBuilder::from_database(
            tmp_repo_client.remote_repo(),
            &repo_keys_new,
            tmp_repo_client.database(),
        )
        .signing_repo_keys(&repo_signing_keys)
        .commit()
        .await
        .unwrap();

        assert_eq!(tmp_repo_client.database().trusted_timestamp().unwrap().version(), 1);

        // Delete 1.root.json to ensure it can't be accessed when initializing
        // root of trust with 2.root.json
        std::fs::remove_file(tmp_repo_path.join("repository").join("1.root.json")).unwrap();
        // Move root.json and 2.root.json out of the repository/ dir, to verify
        // we can pass the root of trust file from anywhere to a repo constructor
        let tmp_root = tempfile::tempdir().unwrap();
        let tmp_root_dir = Utf8Path::from_path(tmp_root.path()).unwrap();
        let trusted_root_path = tmp_root_dir.join("2.root.json");
        std::fs::rename(
            tmp_repo_path.join("repository").join("root.json"),
            tmp_root_dir.join("root.json"),
        )
        .unwrap();
        std::fs::rename(tmp_repo_path.join("repository").join("2.root.json"), &trusted_root_path)
            .unwrap();
        (tmp_repo, tmp_root, trusted_root_path)
    }

    #[fuchsia::test]
    async fn test_trusted_root_file() {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .unwrap();

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();

        let (tmp_repo, _tmp_root, _trusted_root_path) = setup_trusted_root().await;
        let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();

        let (fake_repo, _fake_repo_rx) = FakeRepositoryManager::new();
        let (fake_engine, _fake_engine_rx) = FakeEngine::new();

        let (fdomain_client, _) = setup_fake_target(fake_repo.clone(), fake_engine.clone());

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        // Prepare serving the repo without passing the trusted root, and
        // passing of the trusted root 2.root.json explicitly
        let serve_cmd_without_root = StartCommand {
            repository: Some(REPO_NAME.to_string()),
            trusted_root: None,
            address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
            repo_path: Some(tmp_repo_path.into()),
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
            port_path: Some(tmp_port_file.path().to_owned()),
            tunnel_addr: None,
            no_device: true,
            refresh_metadata: false,
            auto_publish: None,
            background: false,
            foreground: true,
            disconnected: false,
        };

        // Serving the repo should error out since it does not find root.json and
        // and can't initialize root of trust 1.root.json.
        let (mut tx, _rx) = futures::channel::mpsc::unbounded();
        assert_eq!(
            Box::pin(serve_impl(
                Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                    TargetInfoQuery::try_from("127.0.1.1".to_string()).unwrap()
                ))),
                Connector::try_from_env(&env).await.expect("Could not make RCS test connector"),
                Deferred::from_output(Ok(HostAddrHolder::from("127.0.0.1".to_string()))),
                serve_cmd_without_root,
                test_env.context.clone(),
                ServerMode::Foreground,
                &mut tx,
                None,
            ))
            .await
            .is_err(),
            true
        );
    }

    #[fuchsia::test]
    async fn test_trusted_root_file_dynamic_port() {
        let mut builder = get_test_env_builder();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .unwrap();

        let tmp_port_file = tempfile::NamedTempFile::new().unwrap();

        let (tmp_repo, _tmp_root, trusted_root_path) = setup_trusted_root().await;
        let tmp_repo_path = Utf8Path::from_path(tmp_repo.path()).unwrap();

        let (fake_repo, _fake_repo_rx) = FakeRepositoryManager::new();
        let (fake_engine, _fake_engine_rx) = FakeEngine::new();

        let (fdomain_client, _) = setup_fake_target(fake_repo.clone(), fake_engine.clone());

        let behavior = make_direct_connector_behavior(fdomain_client);

        let env =
            FhoEnvironment::new_with_args(&test_env.context, &["some", "repo", "start", "test"]);
        let target_env = target_interface(&env);
        target_env.set_behavior_for_test(behavior);

        let serve_cmd_with_root = StartCommand {
            repository: Some(REPO_NAME.to_string()),
            trusted_root: trusted_root_path.clone().into(),
            address: Some((REPO_LOCALHOST_IPV4_ADDR, REPO_PORT).into()),
            repo_path: Some(tmp_repo_path.into()),
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
            port_path: Some(tmp_port_file.path().to_owned()),
            tunnel_addr: None,
            no_device: false,
            refresh_metadata: false,
            auto_publish: None,
            background: false,
            foreground: true,
            disconnected: false,
        };

        let (mut tx, mut rx) = futures::channel::mpsc::unbounded();

        let connector =
            Connector::try_from_env(&env).await.expect("Could not make RCS test connector");

        // Run main in background
        let _task = fasync::Task::local(async move {
            Box::pin(serve_impl(
                Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                    TargetInfoQuery::try_from("test_target_info".to_string()).unwrap(),
                ))),
                connector,
                Deferred::from_output(Ok(HostAddrHolder::from("127.0.0.1".to_string()))),
                serve_cmd_with_root,
                test_env.context.clone(),
                ServerMode::Foreground,
                &mut tx,
                None,
            ))
            .await
            .unwrap()
        });

        // Wait for the "Serving repository ..." output
        assert!(rx.next().await.is_some(), "Should pull serving repo event!");

        // Get dynamic port
        let dynamic_repo_port =
            fs::read_to_string(tmp_port_file.path()).unwrap().parse::<u16>().unwrap();
        tmp_port_file.close().unwrap();

        let repo_url = format!("http://{LOCALHOST}:{dynamic_repo_port}/{REPO_NAME}");

        // Check repository state.
        let http_repo = HttpRepository::new(
            fuchsia_hyper::new_client(),
            Url::parse(&repo_url).unwrap(),
            Url::parse(&format!("{repo_url}/blobs")).unwrap(),
            BTreeSet::new(),
        );

        // As there was no key rotation since we created 2.root.json above,
        // and we removed root.json out of the repo, creating a repo client via
        // RepoClient::from_trusted_remote would error out trying to find root.json.
        // Hence we need to initialize the http client with 2.root.json, too.
        let mut repo_client =
            repo_client_from_optional_trusted_root(Some(trusted_root_path), http_repo)
                .await
                .unwrap();

        // The repo metadata should be at version 2
        assert_matches!(repo_client.update().await, Ok(true));
        assert_eq!(repo_client.database().trusted_timestamp().unwrap().version(), 2);
    }
}
