// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![cfg(test)]
use anyhow::anyhow;
use assert_matches::assert_matches;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_paver as fpaver;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fidl_fuchsia_update::{
    AttemptsMonitorMarker, AttemptsMonitorRequest, AttemptsMonitorRequestStream, CheckOptions,
    CheckingForUpdatesData, CommitStatusProviderMarker, CommitStatusProviderProxy, Initiator,
    InstallationDeferralReason, InstallationDeferredData, InstallationErrorData,
    InstallationProgress, InstallingData, ManagerMarker, ManagerProxy, MonitorMarker,
    MonitorRequest, MonitorRequestStream, State, UpdateInfo,
};
use fidl_fuchsia_update_channel::{ProviderMarker, ProviderProxy};
use fidl_fuchsia_update_installer as finstaller;
use fidl_fuchsia_update_installer_ext as installer;
use fidl_fuchsia_update_verify as fupdate_verify;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use fuchsia_pkg_testing::make_packages_json;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use mock_health_verification::MockHealthVerificationService;
use mock_installer::MockUpdateInstallerService;
use mock_paver::{MockPaverService, MockPaverServiceBuilder, PaverEvent, hooks as mphooks};
use mock_resolver::MockResolverService;
use mock_space::MockSpaceService;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tempfile::TempDir;

struct Mounts {
    system: TempDir,
}

struct Proxies {
    resolver: Arc<MockResolverService>,
    channel_provider: ProviderProxy,
    update_manager: ManagerProxy,
    commit_status_provider: CommitStatusProviderProxy,
    _space: Arc<MockSpaceService>,
    _verifier: Arc<MockHealthVerificationService>,
}

impl Mounts {
    fn new() -> Self {
        Self { system: tempfile::tempdir().expect("/tmp to exist") }
    }
}

struct TestEnvBuilder {
    installer: MockUpdateInstallerService,
    paver: Option<MockPaverService>,
    space: Option<MockSpaceService>,
}

impl TestEnvBuilder {
    fn new() -> Self {
        Self { installer: MockUpdateInstallerService::builder().build(), paver: None, space: None }
    }

    fn installer(self, installer: MockUpdateInstallerService) -> Self {
        Self { installer, ..self }
    }

    fn space(self, space: MockSpaceService) -> Self {
        assert!(self.space.is_none());
        Self { space: Some(space), ..self }
    }

    fn paver(self, paver: MockPaverService) -> Self {
        Self { paver: Some(paver), ..self }
    }

    async fn build(self) -> TestEnv {
        let mounts = Mounts::new();
        std::fs::write(
            mounts.system.path().join("meta"),
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .expect("write system/meta");

        let mut fs = ServiceFs::new();
        // Add fake directories.
        let system = fuchsia_fs::directory::open_in_namespace(
            mounts.system.path().to_str().unwrap(),
            fuchsia_fs::PERM_READABLE,
        )
        .unwrap();
        fs.add_remote("system", system);

        let mut svc = fs.dir("svc");

        // Setup the mock resolver service.
        let resolver = Arc::new(MockResolverService::new(None));
        {
            let resolver = Arc::clone(&resolver);
            svc.add_fidl_service(move |stream| {
                fasync::Task::spawn(
                    Arc::clone(&resolver)
                        .run_resolver_service(stream)
                        .unwrap_or_else(|e| panic!("error running resolver service {e:?}")),
                )
                .detach()
            });
        }

        // Setup the mock installer service.
        let installer = Arc::new(self.installer);
        {
            let installer = Arc::clone(&installer);
            svc.add_fidl_service(move |stream| {
                fasync::Task::spawn(Arc::clone(&installer).run_service(stream)).detach()
            });
        }

        // Setup the mock paver service
        let paver = Arc::new(self.paver.unwrap_or_else(|| MockPaverServiceBuilder::new().build()));
        svc.add_fidl_service(move |stream: fpaver::PaverRequestStream| {
            fasync::Task::spawn(
                Arc::clone(&paver)
                    .run_paver_service(stream)
                    .unwrap_or_else(|e| panic!("error running paver service: {:#}", anyhow!(e))),
            )
            .detach();
        });

        // Setup the mock space service
        let space =
            Arc::new(self.space.unwrap_or_else(|| MockSpaceService::new(Box::new(|| Ok(())))));
        {
            let space = Arc::clone(&space);
            svc.add_fidl_service(move |stream| {
                fasync::Task::spawn(
                    Arc::clone(&space)
                        .run_space_service(stream)
                        .unwrap_or_else(|e| panic!("error running space service {e:?}")),
                )
                .detach()
            });
        }

        // Setup the mock verifier service.
        let verifier = Arc::new(MockHealthVerificationService::new(|| Ok(())));
        {
            let verifier = Arc::clone(&verifier);
            svc.add_fidl_service(move |stream| {
                fasync::Task::spawn(Arc::clone(&verifier).run_health_verification_service(stream))
                    .detach()
            });
        }

        let fs_holder = Mutex::new(Some(fs));
        let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
        let system_update_checker = builder
            .add_child(
                "system_update_checker",
                "#meta/system-update-checker.cm",
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();
        let system_update_committer = builder
            .add_child(
                "system_update_committer",
                "#meta/system-update-committer.cm",
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
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fidl_fuchsia_logger::LogSinkMarker>())
                    .from(Ref::parent())
                    .to(&system_update_checker)
                    .to(&system_update_committer),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fpaver::PaverMarker>())
                    .from(&fake_capabilities)
                    .to(&system_update_checker)
                    .to(&system_update_committer),
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
                    .capability(Capability::protocol::<finstaller::InstallerMarker>())
                    .capability(Capability::protocol::<fpkg::PackageResolverMarker>())
                    .capability(Capability::protocol::<fpkg::RepositoryManagerMarker>())
                    .capability(Capability::protocol::<fidl_fuchsia_pkg_rewrite::EngineMarker>())
                    .capability(Capability::protocol::<fpkg_gc::ManagerMarker>())
                    .capability(
                        Capability::directory("system").path("/system").rights(fio::R_STAR_DIR),
                    )
                    .from(&fake_capabilities)
                    .to(&system_update_checker),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<ProviderMarker>())
                    .capability(Capability::protocol::<
                        fidl_fuchsia_update_channelcontrol::ChannelControlMarker,
                    >())
                    .capability(Capability::protocol::<ManagerMarker>())
                    .from(&system_update_checker)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<CommitStatusProviderMarker>())
                    .from(&system_update_committer)
                    .to(&system_update_checker)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration("fuchsia.ota_channel"))
                    .from(Ref::void())
                    .to(&system_update_checker),
            )
            .await
            .unwrap();

        let realm_instance = builder.build().await.unwrap();
        let channel_provider = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to channel provider");
        let update_manager = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to update manager");
        let commit_status_provider = realm_instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .expect("connect to commit status provider");

        TestEnv {
            _realm_instance: realm_instance,
            _mounts: mounts,
            proxies: Proxies {
                resolver,
                channel_provider,
                update_manager,
                commit_status_provider,
                _space: space,
                _verifier: verifier,
            },
        }
    }
}

struct TestEnv {
    _realm_instance: RealmInstance,
    _mounts: Mounts,
    proxies: Proxies,
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
}

async fn expect_states(stream: &mut MonitorRequestStream, expected_states: &[State]) {
    for expected_state in expected_states {
        let MonitorRequest::OnState { state, responder } =
            stream.try_next().await.unwrap().unwrap();
        assert_eq!(&state, expected_state);
        responder.send().unwrap();
    }
}

fn update_info(download_size: Option<u64>) -> Option<UpdateInfo> {
    Some(UpdateInfo {
        version_available: Some(
            "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".to_string(),
        ),
        download_size,
        ..Default::default()
    })
}

fn progress(fraction_completed: Option<f32>) -> Option<InstallationProgress> {
    Some(InstallationProgress { fraction_completed, ..Default::default() })
}

#[fuchsia::test]
async fn test_channel_provider_get_current() {
    let env = TestEnvBuilder::new().build().await;

    assert_eq!(
        env.proxies.channel_provider.get_current().await.expect("get_current"),
        "".to_string()
    );
}

#[fuchsia::test]
async fn test_update_manager_check_now_error_checking_for_update() {
    let env = TestEnvBuilder::new().build().await;

    let (client_end, request_stream) = fidl::endpoints::create_request_stream();

    assert!(
        env.proxies
            .update_manager
            .check_now(
                &fidl_fuchsia_update::CheckOptions {
                    initiator: Some(fidl_fuchsia_update::Initiator::User),
                    allow_attaching_to_existing_update_check: Some(true),
                    ..Default::default()
                },
                Some(client_end),
            )
            .await
            .is_ok()
    );
    assert_eq!(
        request_stream
            .map(|r| {
                let MonitorRequest::OnState { state, responder } = r.unwrap();
                responder.send().unwrap();
                state
            })
            .collect::<Vec<State>>()
            .await,
        vec![
            State::CheckingForUpdates(fidl_fuchsia_update::CheckingForUpdatesData::default()),
            State::ErrorCheckingForUpdate(
                fidl_fuchsia_update::ErrorCheckingForUpdateData::default()
            ),
        ]
    );
}

#[fuchsia::test]
async fn test_update_manager_progress() {
    let (mut sender, receiver) = mpsc::channel(0);
    let installer = MockUpdateInstallerService::builder().states_receiver(receiver).build();

    let env = TestEnvBuilder::new().installer(installer).build().await;

    env.proxies.resolver.url("fuchsia-pkg://fuchsia.com/update").resolve(
        &env.proxies
            .resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file("zbi", "fake zbi"),
    );
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead")
        .resolve(
        &env.proxies
            .resolver
            .package("system_image", "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeada")
    );

    let mut stream = env.check_now().await;

    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(None),
                installation_progress: None,
                ..Default::default()
            }),
        ],
    )
    .await;
    sender.send(installer::State::Prepare).await.unwrap();
    expect_states(
        &mut stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(None),
            installation_progress: progress(None),
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
            update: update_info(Some(1000)),
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
            update: update_info(Some(1000)),
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
        &[State::WaitingForReboot(InstallingData {
            update: update_info(Some(1000)),
            installation_progress: progress(Some(1.0)),
            ..Default::default()
        })],
    )
    .await;
}

#[fuchsia::test]
async fn test_monitor_all_updates() {
    let (mut sender, receiver) = mpsc::channel(0);
    let installer = MockUpdateInstallerService::builder().states_receiver(receiver).build();

    let env = TestEnvBuilder::new().installer(installer).build().await;

    env.proxies.resolver.url("fuchsia-pkg://fuchsia.com/update").resolve(
        &env.proxies
            .resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file("zbi", "fake zbi"),
    );
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead")
        .resolve(
        &env.proxies
            .resolver
            .package("system_image", "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeada")
    );

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
                update: update_info(None),
                installation_progress: None,
                ..Default::default()
            }),
        ],
    )
    .await;
    sender.send(installer::State::Prepare).await.unwrap();
    expect_states(
        &mut monitor_stream,
        &[State::InstallingUpdate(InstallingData {
            update: update_info(None),
            installation_progress: progress(None),
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
            update: update_info(Some(1000)),
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
            update: update_info(Some(1000)),
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
        &[State::WaitingForReboot(InstallingData {
            update: update_info(Some(1000)),
            installation_progress: progress(Some(1.0)),
            ..Default::default()
        })],
    )
    .await;
}

#[fuchsia::test]
async fn test_update_manager_out_of_space_gc_succeeds() {
    let called = Arc::new(AtomicU32::new(0));
    let space = {
        let called = Arc::clone(&called);
        MockSpaceService::new(Box::new(move || {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }))
    };

    let env = TestEnvBuilder::new().space(space).build().await;

    env.proxies.resolver.url("fuchsia-pkg://fuchsia.com/update").respond_serially(vec![
        Err(fpkg::ResolveError::NoSpace),
        Ok(env.proxies.resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file("zbi", "fake zbi"),
        ),
    ]);
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead")
        .resolve(
        &env.proxies
            .resolver
            .package("system_image", "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeada")
    );

    let mut stream = env.check_now().await;

    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(None),
                installation_progress: None,
                ..Default::default()
            }),
            State::InstallationError(InstallationErrorData {
                update: Some(UpdateInfo {
                    version_available: Some(
                        "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".into(),
                    ),
                    download_size: None,
                    ..Default::default()
                }),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;

    // Make sure we tried to call `fuchsia.pkg.garbagecollector.Manager.Gc()`.
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[fuchsia::test]
async fn test_update_manager_out_of_space_gc_fails() {
    let called = Arc::new(AtomicU32::new(0));
    let space = {
        let called = Arc::clone(&called);
        MockSpaceService::new(Box::new(move || {
            called.fetch_add(1, Ordering::SeqCst);
            Err(fpkg_gc::GcError::Internal)
        }))
    };

    let env = TestEnvBuilder::new().space(space).build().await;

    env.proxies.resolver.url("fuchsia-pkg://fuchsia.com/update").respond_serially(vec![
        Err(fpkg::ResolveError::NoSpace),
        Ok(env.proxies.resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file("zbi", "fake zbi"),
        ),
    ]);
    env.proxies
        .resolver.url("fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead")
        .resolve(
        &env.proxies
            .resolver
            .package("system_image", "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeada")
    );

    let mut stream = env.check_now().await;

    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(None),
                installation_progress: None,
                ..Default::default()
            }),
            State::InstallationError(InstallationErrorData {
                update: Some(UpdateInfo {
                    version_available: Some(
                        "beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead".into(),
                    ),
                    download_size: None,
                    ..Default::default()
                }),
                installation_progress: progress(None),
                ..Default::default()
            }),
        ],
    )
    .await;

    // Make sure we tried to call `fuchsia.pkg.garbagecollector.Manager.Gc()`.
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[fuchsia::test]
async fn test_installation_deferred() {
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
            .build()
            .await
    };

    env.proxies.resolver.url("fuchsia-pkg://fuchsia.com/update").resolve(
        &env.proxies
            .resolver
            .package("update", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .add_file(
                "packages.json",
                make_packages_json(["fuchsia-pkg://fuchsia.com/system_image/0?hash=beefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdead"]),
            )
            .add_file("zbi", "fake zbi"),
    );

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
                update: update_info(None),
                deferral_reason: Some(InstallationDeferralReason::CurrentSystemNotCommitted),
                ..Default::default()
            }),
        ],
    )
    .await;
    assert_matches!(stream.next().await, None);

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

    stream = env.check_now().await;
    expect_states(
        &mut stream,
        &[
            State::CheckingForUpdates(CheckingForUpdatesData::default()),
            State::InstallingUpdate(InstallingData {
                update: update_info(None),
                installation_progress: None,
                ..Default::default()
            }),
        ],
    )
    .await;
}
