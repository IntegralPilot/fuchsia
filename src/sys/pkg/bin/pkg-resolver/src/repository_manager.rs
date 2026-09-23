// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cache::{BlobFetcher, CacheError, MerkleForError, ToResolveError, ToResolveStatus};
use crate::inspect_util::{self, InspectableRepositoryConfig};
use crate::repository::Repository;
use anyhow::{Context as _, anyhow};
use cobalt_sw_delivery_registry as metrics;
use delivery_blob::DeliveryBlobType;
use fidl_contrib::protocol_connector::ProtocolSender;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics::MetricEvent;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_ext::{self as pkg, BlobId, RepositoryConfig, RepositoryConfigs, cache};
use fuchsia_inspect as inspect;
use fuchsia_pkg::PackageDirectory;
use fuchsia_sync::{Mutex, RwLock};
use fuchsia_trace as ftrace;
use fuchsia_url::RepositoryUrl;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::future::LocalBoxFuture;
use futures::lock::Mutex as AsyncMutex;
use futures::prelude::*;
use http_uri_ext::HttpUriExt as _;
use log::{error, info};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, btree_set};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};
use thiserror::Error;
use zx::Status;

/// [RepositoryManager] controls access to all the repository configs used by the package resolver.
pub struct RepositoryManager {
    dynamic_configs_path: Option<String>,
    static_configs: HashMap<RepositoryUrl, InspectableRepositoryConfig>,
    dynamic_configs: HashMap<RepositoryUrl, InspectableRepositoryConfig>,
    persisted_repos_dir: Arc<Option<String>>,
    repositories: Arc<RwLock<HashMap<RepositoryUrl, Arc<AsyncMutex<Repository>>>>>,
    cobalt_sender: ProtocolSender<MetricEvent>,
    inspect: RepositoryManagerInspectState,
    tuf_timeouts: crate::TufTimeouts,
    data_proxy: Option<fio::DirectoryProxy>,
    delivery_blob_type: DeliveryBlobType,
}

#[derive(Debug)]
struct RepositoryManagerInspectState {
    #[expect(dead_code)]
    node: inspect::Node,
    dynamic_configs_node: inspect::Node,
    static_configs_node: inspect::Node,
    stats: Arc<Mutex<Stats>>,
    repos_node: Arc<inspect::Node>,
}

#[derive(Debug)]
pub struct Stats {
    #[expect(dead_code)]
    node: inspect::Node,

    mirrors_node: inspect::Node,
    mirrors: HashMap<String, Arc<MirrorStats>>,
}

impl Stats {
    fn new(node: inspect::Node) -> Self {
        Self { mirrors_node: node.create_child("mirrors"), mirrors: HashMap::new(), node }
    }
    pub fn for_mirror(&mut self, mirror: String) -> Arc<MirrorStats> {
        match self.mirrors.entry(mirror) {
            Entry::Occupied(entry) => Arc::clone(entry.get()),
            Entry::Vacant(entry) => {
                let stats = Arc::new(MirrorStats::new(self.mirrors_node.create_child(entry.key())));
                entry.insert(Arc::clone(&stats));
                stats
            }
        }
    }
}

#[derive(Debug)]
pub struct MirrorStats {
    #[expect(dead_code)]
    node: inspect::Node,
    /// web requests that failed with a network error and then succeeded when retried
    network_blips: inspect_util::Counter,
    /// web requests that received a response asking us to try again later
    network_rate_limits: inspect_util::Counter,
}

impl MirrorStats {
    fn new(node: inspect::Node) -> Self {
        Self {
            network_blips: inspect_util::Counter::new(&node, "network_blips"),
            network_rate_limits: inspect_util::Counter::new(&node, "network_rate_limits"),
            node,
        }
    }
    pub fn network_blips(&self) -> &inspect_util::Counter {
        &self.network_blips
    }
    pub fn network_rate_limits(&self) -> &inspect_util::Counter {
        &self.network_rate_limits
    }
}

impl RepositoryManager {
    /// Returns a reference to the [RepositoryConfig] config identified by the config `repo_url`,
    /// or `None` if it does not exist.
    pub fn get(&self, repo_url: &RepositoryUrl) -> Option<&Arc<RepositoryConfig>> {
        self.dynamic_configs
            .get(repo_url)
            .or_else(|| self.static_configs.get(repo_url))
            .map(Deref::deref)
    }

    /// Returns a handle to this repo manager's inspect statistics.
    pub fn stats(&self) -> Arc<Mutex<Stats>> {
        Arc::clone(&self.inspect.stats)
    }

    /// Inserts a [RepositoryConfig] into this manager.
    ///
    /// If dynamic configuration is disabled (if the manager does not have a dynamic config path)
    /// `Err(InsertError)` is returned.
    ///
    /// If the manager did not have a [RepositoryConfig] with a corresponding repository url for
    /// the repository, `Ok(None)` is returned.
    ///
    /// If the manager did have this repository present as a dynamic config, the value is replaced
    /// and the old [RepositoryConfig] is returned. If this repository is a static config, the
    /// static config is shadowed by the dynamic config until it is removed.
    pub async fn insert(
        &mut self,
        config: impl Into<Arc<RepositoryConfig>>,
    ) -> Result<Option<Arc<RepositoryConfig>>, InsertError> {
        let dynamic_configs_path =
            self.dynamic_configs_path.as_ref().ok_or(InsertError::DynamicConfigurationDisabled)?;

        let config = config.into();

        let connected = self.repositories.write().remove(config.repo_url()).is_some();
        if connected {
            info!("re-opening {} repo because config changed", config.repo_url());
        }

        let inspectable_config = InspectableRepositoryConfig::new(
            Arc::clone(&config),
            &self.inspect.dynamic_configs_node,
            config.repo_url().host(),
        );
        let result = self.dynamic_configs.insert(config.repo_url().clone(), inspectable_config);

        Self::save(&self.data_proxy, dynamic_configs_path, &mut self.dynamic_configs).await;
        Ok(result.map(|a| Arc::clone(&*a)))
    }

    /// Removes a [RepositoryConfig] identified by the config `repo_url`.
    ///
    /// If dynamic configuration is disabled (if the manager does not have a dynamic config path)
    /// `Err(RemoveError)` is returned.
    pub async fn remove(
        &mut self,
        repo_url: &RepositoryUrl,
    ) -> Result<Option<Arc<RepositoryConfig>>, RemoveError> {
        let dynamic_configs_path =
            self.dynamic_configs_path.as_ref().ok_or(RemoveError::DynamicConfigurationDisabled)?;

        if let Some(config) = self.dynamic_configs.remove(repo_url) {
            let connected = self.repositories.write().remove(config.repo_url()).is_some();
            if connected {
                info!("closing {}", config.repo_url());
            }

            Self::save(&self.data_proxy, dynamic_configs_path, &mut self.dynamic_configs).await;
            return Ok(Some(Arc::clone(&*config)));
        }
        if self.static_configs.contains_key(repo_url) {
            Err(RemoveError::CannotRemoveStaticRepositories)
        } else {
            Ok(None)
        }
    }

    /// Returns an iterator over all the managed [RepositoryConfig]s.
    pub fn list(&self) -> List<'_> {
        let keys = self
            .dynamic_configs
            .iter()
            .chain(self.static_configs.iter())
            .map(|(k, _)| k)
            .collect::<BTreeSet<_>>();

        List { keys: keys.into_iter(), repo_mgr: self }
    }

    /// If persistent dynamic configs are enabled, save the current configs to disk. Log, and
    /// ultimately ignore, any errors that occur to make sure forward progress can always be made.
    async fn save(
        data_proxy: &Option<fio::DirectoryProxy>,
        dynamic_configs_path: &str,
        dynamic_configs: &mut HashMap<RepositoryUrl, InspectableRepositoryConfig>,
    ) {
        let data_proxy = match data_proxy.as_ref() {
            Some(proxy) => proxy,
            None => {
                error!("unable to save repositories because /data proxy is not available");
                return;
            }
        };
        let configs = dynamic_configs.values().map(|c| (***c).clone()).collect::<Vec<_>>();

        let result = async {
            let data = serde_json::to_vec(&RepositoryConfigs::Version1(configs))
                .context("serialize config")?;

            // TODO(https://fxbug.dev/42164009): We need to reopen because `resolve_succeeds_with_broken_minfs`
            // expects it, this should be removed once the test is fixed.
            let data_proxy =
                fuchsia_fs::directory::open_directory(data_proxy, ".", fio::PERM_WRITABLE)
                    .await
                    .context("reopen /data")?;

            let temp_path = &format!("{dynamic_configs_path}.new");
            crate::util::do_with_atomic_file(
                &data_proxy,
                temp_path,
                dynamic_configs_path,
                |proxy| async move {
                    fuchsia_fs::file::write(&proxy, &data)
                        .await
                        .with_context(|| format!("writing file: {temp_path}"))
                },
            )
            .await
        }
        .await;

        match result {
            Ok(()) => {}
            Err(err) => {
                error!("error while saving repositories: {:#}", anyhow!(err));
            }
        }
    }

    pub fn get_package<'a>(
        &self,
        url: &'a AbsolutePackageUrl,
        gc_protection: fpkg::GcProtection,
        cache: &'a cache::Client,
        blob_fetcher: &'a BlobFetcher,
        trace_id: ftrace::Id,
    ) -> LocalBoxFuture<'a, Result<(BlobId, PackageDirectory), GetPackageError>> {
        let config = if let Some(config) = self.get(url.repository()) {
            Arc::clone(config)
        } else {
            return futures::future::ready(Err(GetPackageError::RepoNotFound(
                url.repository().clone(),
            )))
            .boxed_local();
        };

        let fut = open_cached_or_new_repository(
            Clone::clone(&self.data_proxy),
            Arc::clone(&self.persisted_repos_dir),
            Arc::clone(&self.repositories),
            Arc::clone(&config),
            url.repository(),
            self.cobalt_sender.clone(),
            Arc::clone(&self.inspect.repos_node),
            self.tuf_timeouts,
        );

        let delivery_blob_type = self.delivery_blob_type;
        let cobalt_sender = self.cobalt_sender.clone();
        async move {
            let repo = fut.await?;
            let blob_mirror_url = config
                .mirrors()
                .first()
                .ok_or(GetPackageError::NoMirrors(url.repository().clone()))?
                .blob_mirror_url()
                .to_owned();
            let blob_base_url = blob_mirror_url
                .extend_dir_with_path(&u32::from(delivery_blob_type).to_string())
                .map_err(GetPackageError::BlobUrl)?;
            crate::cache::cache_package(
                repo,
                url,
                gc_protection,
                cache,
                blob_fetcher,
                blob_base_url,
                cobalt_sender,
                trace_id,
            )
            .await
            .map_err(Into::into)
        }
        .boxed_local()
    }

    pub fn get_package_hash<'a>(
        &self,
        url: &'a AbsolutePackageUrl,
    ) -> LocalBoxFuture<'a, Result<(BlobId, http::Uri), GetPackageHashError>> {
        let config = if let Some(config) = self.get(url.repository()) {
            Arc::clone(config)
        } else {
            return futures::future::ready(Err(GetPackageHashError::RepoNotFound(
                url.repository().clone(),
            )))
            .boxed_local();
        };

        let repo = open_cached_or_new_repository(
            Clone::clone(&self.data_proxy),
            Arc::clone(&self.persisted_repos_dir),
            Arc::clone(&self.repositories),
            Arc::clone(&config),
            url.repository(),
            self.cobalt_sender.clone(),
            Arc::clone(&self.inspect.repos_node),
            self.tuf_timeouts,
        );

        let cobalt_sender = self.cobalt_sender.clone();
        let delivery_blob_type = self.delivery_blob_type;

        async move {
            let repo = repo.await?;
            let blob_mirror_url = config
                .mirrors()
                .first()
                .ok_or(GetPackageHashError::NoMirrors(url.repository().clone()))?
                .blob_mirror_url()
                .to_owned();
            let blob_base_url = blob_mirror_url
                .extend_dir_with_path(&u32::from(delivery_blob_type).to_string())
                .map_err(GetPackageHashError::BlobUrl)?;
            Ok((
                crate::cache::merkle_for_url(repo, url, cobalt_sender)
                    .await
                    .map_err(GetPackageHashError::MerkleFor)?,
                blob_base_url,
            ))
        }
        .boxed_local()
    }
}

#[allow(clippy::too_many_arguments)]
async fn open_cached_or_new_repository(
    data_proxy: Option<fio::DirectoryProxy>,
    persisted_repos_dir: Arc<Option<String>>,
    repositories: Arc<RwLock<HashMap<RepositoryUrl, Arc<AsyncMutex<Repository>>>>>,
    config: Arc<RepositoryConfig>,
    url: &RepositoryUrl,
    cobalt_sender: ProtocolSender<MetricEvent>,
    inspect_node: Arc<inspect::Node>,
    tuf_timeouts: crate::TufTimeouts,
) -> Result<Arc<AsyncMutex<Repository>>, OpenRepoError> {
    if let Some(conn) = repositories.read().get(url) {
        return Ok(conn.clone());
    }

    let persisted_repos_dir = (*persisted_repos_dir).as_deref();

    // Create the rust tuf client. In order to minimize our time with the lock held, we'll
    // create the client first, even if it proves to be redundant because we lost the race with
    // another thread.
    let mut repo = Arc::new(futures::lock::Mutex::new(
        Repository::new(
            data_proxy,
            persisted_repos_dir,
            &config,
            cobalt_sender,
            inspect_node.create_child(url.host()),
            tuf_timeouts,
        )
        .await
        .map_err(|e| OpenRepoError { repo_url: config.repo_url().clone(), source: e })?,
    ));

    // It's still possible we raced with some other connection attempt
    let mut repositories = repositories.write();
    repo = Arc::clone(repositories.entry(url.clone()).or_insert_with(|| repo.clone()));
    Ok(repo)
}

#[derive(Debug)]
pub struct UnsetInspectNode;

#[derive(Debug)]
pub struct UnsetCobaltSender;

/// [RepositoryManagerBuilder] constructs a [RepositoryManager], optionally initializing it
/// with [RepositoryConfig]s passed in directly or loaded out of the filesystem.
#[derive(Clone, Debug)]
pub struct RepositoryManagerBuilder<S = UnsetCobaltSender, N = UnsetInspectNode> {
    dynamic_configs_path: Option<String>,
    persisted_repos_dir: Option<String>,
    static_configs: HashMap<RepositoryUrl, Arc<RepositoryConfig>>,
    dynamic_configs: HashMap<RepositoryUrl, Arc<RepositoryConfig>>,
    cobalt_sender: S,
    inspect_node: N,
    tuf_timeouts: crate::TufTimeouts,
    data_proxy: Option<fio::DirectoryProxy>,
    delivery_blob_type: DeliveryBlobType,
}

impl<S, N> RepositoryManagerBuilder<S, N> {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255328)
    /// Load a directory of [RepositoryConfigs](RepositoryConfig) files into the
    /// [RepositoryManager], or error out if we encounter errors during the load. The
    /// [RepositoryManagerBuilder] is also returned on error in case the errors should be ignored.
    pub fn load_static_configs_dir<P>(
        mut self,
        static_configs_dir: P,
    ) -> Result<Self, (Self, Vec<LoadError>)>
    where
        P: AsRef<Path>,
    {
        let static_configs_dir = static_configs_dir.as_ref();

        let (static_configs, errs) = load_configs_dir(static_configs_dir);
        self.static_configs = static_configs
            .into_iter()
            .map(|(repo_url, config)| (repo_url, Arc::new(config)))
            .collect();
        if errs.is_empty() { Ok(self) } else { Err((self, errs)) }
    }

    /// Customize the repository manager with the persisted repository directory.
    pub fn with_persisted_repos_dir<P>(mut self, path: P) -> Self
    where
        P: Into<String>,
    {
        self.persisted_repos_dir = Some(path.into());
        self
    }
}

impl RepositoryManagerBuilder<UnsetCobaltSender, UnsetInspectNode> {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255328)
    /// Create a new builder and initialize it with the dynamic
    /// [RepositoryConfigs](RepositoryConfig) from this path if it exists, and add it to the
    /// [RepositoryManager], or error out if we encounter errors during the load. The
    /// [RepositoryManagerBuilder] is also returned on error in case the errors should be ignored.
    pub async fn new<P>(
        data_proxy: Option<fio::DirectoryProxy>,
        dynamic_configs_path: Option<P>,
        tuf_timeouts: crate::TufTimeouts,
    ) -> Result<Self, (Self, LoadError)>
    where
        P: Into<String>,
    {
        let dynamic_configs_path = dynamic_configs_path.map(|p| p.into());

        let (dynamic_configs, err) = match (data_proxy.as_ref(), dynamic_configs_path.as_ref()) {
            (Some(data_proxy), Some(dynamic_configs_path)) => {
                match load_configs_file_from_proxy(data_proxy, dynamic_configs_path).await {
                    Ok(dynamic_configs) => (dynamic_configs, None),
                    Err(err) => (vec![], Some(err)),
                }
            }
            _ => (vec![], None),
        };

        let builder = RepositoryManagerBuilder {
            dynamic_configs_path,
            persisted_repos_dir: None,
            static_configs: HashMap::new(),
            dynamic_configs: dynamic_configs
                .into_iter()
                .map(|config| (config.repo_url().clone(), Arc::new(config)))
                .collect(),
            cobalt_sender: UnsetCobaltSender,
            inspect_node: UnsetInspectNode,
            tuf_timeouts,
            data_proxy,
            delivery_blob_type: DeliveryBlobType::Type1,
        };

        if let Some(err) = err { Err((builder, err)) } else { Ok(builder) }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255328)
    #[cfg(test)]
    pub async fn new_test<P>(
        data_dir: &tempfile::TempDir,
        dynamic_configs_path: Option<P>,
    ) -> Result<Self, (Self, LoadError)>
    where
        P: Into<String>,
    {
        let proxy = fuchsia_fs::directory::open_in_namespace(
            data_dir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();
        Self::new(
            Some(proxy),
            dynamic_configs_path,
            crate::TufTimeouts {
                metadata: std::time::Duration::from_secs(30),
                network_header: zx::BootDuration::from_seconds(30),
            },
        )
        .await
    }

    /// Adds these static [RepoConfigs](RepoConfig) to the [RepositoryManager].
    #[cfg(test)]
    pub fn static_configs<I>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = RepositoryConfig>,
    {
        for config in iter.into_iter() {
            self.static_configs.insert(config.repo_url().clone(), Arc::new(config));
        }

        self
    }
}

impl<S> RepositoryManagerBuilder<S, UnsetInspectNode> {
    /// Use the given inspect_node in the [RepositoryManager].
    pub fn inspect_node(
        self,
        inspect_node: inspect::Node,
    ) -> RepositoryManagerBuilder<S, inspect::Node> {
        RepositoryManagerBuilder {
            dynamic_configs_path: self.dynamic_configs_path,
            persisted_repos_dir: self.persisted_repos_dir,
            static_configs: self.static_configs,
            dynamic_configs: self.dynamic_configs,
            cobalt_sender: self.cobalt_sender,
            inspect_node,
            tuf_timeouts: self.tuf_timeouts,
            data_proxy: self.data_proxy,
            delivery_blob_type: self.delivery_blob_type,
        }
    }

    /// Use the given delivery_blob_type in the [RepositoryManager].
    pub fn delivery_blob_type(mut self, delivery_blob_type: DeliveryBlobType) -> Self {
        self.delivery_blob_type = delivery_blob_type;
        self
    }
}

impl<N> RepositoryManagerBuilder<UnsetCobaltSender, N> {
    /// Use the given cobalt_sender in the [RepositoryManager].
    pub fn cobalt_sender(
        self,
        cobalt_sender: ProtocolSender<MetricEvent>,
    ) -> RepositoryManagerBuilder<ProtocolSender<MetricEvent>, N> {
        RepositoryManagerBuilder {
            dynamic_configs_path: self.dynamic_configs_path,
            persisted_repos_dir: self.persisted_repos_dir,
            static_configs: self.static_configs,
            dynamic_configs: self.dynamic_configs,
            cobalt_sender,
            inspect_node: self.inspect_node,
            tuf_timeouts: self.tuf_timeouts,
            data_proxy: self.data_proxy,
            delivery_blob_type: self.delivery_blob_type,
        }
    }
}

#[cfg(test)]
impl RepositoryManagerBuilder<UnsetCobaltSender, UnsetInspectNode> {
    /// In test configurations, allow building the [RepositoryManager] without configuring Inspect
    /// or Cobalt.
    pub fn build(self) -> RepositoryManager {
        let (sender, _) = futures::channel::mpsc::channel(0);
        let cobalt_sender = ProtocolSender::new(sender);
        let node = inspect::Inspector::default().root().create_child("test");
        self.cobalt_sender(cobalt_sender).inspect_node(node).build()
    }
}

#[cfg(test)]
impl RepositoryManagerBuilder<UnsetCobaltSender, inspect::Node> {
    /// In test configurations, allow building the [RepositoryManager] without configuring Cobalt.
    pub fn build(self) -> RepositoryManager {
        let (sender, _) = futures::channel::mpsc::channel(0);
        let cobalt_sender = ProtocolSender::new(sender);
        self.cobalt_sender(cobalt_sender).build()
    }
}

fn to_inspectable_map_with_node(
    from: HashMap<RepositoryUrl, Arc<RepositoryConfig>>,
    node: &inspect::Node,
) -> HashMap<RepositoryUrl, InspectableRepositoryConfig> {
    let mut out = HashMap::new();
    for (repo_url, repo_config) in from.into_iter() {
        let config = InspectableRepositoryConfig::new(repo_config, node, repo_url.host());
        out.insert(repo_url, config);
    }
    out
}

impl RepositoryManagerBuilder<ProtocolSender<MetricEvent>, inspect::Node> {
    /// Build the [RepositoryManager].
    pub fn build(self) -> RepositoryManager {
        self.inspect_node
            .record_uint("tuf_metadata_timeout_seconds", self.tuf_timeouts.metadata.as_secs());
        self.inspect_node.record_int(
            "tuf_network_header_timeout_seconds",
            self.tuf_timeouts.network_header.into_seconds(),
        );
        self.inspect_node
            .record_string("dynamic_configs_path", format!("{:?}", self.dynamic_configs_path));
        self.inspect_node
            .record_string("persisted_repos_dir", format!("{:?}", self.persisted_repos_dir));
        let inspect = RepositoryManagerInspectState {
            dynamic_configs_node: self.inspect_node.create_child("dynamic_configs"),
            static_configs_node: self.inspect_node.create_child("static_configs"),
            stats: Arc::new(Mutex::new(Stats::new(self.inspect_node.create_child("stats")))),
            repos_node: Arc::new(self.inspect_node.create_child("repos")),
            node: self.inspect_node,
        };

        RepositoryManager {
            dynamic_configs_path: self.dynamic_configs_path,
            persisted_repos_dir: Arc::new(self.persisted_repos_dir),
            static_configs: to_inspectable_map_with_node(
                self.static_configs,
                &inspect.static_configs_node,
            ),
            dynamic_configs: to_inspectable_map_with_node(
                self.dynamic_configs,
                &inspect.dynamic_configs_node,
            ),
            repositories: Arc::new(RwLock::new(HashMap::new())),
            cobalt_sender: self.cobalt_sender,
            inspect,
            tuf_timeouts: self.tuf_timeouts,
            data_proxy: self.data_proxy,
            delivery_blob_type: self.delivery_blob_type,
        }
    }
}

/// Load a directory of [RepositoryConfigs] files into a [RepositoryManager], or error out if we
/// encounter io errors during the load. It returns a [RepositoryManager], as well as all the
/// individual [LoadError] errors encountered during the load.
fn load_configs_dir<T: AsRef<Path>>(
    dir: T,
) -> (HashMap<RepositoryUrl, RepositoryConfig>, Vec<LoadError>) {
    let dir = dir.as_ref();

    let mut entries = match dir.read_dir() {
        Ok(entries) => {
            let entries: Result<Vec<_>, _> = entries.collect();

            match entries {
                Ok(entries) => entries,
                Err(err) => {
                    return (HashMap::new(), vec![LoadError::Io { path: dir.into(), error: err }]);
                }
            }
        }
        Err(err) => {
            return (HashMap::new(), vec![LoadError::Io { path: dir.into(), error: err }]);
        }
    };

    // Make sure we always process entries in order to make config loading order deterministic.
    entries.sort_by_key(|e| e.file_name());

    let mut map = HashMap::new();
    let mut errors = Vec::new();

    for entry in entries {
        let path = entry.path();

        // Skip over any directories in this path.
        match entry.file_type() {
            Ok(file_type) => {
                if !file_type.is_file() {
                    continue;
                }
            }
            Err(err) => {
                errors.push(LoadError::Io { path, error: err });
                continue;
            }
        }

        let expected_url = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| RepositoryUrl::parse_host(name.to_string()).ok());

        let configs = match load_configs_file(&path) {
            Ok(configs) => configs,
            Err(err) => {
                errors.push(err);
                continue;
            }
        };

        // Insert the configs in filename lexographical order, and treating any duplicated
        // configs as a recoverable error. As a special case, if the file the config comes from
        // happens to be named the same as the repository hostname, use that config over some
        // other config that came from some other file.
        for config in configs {
            match map.entry(config.repo_url().clone()) {
                Entry::Occupied(mut entry) => {
                    let replaced_config = if Some(entry.key()) == expected_url.as_ref() {
                        entry.insert(config)
                    } else {
                        config
                    };
                    errors.push(LoadError::Overridden { replaced_config });
                }
                Entry::Vacant(entry) => {
                    entry.insert(config);
                }
            }
        }
    }

    (map, errors)
}

fn load_configs_file<T: AsRef<Path>>(path: T) -> Result<Vec<RepositoryConfig>, LoadError> {
    let path = path.as_ref();
    match fs::File::open(path) {
        Ok(f) => match serde_json::from_reader(io::BufReader::new(f)) {
            Ok(RepositoryConfigs::Version1(configs)) => Ok(configs),
            Err(err) => Err(LoadError::Parse { path: path.into(), error: err }),
        },
        Err(err) => Err(LoadError::Io { path: path.into(), error: err }),
    }
}

async fn load_configs_file_from_proxy(
    proxy: &fio::DirectoryProxy,
    path: &str,
) -> Result<Vec<RepositoryConfig>, LoadError> {
    let file = match fuchsia_fs::directory::open_file(proxy, path, fio::PERM_READABLE).await {
        Ok(file) => file,
        Err(fuchsia_fs::node::OpenError::OpenError(Status::NOT_FOUND)) => return Ok(vec![]),
        Err(error) => return Err(LoadError::Open { path: path.into(), error }),
    };
    let buf = fuchsia_fs::file::read(&file)
        .await
        .map_err(|error| LoadError::Read { path: path.into(), error })?;

    match serde_json::from_slice(&buf) {
        Ok(RepositoryConfigs::Version1(configs)) => Ok(configs),
        Err(err) => Err(LoadError::Parse { path: path.into(), error: err }),
    }
}

/// [LoadError] describes all the recoverable error conditions that can be encountered when
/// parsing a [RepositoryConfigs] struct from a directory.
#[derive(Debug, Error)]
pub enum LoadError {
    /// This [std::io::Error] error occurred while reading the file.
    #[error("file {path} io error")]
    Io {
        path: PathBuf,
        #[source]
        error: io::Error,
    },

    /// This [fuchsia_fs::node::OpenError] error occurred while opening the file.
    #[error("file {path} open error")]
    Open {
        path: String,
        #[source]
        error: fuchsia_fs::node::OpenError,
    },

    /// This [fuchsia_fs::node::ReadError] error occurred while reading the file.
    #[error("file {path} read error")]
    Read {
        path: String,
        #[source]
        error: fuchsia_fs::file::ReadError,
    },

    /// This file failed to parse into a valid [RepositoryConfigs].
    #[error("file {path} failed to parse")]
    Parse {
        path: PathBuf,
        #[source]
        error: serde_json::Error,
    },

    /// This [RepositoryManager] already contains a config for this repo_url.
    #[error("repository config for {} was overridden", .replaced_config.repo_url())]
    Overridden { replaced_config: RepositoryConfig },
}

impl From<&LoadError> for metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult {
    fn from(error: &LoadError) -> Self {
        match error {
            LoadError::Io { .. } | LoadError::Open { .. } | LoadError::Read { .. } => {
                metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult::Io
            }
            LoadError::Parse { .. } => {
                metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult::Parse
            }
            LoadError::Overridden { .. } => {
                metrics::RepositoryManagerLoadStaticConfigsMigratedMetricDimensionResult::Overridden
            }
        }
    }
}

/// `List` is an iterator over all the [RepoConfig].
///
/// See its documentation for more.
pub struct List<'a> {
    keys: btree_set::IntoIter<&'a RepositoryUrl>,
    repo_mgr: &'a RepositoryManager,
}

impl<'a> Iterator for List<'a> {
    type Item = &'a RepositoryConfig;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(key) = self.keys.next() {
            self.repo_mgr.get(key).map(|config| &**config)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum InsertError {
    #[error("editing repository configs is permanently disabled")]
    DynamicConfigurationDisabled,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RemoveError {
    #[error("cannot remove static repositories")]
    CannotRemoveStaticRepositories,
    #[error("editing repository configs is permanently disabled")]
    DynamicConfigurationDisabled,
}

#[derive(Debug, Error)]
#[error("Could not create Repository for {repo_url}")]
pub struct OpenRepoError {
    repo_url: RepositoryUrl,
    #[source]
    source: anyhow::Error,
}

impl ToResolveStatus for OpenRepoError {
    fn to_resolve_status(&self) -> Status {
        Status::UNAVAILABLE
    }
}
impl ToResolveError for OpenRepoError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        pkg::ResolveError::UnavailableRepoMetadata
    }
}

#[derive(Debug, Error)]
pub enum GetPackageError {
    #[error(
        "the repository manager does not have a repository config for: {0}. Try running `ffx repository server start`. If that doesn't work, the package might be missing from the build or there could be a network issue."
    )]
    RepoNotFound(RepositoryUrl),

    #[error("while opening the repo")]
    OpenRepo(#[from] OpenRepoError),

    #[error("while caching the package")]
    Cache(#[from] CacheError),

    #[error("while opening the package")]
    OpenPackage(#[from] cache::OpenError),

    #[error("repository has no configured mirrors: {0}")]
    NoMirrors(RepositoryUrl),

    #[error("blob url error")]
    BlobUrl(#[source] http_uri_ext::Error),
}

#[derive(Debug, Error)]
pub enum GetPackageHashError {
    #[error(
        "the repository manager does not have a repository config for: {0}. Try running `ffx repository server start`. If that doesn't work, the package might be missing from the build or there could be a network issue."
    )]
    RepoNotFound(RepositoryUrl),

    #[error("while opening the repo")]
    OpenRepo(#[from] OpenRepoError),

    #[error("while getting the merkle")]
    MerkleFor(#[from] MerkleForError),

    #[error("repository has no configured mirrors: {0}")]
    NoMirrors(RepositoryUrl),

    #[error("blob url error")]
    BlobUrl(#[source] http_uri_ext::Error),
}

impl ToResolveError for GetPackageError {
    fn to_resolve_error(&self) -> pkg::ResolveError {
        match self {
            GetPackageError::RepoNotFound(_) => pkg::ResolveError::RepoNotFound,
            GetPackageError::OpenRepo(err) => err.to_resolve_error(),
            GetPackageError::Cache(err) => err.to_resolve_error(),
            GetPackageError::OpenPackage(err) => err.to_resolve_error(),
            GetPackageError::NoMirrors(_) => pkg::ResolveError::Internal,
            GetPackageError::BlobUrl(_) => pkg::ResolveError::Internal,
        }
    }
}

impl ToResolveStatus for GetPackageHashError {
    fn to_resolve_status(&self) -> Status {
        match self {
            // Not returning NOT_FOUND to be consistent with GetPackageError.
            GetPackageHashError::RepoNotFound(_) => Status::BAD_STATE,
            GetPackageHashError::OpenRepo(err) => err.to_resolve_status(),
            GetPackageHashError::MerkleFor(err) => err.to_resolve_status(),
            GetPackageHashError::NoMirrors(_) => Status::BAD_STATE,
            GetPackageHashError::BlobUrl(_) => Status::BAD_STATE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_pkg_ext::{MirrorConfigBuilder, RepositoryConfigBuilder, RepositoryKey};
    use http::Uri;
    use maplit::hashmap;
    use std::borrow::Borrow;
    use std::fs::File;
    use std::io::Write;

    const DYNAMIC_CONFIG_NAME: &str = "dynamic-config.json";

    struct TestEnvBuilder {
        static_configs: Option<Vec<(String, RepositoryConfigs)>>,
        dynamic_configs: Option<Option<RepositoryConfigs>>,
        persisted_repos: bool,
    }

    impl TestEnvBuilder {
        fn add_static_config(self, name: &str, config: RepositoryConfig) -> Self {
            self.add_static_configs(name, RepositoryConfigs::Version1(vec![config]))
        }

        fn add_static_configs(mut self, name: &str, configs: RepositoryConfigs) -> Self {
            self.static_configs.get_or_insert_with(Vec::new).push((name.into(), configs));
            self
        }

        fn with_empty_dynamic_configs(mut self) -> Self {
            self.dynamic_configs = Some(None);
            self
        }

        fn add_dynamic_configs(mut self, configs: RepositoryConfigs) -> Self {
            self.dynamic_configs = Some(Some(configs));
            self
        }

        fn with_persisted_repos(mut self) -> Self {
            self.persisted_repos = true;
            self
        }

        fn build(self) -> TestEnv {
            let static_configs = self.static_configs.map(|static_configs| {
                let dir = tempfile::tempdir().unwrap();

                for (name, configs) in static_configs.into_iter() {
                    let mut f = io::BufWriter::new(File::create(dir.path().join(name)).unwrap());
                    serde_json::to_writer(&mut f, &configs).unwrap();
                    f.flush().unwrap();
                }

                dir
            });

            let data_dir = tempfile::tempdir().unwrap();

            let dynamic_configs_path = self.dynamic_configs.map(|dynamic_configs| {
                let path = data_dir.path().join(DYNAMIC_CONFIG_NAME);

                if let Some(configs) = dynamic_configs {
                    let mut f = io::BufWriter::new(File::create(path).unwrap());
                    serde_json::to_writer(&mut f, &configs).unwrap();
                    f.flush().unwrap();
                }

                DYNAMIC_CONFIG_NAME.to_string()
            });

            let persisted_repos_dir =
                if self.persisted_repos { Some("repos".to_string()) } else { None };

            TestEnv { static_configs, dynamic_configs_path, persisted_repos_dir, data_dir }
        }
    }

    struct TestEnv {
        static_configs: Option<tempfile::TempDir>,
        dynamic_configs_path: Option<String>,
        persisted_repos_dir: Option<String>,
        data_dir: tempfile::TempDir,
    }

    impl TestEnv {
        fn builder() -> TestEnvBuilder {
            TestEnvBuilder { static_configs: None, dynamic_configs: None, persisted_repos: false }
        }

        fn new() -> Self {
            Self::builder().build()
        }

        async fn repo_manager(&self) -> Result<RepositoryManager, TestError> {
            let builder = self.repo_manager_builder().await.map_err(|(_, err)| err)?;
            Ok(builder.build())
        }

        #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255328)
        async fn repo_manager_builder(
            &self,
        ) -> Result<RepositoryManagerBuilder, (RepositoryManagerBuilder, TestError)> {
            let mut builder = RepositoryManagerBuilder::new_test(
                &self.data_dir,
                self.dynamic_configs_path.as_ref(),
            )
            .await
            .map_err(|(builder, err)| (builder, TestError::Constructor(err)))?;

            if let Some(ref dir) = self.static_configs {
                builder = builder
                    .load_static_configs_dir(dir.path())
                    .map_err(|(builder, errs)| (builder, TestError::LoadStaticConfigs(errs)))?;
            }

            if let Some(persisted_repos_dir) = self.persisted_repos_dir.as_ref() {
                builder = builder.with_persisted_repos_dir(persisted_repos_dir);
            }

            Ok(builder)
        }
    }

    #[derive(Debug)]
    pub(crate) enum TestError {
        Constructor(#[expect(dead_code)] LoadError),
        LoadStaticConfigs(Vec<LoadError>),
    }

    fn assert_does_not_exist_error(err: &LoadError, missing_path: &Path) {
        match &err {
            LoadError::Io { path, error } => {
                assert_eq!(path, missing_path);
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
            }
            err => {
                panic!("unexpected error: {err}");
            }
        }
    }

    fn assert_parse_error(err: &LoadError, invalid_path: impl Into<PathBuf>) {
        match err {
            LoadError::Parse { path, .. } => {
                assert_eq!(path, &invalid_path.into());
            }
            err => {
                panic!("unexpected error: {err}");
            }
        }
    }

    fn assert_overridden_error(err: &LoadError, config: &RepositoryConfig) {
        match err {
            LoadError::Overridden { replaced_config } => {
                assert_eq!(replaced_config, config);
            }
            err => {
                panic!("unexpected error: {err}");
            }
        }
    }

    fn to_inspectable_map(
        from: HashMap<RepositoryUrl, Arc<RepositoryConfig>>,
    ) -> HashMap<RepositoryUrl, InspectableRepositoryConfig> {
        let inspector = inspect::Inspector::default();
        to_inspectable_map_with_node(from, inspector.root())
    }

    #[fuchsia::test]
    async fn test_insert_get_remove() {
        let env = TestEnv::builder().with_empty_dynamic_configs().build();
        let mut repomgr = env.repo_manager().await.unwrap();
        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs, HashMap::new());

        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        assert_eq!(repomgr.get(&fuchsia_url), None);

        let config1 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );
        let config2 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![1]))
                .build(),
        );

        assert_eq!(repomgr.insert(Arc::clone(&config1)).await, Ok(None));
        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(
            repomgr.dynamic_configs,
            to_inspectable_map(hashmap! { fuchsia_url.clone() => Arc::clone(&config1) })
        );

        assert_eq!(repomgr.insert(Arc::clone(&config2)).await, Ok(Some(config1)));
        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(
            repomgr.dynamic_configs,
            to_inspectable_map(hashmap! { fuchsia_url.clone() => Arc::clone(&config2) })
        );

        assert_eq!(repomgr.get(&fuchsia_url), Some(&config2));
        assert_eq!(repomgr.remove(&fuchsia_url).await, Ok(Some(config2)));
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
        assert_eq!(repomgr.remove(&fuchsia_url).await, Ok(None));
    }

    #[fuchsia::test]
    async fn shadowing_static_config() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let fuchsia_config1 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![1]))
                .build(),
        );

        let fuchsia_config2 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![2]))
                .build(),
        );

        let env = TestEnv::builder()
            .add_static_config("fuchsia.com.json", (*fuchsia_config1).clone())
            .with_empty_dynamic_configs()
            .build();

        let mut repomgr = env.repo_manager().await.unwrap();

        assert_eq!(repomgr.get(&fuchsia_url), Some(&fuchsia_config1));
        assert_eq!(repomgr.insert(Arc::clone(&fuchsia_config2)).await, Ok(None));
        assert_eq!(repomgr.get(&fuchsia_url), Some(&fuchsia_config2));
        assert_eq!(repomgr.remove(&fuchsia_url).await, Ok(Some(fuchsia_config2)));
        assert_eq!(repomgr.get(&fuchsia_url), Some(&fuchsia_config1));
    }

    #[fuchsia::test]
    async fn cannot_remove_static_config() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let fuchsia_config1 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![1]))
                .build(),
        );

        let env = TestEnv::builder()
            .add_static_config("fuchsia.com.json", (*fuchsia_config1).clone())
            .with_empty_dynamic_configs()
            .build();

        let mut repomgr = env.repo_manager().await.unwrap();

        assert_eq!(repomgr.get(&fuchsia_url), Some(&fuchsia_config1));
        assert_eq!(
            repomgr.remove(&fuchsia_url).await,
            Err(RemoveError::CannotRemoveStaticRepositories)
        );
        assert_eq!(repomgr.get(&fuchsia_url), Some(&fuchsia_config1));
    }

    #[fuchsia::test]
    async fn test_builder_static_configs_dir_not_exists() {
        let dynamic_dir = tempfile::tempdir().unwrap();

        let static_dir = tempfile::tempdir().unwrap();
        let does_not_exist_dir = static_dir.path().join("not-exists");

        let (_, errors) = RepositoryManagerBuilder::new_test(&dynamic_dir, Some("config"))
            .await
            .unwrap()
            .load_static_configs_dir(&does_not_exist_dir)
            .unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_does_not_exist_error(&errors[0], &does_not_exist_dir);
    }

    #[fuchsia::test]
    async fn test_builder_static_configs_dir_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_path = dir.path().join("invalid");

        let example_url = RepositoryUrl::parse("fuchsia-pkg://example.com").unwrap();
        let example_config = RepositoryConfigBuilder::new(example_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![0]))
            .build();

        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        let fuchsia_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![1]))
            .build();

        {
            let mut f = File::create(&invalid_path).unwrap();
            f.write_all(b"hello world").unwrap();

            let mut f = io::BufWriter::new(File::create(dir.path().join("a")).unwrap());
            serde_json::to_writer(
                &mut f,
                &RepositoryConfigs::Version1(vec![example_config.clone()]),
            )
            .unwrap();
            f.flush().unwrap();

            let mut f = io::BufWriter::new(File::create(dir.path().join("z")).unwrap());
            serde_json::to_writer(
                &mut f,
                &RepositoryConfigs::Version1(vec![fuchsia_config.clone()]),
            )
            .unwrap();
            f.flush().unwrap();
        }

        let dynamic_dir = tempfile::tempdir().unwrap();
        let dynamic_configs_path = "config".to_string();

        let (builder, errors) =
            RepositoryManagerBuilder::new_test(&dynamic_dir, Some(&dynamic_configs_path))
                .await
                .unwrap()
                .load_static_configs_dir(dir.path())
                .unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_parse_error(&errors[0], &invalid_path);
        let repomgr = builder.build();

        assert_eq!(repomgr.dynamic_configs_path, Some(dynamic_configs_path));
        assert_eq!(
            repomgr.static_configs,
            to_inspectable_map(hashmap! {
                example_url => example_config.into(),
                fuchsia_url => fuchsia_config.into(),
            })
        );
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_static_configs_dir() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        let fuchsia_config = RepositoryConfigBuilder::new(fuchsia_url.clone()).build();

        let example_url = RepositoryUrl::parse("fuchsia-pkg://example.com").unwrap();
        let example_config = RepositoryConfigBuilder::new(example_url.clone()).build();

        let env = TestEnv::builder()
            .add_static_config("example.com.json", example_config.clone())
            .add_static_config("fuchsia.com.json", fuchsia_config.clone())
            .build();

        let repomgr = env.repo_manager_builder().await.unwrap().build();

        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(
            repomgr.static_configs,
            to_inspectable_map(hashmap! {
                example_url => example_config.into(),
                fuchsia_url => fuchsia_config.into(),
            })
        );
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_static_configs_dir_overlapping_filename_wins() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let fuchsia_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![0]))
            .build();

        let fuchsia_com_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![1]))
            .build();

        let fuchsia_com_json_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![2]))
            .build();

        let example_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![3]))
            .build();

        let oem_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![4]))
            .build();

        // Even though the example file comes first, the fuchsia repo should take priority over the
        // example file.
        let env = TestEnv::builder()
            .add_static_config("fuchsia", fuchsia_config.clone())
            .add_static_config("fuchsia.com", fuchsia_com_config.clone())
            .add_static_config("example.com.json", example_config.clone())
            .add_static_configs(
                "fuchsia.com.json",
                RepositoryConfigs::Version1(vec![
                    oem_config.clone(),
                    fuchsia_com_json_config.clone(),
                ]),
            )
            .build();

        let builder = match env.repo_manager_builder().await {
            Err((builder, TestError::LoadStaticConfigs(errors))) => {
                assert_eq!(errors.len(), 4);
                assert_overridden_error(&errors[0], &fuchsia_config);
                assert_overridden_error(&errors[1], &fuchsia_com_config);
                assert_overridden_error(&errors[2], &example_config);
                assert_overridden_error(&errors[3], &oem_config);
                builder
            }
            res => panic!("unexpected result: {res:?}"),
        };

        let repomgr = builder.build();

        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(
            repomgr.static_configs,
            to_inspectable_map(hashmap! { fuchsia_url => fuchsia_com_json_config.into() })
        );
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_static_configs_dir_overlapping_first_wins() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let fuchsia_config1 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );

        let fuchsia_config2 = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![1]))
                .build(),
        );

        // Even though the example file comes first, the fuchsia repo should take priority over the
        // example file.
        let env = TestEnv::builder()
            .add_static_config("1", (*fuchsia_config1).clone())
            .add_static_config("2", (*fuchsia_config2).clone())
            .build();

        let builder = match env.repo_manager_builder().await {
            Err((builder, TestError::LoadStaticConfigs(errors))) => {
                assert_eq!(errors.len(), 1);
                assert_overridden_error(&errors[0], &fuchsia_config2);
                builder
            }
            res => {
                panic!("unexpected result: {res:?}");
            }
        };

        let repomgr = builder.build();

        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(
            repomgr.static_configs,
            to_inspectable_map(hashmap! { fuchsia_url => fuchsia_config1 })
        );
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_dynamic_configs_path_ignores_if_not_exists() {
        let dynamic_dir = tempfile::tempdir().unwrap();
        let dynamic_configs_path = "config".to_string();
        let repomgr = RepositoryManagerBuilder::new_test(&dynamic_dir, Some(&dynamic_configs_path))
            .await
            .unwrap()
            .build();

        assert_eq!(repomgr.dynamic_configs_path, Some(dynamic_configs_path));
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_dynamic_configs_path_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_path = "invalid".to_string();

        {
            let mut f = File::create(dir.path().join(&invalid_path)).unwrap();
            f.write_all(b"hello world").unwrap();
        }

        let (builder, err) =
            RepositoryManagerBuilder::new_test(&dir, Some(&invalid_path)).await.unwrap_err();
        assert_parse_error(&err, &invalid_path);
        let repomgr = builder.build();

        assert_eq!(repomgr.dynamic_configs_path, Some(invalid_path));
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
    }

    #[fuchsia::test]
    async fn test_builder_dynamic_configs_path() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );

        let env = TestEnv::builder()
            .add_dynamic_configs(RepositoryConfigs::Version1(vec![(*config).clone()]))
            .build();
        let repomgr = env.repo_manager().await.unwrap();

        assert_eq!(repomgr.dynamic_configs_path, env.dynamic_configs_path);
        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs, to_inspectable_map(hashmap! { fuchsia_url => config }));
    }

    #[fuchsia::test]
    async fn test_persistence() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();

        let static_config = RepositoryConfigBuilder::new(fuchsia_url.clone())
            .add_root_key(RepositoryKey::Ed25519(vec![1]))
            .build();

        let old_dynamic_config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![2]))
                .build(),
        );
        let old_dynamic_configs = RepositoryConfigs::Version1(vec![(*old_dynamic_config).clone()]);

        let env = TestEnv::builder()
            .add_static_config("config", static_config.clone())
            .add_dynamic_configs(old_dynamic_configs.clone())
            .build();
        let dynamic_configs_path =
            env.data_dir.path().join(env.dynamic_configs_path.as_ref().unwrap());

        let mut repomgr = env.repo_manager().await.unwrap();

        // make sure the dynamic config file didn't change just from opening it.
        let f = File::open(&dynamic_configs_path).unwrap();
        let actual: RepositoryConfigs = serde_json::from_reader(io::BufReader::new(f)).unwrap();
        assert_eq!(actual, old_dynamic_configs);

        let new_dynamic_config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![3]))
                .build(),
        );
        let new_dynamic_configs = RepositoryConfigs::Version1(vec![(*new_dynamic_config).clone()]);

        // Inserting a new repo should update the config file.
        assert_eq!(
            repomgr.insert(Arc::clone(&new_dynamic_config)).await,
            Ok(Some(old_dynamic_config))
        );
        let f = File::open(&dynamic_configs_path).unwrap();
        let actual: RepositoryConfigs = serde_json::from_reader(io::BufReader::new(f)).unwrap();
        assert_eq!(actual, new_dynamic_configs);

        // Removing the repo should empty out the file.
        assert_eq!(repomgr.remove(&fuchsia_url).await, Ok(Some(new_dynamic_config)));
        let f = File::open(&dynamic_configs_path).unwrap();
        let actual: RepositoryConfigs = serde_json::from_reader(io::BufReader::new(f)).unwrap();
        assert_eq!(actual, RepositoryConfigs::Version1(vec![]));

        // We should now be back to the static config.
        assert_eq!(repomgr.get(&fuchsia_url), Some(&static_config.into()));
        assert_eq!(
            repomgr.remove(&fuchsia_url).await,
            Err(RemoveError::CannotRemoveStaticRepositories)
        );
    }

    #[fuchsia::test]
    async fn test_list_empty() {
        let env = TestEnv::new();
        let repomgr = env.repo_manager().await.unwrap();

        assert_eq!(repomgr.list().collect::<Vec<_>>(), Vec::<&RepositoryConfig>::new());
    }

    #[fuchsia::test]
    async fn test_list() {
        let example_url = RepositoryUrl::parse("fuchsia-pkg://example.com").unwrap();
        let example_config = RepositoryConfigBuilder::new(example_url).build();

        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        let fuchsia_config = RepositoryConfigBuilder::new(fuchsia_url).build();

        let env = TestEnv::builder()
            .add_static_config("example.com", example_config.clone())
            .add_static_config("fuchsia.com", fuchsia_config.clone())
            .build();

        let repomgr = env.repo_manager().await.unwrap();

        assert_eq!(repomgr.list().collect::<Vec<_>>(), vec![&example_config, &fuchsia_config,]);
    }

    #[fuchsia::test]
    async fn test_no_dynamic_repos_if_no_dynamic_repo_path() {
        let repomgr =
            RepositoryManagerBuilder::new_test(&tempfile::tempdir().unwrap(), Option::<&str>::None)
                .await
                .unwrap()
                .build();

        assert_eq!(repomgr.static_configs, HashMap::new());
        assert_eq!(repomgr.dynamic_configs_path, None);
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
        assert_eq!(repomgr.list().collect::<Vec<_>>(), Vec::<&RepositoryConfig>::new());
    }

    #[fuchsia::test]
    async fn test_insert_fails_with_no_change_if_no_dynamic_config_path() {
        let mut repomgr =
            RepositoryManagerBuilder::new_test(&tempfile::tempdir().unwrap(), Option::<&str>::None)
                .await
                .unwrap()
                .build();
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );

        let res = repomgr.insert(config).await;

        assert_eq!(res, Err(InsertError::DynamicConfigurationDisabled));
        assert_eq!(repomgr.dynamic_configs, HashMap::new());
        assert_eq!(repomgr.list().collect::<Vec<_>>(), Vec::<&RepositoryConfig>::new());
    }

    #[fuchsia::test]
    async fn test_remove_fails_with_no_change_if_no_dynamic_config_path() {
        let mut repomgr =
            RepositoryManagerBuilder::new_test(&tempfile::tempdir().unwrap(), Option::<&str>::None)
                .await
                .unwrap()
                .build();
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.com").unwrap();
        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .build(),
        );

        let insp_config = InspectableRepositoryConfig::new(
            Arc::clone(&config),
            inspect::Inspector::default().root(),
            "foo",
        );
        assert_eq!(repomgr.dynamic_configs.insert(fuchsia_url.clone(), insp_config), None);

        let res = repomgr.remove(&fuchsia_url).await;

        assert_eq!(res, Err(RemoveError::DynamicConfigurationDisabled));
        assert_eq!(
            repomgr.dynamic_configs,
            to_inspectable_map(hashmap! { fuchsia_url.clone() => Arc::clone(&config) })
        );
        assert_eq!(repomgr.list().collect::<Vec<_>>(), vec![config.borrow()]);
    }

    #[fuchsia::test]
    async fn test_building_repo_manager_with_static_configs_populates_inspect() {
        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.test").unwrap();
        let mirror_config =
            MirrorConfigBuilder::new("http://fake-mirror.com".parse::<Uri>().unwrap())
                .unwrap()
                .build();
        let fuchsia_config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![1]))
                .add_mirror(mirror_config.clone())
                .build(),
        );

        let env = TestEnv::builder()
            .add_static_config("fuchsia.com.json", (*fuchsia_config).clone())
            .build();

        let inspector = fuchsia_inspect::Inspector::default();

        let _repomgr = env
            .repo_manager_builder()
            .await
            .unwrap()
            .inspect_node(inspector.root().create_child("repository_manager"))
            .build();

        assert_data_tree!(
            inspector,
            root: {
                repository_manager: {
                    dynamic_configs_path: format!("{:?}", env.dynamic_configs_path),
                    dynamic_configs: {},
                    static_configs: {
                        "fuchsia.test": {
                            root_keys: {
                                "0": format!("{:?}", fuchsia_config.root_keys()[0])
                            },
                            mirrors: {
                                "0": {
                                    mirror_url: format!("{:?}", mirror_config.mirror_url()),
                                    subscribe: format!("{:?}", mirror_config.subscribe()),
                                    blob_mirror_url: format!("{:?}", mirror_config.blob_mirror_url())
                                }
                            },
                            repo_storage_type: "Ephemeral",
                            repo_url: "fuchsia-pkg://fuchsia.test",
                            root_threshold: 1u64,
                            root_version: 1u64,
                            use_local_mirror: false,
                        }
                    },
                    stats: {
                        mirrors: {},
                    },
                    repos: {},
                    persisted_repos_dir: format!("{:?}", env.persisted_repos_dir),
                    tuf_metadata_timeout_seconds: 30u64,
                    tuf_network_header_timeout_seconds: 30i64,
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_building_repo_manager_with_no_static_configs_populates_inspect() {
        let env = TestEnv::builder().with_empty_dynamic_configs().with_persisted_repos().build();

        let inspector = fuchsia_inspect::Inspector::default();

        let _repomgr = env
            .repo_manager_builder()
            .await
            .unwrap()
            .inspect_node(inspector.root().create_child("repository_manager"))
            .build();

        assert_data_tree!(
            inspector,
            root: {
                repository_manager: {
                    dynamic_configs_path: format!("{:?}", env.dynamic_configs_path),
                    dynamic_configs: {},
                    static_configs: {},
                    stats: {
                        mirrors: {},
                    },
                    repos: {},
                    persisted_repos_dir: format!("{:?}", env.persisted_repos_dir),
                    tuf_metadata_timeout_seconds: 30u64,
                    tuf_network_header_timeout_seconds: 30i64,
                }
            }
        );
    }

    #[fuchsia::test]
    async fn test_insert_remove_updates_inspect() {
        let env = TestEnv::builder().with_empty_dynamic_configs().with_persisted_repos().build();

        let inspector = fuchsia_inspect::Inspector::default();

        let mut repomgr = env
            .repo_manager_builder()
            .await
            .unwrap()
            .inspect_node(inspector.root().create_child("repository_manager"))
            .build();

        assert_data_tree!(
            inspector,
            root: {
                repository_manager: contains {
                    dynamic_configs: {},
                }
            }
        );

        let fuchsia_url = RepositoryUrl::parse("fuchsia-pkg://fuchsia.test").unwrap();
        let mirror_config =
            MirrorConfigBuilder::new("http://fake-mirror.com".parse::<Uri>().unwrap())
                .unwrap()
                .build();
        let config = Arc::new(
            RepositoryConfigBuilder::new(fuchsia_url.clone())
                .add_root_key(RepositoryKey::Ed25519(vec![0]))
                .add_mirror(mirror_config.clone())
                .build(),
        );

        // Insert and make sure inspect state is updated
        repomgr.insert(Arc::clone(&config)).await.expect("insert worked");

        assert_data_tree!(
            inspector,
            root: {
                repository_manager: contains {
                    dynamic_configs: {
                        "fuchsia.test": {
                            root_keys: {
                                "0": format!("{:?}", config.root_keys()[0])
                            },
                            mirrors: {
                                "0": {
                                    mirror_url: format!("{:?}", mirror_config.mirror_url()),
                                    subscribe: format!("{:?}", mirror_config.subscribe()),
                                    blob_mirror_url: format!("{:?}", mirror_config.blob_mirror_url())
                                }
                            },
                            repo_storage_type: "Ephemeral",
                            repo_url: "fuchsia-pkg://fuchsia.test",
                            root_threshold: 1u64,
                            root_version: 1u64,
                            use_local_mirror: false,
                        }
                    },
                }
            }
        );

        // Remove and make sure inspect state is updated
        repomgr.remove(&fuchsia_url).await.expect("remove worked");
        assert_data_tree!(
            inspector,
            root: {
                repository_manager: contains {
                    dynamic_configs: {},
                }
            }
        );
    }
}
