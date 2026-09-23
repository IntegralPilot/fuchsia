// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![cfg(test)]
use anyhow::anyhow;
use assert_matches::assert_matches;
use diagnostics_assertions::{AnyProperty, TreeAssertion, assert_data_tree, tree_assertion};
use diagnostics_hierarchy::DiagnosticsHierarchy;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_feedback::FileReportResults;
use fidl_fuchsia_hardware_power_statecontrol::{ShutdownAction, ShutdownOptions, ShutdownReason};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_metrics as fmetrics;
use fidl_fuchsia_paver as fpaver;
use fidl_fuchsia_pkg::{self as fpkg, PackageCacheRequestStream, PackageResolverRequestStream};
use fidl_fuchsia_update::{
    AttemptsMonitorMarker, AttemptsMonitorRequest, AttemptsMonitorRequestStream,
    CheckNotStartedReason, CheckOptions, CheckingForUpdatesData, CommitStatusProviderMarker,
    CommitStatusProviderProxy, ErrorCheckingForUpdateData, Initiator, InstallationDeferralReason,
    InstallationDeferredData, InstallationErrorData, InstallationProgress, InstallingData,
    ManagerMarker, ManagerProxy, MonitorMarker, MonitorRequest, MonitorRequestStream,
    NoUpdateAvailableData, State, UpdateInfo,
};
use fidl_fuchsia_update_channelcontrol::{ChannelControlMarker, ChannelControlProxy};
use fidl_fuchsia_update_installer as finstaller;
use fidl_fuchsia_update_installer_ext as installer;
use fidl_fuchsia_update_verify as fupdate_verify;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_pkg_testing::{make_current_epoch_json, make_packages_json};
use fuchsia_sync::Mutex;
use fuchsia_url::fuchsia_pkg::{PinnedAbsolutePackageUrl, UnpinnedAbsolutePackageUrl};
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use mock_crash_reporter::{CrashReport, MockCrashReporterService, ThrottleHook};
use mock_health_verification::MockHealthVerificationService;
use mock_installer::MockUpdateInstallerService;
use mock_omaha_server::{
    OmahaResponse, OmahaServer, OmahaServerBuilder, PrivateKeyAndId, PrivateKeys,
    ResponseAndMetadata,
};
use mock_omaha_server_fuchsia::OmahaServerExt as _;
use mock_paver::{MockPaverService, MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
use mock_reboot::MockRebootService;
use mock_resolver::MockResolverService;
use omaha_client::cup_ecdsa::test_support::{
    RAW_PUBLIC_KEY_FOR_TEST, make_default_private_key_for_test,
};
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, create_dir};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const OMAHA_CLIENT_CML: &str = "#meta/omaha-client-service.cm";
const SYSTEM_UPDATER_CML: &str = "#meta/system-updater.cm";
const SYSTEM_UPDATE_COMMITTER_CML: &str = "#meta/system-update-committer.cm";
const STASH_CML: &str = "#meta/stash2.cm";
const APP_ID: &str = "integration-test-appid";

struct Mounts {
    _test_dir: TempDir,
    config_data: PathBuf,
    build_info: PathBuf,
}

impl Mounts {
    fn new() -> Self {
        let test_dir = TempDir::new().expect("create test tempdir");
        let config_data = test_dir.path().join("config_data");
        create_dir(&config_data).expect("create config_data dir");
        let build_info = test_dir.path().join("build_info");
        create_dir(&build_info).expect("create build_info dir");

        Self { _test_dir: test_dir, config_data, build_info }
    }

    fn write_version(&self, version: impl AsRef<[u8]>) {
        let version_path = self.build_info.join("version");
        fs::write(version_path, version).expect("write version");
    }

    fn write_eager_package_config(&self, config: impl AsRef<[u8]>) {
        let eager_package_config_path = self.config_data.join("eager_package_config.json");
        fs::write(eager_package_config_path, config).expect("write eager_package_config.json");
    }
}
struct Proxies {
    config_optout: Arc<fuchsia_update_config_optout::Mock>,
    resolver: Arc<MockResolverService>,
    update_manager: ManagerProxy,
    channel_control: ChannelControlProxy,
    commit_status_provider: CommitStatusProviderProxy,
}

// A builder lambda which accepts as input the full service URL of the mock
// Omaha server and returns eager package config as JSON.
type EagerPackageConfigBuilder = fn(&str) -> serde_json::Value;

struct TestEnvBuilder {
    // Set one of responses, responses_and_metadata.
    responses_by_appid: Vec<(String, ResponseAndMetadata)>,
    version: String,
    installer: Option<MockUpdateInstallerService>,
    paver: Option<MockPaverService>,
    crash_reporter: Option<MockCrashReporterService>,
    eager_package_config_builder: Option<EagerPackageConfigBuilder>,
    cup_info_map: HashMap<UnpinnedAbsolutePackageUrl, (String, String)>,
    private_keys: Option<PrivateKeys>,
    etag_override: Option<String>,
}

impl TestEnvBuilder {
    fn new() -> Self {
        Self {
            responses_by_appid: vec![(APP_ID.to_string(), ResponseAndMetadata::default())],
            version: "0.1.2.3".to_string(),
            installer: None,
            paver: None,
            crash_reporter: None,
            eager_package_config_builder: None,
            cup_info_map: HashMap::new(),
            private_keys: None,
            etag_override: None,
        }
    }

    fn default_with_response(self, response: OmahaResponse) -> Self {
        Self {
            responses_by_appid: vec![(
                APP_ID.to_string(),
                ResponseAndMetadata { response, ..Default::default() },
            )],
            ..self
        }
    }

    fn responses_and_metadata(
        self,
        responses_by_appid: Vec<(String, ResponseAndMetadata)>,
    ) -> Self {
        Self { responses_by_appid, ..self }
    }

    fn version(self, version: impl Into<String>) -> Self {
        Self { version: version.into(), ..self }
    }

    fn installer(self, installer: MockUpdateInstallerService) -> Self {
        Self { installer: Some(installer), ..self }
    }

    fn paver(self, paver: MockPaverService) -> Self {
        Self { paver: Some(paver), ..self }
    }

    fn crash_reporter(self, crash_reporter: MockCrashReporterService) -> Self {
        Self { crash_reporter: Some(crash_reporter), ..self }
    }

    fn eager_package_config_builder(
        self,
        eager_package_config_builder: EagerPackageConfigBuilder,
    ) -> Self {
        Self { eager_package_config_builder: Some(eager_package_config_builder), ..self }
    }

    fn add_cup_info(
        mut self,
        url: impl Into<String>,
        version: impl Into<String>,
        channel: impl Into<String>,
    ) -> Self {
        self.cup_info_map.insert(url.into().parse().unwrap(), (version.into(), channel.into()));
        self
    }

    fn private_keys(mut self, private_keys: PrivateKeys) -> Self {
        self.private_keys = Some(private_keys);
        self
    }

    fn etag_override(mut self, etag_override: impl Into<String>) -> Self {
        self.etag_override = Some(etag_override.into());
        self
    }

    async fn build(self) -> TestEnv {
        // Add the mount directories to fs service.
        let mounts = Mounts::new();
        let mut fs = ServiceFs::new();
        let config_data_path = mounts.config_data.clone().into_os_string().into_string().unwrap();
        let build_info_path = mounts.build_info.clone().into_os_string().into_string().unwrap();
        let config_data = fuchsia_fs::directory::open_in_namespace(
            config_data_path.as_str(),
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
        )
        .unwrap();
        let build_info = fuchsia_fs::directory::open_in_namespace(
            build_info_path.as_str(),
            fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
        )
        .unwrap();
        fs.dir("config").add_remote("data", config_data);
        fs.dir("config").add_remote("build-info", build_info);

        let server = Arc::new(std::sync::Mutex::new({
            let mut b = OmahaServerBuilder::default().responses_by_appid(
                self.responses_by_appid
                    .into_iter()
                    .collect::<HashMap<String, ResponseAndMetadata>>(),
            );
            if let Some(pk) = self.private_keys {
                b = b.private_keys(pk);
            }
            b.etag_override(self.etag_override).build().unwrap()
        }));
        let url = OmahaServer::start_and_detach(server.clone(), None).await.expect("start server");
        mounts.write_version(self.version);
        if let Some(eager_package_config_builder) = self.eager_package_config_builder {
            let json = eager_package_config_builder(&url);
            mounts.write_eager_package_config(json.to_string());
        }

        let mut svc = fs.dir("svc");

        let paver = Arc::new(self.paver.unwrap_or_else(|| MockPaverServiceBuilder::new().build()));
        svc.add_fidl_service(move |stream: fpaver::PaverRequestStream| {
            fasync::Task::spawn(
                Arc::clone(&paver)
                    .run_paver_service(stream)
                    .unwrap_or_else(|e| panic!("error running paver service: {:#}", anyhow!(e))),
            )
            .detach();
        });

        let resolver = Arc::new(MockResolverService::new(None));
        {
            let resolver = resolver.clone();
            svc.add_fidl_service(move |stream: PackageResolverRequestStream| {
                fasync::Task::spawn(
                    Arc::clone(&resolver).run_resolver_service(stream).unwrap_or_else(|e| {
                        panic!("error running resolver service {:#}", anyhow!(e))
                    }),
                )
                .detach()
            });
        }

        let cache = Arc::new(MockCache::new());
        svc.add_fidl_service(move |stream: PackageCacheRequestStream| {
            fasync::Task::spawn(Arc::clone(&cache).run_cache_service(stream)).detach()
        });

        let config_optout = Arc::new(fuchsia_update_config_optout::Mock::new());
        svc.add_fidl_service({
            let config_optout = Arc::clone(&config_optout);
            move |stream| fasync::Task::spawn(Arc::clone(&config_optout).serve(stream)).detach()
        });

        let cup = Arc::new(fuchsia_pkg_cup::Mock::new(self.cup_info_map));
        svc.add_fidl_service({
            let cup = Arc::clone(&cup);
            move |stream| fasync::Task::spawn(Arc::clone(&cup).serve(stream)).detach()
        });

        let (send, reboot_called) = oneshot::channel();
        let send = Mutex::new(Some(send));
        let reboot_service = Arc::new(MockRebootService::new(Box::new(move |options| {
            assert_eq!(
                options,
                ShutdownOptions {
                    action: Some(ShutdownAction::Reboot),
                    reasons: Some(vec![ShutdownReason::SystemUpdate]),
                    ..Default::default()
                }
            );
            send.lock().take().unwrap().send(()).unwrap();
            Ok(())
        })));
        svc.add_fidl_service(move |stream| {
            fasync::Task::spawn(
                Arc::clone(&reboot_service)
                    .run_reboot_service(stream)
                    .unwrap_or_else(|e| panic!("error running reboot service: {:#}", anyhow!(e))),
            )
            .detach()
        });

        // Set up verifier service.
        let verifier = Arc::new(MockHealthVerificationService::new(|| Ok(())));
        {
            let verifier = Arc::clone(&verifier);
            svc.add_fidl_service(move |stream| {
                fasync::Task::spawn(Arc::clone(&verifier).run_health_verification_service(stream))
                    .detach()
            });
        }

        // Set up crash reporter service.
        let crash_reporter = Arc::new(self.crash_reporter.unwrap_or_else(|| {
            MockCrashReporterService::new(|_| Ok(FileReportResults::default()))
        }));
        svc.add_fidl_service(move |stream| {
            fasync::Task::spawn(Arc::clone(&crash_reporter).run_crash_reporter_service(stream))
                .detach()
        });

        let mut use_real_system_updater = true;
        if let Some(installer) = self.installer {
            use_real_system_updater = false;
            let installer = Arc::new(installer);
            {
                let installer = Arc::clone(&installer);
                svc.add_fidl_service(move |stream| {
                    fasync::Task::spawn(Arc::clone(&installer).run_service(stream)).detach()
                });
            }
        }

        let fs_holder = Mutex::new(Some(fs));
        let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
        let omaha_client_service = builder
            .add_child("omaha_client_service", OMAHA_CLIENT_CML, ChildOptions::new().eager())
            .await
            .unwrap();

        let system_update_committer = builder
            .add_child(
                "system_update_committer",
                SYSTEM_UPDATE_COMMITTER_CML,
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();
        let fake_capabilities = builder
            .add_local_child(
                "fake_capabilities",
                move |handles| {
                    let mut rfs = fs_holder
                        .lock()
                        .take()
                        .expect("mock component should only be launched once");
                    async {
                        let _ = &handles;
                        rfs.serve_connection(handles.outgoing_dir).unwrap();
                        let () = rfs.collect().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
        let stash2 = builder.add_child("stash2", STASH_CML, ChildOptions::new()).await.unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::storage("data"))
                    .from(Ref::parent())
                    .to(&stash2),
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
                    .capability(Capability::protocol::<fpaver::PaverMarker>())
                    .capability(Capability::protocol::<
                        fidl_fuchsia_hardware_power_statecontrol::AdminMarker,
                    >())
                    .from(&fake_capabilities)
                    .to(&omaha_client_service)
                    .to(&system_update_committer),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("build-info")
                            .path("/config/build-info")
                            .rights(fio::R_STAR_DIR),
                    )
                    .capability(Capability::protocol::<fidl_fuchsia_feedback::CrashReporterMarker>())
                    .capability(Capability::protocol::<
                        fidl_fuchsia_feedback::ComponentDataRegisterMarker,
                    >())
                    .capability(Capability::protocol::<fpkg::CupMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_update_config::OptOutMarker>())
                    .from(&fake_capabilities)
                    .to(&omaha_client_service),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::directory("root-ssl-certificates"))
                    .capability(Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_posix_socket::ProviderMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_net_name::LookupMarker>())
                    .from(Ref::parent())
                    .to(&omaha_client_service),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                    .from(Ref::parent())
                    .to(&omaha_client_service)
                    .to(&system_update_committer)
                    .to(&stash2),
            )
            .await
            .unwrap();
        builder
            .add_capability(
                cm_rust::ConfigurationDecl {
                    name: "fuchsia.system-update-committer.StopOnIdleTimeoutMillis"
                        .parse()
                        .unwrap(),
                    value: (-1i64).into(),
                }
                .into(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.system-update-committer.StopOnIdleTimeoutMillis",
                    ))
                    .from(Ref::self_())
                    .to(&system_update_committer),
            )
            .await
            .unwrap();
        builder
            .add_capability(
                cm_rust::ConfigurationDecl {
                    name: "fuchsia.system-update-committer.CommitTimeoutSeconds".parse().unwrap(),
                    value: (-1i64).into(),
                }
                .into(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.system-update-committer.CommitTimeoutSeconds",
                    ))
                    .from(Ref::self_())
                    .to(&system_update_committer),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fupdate_verify::HealthVerificationMarker>())
                    .from(&fake_capabilities)
                    .to(&system_update_committer),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.stash.Store2"))
                    .from(&stash2)
                    .to(&omaha_client_service),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<CommitStatusProviderMarker>())
                    .from(&system_update_committer)
                    .to(&omaha_client_service)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::protocol::<fidl_fuchsia_update_channel::ProviderMarker>(),
                    )
                    .capability(Capability::protocol::<ChannelControlMarker>())
                    .capability(Capability::protocol::<ManagerMarker>())
                    .from(&omaha_client_service)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha.PeriodicIntervalMinutes".parse().unwrap(),
                value: 60u16.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha.StartupDelaySeconds".parse().unwrap(),
                value: 60u16.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha.AllowRebootWhenIdle".parse().unwrap(),
                value: true.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha.RetryDelaySeconds".parse().unwrap(),
                value: 300u16.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha.FuzzPercentageRange".parse().unwrap(),
                value: 25u8.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha_app_id".parse().unwrap(),
                value: APP_ID.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.omaha_url".parse().unwrap(),
                value: url.into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.ota_channel".parse().unwrap(),
                value: "".into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.ota_realm".parse().unwrap(),
                value: "".into(),
            }))
            .await
            .unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "fuchsia.product_id".parse().unwrap(),
                value: "".into(),
            }))
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.omaha.AllowRebootWhenIdle"))
                    .capability(Capability::configuration("fuchsia.omaha.FuzzPercentageRange"))
                    .capability(Capability::configuration("fuchsia.omaha.PeriodicIntervalMinutes"))
                    .capability(Capability::configuration("fuchsia.omaha.RetryDelaySeconds"))
                    .capability(Capability::configuration("fuchsia.omaha.StartupDelaySeconds"))
                    .capability(Capability::configuration("fuchsia.omaha_app_id"))
                    .capability(Capability::configuration("fuchsia.omaha_url"))
                    .capability(Capability::configuration("fuchsia.ota_channel"))
                    .capability(Capability::configuration("fuchsia.ota_realm"))
                    .capability(Capability::configuration("fuchsia.product_id"))
                    .from(Ref::self_())
                    .to(&omaha_client_service),
            )
            .await
            .unwrap();

        if use_real_system_updater {
            let system_updater = builder
                .add_child("system_updater", SYSTEM_UPDATER_CML, ChildOptions::new().eager())
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
                        .capability(
                            Capability::directory("build-info")
                                .path("/config/build-info")
                                .rights(fio::R_STAR_DIR),
                        )
                        .capability(Capability::protocol::<fpaver::PaverMarker>())
                        .capability(Capability::protocol::<fpkg::PackageCacheMarker>())
                        .capability(Capability::protocol::<fpkg::PackageResolverMarker>())
                        .capability(Capability::protocol::<
                            fidl_fuchsia_hardware_power_statecontrol::AdminMarker,
                        >())
                        .from(&fake_capabilities)
                        .to(&system_updater),
                )
                .await
                .unwrap();
            builder
                .add_route(
                    Route::new()
                        .capability(Capability::storage("data"))
                        .capability(
                            Capability::protocol::<fmetrics::MetricEventLoggerFactoryMarker>(),
                        )
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
                        .to(&omaha_client_service),
                )
                .await
                .unwrap();
            builder
                .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                    name: "fuchsia.system-updater.ManifestPublicKeys".parse().unwrap(),
                    value: Vec::<String>::new().into(),
                }))
                .await
                .unwrap();
            builder
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
            builder
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
        } else {
            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<finstaller::InstallerMarker>())
                        .from(&fake_capabilities)
                        .to(&omaha_client_service),
                )
                .await
                .unwrap();
        }

        let realm_instance = builder.build().await.unwrap();
        let channel_control = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to channel control provider");
        let update_manager = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to update manager");
        let commit_status_provider = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to commit status provider");

        TestEnv {
            realm_instance,
            _mounts: mounts,
            proxies: Proxies {
                config_optout,
                resolver,
                update_manager,
                channel_control,
                commit_status_provider,
            },
            server,
            reboot_called,
        }
    }
}

struct TestEnv {
    realm_instance: RealmInstance,
    _mounts: Mounts,
    proxies: Proxies,
    server: Arc<std::sync::Mutex<OmahaServer>>,
    reboot_called: oneshot::Receiver<()>,
}

impl TestEnv {
    async fn check_now(&self) -> MonitorRequestStream {
        let options = CheckOptions {
            initiator: Some(Initiator::User),
            allow_attaching_to_existing_update_check: Some(false),
            ..Default::default()
        };
        let (client_end, stream) = fidl::endpoints::create_request_stream::<MonitorMarker>();
        self.proxies
            .update_manager
            .check_now(&options, Some(client_end))
            .await
            .expect("make check_now call")
            .expect("check started");
        stream
    }

    async fn monitor_all_update_checks(&self) -> AttemptsMonitorRequestStream {
        let (client_end, stream) =
            fidl::endpoints::create_request_stream::<AttemptsMonitorMarker>();
        self.proxies
            .update_manager
            .monitor_all_update_checks(client_end)
            .expect("make monitor_all_update call");
        stream
    }

    async fn inspect_hierarchy(&self) -> DiagnosticsHierarchy {
        let nested_environment_label = format!(
            "test_driver/realm_builder\\:{}/omaha_client_service:root",
            self.realm_instance.root.child_name()
        );
        ArchiveReader::inspect()
            .add_selector(nested_environment_label.to_string())
            .snapshot()
            .await
            .expect("read inspect hierarchy")
            .into_iter()
            .next()
            .expect("one result")
            .payload
            .expect("payload is not none")
    }

    async fn assert_platform_metrics(&self, children: TreeAssertion) {
        assert_data_tree!(
            self.inspect_hierarchy().await,
            "root": contains {
                "platform_metrics": contains {
                    "events": contains {
                        "capacity": 50u64,
                        children,
                    }
                }
            }
        );
    }
}

struct MockCache;

impl MockCache {
    fn new() -> Self {
        Self
    }
    async fn run_cache_service(self: Arc<Self>, mut stream: PackageCacheRequestStream) {
        while let Some(request) = stream.try_next().await.unwrap() {
            match request {
                fidl_fuchsia_pkg::PackageCacheRequest::Sync { responder } => {
                    responder.send(Ok(())).unwrap();
                }
                other => panic!("unsupported PackageCache request: {other:?}"),
            }
        }
    }
}

pub mod fuchsia_update_config_optout {
    use super::*;
    pub use fidl_fuchsia_update_config::OptOutPreference;
    use fidl_fuchsia_update_config::{OptOutRequest, OptOutRequestStream};

    #[derive(Debug)]
    pub struct Mock(Mutex<OptOutPreference>);

    impl Mock {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self(Mutex::new(OptOutPreference::AllowAllUpdates))
        }

        pub fn set(&self, value: OptOutPreference) {
            *self.0.lock() = value;
        }

        pub async fn serve(self: Arc<Self>, mut stream: OptOutRequestStream) {
            while let Some(request) = stream.try_next().await.unwrap() {
                match request {
                    OptOutRequest::Get { responder } => {
                        let value = *self.0.lock();
                        if let Err(e) = responder.send(value) {
                            eprintln!(
                                "fuchsia_update_config_optout::Mock::serve() failed to send a response, possibly because the client is shut down: {e:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

pub mod fuchsia_pkg_cup {
    use super::*;
    use fidl_fuchsia_pkg::{CupRequest, CupRequestStream};

    #[derive(Debug)]
    pub struct Mock {
        info_map: HashMap<UnpinnedAbsolutePackageUrl, (String, String)>,
    }

    impl Mock {
        pub fn new(info_map: HashMap<UnpinnedAbsolutePackageUrl, (String, String)>) -> Self {
            Self { info_map }
        }

        pub async fn serve(self: Arc<Self>, mut stream: CupRequestStream) {
            while let Some(request) = stream.try_next().await.unwrap() {
                match request {
                    CupRequest::Write { url, cup, responder } => {
                        let url: PinnedAbsolutePackageUrl = url.url.parse().unwrap();
                        assert_eq!(url.host(), "integration.test.fuchsia.com");
                        assert_eq!(
                            url.hash().to_string(),
                            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                        );
                        assert!(cup.request.is_some());
                        assert!(cup.key_id.is_some());
                        assert!(cup.nonce.is_some());
                        assert!(cup.response.is_some());
                        assert!(cup.signature.is_some());
                        responder.send(Ok(())).unwrap();
                    }
                    CupRequest::GetInfo { url, responder } => {
                        let (version, channel) = &self.info_map[&url.url.parse().unwrap()];
                        responder.send(Ok((version, channel))).unwrap();
                    }
                }
            }
        }
    }
}

async fn expect_states(stream: &mut MonitorRequestStream, expected_states: &[State]) {
    for expected_state in expected_states {
        let MonitorRequest::OnState { state, responder } =
            stream.try_next().await.unwrap().unwrap();
        assert_eq!(&state, expected_state);
        responder.send().unwrap();
    }
}

fn update_info() -> Option<UpdateInfo> {
    // TODO(https://fxbug.dev/42124218): version_available should be `Some("0.1.2.3".to_string())` once omaha-client
    // returns version_available.
    Some(UpdateInfo {
        version_available: None,
        download_size: None,
        urgent: Some(false),
        ..Default::default()
    })
}

fn progress(fraction_completed: Option<f32>) -> Option<InstallationProgress> {
    Some(InstallationProgress { fraction_completed, ..Default::default() })
}

async fn omaha_client_update(
    mut env: TestEnv,
    platform_metrics: TreeAssertion,
    should_wait_for_reboot: bool,
    update_info: Option<UpdateInfo>,
) {
    env.proxies
        .resolver
        .url("fuchsia-pkg://integration.test.fuchsia.com/update?hash=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
        .resolve(
        &env.proxies
            .resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file(
                "images.json",
                serde_json::to_vec(
                    &update_package::ImagePackagesManifest::builder()
                    .fuchsia_package(
                        update_package::ImageMetadata::new(
                            8,
                            [0; 32].into(),
                            "fuchsia-pkg://fuchsia.com/update_images_fuchsia/0?hash=2222222222222222222222222222222222222222222222222222222222222222#zbi".parse().unwrap(),
                        ),
                        None
                    )
                    .clone()
                    .build()
                )
                .unwrap()
            )
            .add_file("epoch.json", make_current_epoch_json())
    );
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead")
        .resolve(
        &env.proxies
            .resolver
            .package("system_image", "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeada")
    );
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/update_images_fuchsia/0?hash=2222222222222222222222222222222222222222222222222222222222222222")
        .resolve(
        &env.proxies
            .resolver
            .package("update_images_fuchsia", "2222222222222222222222222222222222222222222222222222222222222222")
            .add_file("zbi", "fake zbi")
    );

    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info.clone(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;
    let mut last_progress: Option<InstallationProgress> = None;
    let mut waiting_for_reboot = false;
    while let Some(request) = stream.try_next().await.unwrap() {
        let MonitorRequest::OnState { state, responder } = request;
        match state {
            State::InstallingUpdate(InstallingData { update, installation_progress, .. }) => {
                assert_eq!(update, update_info.clone());
                assert!(!waiting_for_reboot);
                if let Some(last_progress) = last_progress {
                    let last = last_progress.fraction_completed.unwrap();
                    let current =
                        installation_progress.as_ref().unwrap().fraction_completed.unwrap();
                    assert!(
                        last <= current,
                        "progress is not increasing, last: {last}, current: {current}",
                    );
                }
                last_progress = installation_progress;
            }
            State::WaitingForReboot(InstallingData { update, installation_progress, .. }) => {
                assert_eq!(update, update_info.clone());
                assert_eq!(installation_progress, progress(Some(1.)));
                assert!(!waiting_for_reboot);
                waiting_for_reboot = true;
                assert_matches!(env.reboot_called.try_recv(), Ok(None));
            }
            state => {
                panic!("Unexpected state: {state:?}");
            }
        }
        responder.send().unwrap();
    }
    assert_matches!(last_progress, Some(_));
    assert_eq!(waiting_for_reboot, should_wait_for_reboot);

    env.assert_platform_metrics(platform_metrics).await;

    if should_wait_for_reboot {
        // This will hang if reboot was not triggered.
        env.reboot_called.await.unwrap();
    } else {
        assert_matches!(env.reboot_called.try_recv(), Ok(None));
    }
}

#[fuchsia::test]
async fn test_omaha_client_update() {
    let env = TestEnvBuilder::new().default_with_response(OmahaResponse::Update).build().await;
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallingUpdate",
                    "target-version": "0.1.2.3",
                },
                "2": contains {
                    "event": "WaitingForReboot",
                    "target-version": "0.1.2.3",
                }
            }
        ),
        true,
        update_info(),
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_multi_app() {
    use omaha_client::cup_ecdsa::test_support::make_default_public_key_id_for_test;
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![
            (
                APP_ID.to_string(),
                ResponseAndMetadata { response: OmahaResponse::Update, ..Default::default() },
            ),
            (
                "foo".to_string(),
                ResponseAndMetadata {
                    response: OmahaResponse::NoUpdate,
                    version: Some("0.0.4.1".to_string()),
                    ..Default::default()
                },
            ),
            (
                "bar".to_string(),
                ResponseAndMetadata {
                    response: OmahaResponse::NoUpdate,
                    version: Some("0.0.4.2".to_string()),
                    ..Default::default()
                },
            ),
        ])
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": make_default_public_key_id_for_test(),
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages":
                        [
                            {
                                "url": "fuchsia-pkg://example.com/package",
                                "flavor": "debug",
                                "channel_config":
                                    {
                                        "channels":
                                            [
                                                {
                                                    "name": "stable",
                                                    "repo": "stable",
                                                    "appid": "foo"
                                                },
                                            ],
                                        "default_channel": "stable"
                                    }
                            },
                            {
                                "url": "fuchsia-pkg://example.com/package2",
                                "channel_config":
                                    {
                                        "channels":
                                            [
                                                {
                                                    "name": "stable",
                                                    "repo": "stable",
                                                    "appid": "bar"
                                                }
                                            ],
                                        "default_channel": "stable"
                                    }
                            }
                        ]
                    }
                ]
            })
        })
        .add_cup_info("fuchsia-pkg://example.com/package", "0.0.4.1", "stable")
        .add_cup_info("fuchsia-pkg://example.com/package2", "0.0.4.2", "stable")
        .build()
        .await;
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallingUpdate",
                    "target-version": "0.1.2.3",
                },
                "2": contains {
                    "event": "WaitingForReboot",
                    "target-version": "0.1.2.3",
                }
            }
        ),
        true,
        update_info(),
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_eager_package() {
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![
            (
                APP_ID.to_string(),
                ResponseAndMetadata { response: OmahaResponse::NoUpdate, ..Default::default() },
            ),
            (
                "foo".to_string(),
                ResponseAndMetadata {
                    response: OmahaResponse::Update,
                    version: Some("0.0.4.1".to_string()),
                    ..Default::default()
                },
            ),
            (
                "bar".to_string(),
                ResponseAndMetadata {
                    response: OmahaResponse::NoUpdate,
                    version: Some("0.0.4.2".to_string()),
                    ..Default::default()
                },
            ),
        ])
        .private_keys(PrivateKeys {
            latest: PrivateKeyAndId {
                id: 100_i32.try_into().unwrap(),
                key: make_default_private_key_for_test(),
            },
            historical: vec![PrivateKeyAndId {
                id: 42_i32.try_into().unwrap(),
                key: make_default_private_key_for_test(),
            }],
        })
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": 42,
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages":
                        [
                            {
                                "url": "fuchsia-pkg://example.com/package",
                                "flavor": "debug",
                                "channel_config":
                                    {
                                        "channels":
                                            [
                                                {
                                                    "name": "stable",
                                                    "repo": "stable",
                                                    "appid": "foo"
                                                },
                                            ],
                                        "default_channel": "stable"
                                    }
                            },
                            {
                                "url": "fuchsia-pkg://example.com/package2",
                                "channel_config":
                                    {
                                        "channels":
                                            [
                                                {
                                                    "name": "stable",
                                                    "repo": "stable",
                                                    "appid": "bar"
                                                }
                                            ]
                                    }
                            }
                        ]
                    }
                ]
            })
        })
        .add_cup_info("fuchsia-pkg://example.com/package", "0.0.4.1", "stable")
        .add_cup_info("fuchsia-pkg://example.com/package2", "0.0.4.2", "stable")
        .build()
        .await;
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallingUpdate",
                    "target-version": "",
                },
            }
        ),
        false,
        update_info(),
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_cup_force_historical_key() {
    // This test forces usage of a historical key -- the server is passed a
    // private key struct has key #100 as the latest and #42 in the historical
    // vector, but the client is passed a public key struct which has key #42 as
    // the latest.
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![(
            APP_ID.to_string(),
            ResponseAndMetadata { response: OmahaResponse::Update, ..Default::default() },
        )])
        .private_keys(PrivateKeys {
            latest: PrivateKeyAndId {
                id: 100_i32.try_into().unwrap(),
                key: make_default_private_key_for_test(),
            },
            historical: vec![PrivateKeyAndId {
                id: 42_i32.try_into().unwrap(),
                key: make_default_private_key_for_test(),
            }],
        })
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": 42,
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages": [ ]
                    }
                ]
            })
        })
        .build()
        .await;
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallingUpdate",
                    "target-version": "0.1.2.3",
                },
                "2": contains {
                    "event": "WaitingForReboot",
                    "target-version": "0.1.2.3",
                }
            }
        ),
        true,
        update_info(),
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_cup_key_mismatch() {
    // If the server and client don't share a public/private keypair, no
    // handshake and no response.
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![(
            APP_ID.to_string(),
            ResponseAndMetadata { response: OmahaResponse::Update, ..Default::default() },
        )])
        .private_keys(PrivateKeys {
            latest: PrivateKeyAndId {
                id: 100_i32.try_into().unwrap(),
                key: make_default_private_key_for_test(),
            },
            historical: vec![],
        })
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": 42,
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages": [ ]
                    }
                ]
            })
        })
        .build()
        .await;
    do_failed_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_update_cup_bad_etag() {
    // What if the server returns an empty etag?
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![(
            APP_ID.to_string(),
            ResponseAndMetadata { response: OmahaResponse::Update, ..Default::default() },
        )])
        .etag_override("a1b2c3d4e5")
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": 42,
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages": [ ]
                    }
                ]
            })
        })
        .build()
        .await;
    do_failed_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_update_cup_empty_etag() {
    // What if the server returns an empty etag?
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![(
            APP_ID.to_string(),
            ResponseAndMetadata { response: OmahaResponse::Update, ..Default::default() },
        )])
        .etag_override("")
        .eager_package_config_builder(|url: &str| {
            json!(
            {
                "eager_package_configs": [
                    {
                        "server": {
                            "service_url": url,
                            "public_keys": {
                                "latest": {
                                    "id": 42,
                                    "key": RAW_PUBLIC_KEY_FOR_TEST,
                                },
                                "historical": [],
                            }
                        },
                        "packages": [ ]
                    }
                ]
            })
        })
        .build()
        .await;
    do_failed_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_attempt_monitor_update_progress_with_mock_installer() {
    let (mut sender, receiver) = mpsc::channel(0);
    let installer = MockUpdateInstallerService::builder().states_receiver(receiver).build();
    let env = TestEnvBuilder::new()
        .default_with_response(OmahaResponse::Update)
        .installer(installer)
        .build()
        .await;

    env.check_now().await;
    let mut request_stream = env.monitor_all_update_checks().await;
    let AttemptsMonitorRequest::OnStart { options, monitor, responder } =
        request_stream.next().await.unwrap().unwrap();

    assert_matches!(options.initiator, Some(fidl_fuchsia_update::Initiator::User));

    assert_matches!(responder.send(), Ok(()));
    let mut monitor_stream = monitor.into_stream();

    expect_states(
        &mut monitor_stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;

    // Send installer state and expect manager step in lockstep to make sure that event queue
    // won't merge any progress.
    sender.send(installer::State::Prepare).await.unwrap();
    expect_states(
        &mut monitor_stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.0)),
            ..Default::default()
        })],
    )
    .await;

    let installer_update_info = installer::UpdateInfo::builder().download_size(1000).build();
    sender
        .send(installer::State::Fetch(
            installer::UpdateInfoAndProgress::new(
                installer_update_info,
                installer::Progress::none(),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    expect_states(
        &mut monitor_stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.0)),
            ..Default::default()
        })],
    )
    .await;

    sender
        .send(installer::State::Stage(
            installer::UpdateInfoAndProgress::new(
                installer_update_info,
                installer::Progress::builder()
                    .fraction_completed(0.5)
                    .bytes_downloaded(500)
                    .build(),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    expect_states(
        &mut monitor_stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.5)),
            ..Default::default()
        })],
    )
    .await;

    sender
        .send(installer::State::WaitToReboot(installer::UpdateInfoAndProgress::done(
            installer_update_info,
        )))
        .await
        .unwrap();
    expect_states(
        &mut monitor_stream,
        &[
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(Some(1.0)),
                ..Default::default()
            }),
            State::WaitingForReboot(InstallingData {
                update: update_info(),
                installation_progress: progress(Some(1.0)),
                ..Default::default()
            }),
        ],
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_progress_with_mock_installer() {
    let (mut sender, receiver) = mpsc::channel(0);
    let installer = MockUpdateInstallerService::builder().states_receiver(receiver).build();
    let env = TestEnvBuilder::new()
        .default_with_response(OmahaResponse::Update)
        .installer(installer)
        .build()
        .await;

    let mut stream = env.check_now().await;

    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;

    // Send installer state and expect manager step in lockstep to make sure that event queue
    // won't merge any progress.
    sender.send(installer::State::Prepare).await.unwrap();
    expect_states(
        &mut stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.0)),
            ..Default::default()
        })],
    )
    .await;

    let installer_update_info = installer::UpdateInfo::builder().download_size(1000).build();
    sender
        .send(installer::State::Fetch(
            installer::UpdateInfoAndProgress::new(
                installer_update_info,
                installer::Progress::none(),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    expect_states(
        &mut stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.0)),
            ..Default::default()
        })],
    )
    .await;

    sender
        .send(installer::State::Stage(
            installer::UpdateInfoAndProgress::new(
                installer_update_info,
                installer::Progress::builder()
                    .fraction_completed(0.5)
                    .bytes_downloaded(500)
                    .build(),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    expect_states(
        &mut stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(),
            installation_progress: progress(Some(0.5)),
            ..Default::default()
        })],
    )
    .await;

    sender
        .send(installer::State::WaitToReboot(installer::UpdateInfoAndProgress::done(
            installer_update_info,
        )))
        .await
        .unwrap();
    expect_states(
        &mut stream,
        &[
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(Some(1.0)),
                ..Default::default()
            }),
            State::WaitingForReboot(InstallingData {
                update: update_info(),
                installation_progress: progress(Some(1.0)),
                ..Default::default()
            }),
        ],
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_installation_deferred() {
    let (throttle_hook, throttler) = mphooks::throttle();
    let config_status_response = Arc::new(Mutex::new(Some(fpaver::ConfigurationStatus::Pending)));
    let env = {
        let config_status_response = Arc::clone(&config_status_response);
        TestEnvBuilder::new()
            .paver(
                MockPaverServiceBuilder::new()
                    .insert_hook(throttle_hook)
                    .insert_hook(mphooks::config_status_and_boot_attempts(move |_| {
                        Ok((*config_status_response.lock().as_ref().unwrap(), Some(1)))
                    }))
                    .build(),
            )
            .default_with_response(OmahaResponse::Update)
            .build()
            .await
    };

    // Allow the paver to emit enough events to unblock the CommitStatusProvider FIDL server, but
    // few enough to guarantee the commit is still pending.
    let () = throttler.emit_next_paver_events(&[
        PaverEvent::QueryCurrentConfiguration,
        PaverEvent::QueryConfigurationStatusAndBootAttempts {
            configuration: fpaver::Configuration::A,
        },
    ]);

    // The update attempt should start, but the install should be deferred b/c we're pending commit.
    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallationDeferredByPolicy(InstallationDeferredData {
                update: update_info(),
                deferral_reason: Some(InstallationDeferralReason::CurrentSystemNotCommitted),
                ..Default::default()
            }),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);
    env.assert_platform_metrics(tree_assertion!(
        "children": {
            "0": contains {
                "event": "CheckingForUpdates",
            },
            "1": contains {
                "event": "InstallationDeferredByPolicy",
            },
        }
    ))
    .await;

    // Unblock any subsequent paver requests so that the system can commit.
    drop(throttler);

    // Wait for system to commit.
    let event_pair =
        env.proxies.commit_status_provider.is_current_system_committed().await.unwrap();
    assert_eq!(
        fasync::OnSignals::new(&event_pair, zx::Signals::USER_0).await,
        Ok(zx::Signals::USER_0)
    );

    // Now that the system is committed, we should be able to perform an update. Before we do the
    // update, make sure QueryConfigurationStatus returns Healthy. Otherwise, the update will fail
    // because the system-updater enforces the current slot is Healthy before applying an update.
    assert_eq!(
        config_status_response.lock().replace(fpaver::ConfigurationStatus::Healthy).unwrap(),
        fpaver::ConfigurationStatus::Pending
    );
    env.server.lock().unwrap().set_all_cohort_assertions(Some("1:1:".to_string()));
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallationDeferredByPolicy",
                },
                "2": contains {
                    "event": "CheckingForUpdates",
                },
                "3": contains {
                    "event": "InstallingUpdate",
                    "target-version": "0.1.2.3",
                },
                "4": contains {
                    "event": "WaitingForReboot",
                    "target-version": "0.1.2.3",
                }
            }
        ),
        true,
        update_info(),
    )
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_update_error() {
    let env = TestEnvBuilder::new().default_with_response(OmahaResponse::Update).build().await;

    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;
    let mut last_progress: Option<InstallationProgress> = None;
    let mut installation_error = false;
    while let Some(request) = stream.try_next().await.unwrap() {
        let MonitorRequest::OnState { state, responder } = request;
        match state {
            State::InstallingUpdate(InstallingData { update, installation_progress, .. }) => {
                assert_eq!(update, update_info());
                assert!(!installation_error);
                if let Some(last_progress) = last_progress {
                    let last = last_progress.fraction_completed.unwrap();
                    let current =
                        installation_progress.as_ref().unwrap().fraction_completed.unwrap();
                    assert!(
                        last <= current,
                        "progress is not increasing, last: {last}, current: {current}",
                    );
                }
                last_progress = installation_progress;
            }

            State::InstallationError(InstallationErrorData { update, .. }) => {
                assert_eq!(update, update_info());
                assert!(!installation_error);
                installation_error = true;
            }
            state => {
                panic!("Unexpected state: {state:?}");
            }
        }
        responder.send().unwrap();
    }
    assert!(installation_error);

    env.assert_platform_metrics(tree_assertion!(
        "children": {
            "0": contains {
                "event": "CheckingForUpdates",
            },
            "1": contains {
                "event": "InstallingUpdate",
                "target-version": "0.1.2.3",
            },
            "2": contains {
                "event": "InstallationError",
                "target-version": "0.1.2.3",
            }
        }
    ))
    .await;

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "platform_metrics": contains {
                "installation_error_events": contains {
                    "capacity": 50u64,
                    "children": {
                        "0": contains {
                            "event": "InstallationError",
                            "target-version": "0.1.2.3",
                            "ts": AnyProperty,
                        }
                    }

                }
            }
        }
    );
}

#[fuchsia::test]
async fn test_omaha_client_no_update() {
    let env = TestEnvBuilder::new().build().await;

    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::NoUpdateAvailable(NoUpdateAvailableData::default()),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);

    env.assert_platform_metrics(tree_assertion!(
        "children": {
            "0": contains {
                "event": "CheckingForUpdates",
            },
            "1": contains {
                "event": "NoUpdateAvailable",
            },
        }
    ))
    .await;
}

async fn do_failed_update_check(env: &TestEnv) {
    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::ErrorCheckingForUpdate(ErrorCheckingForUpdateData::default()),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);
}

async fn do_nop_update_check(env: &TestEnv) {
    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::NoUpdateAvailable(NoUpdateAvailableData::default()),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);
}

#[fuchsia::test]
async fn test_omaha_client_invalid_response() {
    let env =
        TestEnvBuilder::new().default_with_response(OmahaResponse::InvalidResponse).build().await;

    do_failed_update_check(&env).await;

    env.assert_platform_metrics(tree_assertion!(
        "children": {
            "0": contains {
                "event": "CheckingForUpdates",
            },
            "1": contains {
                "event": "ErrorCheckingForUpdate",
            }
        }
    ))
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_invalid_url() {
    let env = TestEnvBuilder::new()
        .responses_and_metadata(vec![(
            APP_ID.to_string(),
            ResponseAndMetadata {
                response: OmahaResponse::Update,
                codebase: "invalid-url".into(),
                ..Default::default()
            },
        )])
        .build()
        .await;

    let mut stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
            State::InstallationError(InstallationErrorData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);

    env.assert_platform_metrics(tree_assertion!(
        "children": {
            "0": contains {
                "event": "CheckingForUpdates",
            },
            "1": contains {
                "event": "InstallingUpdate",
                "target-version": "0.1.2.3",
            },
            "2": contains {
                "event": "InstallationError",
                "target-version": "0.1.2.3",
            }
        }
    ))
    .await;
}

#[fuchsia::test]
async fn test_omaha_client_invalid_app_set() {
    let env = TestEnvBuilder::new().version("invalid-version").build().await;

    let options = CheckOptions {
        initiator: Some(Initiator::User),
        allow_attaching_to_existing_update_check: None,
        ..Default::default()
    };
    assert_matches!(
        env.proxies.update_manager.check_now(&options, None).await.expect("check_now"),
        Err(CheckNotStartedReason::Internal)
    );
}

#[fuchsia::test]
async fn test_omaha_client_policy_config_inspect() {
    let env = TestEnvBuilder::new().build().await;

    // Wait for omaha client to start.
    let _ = env.proxies.channel_control.get_current().await;

    assert_data_tree!(
        env.inspect_hierarchy().await,
        "root": contains {
            "policy_config": {
                "periodic_interval": 60 * 60u64,
                "startup_delay": 60u64,
                "retry_delay": 5 * 60u64,
                "allow_reboot_when_idle": true,
                "fuzz_percentage_range": 25u64,
            }
        }
    );
}

/// Verifies the signature of the CrashReport is what's expected.
fn assert_signature(report: CrashReport, expected_signature: &str) {
    assert_matches::assert_matches!(
        report,
        CrashReport {
            crash_signature: Some(signature),
            program_name: Some(program),
            program_uptime: Some(_),
            is_fatal: Some(false),
            ..
        } if signature == expected_signature && program == "system"
    )
}

/// When we fail with an installation error, we should file a crash report.
#[fuchsia::test]
async fn test_crash_report_installation_error() {
    let (hook, mut recv) = ThrottleHook::new(Ok(FileReportResults::default()));
    let env = TestEnvBuilder::new()
        .default_with_response(OmahaResponse::Update)
        .installer(MockUpdateInstallerService::with_response(Err(
            finstaller::UpdateNotStartedReason::AlreadyInProgress,
        )))
        .crash_reporter(MockCrashReporterService::new(hook))
        .build()
        .await;

    let mut stream = env.check_now().await;

    // Consume states to get InstallationError.
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
            State::InstallationError(InstallationErrorData {
                update: update_info(),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;

    // Observe the crash report was filed.
    assert_signature(recv.next().await.unwrap(), "fuchsia-installation-error");
}

/// When we fail 5 times to check for updates, we should file a crash report.
#[fuchsia::test]
async fn test_crash_report_consecutive_failed_update_checks() {
    let (hook, mut recv) = ThrottleHook::new(Ok(FileReportResults::default()));
    let env = TestEnvBuilder::new()
        .default_with_response(OmahaResponse::InvalidResponse)
        .crash_reporter(MockCrashReporterService::new(hook))
        .build()
        .await;

    // Failing <5 times will not yield crash reports.
    do_failed_update_check(&env).await;
    do_failed_update_check(&env).await;
    do_failed_update_check(&env).await;
    do_failed_update_check(&env).await;
    assert_matches!(recv.try_next(), Err(_));

    // But failing >=5 times will.
    do_failed_update_check(&env).await;
    assert_signature(recv.next().await.unwrap(), "fuchsia-5-consecutive-failed-update-checks");
    do_failed_update_check(&env).await;
    assert_signature(recv.next().await.unwrap(), "fuchsia-6-consecutive-failed-update-checks");
}

#[fuchsia::test]
async fn test_update_check_sets_updatedisabled_when_opted_out() {
    use mock_omaha_server::UpdateCheckAssertion;

    let env = TestEnvBuilder::new().default_with_response(OmahaResponse::NoUpdate).build().await;

    // The default is to enable updates.
    env.server
        .lock()
        .unwrap()
        .set_all_update_check_assertions(UpdateCheckAssertion::UpdatesEnabled);
    do_nop_update_check(&env).await;

    // The user preference is read for each update check.
    env.server
        .lock()
        .unwrap()
        .set_all_update_check_assertions(UpdateCheckAssertion::UpdatesDisabled);
    env.proxies
        .config_optout
        .set(fuchsia_update_config_optout::OptOutPreference::AllowOnlySecurityUpdates);
    env.server.lock().unwrap().set_all_cohort_assertions(Some("1:1:".to_string()));
    do_nop_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_keeps_cohort() {
    let env = TestEnvBuilder::new().build().await;

    do_nop_update_check(&env).await;

    env.server.lock().unwrap().set_all_cohort_assertions(Some("1:1:".to_string()));
    do_nop_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_persists_cohort() {
    let mut env = TestEnvBuilder::new().build().await;

    do_nop_update_check(&env).await;

    // Stop omaha-client and restart it.
    let lifecycle_controller =
        connect_to_protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>().unwrap();
    lifecycle_controller
        .stop_instance(&format!(
            "./realm_builder:{}/omaha_client_service",
            env.realm_instance.root.child_name()
        ))
        .await
        .unwrap()
        .unwrap();
    env.proxies.update_manager = env
        .realm_instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("connect to update manager");

    env.server.lock().unwrap().set_all_cohort_assertions(Some("1:1:".to_string()));
    do_nop_update_check(&env).await;
}

#[fuchsia::test]
async fn test_omaha_client_urgent_update() {
    let env =
        TestEnvBuilder::new().default_with_response(OmahaResponse::UrgentUpdate).build().await;
    omaha_client_update(
        env,
        tree_assertion!(
            "children": {
                "0": contains {
                    "event": "CheckingForUpdates",
                },
                "1": contains {
                    "event": "InstallingUpdate",
                    "target-version": "0.1.2.3",
                },
                "2": contains {
                    "event": "WaitingForReboot",
                    "target-version": "0.1.2.3",
                }
            }
        ),
        true,
        Some(UpdateInfo {
            version_available: None,
            download_size: None,
            urgent: Some(true),
            ..Default::default()
        }),
    )
    .await;
}
