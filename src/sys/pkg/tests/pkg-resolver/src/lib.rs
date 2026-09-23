// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

use assert_matches::assert_matches;
use blobfs_ramdisk::BlobfsRamdisk;
use cobalt_client::traits::AsEventCodes;
use diagnostics_assertions::TreeAssertion;
use diagnostics_hierarchy::DiagnosticsHierarchy;
use diagnostics_reader::{ArchiveReader, ComponentSelector};
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker as _};
use fidl::persist;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics::{self as fmetrics, MetricEvent, MetricEventPayload};
use fidl_fuchsia_net_http as fnet_http;
use fidl_fuchsia_pkg::{
    self as fpkg, CupProxy, GetInfoError, PackageResolverMarker, PackageResolverProxy,
    RepositoryManagerProxy, WriteError,
};
use fidl_fuchsia_pkg_ext::{
    self as pkg, RepositoryConfig, RepositoryConfigBuilder, RepositoryConfigs,
};
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fidl_fuchsia_pkg_http as fpkg_http;
use fidl_fuchsia_pkg_internal as fpkg_internal;
use fidl_fuchsia_pkg_internal::{PersistentEagerPackage, PersistentEagerPackages};
use fidl_fuchsia_pkg_rewrite as fpkg_rewrite;
use fidl_fuchsia_pkg_rewrite_ext::{Rule, RuleConfig};
use fidl_fuchsia_sys2 as fsys2;
use fidl_fuchsia_update as fupdate;
use fuchsia_async as fasync;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route, ScopedInstance,
};
use fuchsia_merkle::Hash;
use fuchsia_pkg_testing::serve::ServedRepository;
use fuchsia_pkg_testing::{Package, PackageBuilder};
use fuchsia_sync::Mutex;
use fuchsia_url::RepositoryUrl;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use futures::prelude::*;
use mock_metrics::MockMetricEventLoggerFactory;
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use vfs::directory::helper::DirectlyMutable as _;

// If the body of an https response is not large enough, hyper will download the body
// along with the header in the initial fuchsia_hyper::HttpsClient.request(). This means
// that even if the body is implemented with a stream that sends some bytes and then fails
// before the transfer is complete, the error will occur on the initial request instead
// of when looping over the Response body bytes.
// This value probably just needs to be larger than the Hyper buffer, which defaults to 400 kB
// https://docs.rs/hyper/0.13.10/hyper/client/struct.Builder.html#method.http1_max_buf_size
pub const FILE_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING: usize = 600_000;

static PKG_RESOLVER_CHILD_NAME: &str = "pkg_resolver";

pub mod mock_filesystem;

pub trait Blobfs {
    fn root_dir_handle(&self) -> ClientEnd<fio::DirectoryMarker>;
    fn svc_dir(&self) -> fio::DirectoryProxy;
}

impl Blobfs for BlobfsRamdisk {
    fn root_dir_handle(&self) -> ClientEnd<fio::DirectoryMarker> {
        self.root_dir_handle().unwrap()
    }
    fn svc_dir(&self) -> fio::DirectoryProxy {
        self.svc_dir().unwrap()
    }
}

pub struct Mounts {
    pkg_resolver_data: DirOrProxy,
    pkg_resolver_config_data: DirOrProxy,
}

#[derive(Serialize)]
pub struct EnableDynamicConfig {
    pub enable_dynamic_configuration: bool,
}

#[derive(Serialize)]
pub struct PersistedReposConfig {
    pub persisted_repos_dir: String,
}

#[derive(Default)]
pub struct MountsBuilder {
    pkg_resolver_data: Option<DirOrProxy>,
    pkg_resolver_config_data: Option<DirOrProxy>,
    enable_dynamic_config: Option<EnableDynamicConfig>,
    static_repository: Option<RepositoryConfig>,
    dynamic_rewrite_rules: Option<RuleConfig>,
    dynamic_repositories: Option<RepositoryConfigs>,
    custom_config_data: Vec<(PathBuf, String)>,
    persisted_repos_config: Option<PersistedReposConfig>,
    eager_packages: Vec<(PinnedAbsolutePackageUrl, pkg::CupData)>,
}

impl MountsBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pkg_resolver_data(mut self, pkg_resolver_data: DirOrProxy) -> Self {
        self.pkg_resolver_data = Some(pkg_resolver_data);
        self
    }
    pub fn pkg_resolver_config_data(mut self, pkg_resolver_config_data: DirOrProxy) -> Self {
        self.pkg_resolver_config_data = Some(pkg_resolver_config_data);
        self
    }
    pub fn enable_dynamic_config(mut self, config: EnableDynamicConfig) -> Self {
        self.enable_dynamic_config = Some(config);
        self
    }
    pub fn persisted_repos_config(mut self, config: PersistedReposConfig) -> Self {
        self.persisted_repos_config = Some(config);
        self
    }
    pub fn static_repository(mut self, static_repository: RepositoryConfig) -> Self {
        self.static_repository = Some(static_repository);
        self
    }
    pub fn dynamic_rewrite_rules(mut self, dynamic_rewrite_rules: RuleConfig) -> Self {
        self.dynamic_rewrite_rules = Some(dynamic_rewrite_rules);
        self
    }
    pub fn dynamic_repositories(mut self, dynamic_repositories: RepositoryConfigs) -> Self {
        self.dynamic_repositories = Some(dynamic_repositories);
        self
    }
    /// Injects a file with custom contents into /config/data. Panics if file already exists.
    pub fn custom_config_data(mut self, path: impl Into<PathBuf>, data: impl Into<String>) -> Self {
        self.custom_config_data.push((path.into(), data.into()));
        self
    }
    pub fn eager_packages(
        mut self,
        package_urls: Vec<(PinnedAbsolutePackageUrl, pkg::CupData)>,
    ) -> Self {
        assert!(self.eager_packages.is_empty());
        self.eager_packages = package_urls;
        self
    }

    pub fn build(self) -> Mounts {
        let mounts = Mounts {
            pkg_resolver_data: self
                .pkg_resolver_data
                .unwrap_or_else(|| DirOrProxy::Dir(tempfile::tempdir().expect("/tmp to exist"))),
            pkg_resolver_config_data: self
                .pkg_resolver_config_data
                .unwrap_or_else(|| DirOrProxy::Dir(tempfile::tempdir().expect("/tmp to exist"))),
        };
        if let Some(config) = self.enable_dynamic_config {
            mounts.add_enable_dynamic_config(&config);
        }
        if let Some(config) = self.persisted_repos_config {
            mounts.add_persisted_repos_config(&config);
        }
        if let Some(config) = self.static_repository {
            mounts.add_static_repository(config);
        }
        if let Some(config) = self.dynamic_rewrite_rules {
            mounts.add_dynamic_rewrite_rules(&config);
        }
        if let Some(config) = self.dynamic_repositories {
            mounts.add_dynamic_repositories(&config);
        }
        for (path, data) in self.custom_config_data {
            mounts.add_custom_config_data(path, data);
        }
        if !self.eager_packages.is_empty() {
            mounts.add_eager_packages(self.eager_packages);
        }
        mounts
    }
}

impl Mounts {
    fn add_enable_dynamic_config(&self, config: &EnableDynamicConfig) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_config_data {
            let mut f = BufWriter::new(File::create(d.path().join("config.json")).unwrap());
            serde_json::to_writer(&mut f, &config).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }

    fn add_persisted_repos_config(&self, config: &PersistedReposConfig) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_config_data {
            let mut f =
                BufWriter::new(File::create(d.path().join("persisted_repos_dir.json")).unwrap());
            serde_json::to_writer(&mut f, &config).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }

    fn add_static_repository(&self, config: RepositoryConfig) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_config_data {
            let static_repo_path = d.path().join("repositories");
            if !static_repo_path.exists() {
                std::fs::create_dir(&static_repo_path).unwrap();
            }
            let mut f = BufWriter::new(
                File::create(static_repo_path.join(format!("{}.json", config.repo_url().host())))
                    .unwrap(),
            );
            serde_json::to_writer(&mut f, &RepositoryConfigs::Version1(vec![config])).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }

    fn add_dynamic_rewrite_rules(&self, rule_config: &RuleConfig) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_data {
            let mut f = BufWriter::new(File::create(d.path().join("rewrites.json")).unwrap());
            serde_json::to_writer(&mut f, rule_config).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }
    fn add_dynamic_repositories(&self, repo_configs: &RepositoryConfigs) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_data {
            let mut f = BufWriter::new(File::create(d.path().join("repositories.json")).unwrap());
            serde_json::to_writer(&mut f, repo_configs).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }

    fn add_custom_config_data(&self, path: impl AsRef<Path>, data: String) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_config_data {
            let path = d.path().join(path);
            assert!(!path.exists());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, data).unwrap();
        } else {
            panic!("not supported");
        }
    }

    fn add_eager_packages(&self, packages: Vec<(PinnedAbsolutePackageUrl, pkg::CupData)>) {
        if let DirOrProxy::Dir(ref d) = self.pkg_resolver_data {
            let mut f = BufWriter::new(File::create(d.path().join("eager_packages.pf")).unwrap());

            let packages = PersistentEagerPackages {
                packages: Some(
                    packages
                        .into_iter()
                        .map(|(url, cup)| {
                            let pkg_url = fpkg::PackageUrl { url: url.as_unpinned().to_string() };
                            PersistentEagerPackage {
                                url: Some(pkg_url),
                                cup: Some(cup.into()),
                                ..Default::default()
                            }
                        })
                        .collect(),
                ),
                ..Default::default()
            };

            let data = persist(&packages).unwrap();
            f.write_all(&data).unwrap();
            f.flush().unwrap();
        } else {
            panic!("not supported");
        }
    }
}

pub enum DirOrProxy {
    Dir(TempDir),
    Proxy(fio::DirectoryProxy),
}

impl DirOrProxy {
    fn to_proxy(&self, rights: fio::Rights) -> fio::DirectoryProxy {
        let flags = fio::Flags::from_bits(rights.bits()).unwrap();
        match self {
            DirOrProxy::Dir(temp_dir) => {
                let path = temp_dir.path().to_str().unwrap();
                fuchsia_fs::directory::open_in_namespace(path, flags).unwrap()
            }
            DirOrProxy::Proxy(proxy) => {
                fuchsia_fs::directory::open_directory_async(proxy, ".", flags).unwrap()
            }
        }
    }
}
pub struct TestEnvBuilder<BlobfsAndSystemImageFut, MountsFn> {
    blobfs_and_system_image:
        Box<dyn FnOnce(blobfs_ramdisk::Implementation) -> BlobfsAndSystemImageFut>,
    mounts: MountsFn,
    tuf_metadata_timeout_seconds: Option<u32>,
    tuf_network_header_timeout_seconds: Option<u32>,
    blob_network_header_timeout_seconds: Option<u32>,
    blob_network_body_timeout_seconds: Option<u32>,
    blob_download_resumption_attempts_limit: Option<u32>,
    blob_implementation: Option<blobfs_ramdisk::Implementation>,
    blob_download_concurrency_limit: Option<u16>,
}

impl TestEnvBuilder<future::BoxFuture<'static, (BlobfsRamdisk, Option<Hash>)>, fn() -> Mounts> {
    #![allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            blobfs_and_system_image: Box::new(|blob_impl| {
                async move {
                    let system_image_package =
                        fuchsia_pkg_testing::SystemImageBuilder::new().build().await;
                    let blobfs =
                        BlobfsRamdisk::builder().implementation(blob_impl).start().await.unwrap();
                    let () = system_image_package.write_to_blobfs(&blobfs).await;
                    (blobfs, Some(*system_image_package.hash()))
                }
                .boxed()
            }),
            // If it's not overridden, the default state of the mounts allows for dynamic
            // configuration. We do this because in the majority of tests, we'll want to use
            // dynamic repos and rewrite rules.
            // Note: this means that we'll produce different envs from
            // TestEnvBuilder::new().build().await
            // vs TestEnvBuilder::new().mounts(MountsBuilder::new().build()).build()
            mounts: || {
                MountsBuilder::new()
                    .enable_dynamic_config(EnableDynamicConfig {
                        enable_dynamic_configuration: true,
                    })
                    .build()
            },
            tuf_metadata_timeout_seconds: None,
            tuf_network_header_timeout_seconds: None,
            blob_network_header_timeout_seconds: None,
            blob_network_body_timeout_seconds: None,
            blob_download_resumption_attempts_limit: None,
            blob_implementation: None,
            blob_download_concurrency_limit: None,
        }
    }
}

impl<BlobfsAndSystemImageFut, ConcreteBlobfs, MountsFn>
    TestEnvBuilder<BlobfsAndSystemImageFut, MountsFn>
where
    BlobfsAndSystemImageFut: Future<Output = (ConcreteBlobfs, Option<Hash>)>,
    ConcreteBlobfs: Blobfs,
    MountsFn: FnOnce() -> Mounts,
{
    pub fn blobfs_and_system_image_hash<OtherBlobfs>(
        self,
        blobfs: OtherBlobfs,
        system_image: Option<Hash>,
    ) -> TestEnvBuilder<future::Ready<(OtherBlobfs, Option<Hash>)>, MountsFn>
    where
        OtherBlobfs: Blobfs + 'static,
    {
        TestEnvBuilder::<_, MountsFn> {
            blobfs_and_system_image: Box::new(move |_| future::ready((blobfs, system_image))),
            mounts: self.mounts,
            tuf_metadata_timeout_seconds: self.tuf_metadata_timeout_seconds,
            tuf_network_header_timeout_seconds: self.tuf_network_header_timeout_seconds,
            blob_network_header_timeout_seconds: self.blob_network_header_timeout_seconds,
            blob_network_body_timeout_seconds: self.blob_network_body_timeout_seconds,
            blob_download_resumption_attempts_limit: self.blob_download_resumption_attempts_limit,
            blob_implementation: self.blob_implementation,
            blob_download_concurrency_limit: self.blob_download_concurrency_limit,
        }
    }

    /// Creates a BlobfsRamdisk loaded with the supplied packages and configures the system to use
    /// the supplied `system_image` package.
    /// Sets the blob implementation to Blobfs.
    pub async fn system_image_and_extra_packages(
        self,
        system_image: &Package,
        extra_packages: &[&Package],
    ) -> TestEnvBuilder<future::Ready<(BlobfsRamdisk, Option<Hash>)>, MountsFn> {
        assert_eq!(self.blob_implementation, None);
        let blobfs = BlobfsRamdisk::start().await.unwrap();
        let () = system_image.write_to_blobfs(&blobfs).await;
        for pkg in extra_packages {
            let () = pkg.write_to_blobfs(&blobfs).await;
        }
        let system_image_hash = *system_image.hash();

        TestEnvBuilder::<_, MountsFn> {
            blobfs_and_system_image: Box::new(move |_| {
                future::ready((blobfs, Some(system_image_hash)))
            }),
            mounts: self.mounts,
            tuf_metadata_timeout_seconds: self.tuf_metadata_timeout_seconds,
            tuf_network_header_timeout_seconds: self.tuf_network_header_timeout_seconds,
            blob_network_header_timeout_seconds: self.blob_network_header_timeout_seconds,
            blob_network_body_timeout_seconds: self.blob_network_body_timeout_seconds,
            blob_download_resumption_attempts_limit: self.blob_download_resumption_attempts_limit,
            blob_implementation: Some(blobfs_ramdisk::Implementation::CppBlobfs),
            blob_download_concurrency_limit: self.blob_download_concurrency_limit,
        }
    }

    pub fn mounts(
        self,
        mounts: Mounts,
    ) -> TestEnvBuilder<BlobfsAndSystemImageFut, impl FnOnce() -> Mounts> {
        TestEnvBuilder::<_, _> {
            blobfs_and_system_image: self.blobfs_and_system_image,
            mounts: || mounts,
            tuf_metadata_timeout_seconds: self.tuf_metadata_timeout_seconds,
            tuf_network_header_timeout_seconds: self.tuf_network_header_timeout_seconds,
            blob_network_header_timeout_seconds: self.blob_network_header_timeout_seconds,
            blob_network_body_timeout_seconds: self.blob_network_body_timeout_seconds,
            blob_download_resumption_attempts_limit: self.blob_download_resumption_attempts_limit,
            blob_implementation: self.blob_implementation,
            blob_download_concurrency_limit: self.blob_download_concurrency_limit,
        }
    }

    pub fn tuf_metadata_timeout_seconds(mut self, seconds: u32) -> Self {
        assert_eq!(self.tuf_metadata_timeout_seconds, None);
        self.tuf_metadata_timeout_seconds = Some(seconds);
        self
    }

    pub fn tuf_network_header_timeout_seconds(mut self, seconds: u32) -> Self {
        assert_eq!(self.tuf_network_header_timeout_seconds, None);
        self.tuf_network_header_timeout_seconds = Some(seconds);
        self
    }

    pub fn blob_network_header_timeout_seconds(mut self, seconds: u32) -> Self {
        assert_eq!(self.blob_network_header_timeout_seconds, None);
        self.blob_network_header_timeout_seconds = Some(seconds);
        self
    }

    pub fn blob_network_body_timeout_seconds(mut self, seconds: u32) -> Self {
        assert_eq!(self.blob_network_body_timeout_seconds, None);
        self.blob_network_body_timeout_seconds = Some(seconds);
        self
    }

    pub fn blob_download_resumption_attempts_limit(mut self, limit: u32) -> Self {
        assert_eq!(self.blob_download_resumption_attempts_limit, None);
        self.blob_download_resumption_attempts_limit = Some(limit);
        self
    }

    pub fn blob_download_concurrency_limit(mut self, limit: u16) -> Self {
        assert_eq!(self.blob_download_concurrency_limit, None);
        self.blob_download_concurrency_limit = Some(limit);
        self
    }

    pub fn fxblob(self) -> Self {
        assert_eq!(self.blob_implementation, None);
        Self { blob_implementation: Some(blobfs_ramdisk::Implementation::Fxblob), ..self }
    }

    pub fn cpp_blobfs(self) -> Self {
        assert_eq!(self.blob_implementation, None);
        Self { blob_implementation: Some(blobfs_ramdisk::Implementation::CppBlobfs), ..self }
    }

    pub async fn build(self) -> TestEnv<ConcreteBlobfs> {
        let blob_implementation =
            self.blob_implementation.unwrap_or(blobfs_ramdisk::Implementation::CppBlobfs);
        let (blobfs, system_image) = (self.blobfs_and_system_image)(blob_implementation).await;
        let mounts = (self.mounts)();

        let local_child_svc_dir = vfs::pseudo_directory! {};

        let logger_factory = Arc::new(MockMetricEventLoggerFactory::new());
        let logger_factory_clone = Arc::clone(&logger_factory);
        local_child_svc_dir
            .add_entry(
                fmetrics::MetricEventLoggerFactoryMarker::PROTOCOL_NAME,
                vfs::service::host(move |stream| {
                    Arc::clone(&logger_factory_clone).run_logger_factory(stream)
                }),
            )
            .unwrap();

        let commit_status_provider_service = Arc::new(MockCommitStatusProviderService::new());
        local_child_svc_dir
            .add_entry(
                fupdate::CommitStatusProviderMarker::PROTOCOL_NAME,
                vfs::service::host(move |stream| {
                    Arc::clone(&commit_status_provider_service).handle_request_stream(stream)
                }),
            )
            .unwrap();

        let local_child_out_dir = vfs::pseudo_directory! {
            "blob" => vfs::remote::remote_dir(
                blobfs.root_dir_handle().into_proxy()
            ),
            "data" => vfs::remote::remote_dir(
                mounts.pkg_resolver_data.to_proxy(fio::RW_STAR_DIR)
            ),
            "config" => vfs::pseudo_directory! {
                "data" => vfs::remote::remote_dir(
                    mounts.pkg_resolver_config_data.to_proxy(fio::R_STAR_DIR)
                ),
                "ssl" => vfs::remote::remote_dir(
                    fuchsia_fs::directory::open_in_namespace(
                        "/pkg/data/ssl",
                        fio::PERM_READABLE
                    ).unwrap()
                ),
            },
            "svc" => local_child_svc_dir,
        };
        local_child_out_dir
            .add_entry("blob-svc", vfs::remote::remote_dir(blobfs.svc_dir()))
            .unwrap();

        let local_child_out_dir = Mutex::new(Some(local_child_out_dir));

        let builder = RealmBuilder::new().await.unwrap();
        let pkg_cache = builder
            .add_child("pkg_cache", "#meta/pkg-cache.cm", ChildOptions::new())
            .await
            .unwrap();
        let system_update_committer = builder
            .add_child(
                "system_update_committer",
                "#meta/system-update-committer.cm",
                ChildOptions::new(),
            )
            .await
            .unwrap();
        let http_client = builder
            .add_child("http_client", "#meta/http-client.cm", ChildOptions::new())
            .await
            .unwrap();
        let service_reflector = builder
            .add_local_child(
                "service_reflector",
                move |handles| {
                    let local_child_out_dir = local_child_out_dir
                        .lock()
                        .take()
                        .expect("mock component should only be launched once");
                    let scope = vfs::execution_scope::ExecutionScope::new();
                    vfs::directory::serve_on(
                        local_child_out_dir,
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
                        scope.clone(),
                        handles.outgoing_dir,
                    );
                    async move {
                        scope.wait().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();

        let pkg_resolver = builder
            .add_child(PKG_RESOLVER_CHILD_NAME, "#meta/pkg-resolver.cm", ChildOptions::new())
            .await
            .unwrap();

        if self.tuf_metadata_timeout_seconds.is_some()
            || self.blob_download_resumption_attempts_limit.is_some()
            || self.blob_download_concurrency_limit.is_some()
        {
            builder.init_mutable_config_from_package(&pkg_resolver).await.unwrap();
            if let Some(tuf_metadata_timeout_seconds) = self.tuf_metadata_timeout_seconds {
                builder
                    .set_config_value(
                        &pkg_resolver,
                        "tuf_metadata_timeout_seconds",
                        tuf_metadata_timeout_seconds.into(),
                    )
                    .await
                    .unwrap();
            }
            if let Some(blob_download_resumption_attempts_limit) =
                self.blob_download_resumption_attempts_limit
            {
                builder
                    .set_config_value(
                        &pkg_resolver,
                        "blob_download_resumption_attempts_limit",
                        blob_download_resumption_attempts_limit.into(),
                    )
                    .await
                    .unwrap();
            }
            if let Some(blob_download_concurrency_limit) = self.blob_download_concurrency_limit {
                builder
                    .set_config_value(
                        &pkg_resolver,
                        "blob_download_concurrency_limit",
                        blob_download_concurrency_limit.into(),
                    )
                    .await
                    .unwrap();
            }
        }

        for (key, value) in [
            (
                "fuchsia.pkgresolver.TufNetworkHeaderTimeoutSeconds",
                self.tuf_network_header_timeout_seconds,
            ),
            (
                "fuchsia.pkgresolver.BlobNetworkHeaderTimeoutSeconds",
                self.blob_network_header_timeout_seconds,
            ),
            (
                "fuchsia.pkgresolver.BlobNetworkBodyTimeoutSeconds",
                self.blob_network_body_timeout_seconds,
            ),
        ] {
            if let Some(value) = value {
                builder
                    .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                        name: key.parse().unwrap(),
                        value: value.into(),
                    }))
                    .await
                    .unwrap();
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::configuration(key))
                            .from(Ref::self_())
                            .to(&pkg_resolver),
                    )
                    .await
                    .unwrap();
            } else {
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::configuration(key))
                            .from(Ref::void())
                            .to(&pkg_resolver),
                    )
                    .await
                    .unwrap();
            }
        }

        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.http-client.StopOnIdleTimeoutMillis",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.http-client.TcpReceiveBufferSizeBytes",
                    ))
                    .from(Ref::void())
                    .to(&http_client),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("root-ssl-certificates")
                            .path("/config/ssl")
                            .rights(fio::R_STAR_DIR),
                    )
                    .from(&service_reflector)
                    .to(&http_client),
            )
            .await
            .unwrap();

        // Unconditionally overwrite `use_fxblob` because the value in the production config is
        // outside of SWD control.
        let pkg_cache_config = builder
            .add_child("pkg_cache_config", "#meta/pkg-cache-config.cm", ChildOptions::new())
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.pkgcache.AllPackagesExecutable"))
                    .capability(Capability::configuration(
                        "fuchsia.pkgcache.EnableUpgradablePackages",
                    ))
                    .from(&pkg_cache_config)
                    .to(&pkg_cache),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.pkgcache.BlobFetchConcurrencyLimit",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.pkgcache.BlobNetworkHeaderTimeoutSeconds",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.pkgcache.BlobNetworkBodyTimeoutSeconds",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.pkgcache.BlobDownloadResumptionAttemptsLimit",
                    ))
                    .from(Ref::void())
                    .to(&pkg_cache),
            )
            .await
            .unwrap();

        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.pkgcache.RequireSystemImage".parse().unwrap(),
                value: system_image.is_some().into(),
            }))
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.pkgcache.RequireSystemImage"))
                    .from(Ref::self_())
                    .to(&pkg_cache),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                    .from(Ref::parent())
                    .to(&pkg_cache)
                    .to(&system_update_committer)
                    .to(&http_client)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();

        // Networking capabilities needed for the hyper client shard.
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_posix_socket::ProviderMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_net_name::LookupMarker>())
                    .from(Ref::parent())
                    .to(&http_client),
            )
            .await
            .unwrap();

        // Fill out the rest of the `use` stanzas for pkg_resolver and pkg_cache
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>())
                    .capability(
                        Capability::protocol::<fidl_fuchsia_tracing_provider::RegistryMarker>(),
                    )
                    .from(&service_reflector)
                    .to(&pkg_cache)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fpkg_http::ClientMarker>())
                    .capability(Capability::protocol::<fnet_http::LoaderMarker>())
                    .from(&http_client)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fpkg::PackageCacheMarker>())
                    .from(&pkg_cache)
                    .to(&pkg_resolver)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fpkg_gc::ManagerMarker>())
                    .from(&pkg_cache)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fpkg::PackageResolverMarker>())
                    .capability(Capability::protocol_by_name(format!(
                        "{}-ota",
                        fpkg::PackageResolverMarker::PROTOCOL_NAME
                    )))
                    .capability(Capability::protocol::<fpkg::RepositoryManagerMarker>())
                    .capability(Capability::protocol::<fpkg_rewrite::EngineMarker>())
                    .capability(Capability::protocol::<fpkg::CupMarker>())
                    .capability(Capability::protocol::<fpkg_internal::OtaDownloaderMarker>())
                    .capability(Capability::protocol::<fpkg::AuthorityMarker>())
                    .from(&pkg_resolver)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                    .from(Ref::framework())
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("blob-exec")
                            .path("/blob")
                            .rights(fio::RW_STAR_DIR | fio::Operations::EXECUTE),
                    )
                    .capability(Capability::protocol::<fupdate::CommitStatusProviderMarker>())
                    .from(&service_reflector)
                    .to(&pkg_cache),
            )
            .await
            .unwrap();

        let system_image_value = if let Some(hash) = system_image {
            static PKGFS_BOOT_ARG_VALUE_PREFIX: &str = "bin/pkgsvr+";
            format!("{PKGFS_BOOT_ARG_VALUE_PREFIX}{hash}")
        } else {
            "".to_string()
        };
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.zircon.system.pkgfs.cmd".parse().unwrap(),
                value: system_image_value.into(),
            }))
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.zircon.system.pkgfs.cmd"))
                    .from(Ref::self_())
                    .to(&pkg_cache),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol::<ffxfs::BlobCreatorMarker>()
                            .path(format!("/blob-svc/{}", ffxfs::BlobCreatorMarker::PROTOCOL_NAME)),
                    )
                    .capability(
                        Capability::protocol::<ffxfs::BlobReaderMarker>()
                            .path(format!("/blob-svc/{}", ffxfs::BlobReaderMarker::PROTOCOL_NAME)),
                    )
                    .from(&service_reflector)
                    .to(&pkg_cache),
            )
            .await
            .unwrap();

        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("config-data")
                            .path("/config/data")
                            .rights(fio::R_STAR_DIR),
                    )
                    // TODO(https://fxbug.dev/42155475): Change to storage once convenient.
                    .capability(
                        Capability::directory("data").path("/data").rights(fio::RW_STAR_DIR),
                    )
                    .from(&service_reflector)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();

        let realm_instance = builder.build().await.unwrap();

        TestEnv {
            blobfs,
            proxies: Proxies::from_instance(&realm_instance.root),
            realm_instance,
            _mounts: mounts,
            mocks: Mocks { logger_factory },
        }
    }
}

pub struct Proxies {
    pub resolver: PackageResolverProxy,
    pub resolver_ota: PackageResolverProxy,
    pub repo_manager: RepositoryManagerProxy,
    pub rewrite_engine: fpkg_rewrite::EngineProxy,
    pub cup: CupProxy,
    pub space_manager: fpkg_gc::ManagerProxy,
    pub ota_downloader: fpkg_internal::OtaDownloaderProxy,
    pub authority: fpkg::AuthorityProxy,
}

impl Proxies {
    fn from_instance(realm: &ScopedInstance) -> Proxies {
        Proxies {
            resolver: realm
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to package resolver"),
            resolver_ota: realm
                .connect_to_named_protocol_at_exposed_dir::<PackageResolverMarker>(&format!(
                    "{}-ota",
                    fpkg::PackageResolverMarker::PROTOCOL_NAME
                ))
                .expect("connect to package resolver"),
            repo_manager: realm
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to repository manager"),
            rewrite_engine: realm
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to rewrite engine"),
            cup: realm.connect_to_protocol_at_exposed_dir().expect("connect to cup"),
            space_manager: realm
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to space manager"),
            ota_downloader: realm
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to ota downloader"),
            authority: realm.connect_to_protocol_at_exposed_dir().expect("connect to authority"),
        }
    }
}

pub struct Mocks {
    pub logger_factory: Arc<MockMetricEventLoggerFactory>,
}

pub struct TestEnv<B = BlobfsRamdisk> {
    pub blobfs: B,
    pub realm_instance: RealmInstance,
    pub proxies: Proxies,
    pub _mounts: Mounts,
    pub mocks: Mocks,
}

impl TestEnv<BlobfsRamdisk> {
    pub async fn add_slice_to_blobfs(&self, slice: &[u8]) {
        let merkle = fuchsia_merkle::root_from_slice(slice);
        self.blobfs.write_blob(merkle, slice).await.expect("add blob to blobfs")
    }

    pub async fn add_file_with_hash_to_blobfs(&self, file: File, hash: &Hash) {
        self.blobfs.add_blob_from(*hash, file).await.expect("add blob to blobfs")
    }

    pub async fn stop(self) {
        // Tear down the environment in reverse order, ending with the storage.
        drop(self.proxies);
        drop(self.realm_instance);
        self.blobfs.stop().await.expect("blobfs to stop gracefully");
    }
}

impl<B: Blobfs> TestEnv<B> {
    pub async fn register_repo(&self, repo: &ServedRepository) {
        self.register_repo_at_url(repo, "fuchsia-pkg://test").await;
    }

    pub async fn register_repo_at_url<R>(&self, repo: &ServedRepository, url: R)
    where
        R: TryInto<RepositoryUrl>,
        R::Error: std::fmt::Debug,
    {
        let repo_config = repo.make_repo_config(url.try_into().unwrap());
        let () = self.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();
    }

    pub async fn restart_pkg_resolver(&mut self) {
        let lifecycle_controller: fsys2::LifecycleControllerProxy =
            self.realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
        let () = lifecycle_controller
            .stop_instance(&format!("./{PKG_RESOLVER_CHILD_NAME}"))
            .await
            .unwrap()
            .unwrap();
        let (_, binder_server) = fidl::endpoints::create_endpoints();
        lifecycle_controller
            .start_instance(&format!("./{PKG_RESOLVER_CHILD_NAME}"), binder_server)
            .await
            .unwrap()
            .unwrap();
        self.proxies = Proxies::from_instance(&self.realm_instance.root);
        self.wait_for_pkg_resolver_to_start().await;
    }

    pub async fn wait_for_pkg_resolver_to_start(&self) {
        self.proxies
            .rewrite_engine
            .test_apply("fuchsia-pkg://test.com/name")
            .await
            .expect("fidl call succeeds")
            .expect("test apply result is ok");
    }

    /// Obtain a new proxy, different than self.proxies.resolver, to issue concurrent requests.
    pub fn connect_to_resolver(&self) -> PackageResolverProxy {
        self.realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to package resolver")
    }

    pub fn resolve_package(
        &self,
        url: &str,
    ) -> impl Future<
        Output = Result<(fio::DirectoryProxy, pkg::ResolutionContext), fpkg::ResolveError>,
    > + use<B> {
        resolve_package(&self.proxies.resolver, url)
    }

    pub fn resolve_with_context(
        &self,
        url: &str,
        context: pkg::ResolutionContext,
    ) -> impl Future<
        Output = Result<(fio::DirectoryProxy, pkg::ResolutionContext), fpkg::ResolveError>,
    > + use<B> {
        resolve_with_context(&self.proxies.resolver, url, context)
    }

    pub fn get_hash(
        &self,
        url: impl Into<String>,
    ) -> impl Future<Output = Result<pkg::BlobId, zx::Status>> {
        let fut = self.proxies.resolver.get_hash(&fpkg::PackageUrl { url: url.into() });
        async move { fut.await.unwrap().map(|blob_id| blob_id.into()).map_err(zx::Status::err_from_raw) }
    }

    pub fn lookup(
        &self,
        url: impl Into<String>,
    ) -> impl Future<Output = Result<(pkg::BlobId, http::uri::Uri), fpkg::AuthorityLookupError>>
    {
        let fut = self.proxies.authority.lookup(&fpkg::PackageUrl { url: url.into() });
        async move { fut.await.unwrap().map(|(blob_id, url)| (blob_id.into(), url.parse().unwrap())) }
    }

    pub async fn get_already_cached(
        &self,
        hash: pkg::BlobId,
    ) -> Result<fio::DirectoryProxy, pkg::cache::GetAlreadyCachedError> {
        pkg::cache::Client::from_proxy(
            self.realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap(),
        )
        .get_already_cached(hash)
        .await
        .map(|pd| pd.into_proxy())
    }

    pub async fn pkg_resolver_inspect_hierarchy(&self) -> DiagnosticsHierarchy {
        let data = ArchiveReader::inspect()
            .add_selector(ComponentSelector::new(vec![
                format!("realm_builder\\:{}", self.realm_instance.root.child_name()),
                PKG_RESOLVER_CHILD_NAME.into(),
            ]))
            .snapshot()
            .await
            .expect("read inspect hierarchy")
            .into_iter()
            .next()
            .expect("one result");

        if data.payload.is_none() {
            log::error!(data:?; "Unexpected empty payload");
        }

        data.payload.unwrap()
    }

    /// Wait until pkg-resolver inspect state satisfies `desired_state`.
    pub async fn wait_for_pkg_resolver_inspect_state(&self, desired_state: TreeAssertion<String>) {
        while desired_state.run(&self.pkg_resolver_inspect_hierarchy().await).is_err() {
            fasync::Timer::new(Duration::from_millis(10)).await;
        }
    }
    /// Wait until at least `expected_event_codes.len()` events of metric id `expected_metric_id`
    /// are received, then assert that the event codes of the received events correspond, in order,
    /// to the event codes in `expected_event_codes`.
    pub async fn assert_count_events(
        &self,
        expected_metric_id: u32,
        expected_event_codes: Vec<impl AsEventCodes + std::fmt::Debug>,
    ) {
        let actual_events = self
            .mocks
            .logger_factory
            .wait_for_at_least_n_events_with_metric_id(
                expected_event_codes.len(),
                expected_metric_id,
            )
            .await;
        assert_eq!(
            actual_events.len(),
            expected_event_codes.len(),
            "event count different than expected, actual_events: {actual_events:?}"
        );

        for ((i, event), expected_codes) in
            actual_events.into_iter().enumerate().zip(expected_event_codes)
        {
            assert_matches!(
                event,
                MetricEvent {
                    metric_id,
                    event_codes,
                    payload: MetricEventPayload::Count(1),
                } if metric_id == expected_metric_id && event_codes == expected_codes.as_event_codes(),
                "event {i} expected metric id: {expected_metric_id}, expected codes: {expected_codes:?}",
            )
        }
    }

    pub async fn cup_write(
        &self,
        url: impl Into<String>,
        cup: pkg::CupData,
    ) -> Result<(), WriteError> {
        self.proxies.cup.write(&fpkg::PackageUrl { url: url.into() }, &cup.into()).await.unwrap()
    }

    pub async fn cup_get_info(
        &self,
        url: impl Into<String>,
    ) -> Result<(String, String), GetInfoError> {
        self.proxies.cup.get_info(&fpkg::PackageUrl { url: url.into() }).await.unwrap()
    }

    pub async fn fetch_blob(
        &self,
        hash: pkg::BlobId,
        base_url: impl AsRef<str>,
    ) -> Result<u64, fpkg::ResolveError> {
        self.proxies
            .ota_downloader
            .fetch_blob(&hash.into(), base_url.as_ref(), false)
            .await
            .unwrap()
    }
}

pub const EMPTY_REPO_PATH: &str = "/pkg/empty-repo";

// The following functions generate unique test package dummy content. Callers are recommended
// to pass in the name of the test case.
pub fn test_package_bin(s: &str) -> Vec<u8> {
    format!("!/boot/bin/sh\n{s}").as_bytes().to_owned()
}

pub fn test_package_cml(s: &str) -> Vec<u8> {
    format!("{{program:{{runner:\"elf\",binary:\"bin/{s}\"}}}}").as_bytes().to_owned()
}

pub fn extra_blob_contents(s: &str, i: u32) -> Vec<u8> {
    format!("contents of file {s}-{i}").as_bytes().to_owned()
}

pub async fn make_pkg_with_extra_blobs(s: &str, n: u32) -> Package {
    let mut pkg = PackageBuilder::new(s)
        .add_resource_at(format!("bin/{s}"), &test_package_bin(s)[..])
        .add_resource_at(format!("meta/{s}.cml"), &test_package_cml(s)[..]);
    for i in 0..n {
        pkg = pkg.add_resource_at(format!("data/{s}-{i}"), extra_blob_contents(s, i).as_slice());
    }
    pkg.build().await.unwrap()
}

pub fn resolve_package(
    resolver: &PackageResolverProxy,
    url: &str,
) -> impl Future<Output = Result<(fio::DirectoryProxy, pkg::ResolutionContext), fpkg::ResolveError>>
+ use<> {
    let (package, package_server_end) = fidl::endpoints::create_proxy();
    let response_fut = resolver.resolve(url, package_server_end);
    async move {
        let resolved_context = response_fut.await.unwrap()?;
        Ok((package, (&resolved_context).try_into().unwrap()))
    }
}

pub fn resolve_with_context(
    resolver: &PackageResolverProxy,
    url: &str,
    context: pkg::ResolutionContext,
) -> impl Future<Output = Result<(fio::DirectoryProxy, pkg::ResolutionContext), fpkg::ResolveError>>
+ use<> {
    let (package, package_server_end) = fidl::endpoints::create_proxy();
    let response_fut = resolver.resolve_with_context(url, &context.into(), package_server_end);
    async move {
        let resolved_context = response_fut.await.unwrap()?;
        Ok((package, (&resolved_context).try_into().unwrap()))
    }
}

pub fn make_repo_config(repo: &RepositoryConfig) -> RepositoryConfigs {
    RepositoryConfigs::Version1(vec![repo.clone()])
}

pub fn make_repo() -> RepositoryConfig {
    RepositoryConfigBuilder::new("fuchsia-pkg://example.com".parse().unwrap()).build()
}

pub async fn get_repos(repository_manager: &RepositoryManagerProxy) -> Vec<RepositoryConfig> {
    let (repo_iterator, repo_iterator_server) = fidl::endpoints::create_proxy();
    repository_manager.list(repo_iterator_server).expect("list repos");
    let mut ret = vec![];
    loop {
        let repos = repo_iterator.next().await.expect("advance repo iterator");
        if repos.is_empty() {
            return ret;
        }
        ret.extend(repos.into_iter().map(|r| r.try_into().unwrap()))
    }
}

pub async fn get_rules(rewrite_engine: &fpkg_rewrite::EngineProxy) -> Vec<Rule> {
    let (rule_iterator, rule_iterator_server) = fidl::endpoints::create_proxy();
    rewrite_engine.list(rule_iterator_server).expect("list rules");
    let mut ret = vec![];
    loop {
        let rules = rule_iterator.next().await.expect("advance rule iterator");
        if rules.is_empty() {
            return ret;
        }
        ret.extend(rules.into_iter().map(|r| r.try_into().unwrap()))
    }
}

pub fn get_cup_response_with_name(package_url: &PinnedAbsolutePackageUrl) -> Vec<u8> {
    let response = serde_json::json!({"response":{
      "server": "prod",
      "protocol": "3.0",
      "app": [{
        "appid": "appid",
        "cohortname": "stable",
        "status": "ok",
        "updatecheck": {
          "status": "ok",
          "urls":{
            "url":[
                {"codebase": format!("{}/", package_url.repository()) },
            ]
          },
          "manifest": {
            "version": "1.2.3.4",
            "actions": {
              "action": [],
            },
            "packages": {
              "package": [
                {
                 "name": format!("{}?hash={}", package_url.name(), package_url.hash()),
                 "required": true,
                 "fp": "",
                }
              ],
            },
          }
        }
      }],
    }});
    serde_json::to_vec(&response).unwrap()
}

/// Always says the system is committed so that pkg-cache can run GC.
struct MockCommitStatusProviderService {
    _local: zx::EventPair,
    remote: zx::EventPair,
}

impl MockCommitStatusProviderService {
    fn new() -> Self {
        let (_local, remote) = zx::EventPair::create();
        let () = remote.signal(zx::Signals::NONE, zx::Signals::USER_0).unwrap();
        Self { _local, remote }
    }

    async fn handle_request_stream(
        self: Arc<Self>,
        mut stream: fupdate::CommitStatusProviderRequestStream,
    ) {
        while let Some(event) =
            stream.try_next().await.expect("received fuchsia.update/CommitStatusProvider request")
        {
            match event {
                fupdate::CommitStatusProviderRequest::IsCurrentSystemCommitted { responder } => {
                    let () = responder
                        .send(self.remote.duplicate_handle(zx::Rights::BASIC).unwrap())
                        .unwrap();
                }
            }
        }
    }
}
