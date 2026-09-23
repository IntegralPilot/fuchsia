// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![cfg(test)]

use self::SystemUpdaterInteraction::*;
use ::update_package::manifest::{self, AssetType, OtaManifest};
use anyhow::{Context as _, Error, anyhow};
use assert_matches::assert_matches;
use blobfs_ramdisk::BlobfsRamdisk;
use cobalt_sw_delivery_registry as metrics;
use fidl::endpoints::{DiscoverableProtocolMarker as _, ServerEnd};
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_hardware_power_statecontrol::{ShutdownAction, ShutdownOptions, ShutdownReason};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_net_http as fhttp;
use fidl_fuchsia_paver as paver;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fidl_fuchsia_pkg_internal as fpkg_internal;
use fidl_fuchsia_update_installer as finstaller;
use fidl_fuchsia_update_installer_ext::{
    Initiator, Options, UpdateAttempt, UpdateAttemptError, start_update,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_hash::Hash;
use fuchsia_pkg_testing::{
    SOURCE_EPOCH, make_current_epoch_json, make_epoch_json, make_packages_json,
};
use fuchsia_sync::Mutex;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use futures::channel::oneshot;
use futures::prelude::*;
use mock_metrics::MockMetricEventLoggerFactory;
use mock_paver::{MockPaverService, MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
use mock_reboot::MockRebootService;
use mock_resolver::MockResolverService;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::{File, create_dir};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use zx::Status;

mod board;
mod cancel;
mod cobalt_metrics;
mod commits_images;
mod epoch;
mod fetch_packages;
mod history;
mod mode_force_recovery;
mod mode_normal;
mod ota_manifest;
mod overwrites_blobs;
mod progress_reporting;
mod reboot_controller;
mod retained_packages;
mod update_package;
mod verify_existing_blobs;
mod writes_firmware;
mod writes_images;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const MATCHING_SHA256: &str = "e0705e68b0468289858b543f8a57f375a3b4f46391a72f94a28d82d6a3dacaa7";
const EMPTY_MERKLE: &str = "15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b";

fn empty_merkle() -> Hash {
    EMPTY_MERKLE.parse().unwrap()
}
// Generated with `openssl genpkey -algorithm ed25519 -out ed25519.pem`
// openssl pkey -in ed25519.pem -outform DER | tail -c 32 | xxd -p -c 32
const MANIFEST_PRIVATE_KEY: &str =
    "d450f0c0c1a70b89dbe023ec577420cf18c0a0ca27e79a1b9b8ae38d9045250e";
// openssl pkey -in ed25519.pem -pubout -outform DER | tail -c 32 | xxd -p -c 32
const MANIFEST_PUBLIC_KEY: &str =
    "1104bf9d6a201bc5c319573a80def62f826cc21e4cd99f43e52e9ee171463fd7";

pub fn make_images_json_zbi() -> String {
    serde_json::to_string(
        &::update_package::ImagePackagesManifest::builder()
            .fuchsia_package(
                ::update_package::ImageMetadata::new(
                    0,
                    EMPTY_SHA256.parse().unwrap(),
                    image_package_resource_url("update-images-fuchsia", 9, "zbi"),
                ),
                None,
            )
            .clone()
            .build(),
    )
    .unwrap()
}

pub fn make_images_json_recovery() -> String {
    serde_json::to_string(
        &::update_package::ImagePackagesManifest::builder()
            .recovery_package(
                ::update_package::ImageMetadata::new(
                    0,
                    EMPTY_SHA256.parse().unwrap(),
                    image_package_resource_url("update-images-recovery", 9, "zbi"),
                ),
                None,
            )
            .clone()
            .build(),
    )
    .unwrap()
}

fn make_manifest(blobs: impl IntoIterator<Item = ::update_package::manifest::Blob>) -> OtaManifest {
    OtaManifest {
        product_bundle_version: "1.2.3.4".parse().unwrap(),
        board: "x64".to_string(),
        epoch: SOURCE_EPOCH,
        mode: ::update_package::UpdateMode::Normal,
        blob_base_url: "https://fuchsia.com/blobs/1".into(),
        images: vec![manifest::Image {
            slot: manifest::Slot::AB,
            image_type: manifest::ImageType::Asset(AssetType::Zbi),
            blob: manifest::Blob { uncompressed_size: 0, fuchsia_merkle_root: empty_merkle() },
        }],
        blobs: blobs.into_iter().collect(),
    }
}

fn make_forced_recovery_manifest() -> OtaManifest {
    let mut manifest =
        OtaManifest { mode: ::update_package::UpdateMode::ForceRecovery, ..make_manifest([]) };
    manifest.images[0].slot = manifest::Slot::R;
    manifest
}

// A set of tags for interactions the system updater has with external services.
// We aren't tracking Cobalt interactions, since those may arrive out of order,
// and they are tested in individual tests which care about them specifically.
#[derive(Debug, PartialEq, Clone)]
enum SystemUpdaterInteraction {
    BlobfsSync,
    Gc,
    PackageResolve(String),
    Paver(PaverEvent),
    Reboot,
    ClearRetainedPackages,
    ReplaceRetainedPackages(Vec<fidl_fuchsia_pkg_ext::BlobId>),
    ClearRetainedBlobs,
    ReplaceRetainedBlobs(Vec<fidl_fuchsia_pkg_ext::BlobId>),
    OtaDownloader(OtaDownloaderEvent),
}

#[derive(Debug, PartialEq, Clone)]
enum OtaDownloaderEvent {
    FetchBlob(fidl_fuchsia_pkg_ext::BlobId),
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
enum Protocol {
    PackageResolver,
    PackageCache,
    SpaceManager,
    Paver,
    Reboot,
    FuchsiaMetrics,
    RetainedPackages,
    RetainedBlobs,
    OtaDownloader,
    HttpLoader,
}

type SystemUpdaterInteractions = Arc<Mutex<Vec<SystemUpdaterInteraction>>>;

struct TestEnvBuilder {
    paver_service_builder: MockPaverServiceBuilder,
    blocked_protocols: HashSet<Protocol>,
    mount_data: bool,
    history: Option<serde_json::Value>,
    system_image_hash: Option<fuchsia_hash::Hash>,
    ota_manifest: Option<Vec<u8>>,
    blobs: HashMap<Hash, Vec<u8>>,
    verify_existing_blobs: bool,
    blob_reader_mock:
        Option<Box<dyn FnMut(ffxfs::BlobReaderRequestStream) + Send + Sync + 'static>>,
    blob_implementation: Option<blobfs_ramdisk::Implementation>,
}

impl TestEnvBuilder {
    fn new() -> Self {
        TestEnvBuilder {
            paver_service_builder: MockPaverServiceBuilder::new(),
            blocked_protocols: HashSet::new(),
            mount_data: true,
            history: None,
            system_image_hash: None,
            ota_manifest: None,
            blobs: HashMap::new(),
            verify_existing_blobs: false,
            blob_reader_mock: None,
            blob_implementation: None,
        }
    }

    fn paver_service<F>(mut self, f: F) -> Self
    where
        F: FnOnce(MockPaverServiceBuilder) -> MockPaverServiceBuilder,
    {
        self.paver_service_builder = f(self.paver_service_builder);
        self
    }

    fn unregister_protocol(mut self, protocol: Protocol) -> Self {
        self.blocked_protocols.insert(protocol);
        self
    }

    fn mount_data(mut self, mount_data: bool) -> Self {
        self.mount_data = mount_data;
        self
    }

    fn system_image_hash(mut self, system_image: fuchsia_hash::Hash) -> Self {
        assert_eq!(self.system_image_hash, None);
        self.system_image_hash = Some(system_image);
        self
    }

    fn ota_manifest(mut self, manifest: OtaManifest) -> Self {
        let key_bytes = hex::decode(MANIFEST_PRIVATE_KEY).unwrap();
        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&key_bytes).unwrap();
        self.ota_manifest =
            Some(::update_package::signed_manifest::generate(manifest, &key_pair, None).unwrap());
        self
    }

    fn ota_manifest_raw(mut self, manifest_bytes: impl Into<Vec<u8>>) -> Self {
        self.ota_manifest = Some(manifest_bytes.into());
        self
    }

    fn blob(mut self, hash: Hash, content: Vec<u8>) -> Self {
        self.blobs.insert(hash, content);
        self
    }

    fn verify_existing_blobs(mut self, verify_existing_blobs: bool) -> Self {
        self.verify_existing_blobs = verify_existing_blobs;
        self
    }

    fn mock_blob_reader(
        mut self,
        mock: impl FnMut(ffxfs::BlobReaderRequestStream) + Send + Sync + 'static,
    ) -> Self {
        self.blob_reader_mock = Some(Box::new(mock));
        self
    }

    fn cpp_blobfs(mut self) -> Self {
        assert_eq!(self.blob_implementation, None);
        self.blob_implementation = Some(blobfs_ramdisk::Implementation::CppBlobfs);
        self
    }

    async fn build(self) -> TestEnv {
        let Self {
            paver_service_builder,
            blocked_protocols,
            mount_data,
            history,
            system_image_hash,
            ota_manifest,
            blobs,
            verify_existing_blobs,
            blob_reader_mock,
            blob_implementation,
        } = self;

        let test_dir = TempDir::new().expect("create test tempdir");

        let data_path = test_dir.path().join("data");
        create_dir(&data_path).expect("create data dir");

        let build_info_path = test_dir.path().join("build-info");
        create_dir(&build_info_path).expect("create build-info dir");

        // Optionally write the pre-configured update history.
        if let Some(history) = history {
            serde_json::to_writer(
                File::create(data_path.join("update_history.json")).unwrap(),
                &history,
            )
            .unwrap()
        }

        let mut fs = ServiceFs::new();
        let data = fuchsia_fs::directory::open_in_namespace(
            data_path.to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();
        let build_info = fuchsia_fs::directory::open_in_namespace(
            build_info_path.to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .unwrap();

        fs.add_remote("data", data);
        fs.dir("config").add_remote("build-info", build_info);

        if let Some(hash) = system_image_hash {
            let system_image_path = test_dir.path().join("system");
            create_dir(&system_image_path).expect("crate system-image dir");
            let mut meta = File::create(system_image_path.join("meta")).unwrap();
            let () = meta.write_all(hash.to_string().as_bytes()).unwrap();
            let system = fuchsia_fs::directory::open_in_namespace(
                system_image_path.to_str().unwrap(),
                fio::PERM_READABLE,
            )
            .unwrap();
            fs.add_remote("system", system);
        }

        let mut blobfs_builder = BlobfsRamdisk::builder();
        if let Some(blob_implementation) = blob_implementation {
            blobfs_builder = blobfs_builder.implementation(blob_implementation);
        }
        let blobfs = blobfs_builder.start().await.unwrap();
        let blobfs = Arc::new(blobfs);
        fs.add_remote("blob-svc", blobfs.svc_dir().unwrap());

        let use_blob_reader_mock = blob_reader_mock.is_some();
        if let Some(mock) = blob_reader_mock {
            fs.dir("svc").add_fidl_service(mock);
        }

        // A buffer to store all the interactions the system-updater has with external services.
        let interactions = Arc::new(Mutex::new(vec![]));
        let interactions_paver_clone = Arc::clone(&interactions);
        let paver_service = Arc::new(
            paver_service_builder
                .event_hook(move |event| {
                    interactions_paver_clone.lock().push(Paver(event.clone()));
                })
                .build(),
        );

        let resolver = {
            let interactions = Arc::clone(&interactions);
            Arc::new(MockResolverService::new(Some(Box::new(move |resolved_url: &str| {
                interactions.lock().push(PackageResolve(resolved_url.to_owned()))
            }))))
        };

        let reboot_service = {
            let interactions = Arc::clone(&interactions);
            Arc::new(MockRebootService::new(Box::new(move |options| {
                assert_eq!(
                    options,
                    ShutdownOptions {
                        action: Some(ShutdownAction::Reboot),
                        reasons: Some(vec![ShutdownReason::SystemUpdate]),
                        ..Default::default()
                    }
                );
                interactions.lock().push(Reboot);
                Ok(())
            })))
        };

        let cache_service = Arc::new(MockCacheService::new(Arc::clone(&interactions)));
        let logger_factory = Arc::new(MockMetricEventLoggerFactory::new());
        let space_service = Arc::new(MockSpaceService::new(Arc::clone(&interactions)));
        let retained_packages_service =
            Arc::new(MockRetainedPackagesService::new(Arc::clone(&interactions)));
        let retained_blobs_service =
            Arc::new(MockRetainedBlobsService::new(Arc::clone(&interactions)));
        let ota_downloader_service = Arc::new(MockOtaDownloaderService::new(
            Arc::clone(&interactions),
            blobs,
            Arc::clone(&blobfs),
        ));
        let http_loader_service = Arc::new(MockHttpLoaderService::new(ota_manifest));

        // Register the mock services with the test environment service provider.
        {
            let resolver = Arc::clone(&resolver);
            let paver_service = Arc::clone(&paver_service);
            let reboot_service = Arc::clone(&reboot_service);
            let cache_service = Arc::clone(&cache_service);
            let logger_factory = Arc::clone(&logger_factory);
            let space_service = Arc::clone(&space_service);
            let retained_packages_service = Arc::clone(&retained_packages_service);
            let retained_blobs_service = Arc::clone(&retained_blobs_service);
            let ota_downloader_service = Arc::clone(&ota_downloader_service);
            let http_loader_service = Arc::clone(&http_loader_service);

            let should_register = |protocol: Protocol| !blocked_protocols.contains(&protocol);

            if should_register(Protocol::PackageResolver) {
                fs.dir("svc").add_fidl_service(
                    move |stream: fpkg::PackageResolverRequestStream| {
                        fasync::Task::spawn(
                            Arc::clone(&resolver).run_resolver_service(stream).unwrap_or_else(
                                |e| panic!("error running resolver service: {e:?}"),
                            ),
                        )
                        .detach()
                    },
                );
            }
            if should_register(Protocol::Paver) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&paver_service)
                            .run_paver_service(stream)
                            .unwrap_or_else(|e| panic!("error running paver service: {e:?}")),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::Reboot) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&reboot_service)
                            .run_reboot_service(stream)
                            .unwrap_or_else(|e| panic!("error running reboot service: {e:?}")),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::PackageCache) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&cache_service)
                            .run_cache_service(stream)
                            .unwrap_or_else(|e| panic!("error running cache service: {e:?}")),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::FuchsiaMetrics) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(Arc::clone(&logger_factory).run_logger_factory(stream))
                        .detach()
                });
            }
            if should_register(Protocol::SpaceManager) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&space_service)
                            .run_space_service(stream)
                            .unwrap_or_else(|e| panic!("error running space service: {e:?}")),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::RetainedPackages) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&retained_packages_service)
                            .run_retained_packages_service(stream)
                            .unwrap_or_else(|e| {
                                panic!("error running retained packages service: {e:?}")
                            }),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::RetainedBlobs) {
                fs.dir("svc").add_fidl_service(move |stream| {
                    fasync::Task::spawn(
                        Arc::clone(&retained_blobs_service)
                            .run_retained_blobs_service(stream)
                            .unwrap_or_else(|e| {
                                panic!("error running retained blobs service: {e:?}")
                            }),
                    )
                    .detach()
                });
            }
            if should_register(Protocol::OtaDownloader) {
                fs.dir("svc").add_fidl_service(
                    move |stream: fpkg_internal::OtaDownloaderRequestStream| {
                        fasync::Task::spawn(
                            Arc::clone(&ota_downloader_service)
                                .run_ota_downloader_service(stream)
                                .unwrap_or_else(|e| {
                                    panic!("error running ota downloader service: {e:?}")
                                }),
                        )
                        .detach()
                    },
                );
            }
            if should_register(Protocol::HttpLoader) {
                fs.dir("svc").add_fidl_service(move |stream: fhttp::LoaderRequestStream| {
                    fasync::Task::spawn(
                        Arc::clone(&http_loader_service)
                            .run_http_loader_service(stream)
                            .unwrap_or_else(|e| panic!("error running http loader service: {e:?}")),
                    )
                    .detach()
                });
            }
        }

        let fs_holder = Mutex::new(Some(fs));
        let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
        let system_updater = builder
            .add_child(
                "system_updater",
                "#meta/system-updater-isolated.cm",
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();
        let fake_capabilities = builder
            .add_local_child(
                "fake_capabilities",
                move |mock_handles| {
                    let mut rfs = fs_holder
                        .lock()
                        .take()
                        .expect("mock component should only be launched once");
                    async {
                        let _ = &mock_handles;
                        rfs.serve_connection(mock_handles.outgoing_dir).unwrap();
                        let () = rfs.collect().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                    .from(Ref::parent())
                    .to(&system_updater),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<finstaller::InstallerMarker>())
                    .from(&system_updater)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<
                        fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker,
                    >())
                    .capability(Capability::protocol::<paver::PaverMarker>())
                    .capability(Capability::protocol::<fpkg::PackageCacheMarker>())
                    .capability(Capability::protocol::<fpkg::PackageResolverMarker>())
                    .capability(Capability::protocol::<fpkg::RetainedPackagesMarker>())
                    .capability(Capability::protocol::<fpkg::RetainedBlobsMarker>())
                    .capability(Capability::protocol::<fhttp::LoaderMarker>())
                    .capability(Capability::protocol::<fpkg_internal::OtaDownloaderMarker>())
                    .capability(Capability::protocol::<fpkg_gc::ManagerMarker>())
                    .capability(Capability::protocol::<
                        fidl_fuchsia_hardware_power_statecontrol::AdminMarker,
                    >())
                    .capability(if use_blob_reader_mock {
                        Capability::protocol::<ffxfs::BlobReaderMarker>()
                    } else {
                        Capability::protocol::<ffxfs::BlobReaderMarker>()
                            .path(format!("/blob-svc/{}", ffxfs::BlobReaderMarker::PROTOCOL_NAME))
                    })
                    .capability(
                        Capability::protocol::<ffxfs::BlobCreatorMarker>()
                            .path(format!("/blob-svc/{}", ffxfs::BlobCreatorMarker::PROTOCOL_NAME)),
                    )
                    .capability(
                        Capability::directory("build-info")
                            .path("/config/build-info")
                            .rights(fio::R_STAR_DIR),
                    )
                    .from(&fake_capabilities)
                    .to(&system_updater),
            )
            .await
            .unwrap();

        if mount_data {
            builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::directory("data").path("/data").rights(fio::RW_STAR_DIR),
                        )
                        .from(&fake_capabilities)
                        .to(&system_updater),
                )
                .await
                .unwrap();
        }

        if system_image_hash.is_some() {
            builder
                .add_route(
                    Route::new()
                        .capability(
                            Capability::directory("system").path("/system").rights(fio::R_STAR_DIR),
                        )
                        .from(&fake_capabilities)
                        .to(&system_updater),
                )
                .await
                .unwrap();
        }

        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.system-updater.ManifestPublicKeys".parse().unwrap(),
                value: vec![MANIFEST_PUBLIC_KEY].into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.system-updater.VerifyExistingBlobs".parse().unwrap(),
                value: verify_existing_blobs.into(),
            }))
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.system-updater.ManifestPublicKeys",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.system-updater.VerifyExistingBlobs",
                    ))
                    .from(Ref::self_())
                    .to(&system_updater),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.system-updater.ConcurrentBlobFetches",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.system-updater.ConcurrentPackageResolves",
                    ))
                    .from(Ref::void())
                    .to(&system_updater),
            )
            .await
            .unwrap();

        let realm_instance = builder.build().await.unwrap();

        TestEnv {
            realm_instance,
            resolver,
            http_loader_service,
            ota_downloader_service,
            _paver_service: paver_service,
            _reboot_service: reboot_service,
            cache_service,
            metric_event_logger_factory: logger_factory,
            _space_service: space_service,
            _test_dir: test_dir,
            data_path,
            build_info_path,
            interactions,
            blobfs,
        }
    }
}

struct TestEnv {
    realm_instance: RealmInstance,
    resolver: Arc<MockResolverService>,
    http_loader_service: Arc<MockHttpLoaderService>,
    ota_downloader_service: Arc<MockOtaDownloaderService>,
    _paver_service: Arc<MockPaverService>,
    _reboot_service: Arc<MockRebootService>,
    cache_service: Arc<MockCacheService>,
    metric_event_logger_factory: Arc<MockMetricEventLoggerFactory>,
    _space_service: Arc<MockSpaceService>,
    _test_dir: TempDir,
    data_path: PathBuf,
    build_info_path: PathBuf,
    interactions: SystemUpdaterInteractions,
    blobfs: Arc<BlobfsRamdisk>,
}

impl TestEnv {
    fn builder() -> TestEnvBuilder {
        TestEnvBuilder::new()
    }

    fn take_interactions(&self) -> Vec<SystemUpdaterInteraction> {
        std::mem::take(&mut *self.interactions.lock())
    }

    #[track_caller]
    fn assert_interactions(&self, expected: impl IntoIterator<Item = SystemUpdaterInteraction>) {
        assert_eq!(self.take_interactions(), expected.into_iter().collect::<Vec<_>>());
    }

    #[track_caller]
    fn assert_unordered_interactions(
        &self,
        expected_begin: impl IntoIterator<Item = SystemUpdaterInteraction>,
        expected_unordered_middle: impl IntoIterator<Item = SystemUpdaterInteraction>,
        expected_end: impl IntoIterator<Item = SystemUpdaterInteraction>,
    ) {
        let all_events = self.take_interactions();

        let expected_begin = expected_begin.into_iter().collect::<Vec<_>>();
        let all_events_start = all_events[..expected_begin.len()].to_vec();
        assert_eq!(all_events_start, expected_begin);

        let expected_end = expected_end.into_iter().collect::<Vec<_>>();
        let all_events_end = all_events[all_events.len() - expected_end.len()..].to_vec();
        assert_eq!(all_events_end, expected_end);

        let expected_unordered_middle = expected_unordered_middle.into_iter().collect::<Vec<_>>();
        assert!(
            all_events.len()
                == expected_begin.len() + expected_end.len() + expected_unordered_middle.len()
        );

        let all_events_middle = all_events
            [expected_begin.len()..expected_begin.len() + expected_unordered_middle.len()]
            .to_vec();

        for event in expected_unordered_middle {
            assert!(
                all_events_middle.contains(&event),
                "event {event:?} not found in {all_events_middle:#?}",
            );
        }
    }

    /// Set the name of the board that system-updater is running on.
    fn set_board_name(&self, board: impl AsRef<str>) {
        // Write the "board" file into the build-info directory.
        let mut file = File::create(self.build_info_path.join("board")).expect("create board file");
        file.write_all(board.as_ref().as_bytes()).expect("write board file");
    }

    /// Set the version of the build that system-updater is running on.
    fn set_build_version(&self, version: impl AsRef<str>) {
        // Write the "version" file into the build-info directory.
        let mut file =
            File::create(self.build_info_path.join("version")).expect("create version file");
        file.write_all(version.as_ref().as_bytes()).expect("write version file");
    }

    fn read_history(&self) -> Option<serde_json::Value> {
        match File::open(self.data_path.join("update_history.json")) {
            Ok(f) => Some(serde_json::from_reader(f).unwrap()),
            Err(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "io error: {:?}", e);
                None
            }
        }
    }

    fn write_history(&self, history: serde_json::Value) {
        serde_json::to_writer(
            File::create(self.data_path.join("update_history.json")).unwrap(),
            &history,
        )
        .unwrap()
    }

    async fn write_to_blobfs(&self, hash: Hash, content: &[u8]) {
        self.blobfs.write_blob(hash, content).await.unwrap();
    }

    async fn start_update(&self) -> Result<UpdateAttempt, UpdateAttemptError> {
        self.start_update_with_options(UPDATE_PKG_URL, default_options(), None).await
    }

    fn ota_manifest_size(&self) -> usize {
        self.http_loader_service.manifest.as_ref().unwrap().len()
    }

    async fn start_packageless_update(&self) -> Result<UpdateAttempt, UpdateAttemptError> {
        self.start_update_with_options(MANIFEST_URL, default_options(), None).await
    }

    async fn start_update_with_options(
        &self,
        url: &str,
        options: Options,
        reboot_controller_server_end: Option<ServerEnd<finstaller::RebootControllerMarker>>,
    ) -> Result<UpdateAttempt, UpdateAttemptError> {
        let url: http::Uri = url.parse().unwrap();

        start_update(&url, options, &self.installer_proxy(), reboot_controller_server_end).await
    }

    async fn run_update(&self) -> Result<(), Error> {
        self.run_update_with_options(UPDATE_PKG_URL, default_options()).await
    }

    async fn run_packageless_update(&self) -> Result<(), Error> {
        self.run_update_with_options(MANIFEST_URL, default_options()).await
    }

    async fn run_update_with_options(&self, url: &str, options: Options) -> Result<(), Error> {
        let mut update_attempt = self.start_update_with_options(url, options, None).await?;

        while let Some(state) =
            update_attempt.try_next().await.context("fetching next update state")?
        {
            if state.is_success() {
                // Wait until the stream terminates before returning so that interactions will
                // include reboot.
                assert_matches!(update_attempt.try_next().await, Ok(None));
                return Ok(());
            } else if state.is_failure() {
                // Wait until the stream terminates before returning so that any subsequent
                // attempts won't get already in progress error.
                assert_matches!(update_attempt.try_next().await, Ok(None));
                return Err(anyhow!("update attempt failed"));
            }
        }

        Err(anyhow!("unexpected end of update attempt"))
    }

    /// Opens a connection to the installer fidl service.
    fn installer_proxy(&self) -> finstaller::InstallerProxy {
        self.realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap()
    }

    fn http_loader_service(&self) -> &MockHttpLoaderService {
        &self.http_loader_service
    }

    async fn get_ota_metrics(&self) -> OtaMetrics {
        let loggers = self.metric_event_logger_factory.clone_loggers();
        assert_eq!(loggers.len(), 1);
        let logger = loggers.into_iter().next().unwrap();
        let events = logger.clone_metric_events();
        OtaMetrics::from_events(events)
    }
}

struct MockCacheService {
    sync_response: Mutex<Option<Result<(), Status>>>,
    interactions: SystemUpdaterInteractions,
}
impl MockCacheService {
    fn new(interactions: SystemUpdaterInteractions) -> Self {
        Self { sync_response: Mutex::new(None), interactions }
    }

    fn set_sync_response(&self, response: Result<(), Status>) {
        self.sync_response.lock().replace(response);
    }

    async fn run_cache_service(
        self: Arc<Self>,
        mut stream: fidl_fuchsia_pkg::PackageCacheRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await.expect("received request") {
            match event {
                fidl_fuchsia_pkg::PackageCacheRequest::Sync { responder } => {
                    self.interactions.lock().push(BlobfsSync);
                    responder.send(
                        self.sync_response.lock().unwrap_or(Ok(())).map_err(|s| s.into_raw()),
                    )?;
                }
                other => panic!("unsupported PackageCache request: {other:?}"),
            }
        }

        Ok(())
    }
}

struct MockSpaceService {
    interactions: SystemUpdaterInteractions,
}
impl MockSpaceService {
    fn new(interactions: SystemUpdaterInteractions) -> Self {
        Self { interactions }
    }

    async fn run_space_service(
        self: Arc<Self>,
        mut stream: fidl_fuchsia_pkg_garbagecollector::ManagerRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await.expect("received request") {
            let fidl_fuchsia_pkg_garbagecollector::ManagerRequest::Gc { responder } = event else {
                return Err(anyhow!("Unknown method called on garbage collector."));
            };
            self.interactions.lock().push(Gc);
            responder.send(Ok(()))?;
        }

        Ok(())
    }
}

struct MockRetainedPackagesService {
    interactions: SystemUpdaterInteractions,
}
impl MockRetainedPackagesService {
    fn new(interactions: SystemUpdaterInteractions) -> Self {
        Self { interactions }
    }

    async fn run_retained_packages_service(
        self: Arc<Self>,
        mut stream: fidl_fuchsia_pkg::RetainedPackagesRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await.expect("received request") {
            match event {
                fidl_fuchsia_pkg::RetainedPackagesRequest::Clear { responder } => {
                    self.interactions.lock().push(ClearRetainedPackages);
                    responder.send().unwrap();
                }
                fidl_fuchsia_pkg::RetainedPackagesRequest::Replace { iterator, responder } => {
                    let blobs = collect_blob_id_iterator(iterator.into_proxy()).await;
                    self.interactions.lock().push(ReplaceRetainedPackages(blobs));
                    responder.send().unwrap();
                }
            }
        }

        Ok(())
    }
}

struct MockRetainedBlobsService {
    interactions: SystemUpdaterInteractions,
}
impl MockRetainedBlobsService {
    fn new(interactions: SystemUpdaterInteractions) -> Self {
        Self { interactions }
    }

    async fn run_retained_blobs_service(
        self: Arc<Self>,
        mut stream: fidl_fuchsia_pkg::RetainedBlobsRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await.expect("received request") {
            match event {
                fidl_fuchsia_pkg::RetainedBlobsRequest::Clear { responder } => {
                    self.interactions.lock().push(ClearRetainedBlobs);
                    responder.send().unwrap();
                }
                fidl_fuchsia_pkg::RetainedBlobsRequest::Replace { iterator, responder } => {
                    let blobs = collect_blob_id_iterator(iterator.into_proxy()).await;
                    self.interactions.lock().push(ReplaceRetainedBlobs(blobs));
                    responder.send().unwrap();
                }
            }
        }

        Ok(())
    }
}

async fn collect_blob_id_iterator(
    iterator: fpkg::BlobIdIteratorProxy,
) -> Vec<fidl_fuchsia_pkg_ext::BlobId> {
    let mut blobs = vec![];
    loop {
        let new_blobs = iterator.next().await.unwrap();
        if new_blobs.is_empty() {
            break;
        }
        blobs.extend(new_blobs.into_iter().map(fidl_fuchsia_pkg_ext::BlobId::from));
    }
    blobs
}

type OtaDownloaderResultSender = oneshot::Sender<Result<u64, fpkg::ResolveError>>;
struct MockOtaDownloaderService {
    interactions: SystemUpdaterInteractions,
    blobs: HashMap<Hash, Vec<u8>>,
    blobfs: Arc<BlobfsRamdisk>,
    fetch_blob_response: Mutex<Option<Result<u64, fpkg::ResolveError>>>,
    blockers: Mutex<HashMap<Hash, oneshot::Sender<OtaDownloaderResultSender>>>,
}

impl MockOtaDownloaderService {
    fn new(
        interactions: SystemUpdaterInteractions,
        blobs: HashMap<Hash, Vec<u8>>,
        blobfs: Arc<BlobfsRamdisk>,
    ) -> Self {
        Self {
            interactions,
            blobs,
            blobfs,
            fetch_blob_response: Mutex::new(None),
            blockers: Mutex::new(HashMap::new()),
        }
    }

    fn block_once(&self, hash: Hash) -> oneshot::Receiver<OtaDownloaderResultSender> {
        let (sender, receiver) = oneshot::channel();
        self.blockers.lock().insert(hash, sender);
        receiver
    }

    fn set_fetch_blob_response(&self, response: Result<u64, fpkg::ResolveError>) {
        self.fetch_blob_response.lock().replace(response);
    }

    async fn run_ota_downloader_service(
        self: Arc<Self>,
        stream: fpkg_internal::OtaDownloaderRequestStream,
    ) -> Result<(), Error> {
        stream
            .map_err(Error::new)
            .try_for_each_concurrent(None, |event| {
                let this = Arc::clone(&self);
                async move {
                    match event {
                        fpkg_internal::OtaDownloaderRequest::FetchBlob {
                            hash,
                            base_url,
                            overwrite_existing,
                            responder,
                        } => {
                            this.interactions
                                .lock()
                                .push(OtaDownloader(OtaDownloaderEvent::FetchBlob(hash.into())));
                            assert_eq!(base_url, "https://fuchsia.com/blobs/1");
                            let hash = fidl_fuchsia_pkg_ext::BlobId::from(hash).into();
                            let blocker = this.blockers.lock().remove(&hash);
                            if let Some(blocker) = blocker {
                                let (resume_sender, resume_receiver) = oneshot::channel();
                                // If the test dropped the receiver, it doesn't want to block.
                                if blocker.send(resume_sender).is_ok()
                                    && let Ok(response) = resume_receiver.await
                                {
                                    responder.send(response)?;
                                    return Ok(());
                                }
                            }
                            if let Some(response) = *this.fetch_blob_response.lock() {
                                responder.send(response)?;
                                return Ok(());
                            }
                            if let Some(content) = this.blobs.get(&hash) {
                                let () = this
                                    .blobfs
                                    .write_blob_with_overwrite(hash, content, overwrite_existing)
                                    .await
                                    .unwrap();
                                responder.send(Ok(content.len() as u64))?;
                            } else {
                                responder.send(Err(fpkg::ResolveError::BlobNotFound))?;
                            }
                        }
                    }
                    Ok(())
                }
            })
            .await
    }
}

type ResumeHandle = oneshot::Sender<()>;

struct MockHttpLoaderService {
    manifest: Option<Vec<u8>>,
    blocker: Mutex<Option<oneshot::Sender<ResumeHandle>>>,
    received_headers: Mutex<Vec<Vec<fhttp::Header>>>,
}

impl MockHttpLoaderService {
    fn new(manifest: Option<Vec<u8>>) -> Self {
        Self { manifest, blocker: Mutex::new(None), received_headers: Mutex::new(Vec::new()) }
    }

    fn received_headers(&self) -> Vec<Vec<fhttp::Header>> {
        std::mem::take(&mut *self.received_headers.lock())
    }

    fn block_once(&self) -> oneshot::Receiver<ResumeHandle> {
        let (sender, receiver) = oneshot::channel();
        *self.blocker.lock() = Some(sender);
        receiver
    }

    async fn run_http_loader_service(
        self: Arc<Self>,
        mut stream: fhttp::LoaderRequestStream,
    ) -> Result<(), Error> {
        while let Some(event) = stream.try_next().await? {
            match event {
                fhttp::LoaderRequest::Fetch { request, responder } => {
                    let blocker = self.blocker.lock().take();
                    if let Some(blocker) = blocker {
                        let (resume_sender, resume_receiver) = oneshot::channel();
                        // If the test dropped the receiver, it doesn't want to block.
                        if blocker.send(resume_sender).is_ok() {
                            let _ = resume_receiver.await;
                        }
                    }

                    if let Some(headers) = &request.headers {
                        self.received_headers.lock().push(headers.clone());
                    }

                    let url = request.url.unwrap();
                    let response = if url == MANIFEST_URL {
                        let manifest_bytes = self.manifest.clone().unwrap();
                        let range_header = request.headers.as_ref().and_then(|headers| {
                            headers
                                .iter()
                                .find(|h| h.name.eq_ignore_ascii_case(b"range"))
                                .map(|h| &h.value)
                        });

                        if let Some(range_val) = range_header {
                            let range_str = str::from_utf8(range_val).unwrap();
                            let range_str = range_str.strip_prefix("bytes=").unwrap();
                            let (start_str, end_str) = range_str.split_once('-').unwrap();
                            let start: usize = start_str.parse().unwrap();
                            let end: usize = end_str.parse().unwrap();

                            if start < manifest_bytes.len()
                                && end < manifest_bytes.len()
                                && start <= end
                            {
                                let (client, server) = zx::Socket::create_stream();
                                let mut server = fasync::Socket::from_socket(server);
                                fasync::Task::spawn(async move {
                                    server.write_all(&manifest_bytes[start..=end]).await.unwrap()
                                })
                                .detach();

                                fhttp::Response {
                                    body: Some(client),
                                    status_code: Some(206),
                                    ..Default::default()
                                }
                            } else {
                                fhttp::Response { status_code: Some(416), ..Default::default() }
                            }
                        } else {
                            let (client, server) = zx::Socket::create_stream();
                            let mut server = fasync::Socket::from_socket(server);
                            fasync::Task::spawn(async move {
                                server.write_all(&manifest_bytes).await.unwrap()
                            })
                            .detach();

                            fhttp::Response {
                                body: Some(client),
                                status_code: Some(200),
                                ..Default::default()
                            }
                        }
                    } else {
                        fhttp::Response { status_code: Some(404), ..Default::default() }
                    };
                    let _ = responder.send(response);
                }
                request => panic!("unsupported http loader request {request:?}"),
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Debug)]
struct OtaMetrics {
    initiator: u32,
    phase: u32,
    status_code: u32,
}

impl OtaMetrics {
    fn from_events(mut events: Vec<fidl_fuchsia_metrics::MetricEvent>) -> Self {
        events.sort_by_key(|e| e.metric_id);

        // expecting one of each event
        assert_eq!(
            events.iter().map(|e| e.metric_id).collect::<Vec<_>>(),
            vec![
                metrics::OTA_START_MIGRATED_METRIC_ID,
                metrics::OTA_RESULT_ATTEMPTS_MIGRATED_METRIC_ID,
                metrics::OTA_RESULT_DURATION_MIGRATED_METRIC_ID,
            ]
        );

        // we just asserted that we have the exact 4 things we're expecting, so unwrap them
        let mut iter = events.into_iter();
        let start = iter.next().unwrap();
        let attempt = iter.next().unwrap();
        let duration = iter.next().unwrap();

        // Some basic sanity checks follow
        assert_eq!(attempt.payload, fidl_fuchsia_metrics::MetricEventPayload::Count(1));

        let fidl_fuchsia_metrics::MetricEvent { event_codes, .. } = attempt;

        // metric event_codes and component should line up across all 3 result metrics
        assert_eq!(&duration.event_codes, &event_codes);

        // OtaStart only has initiator and hour_of_day, so just check initiator.
        assert_eq!(start.event_codes[0], event_codes[0]);

        assert_eq!(event_codes.len(), 3);
        let initiator = event_codes[0];
        let phase = event_codes[1];
        let status_code = event_codes[2];

        match duration.payload {
            fidl_fuchsia_metrics::MetricEventPayload::IntegerValue(_time) => {
                // Ignore the value since timing is not predictable.
            }
            other => {
                panic!("unexpected duration payload {other:?}");
            }
        }

        Self { initiator, phase, status_code }
    }
}

#[macro_export]
macro_rules! merkle_str {
    ($seed:literal) => {{
        $crate::merkle_str!(@check $seed);
        $crate::merkle_str!(@unchecked $seed)
    }};
    (@check $seed:literal) => {
        assert_eq!($seed.len(), 2)
    };
    (@unchecked $seed:literal) => {
        concat!(
            $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed,
            $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed,
            $seed, $seed, $seed, $seed, $seed, $seed, $seed, $seed,
        )
    };
}

#[macro_export]
macro_rules! pinned_pkg_url {
    ($path:literal, $merkle_seed:literal) => {{
        $crate::merkle_str!(@check $merkle_seed);
        concat!("fuchsia-pkg://fuchsia.com/", $path, "?hash=", $crate::merkle_str!(@unchecked $merkle_seed))
    }};
}

const UPDATE_HASH: &str = "00112233445566778899aabbccddeeffffeeddccbbaa99887766554433221100";
const SYSTEM_IMAGE_HASH: &str = "42ade6f4fd51636f70c68811228b4271ed52c4eb9a647305123b4f4d0741f296";
const SYSTEM_IMAGE_URL: &str = "fuchsia-pkg://fuchsia.com/system_image/0?hash=42ade6f4fd51636f70c68811228b4271ed52c4eb9a647305123b4f4d0741f296";
const UPDATE_PKG_URL: &str = "fuchsia-pkg://fuchsia.com/update";
const MANIFEST_URL: &str = "https://fuchsia.com/ota_manifest";
const UPDATE_PKG_URL_PINNED: &str = "fuchsia-pkg://fuchsia.com/update?hash=00112233445566778899aabbccddeeffffeeddccbbaa99887766554433221100";

fn resolved_urls(interactions: SystemUpdaterInteractions) -> Vec<String> {
    (*interactions.lock())
        .iter()
        .filter_map(|interaction| match interaction {
            PackageResolve(package_url) => Some(package_url.clone()),
            _ => None,
        })
        .collect()
}

fn default_options() -> Options {
    Options {
        initiator: Initiator::User,
        allow_attach_to_existing_attempt: true,
        should_write_recovery: true,
        manifest_range: None,
        manifest_headers: vec![],
    }
}

fn force_recovery_json() -> String {
    json!({
      "version": "1",
      "content": {
        "mode": "force-recovery",
      }
    })
    .to_string()
}

fn image_package_resource_url(name: &str, hash: u8, resource: &str) -> AbsoluteComponentUrl {
    format!("fuchsia-pkg://fuchsia.com/{name}/0?hash={}#{resource}", hashstr(hash)).parse().unwrap()
}

fn image_package_url_to_string(name: &str, hash: u8) -> String {
    format!("fuchsia-pkg://fuchsia.com/{name}/0?hash={}", hashstr(hash)).parse().unwrap()
}

fn sha256(n: u8) -> fuchsia_hash::Sha256 {
    fuchsia_hash::Sha256::from([n; 32])
}

fn hash(n: u8) -> Hash {
    Hash::from([n; 32])
}

fn hashstr(n: u8) -> String {
    hash(n).to_string()
}

/// Actions the system-updater takes at the beginning of each OTA attempt.
fn initial_interactions() -> impl Iterator<Item = SystemUpdaterInteraction> {
    [
        // Hashes the ZBI and vbmeta from the current configuration to include in the history.
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::VerifiedBootMetadata,
        }),
        Paver(PaverEvent::ReadAsset {
            configuration: paver::Configuration::A,
            asset: paver::Asset::Kernel,
        }),
        // Makes sure the current configuration is healthy and the other is unbootable.
        Paver(PaverEvent::QueryCurrentConfiguration),
        Paver(PaverEvent::QueryConfigurationStatus { configuration: paver::Configuration::A }),
        Paver(PaverEvent::SetConfigurationUnbootable { configuration: paver::Configuration::B }),
        Paver(PaverEvent::BootManagerFlush),
    ]
    .into_iter()
}
