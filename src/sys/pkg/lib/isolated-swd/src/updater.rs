// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_paver::{BootManagerMarker, Configuration, PaverMarker, PaverProxy};
use fidl_fuchsia_update_installer::{InstallerMarker, InstallerProxy, RebootControllerMarker};
use fidl_fuchsia_update_installer_ext::options::{Initiator, Options};
use fidl_fuchsia_update_installer_ext::{UpdateAttempt, start_update};

use futures::prelude::*;

pub const DEFAULT_UPDATE_PACKAGE_URL: &str = "fuchsia-pkg://fuchsia.com/update";

pub struct Updater {
    proxy: InstallerProxy,
    paver_proxy: PaverProxy,
}

impl Updater {
    pub fn new_with_proxies(proxy: InstallerProxy, paver_proxy: PaverProxy) -> Self {
        Self { proxy, paver_proxy }
    }

    pub fn new() -> Result<Self, Error> {
        Ok(Self::new_with_proxies(
            fuchsia_component::client::connect_to_protocol::<InstallerMarker>()?,
            fuchsia_component::client::connect_to_protocol::<PaverMarker>()?,
        ))
    }

    pub async fn start_update(
        &mut self,
        update_package: Option<&http::Uri>,
    ) -> Result<UpdateAttempt, Error> {
        let update_package = match update_package {
            Some(url) => url.to_owned(),
            None => DEFAULT_UPDATE_PACKAGE_URL.parse().unwrap(),
        };

        let (reboot_controller, reboot_controller_server_end) =
            fidl::endpoints::create_proxy::<RebootControllerMarker>();
        let () = reboot_controller.detach().context("disabling automatic reboot")?;

        start_update(
            &update_package,
            Options {
                initiator: Initiator::User,
                allow_attach_to_existing_attempt: false,
                should_write_recovery: false,
                manifest_range: None,
                manifest_headers: vec![],
            },
            &self.proxy,
            Some(reboot_controller_server_end),
        )
        .await
        .context("starting system update")
    }

    /// Perform an update, skipping the final reboot.
    /// If `update_package` is Some, use the given package URL as the URL for the update package.
    /// Otherwise, `system-updater` uses the default URL.
    /// This will not install any images to the recovery partitions.
    pub async fn install_update(
        &mut self,
        update_package: Option<&http::Uri>,
    ) -> Result<(), Error> {
        let attempt = self.start_update(update_package).await?;

        let () = Self::monitor_update_attempt(attempt).await.context("monitoring installation")?;

        let () = Self::activate_installed_slot(&self.paver_proxy)
            .await
            .context("activating installed slot")?;

        Ok(())
    }

    async fn monitor_update_attempt(mut attempt: UpdateAttempt) -> Result<(), Error> {
        while let Some(state) = attempt.try_next().await.context("fetching next update state")? {
            log::info!("Install: {:?}", state);
            if state.is_success() {
                return Ok(());
            } else if state.is_failure() {
                return Err(anyhow!("update attempt failed in state {:?}", state));
            }
        }

        Err(anyhow!("unexpected end of update attempt"))
    }

    async fn activate_installed_slot(paver: &PaverProxy) -> Result<(), Error> {
        let (boot_manager, remote) = fidl::endpoints::create_proxy::<BootManagerMarker>();

        paver.find_boot_manager(remote).context("finding boot manager")?;

        let result = boot_manager.query_active_configuration().await;
        if let Err(fidl::Error::ClientChannelClosed { epitaph, .. }) = result
            && epitaph == zx::Status::NOT_SUPPORTED
        {
            // board does not actually support ABR, so return.
            log::info!("ABR not supported, not configuring slots.");
            return Ok(());
        }
        let result = result?;
        if result.is_ok() {
            // active slot is valid - assume that system-updater handled this for us.
            return Ok(());
        }

        // In recovery, the paver will return ZX_ERR_NOT_SUPPORTED to query_active_configuration(),
        // even on devices which support ABR. Handle this manually in case it is actually
        // supported.
        zx::Status::ok(
            boot_manager
                .set_configuration_active(Configuration::A)
                .await
                .context("Sending set active configuration request")?,
        )
        .context("Setting A to active configuration")?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod for_tests {
    use super::*;
    use crate::resolver::for_tests::{EMPTY_REPO_PATH, ResolverForTest};
    use blobfs_ramdisk::BlobfsRamdisk;
    use fidl_fuchsia_metrics as fmetrics;
    use fidl_fuchsia_paver::PaverRequestStream;
    use fuchsia_async as fasync;
    use fuchsia_component_test::{
        Capability, ChildOptions, DirectoryContents, RealmBuilder, RealmInstance, Ref, Route,
    };
    use fuchsia_merkle::Hash;
    use fuchsia_pkg_testing::serve::ServedRepository;
    use fuchsia_pkg_testing::{Package, RepositoryBuilder, SystemImageBuilder};
    use mock_paver::{MockPaverService, MockPaverServiceBuilder, PaverEvent};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    pub const TEST_REPO_URL: &str = "fuchsia-pkg://fuchsia.com";
    pub struct UpdaterBuilder {
        paver_builder: MockPaverServiceBuilder,
        packages: Vec<Package>,
        // The zbi and optional vbmeta contents.
        fuchsia_image: Option<(Vec<u8>, Option<Vec<u8>>)>,
        // The zbi and optional vbmeta contents of the recovery partition.
        recovery_image: Option<(Vec<u8>, Option<Vec<u8>>)>,
        repo_url: fuchsia_url::RepositoryUrl,
    }

    impl UpdaterBuilder {
        /// Construct a new UpdateBuilder. Initially, this contains no images and an empty system
        /// image package.
        pub async fn new() -> UpdaterBuilder {
            UpdaterBuilder {
                paver_builder: MockPaverServiceBuilder::new(),
                packages: vec![SystemImageBuilder::new().build().await],
                fuchsia_image: None,
                recovery_image: None,
                repo_url: TEST_REPO_URL.parse().unwrap(),
            }
        }

        /// Add a package to the update package this builder will generate.
        pub fn add_package(mut self, package: Package) -> Self {
            self.packages.push(package);
            self
        }

        /// The zbi and optional vbmeta images to write.
        pub fn fuchsia_image(mut self, zbi: Vec<u8>, vbmeta: Option<Vec<u8>>) -> Self {
            assert_eq!(self.fuchsia_image, None);
            self.fuchsia_image = Some((zbi, vbmeta));
            self
        }

        /// The zbi and optional vbmeta images to write to the recovery partition.
        pub fn recovery_image(mut self, zbi: Vec<u8>, vbmeta: Option<Vec<u8>>) -> Self {
            assert_eq!(self.recovery_image, None);
            self.recovery_image = Some((zbi, vbmeta));
            self
        }

        /// Mutate the `MockPaverServiceBuilder` contained in this UpdaterBuilder.
        pub fn paver<F>(mut self, f: F) -> Self
        where
            F: FnOnce(MockPaverServiceBuilder) -> MockPaverServiceBuilder,
        {
            self.paver_builder = f(self.paver_builder);
            self
        }

        pub fn repo_url(mut self, url: &str) -> Self {
            self.repo_url = url.parse().expect("Valid URL supplied to repo_url()");
            self
        }

        fn serve_mock_paver(stream: PaverRequestStream, paver: Arc<MockPaverService>) {
            let paver_clone = Arc::clone(&paver);
            fasync::Task::spawn(
                Arc::clone(&paver_clone)
                    .run_paver_service(stream)
                    .unwrap_or_else(|e| panic!("Failed to run mock paver: {e:?}")),
            )
            .detach();
        }

        async fn run_mock_paver(
            handles: fuchsia_component_test::LocalComponentHandles,
            paver: Arc<MockPaverService>,
        ) -> Result<(), Error> {
            let mut fs = fuchsia_component::server::ServiceFs::new();
            fs.dir("svc")
                .add_fidl_service(move |stream| Self::serve_mock_paver(stream, Arc::clone(&paver)));
            fs.serve_connection(handles.outgoing_dir)?;
            let () = fs.for_each_concurrent(None, |req| async move { req }).await;
            Ok(())
        }

        /// Create an UpdateForTest from this UpdaterBuilder.
        /// This will construct an update package containing all packages and images added to the
        /// builder, create a repository containing the packages, and create a MockPaver.
        pub async fn build(self) -> UpdaterForTest {
            let mut update = fuchsia_pkg_testing::UpdatePackageBuilder::new(self.repo_url.clone())
                .packages(
                    self.packages
                        .iter()
                        .map(|p| {
                            fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl::new(
                                self.repo_url.clone(),
                                p.name().clone(),
                                None,
                                *p.hash(),
                            )
                        })
                        .collect::<Vec<_>>(),
                );
            if let Some((zbi, vbmeta)) = self.fuchsia_image {
                update = update.fuchsia_image(zbi, vbmeta);
            }
            if let Some((zbi, vbmeta)) = self.recovery_image {
                update = update.recovery_image(zbi, vbmeta);
            }
            let (update, images) = update.build().await;

            // Do not include the images package, system-updater triggers GC after resolving it.
            let expected_blobfs_contents = self
                .packages
                .iter()
                .chain([update.as_package()])
                .flat_map(|p| p.list_blobs())
                .collect();

            let repo = Arc::new(
                self.packages
                    .iter()
                    .chain([update.as_package(), &images])
                    .fold(
                        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
                            .add_package(update.as_package()),
                        |repo, package| repo.add_package(package),
                    )
                    .build()
                    .await
                    .expect("Building repo"),
            );

            let realm_builder = RealmBuilder::new().await.unwrap();
            let blobfs = BlobfsRamdisk::start().await.context("starting blobfs").unwrap();

            let served_repo = Arc::new(Arc::clone(&repo).server().start().unwrap());

            let resolver_realm = ResolverForTest::realm_setup(
                &realm_builder,
                Arc::clone(&served_repo),
                self.repo_url.clone(),
                &blobfs,
            )
            .await
            .unwrap();

            let system_updater = realm_builder
                .add_child("system-updater", "#meta/system-updater.cm", ChildOptions::new())
                .await
                .unwrap();

            let service_reflector = realm_builder
                .add_local_child(
                    "system_updater_service_reflector",
                    move |handles| {
                        let mut fs = fuchsia_component::server::ServiceFs::new();
                        // Not necessary for updates, but without this system-updater will wait 30
                        // seconds trying to flush cobalt logs before logging an attempt error,
                        // and the test is torn down before then, so the error is lost. Also
                        // prevents spam of irrelevant error logs.
                        fs.dir("svc").add_fidl_service(move |stream| {
                            fasync::Task::spawn(
                                Arc::new(mock_metrics::MockMetricEventLoggerFactory::new())
                                    .run_logger_factory(stream),
                            )
                            .detach()
                        });
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

            realm_builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>(),
                        )
                        .from(&service_reflector)
                        .to(&system_updater),
                )
                .await
                .unwrap();

            // Set up paver and routes
            let paver = Arc::new(self.paver_builder.build());
            let paver_clone = Arc::clone(&paver);
            let mock_paver = realm_builder
                .add_local_child(
                    "paver",
                    move |handles| Box::pin(Self::run_mock_paver(handles, Arc::clone(&paver))),
                    ChildOptions::new(),
                )
                .await
                .unwrap();

            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.paver.Paver"))
                        .from(&mock_paver)
                        .to(&system_updater),
                )
                .await
                .unwrap();
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.paver.Paver"))
                        .from(&mock_paver)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();

            // Set up build-info and routes
            realm_builder
                .read_only_directory(
                    "build-info",
                    vec![&system_updater],
                    DirectoryContents::new().add_file("board", "test".as_bytes()),
                )
                .await
                .unwrap();

            // Set up pkg-resolver and pkg-cache routes
            realm_builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::protocol_by_name("fuchsia.pkg.PackageResolver-ota")
                                .as_("fuchsia.pkg.PackageResolver"),
                        )
                        .from(&resolver_realm.resolver)
                        .to(&system_updater),
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
                        .from(&resolver_realm.cache)
                        .to(&system_updater),
                )
                .await
                .unwrap();
            realm_builder
                .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                    name: "fuchsia.system-updater.ManifestPublicKeys".parse().unwrap(),
                    value: Vec::<String>::new().into(),
                }))
                .await
                .unwrap();
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::configuration(
                            "fuchsia.system-updater.ManifestPublicKeys",
                        ))
                        .from(Ref::self_())
                        .to(&system_updater),
                )
                .await
                .unwrap();
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::configuration(
                            "fuchsia.system-updater.ConcurrentBlobFetches",
                        ))
                        .capability(Capability::configuration(
                            "fuchsia.system-updater.ConcurrentPackageResolves",
                        ))
                        .capability(Capability::configuration(
                            "fuchsia.system-updater.VerifyExistingBlobs",
                        ))
                        .from(Ref::void())
                        .to(&system_updater),
                )
                .await
                .unwrap();

            // Make sure the component under test can log.
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                        .from(Ref::parent())
                        .to(&system_updater),
                )
                .await
                .unwrap();

            // Expose system_updater to the parent
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name(
                            "fuchsia.update.installer.Installer",
                        ))
                        .from(&system_updater)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();

            // Expose pkg-cache to these tests, for use by verify_packages
            realm_builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name("fuchsia.pkg.PackageCache"))
                        .from(&resolver_realm.cache)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();

            let realm_instance = realm_builder.build().await.unwrap();

            let installer_proxy = realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
            let paver_proxy = realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();

            let updater = Updater::new_with_proxies(installer_proxy, paver_proxy);

            let resolver = ResolverForTest::new(&realm_instance, blobfs, Arc::clone(&served_repo))
                .await
                .unwrap();

            UpdaterForTest {
                served_repo,
                paver: paver_clone,
                expected_blobfs_contents,
                update_merkle_root: *update.as_package().hash(),
                repo_url: self.repo_url,
                updater,
                resolver,
                realm_instance,
            }
        }

        #[cfg(test)]
        pub async fn build_and_run(self) -> UpdaterResult {
            self.build().await.run().await
        }
    }

    /// This wraps the `Updater` in order to reduce test boilerplate.
    /// Should be constructed using `UpdaterBuilder`.
    pub struct UpdaterForTest {
        #[expect(dead_code)]
        pub served_repo: Arc<ServedRepository>,
        pub paver: Arc<MockPaverService>,
        pub expected_blobfs_contents: BTreeSet<Hash>,
        pub update_merkle_root: Hash,
        #[expect(dead_code)]
        pub repo_url: fuchsia_url::RepositoryUrl,
        pub resolver: ResolverForTest,
        pub updater: Updater,
        pub realm_instance: RealmInstance,
    }

    impl UpdaterForTest {
        /// Run the system update, returning an `UpdaterResult` containing information about the
        /// result of the update.
        pub async fn run(mut self) -> UpdaterResult {
            let () = self.updater.install_update(None).await.expect("installing update");

            UpdaterResult {
                paver_events: self.paver.take_events(),
                resolver: self.resolver,
                expected_blobfs_contents: self.expected_blobfs_contents,
                realm_instance: self.realm_instance,
            }
        }
    }

    /// Contains information about the state of the system after the updater was run.
    pub struct UpdaterResult {
        /// All paver events received by the MockPaver during the update.
        pub paver_events: Vec<PaverEvent>,
        /// The resolver used by the updater.
        pub resolver: ResolverForTest,
        /// All the blobs that should be in blobfs after the update.
        pub expected_blobfs_contents: BTreeSet<Hash>,
        // The RealmInstance used to run this update, for introspection into component states.
        #[expect(dead_code)]
        pub realm_instance: RealmInstance,
    }

    impl UpdaterResult {
        /// Verify that all packages that should have been resolved by the update
        /// were resolved.
        pub async fn verify_packages(&self) {
            // Verify directly against blobfs to avoid any trickery pkg-resolver or pkg-cache may
            // engage in.
            let actual_contents =
                self.resolver.cache.blobfs.list_blobs().expect("Listing blobfs blobs");
            assert_eq!(actual_contents, self.expected_blobfs_contents);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::for_tests::UpdaterBuilder;
    use super::*;
    use fidl_fuchsia_paver::Asset;
    use fuchsia_pkg_testing::PackageBuilder;
    use mock_paver::PaverEvent;

    #[fuchsia::test]
    pub async fn test_updater() {
        let data = "hello world!".as_bytes();
        let test_package = PackageBuilder::new("test_package")
            .add_resource_at("bin/hello", "this is a test".as_bytes())
            .add_resource_at("data/file", "this is a file".as_bytes())
            .add_resource_at("meta/test_package.cm", "{}".as_bytes())
            .build()
            .await
            .expect("Building test_package");
        let updater = UpdaterBuilder::new()
            .await
            .paver(|p| {
                // Emulate ABR not being supported
                p.boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
            })
            .add_package(test_package)
            .fuchsia_image(data.to_vec(), Some(data.to_vec()))
            .recovery_image(data.to_vec(), Some(data.to_vec()));
        let result = updater.build_and_run().await;

        assert_eq!(
            result.paver_events,
            vec![
                PaverEvent::WriteAsset {
                    configuration: Configuration::A,
                    asset: Asset::Kernel,
                    payload: data.to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: Configuration::B,
                    asset: Asset::Kernel,
                    payload: data.to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: Configuration::A,
                    asset: Asset::VerifiedBootMetadata,
                    payload: data.to_vec()
                },
                PaverEvent::WriteAsset {
                    configuration: Configuration::B,
                    asset: Asset::VerifiedBootMetadata,
                    payload: data.to_vec()
                },
                // isolated-swd does not write recovery even if an image is provided.
                PaverEvent::DataSinkFlush,
            ]
        );

        let () = result.verify_packages().await;
    }
}
