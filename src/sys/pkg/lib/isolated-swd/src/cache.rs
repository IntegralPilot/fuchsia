// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;

/// Represents the sandboxed package cache.
pub struct Cache {
    _pkg_cache_proxy: fidl_fuchsia_pkg::PackageCacheProxy,
}

impl Cache {
    /// Construct a new `Cache` object with pre-created proxies to package cache, and space
    /// manager.
    pub fn new_with_proxies(
        pkg_cache_proxy: fidl_fuchsia_pkg::PackageCacheProxy,
    ) -> Result<Self, Error> {
        Ok(Self { _pkg_cache_proxy: pkg_cache_proxy })
    }

    /// Construct a new `Cache` object using capabilities available in the namespace of the component
    /// calling this function. Should be the default in production usage, as these capabilities
    /// should be statically routed (i.e. from `pkg-recovery.cml`).
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            _pkg_cache_proxy: fuchsia_component::client::connect_to_protocol::<
                fidl_fuchsia_pkg::PackageCacheMarker,
            >()?,
        })
    }

    /// Get a proxy to an instance of fuchsia.pkg.PackageCache.
    #[cfg(test)]
    pub fn package_cache_proxy(&self) -> Result<fidl_fuchsia_pkg::PackageCacheProxy, Error> {
        Ok(self._pkg_cache_proxy.clone())
    }
}

#[cfg(test)]
pub(crate) mod for_tests {
    use super::*;
    use blobfs_ramdisk::BlobfsRamdisk;
    use fidl::endpoints::{ServerEnd, SynchronousProxy};
    use fidl_fuchsia_fxfs as ffxfs;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_metrics as fmetrics;
    use fuchsia_async as fasync;
    use fuchsia_component_test::{
        Capability, ChildOptions, ChildRef, RealmBuilder, RealmInstance, Ref, Route,
    };
    use futures::prelude::*;
    use std::sync::Arc;

    pub struct CacheForTest {
        pub blobfs: blobfs_ramdisk::BlobfsRamdisk,
        pub cache: Arc<Cache>,
    }

    impl CacheForTest {
        pub async fn realm_setup(
            realm_builder: &RealmBuilder,
            blobfs: &BlobfsRamdisk,
        ) -> Result<ChildRef, Error> {
            let blobfs_proxy = blobfs.root_dir_proxy().expect("getting root dir proxy");
            let svc_dir = blobfs.svc_dir().expect("getting service dir proxy");

            let local_mocks = realm_builder
                .add_local_child(
                    "pkg_cache_service_reflector",
                    move |handles| {
                        let mut fs = fuchsia_component::server::ServiceFs::new();
                        let (creator_dir, server_end) =
                            fidl::endpoints::create_sync_proxy::<fio::DirectoryMarker>();
                        svc_dir
                            .open(
                                ".",
                                fio::PERM_READABLE,
                                &fio::Options::default(),
                                server_end.into_channel(),
                            )
                            .unwrap();
                        let (reader_dir, server_end) =
                            fidl::endpoints::create_sync_proxy::<fio::DirectoryMarker>();
                        svc_dir
                            .open(
                                ".",
                                fio::PERM_READABLE,
                                &fio::Options::default(),
                                server_end.into_channel(),
                            )
                            .unwrap();
                        // Not necessary for updates, but prevents spam of irrelevant error logs.
                        fs.dir("svc")
                            .add_fidl_service(move |stream| {
                                fasync::Task::spawn(
                                    Arc::new(mock_metrics::MockMetricEventLoggerFactory::new())
                                        .run_logger_factory(stream),
                                )
                                .detach()
                            })
                            .add_service_connector(
                                move |server_end: ServerEnd<ffxfs::BlobCreatorMarker>| {
                                    fdio::service_connect_at(
                                        creator_dir.as_channel(),
                                        "fuchsia.fxfs.BlobCreator",
                                        server_end.into_channel(),
                                    )
                                    .unwrap();
                                },
                            )
                            .add_service_connector(
                                move |server_end: ServerEnd<ffxfs::BlobReaderMarker>| {
                                    fdio::service_connect_at(
                                        reader_dir.as_channel(),
                                        "fuchsia.fxfs.BlobReader",
                                        server_end.into_channel(),
                                    )
                                    .unwrap();
                                },
                            );
                        fs.add_remote("blob", Clone::clone(&blobfs_proxy));
                        async move {
                            fs.serve_connection(handles.outgoing_dir).unwrap();
                            let () = fs.collect().await;
                            Ok(())
                        }
                        .boxed()
                    },
                    ChildOptions::new(),
                )
                .await
                .unwrap();

            let pkg_cache = realm_builder
                .add_child("pkg_cache", "#meta/pkg-cache.cm", ChildOptions::new())
                .await
                .unwrap();

            let system_image_package = fuchsia_pkg_testing::SystemImageBuilder::new().build().await;
            system_image_package.write_to_blobfs(blobfs).await;

            for (name, value) in [
                ("fuchsia.zircon.system.pkgfs.cmd", system_image_package.hash().to_string().into()),
                ("fuchsia.pkgcache.AllPackagesExecutable", false.into()),
                ("fuchsia.pkgcache.RequireSystemImage", false.into()),
                ("fuchsia.pkgcache.EnableUpgradablePackages", false.into()),
            ] {
                realm_builder
                    .add_capability(
                        cm_rust::ConfigurationDecl { name: name.parse().unwrap(), value }.into(),
                    )
                    .await
                    .unwrap();
                realm_builder
                    .add_route(
                        Route::new()
                            .capability(Capability::configuration(name))
                            .from(Ref::self_())
                            .to(&pkg_cache),
                    )
                    .await
                    .unwrap();
            }
            realm_builder
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
            let system_update_committer = realm_builder
                .add_child(
                    "system-update-committer",
                    "#meta/fake-system-update-committer.cm",
                    ChildOptions::new(),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::directory("blob-exec")
                                .path("/blob")
                                .rights(fio::RW_STAR_DIR | fio::Operations::EXECUTE),
                        )
                        .from(&local_mocks)
                        .to(&pkg_cache),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                        .from(Ref::parent())
                        .to(&pkg_cache),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.fxfs.BlobCreator"))
                        .capability(Capability::protocol_by_name("fuchsia.fxfs.BlobReader"))
                        .from(&local_mocks)
                        .to(&pkg_cache),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.pkg.PackageCache"))
                        .capability(Capability::protocol_by_name("fuchsia.pkg.RetainedPackages"))
                        .capability(Capability::protocol_by_name(
                            "fuchsia.pkg.garbagecollector.Manager",
                        ))
                        .from(&pkg_cache)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name(
                            "fuchsia.update.CommitStatusProvider",
                        ))
                        .from(&system_update_committer)
                        .to(&pkg_cache),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>(),
                        )
                        .from(&local_mocks)
                        .to(&pkg_cache),
                )
                .await
                .unwrap();
            Ok(pkg_cache)
        }

        pub async fn new(
            realm_instance: &RealmInstance,
            blobfs: BlobfsRamdisk,
        ) -> Result<Self, Error> {
            let pkg_cache_proxy = realm_instance
                .root
                .connect_to_protocol_at_exposed_dir()
                .expect("connect to pkg cache");

            let cache = Cache::new_with_proxies(pkg_cache_proxy).unwrap();

            Ok(CacheForTest { blobfs, cache: Arc::new(cache) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::for_tests::CacheForTest;
    use fuchsia_component_test::RealmBuilder;

    #[fuchsia::test]
    pub async fn test_cache_handles_sync() {
        let realm_builder = RealmBuilder::new().await.unwrap();
        let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.expect("starting blobfs");

        let _cache_ref =
            CacheForTest::realm_setup(&realm_builder, &blobfs).await.expect("setting up realm");
        let realm_instance = realm_builder.build().await.unwrap();
        let cache = CacheForTest::new(&realm_instance, blobfs).await.expect("launching cache");
        let proxy = cache.cache.package_cache_proxy().unwrap();

        assert_eq!(proxy.sync().await.unwrap(), Ok(()));
    }
}
