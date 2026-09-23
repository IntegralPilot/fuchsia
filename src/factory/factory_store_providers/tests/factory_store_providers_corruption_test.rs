// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]
use anyhow::Error;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt, TryStreamExt};
use std::path::Path;
use std::sync::Arc;
use {
    fidl_fuchsia_factory as ffactory, fidl_fuchsia_feedback as ffeedback, fidl_fuchsia_io as fio,
};

static DATA_FILE_PATH: &'static str = "/pkg/data";

macro_rules! connect_to_factory_store_provider {
    ($t:ty, $realm:expr) => {{
        let provider = $realm
            .root
            .connect_to_protocol_at_exposed_dir::<$t>()
            .expect("Failed to connect to protocol");

        let (dir_proxy, dir_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        provider.get_factory_store(dir_server).expect("Failed to get factory store");
        dir_proxy
    }};
}

#[fuchsia::test]
fn test_set_up_properly() {
    assert!(
        Path::new("/pkg/data/empty").exists()
            && Path::new("/pkg/data/fake_factory_items.json").exists()
    );
}

pub enum IncomingService {
    CrashReporter(ffeedback::CrashReporterRequestStream),
}

async fn crash_reporter_server_mock(
    handles: LocalComponentHandles,
    reporter_tx: Arc<mpsc::UnboundedSender<ffeedback::CrashReport>>,
) -> Result<(), Error> {
    // Create a new ServiceFs to host FIDL protocols from
    let mut fs = ServiceFs::new();

    // Add the CrashReporter protocol to the ServiceFs
    fs.dir("svc").add_fidl_service(IncomingService::CrashReporter);

    // Run the ServiceFs on the outgoing directory handle from the mock handles
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(0, move |IncomingService::CrashReporter(stream)| {
        let reporter_tx = reporter_tx.clone();
        async move {
            stream
                .try_for_each(|request| {
                    let reporter_tx = reporter_tx.clone();
                    async move {
                        let reporter_tx = reporter_tx.clone();
                        match request {
                            ffeedback::CrashReporterRequest::FileReport { report, responder } => {
                                let results = ffeedback::FileReportResults {
                                    result: Some(ffeedback::FilingSuccess::ReportUploaded),
                                    report_id: Some("test-report".to_string()),
                                    ..Default::default()
                                };
                                responder
                                    .send(Ok(&results))
                                    .expect("failed to send FileReport response");
                                reporter_tx
                                    .unbounded_send(report)
                                    .expect("failed to send FileReport over mpsc channel");
                            }
                        }
                        Ok(())
                    }
                })
                .await
                .unwrap_or_else(|e| eprintln!("Error encountered: {:?}", e))
        }
    })
    .await;

    Ok(())
}

async fn create_realm() -> (RealmInstance, mpsc::UnboundedReceiver<ffeedback::CrashReport>) {
    let (crash_tx, crash_rx) = mpsc::unbounded();
    let crash_tx = Arc::new(crash_tx);
    let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");

    let factory_store_providers = builder
        .add_child(
            "factory_store_providers",
            "#meta/factory_store_providers.cm".to_string(),
            ChildOptions::new(),
        )
        .await
        .expect("Failed adding factory_store_providers to topology");

    let fake_factory_items = builder
        .add_child(
            "fake_factory_items",
            "#meta/fake_factory_items.cm".to_string(),
            ChildOptions::new(),
        )
        .await
        .expect("Failed adding fake_factory_items to topology");

    let crash_reporter = builder
        .add_local_child(
            "crash_reporter",
            move |handles| Box::pin(crash_reporter_server_mock(handles, crash_tx.clone())),
            ChildOptions::new(),
        )
        .await
        .expect("Failed adding crash_reporter to topology");

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&factory_store_providers),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&fake_factory_items),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&crash_reporter),
        )
        .await
        .unwrap();

    let config_provider = builder
        .add_local_child(
            "config_provider",
            move |handles| {
                let local_config_dir = vfs::pseudo_directory! {
                    "config" => vfs::pseudo_directory! {
                        "data" => vfs::remote::remote_dir(fuchsia_fs::directory::open_in_namespace(DATA_FILE_PATH, fio::PERM_READABLE).unwrap()),
                    },
                    "factory" => vfs::pseudo_directory!{},
                };

                let scope = vfs::execution_scope::ExecutionScope::new();
                vfs::directory::serve_on(
                    local_config_dir,
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
        .expect("Failed adding config_provider to topology");

    // Provide configs for factory_store_providers
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::directory("config-data")
                        .path("/config/data")
                        .rights(fio::R_STAR_DIR),
                )
                .from(&config_provider)
                .to(&factory_store_providers),
        )
        .await
        .unwrap();

    // Route capabilities.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.boot.FactoryItems"))
                .from(&fake_factory_items)
                .to(&factory_store_providers),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.feedback.CrashReporter"))
                .from(&crash_reporter)
                .to(&factory_store_providers),
        )
        .await
        .unwrap();

    // Expose protocols needed by the test component.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.AlphaFactoryStoreProvider",
                ))
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.CastCredentialsFactoryStoreProvider",
                ))
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.MiscFactoryStoreProvider",
                ))
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.PlayReadyFactoryStoreProvider",
                ))
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.WeaveFactoryStoreProvider",
                ))
                .capability(Capability::protocol_by_name(
                    "fuchsia.factory.WidevineFactoryStoreProvider",
                ))
                .from(&factory_store_providers)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    (builder.build().await.unwrap(), crash_rx)
}

#[fuchsia::test]
async fn test_factory_store_providers_files_crash_report_on_bad_factory_data() {
    let (realm, mut crash_rx) = create_realm().await;
    let provider = realm
        .root
        .connect_to_protocol_at_exposed_dir::<ffactory::MiscFactoryStoreProviderProxy>()
        .expect("Failed to connect to protocol");
    let (_dir_proxy, dir_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    provider.get_factory_store(dir_server).expect("Failed to get factory store");

    let report = crash_rx.next().await.unwrap();
    assert_eq!("factory_store_providers", report.program_name.unwrap().as_str());
    assert_eq!("fuchsia-factory-invalid-data", report.crash_signature.unwrap().as_str());
    assert!(!report.is_fatal.unwrap());
}

#[fuchsia::test]
async fn test_factory_directories_are_empty_with_corrupt_factory() -> Result<(), Error> {
    let (realm, _crash_rx) = create_realm().await;
    {
        let dir_proxy =
            connect_to_factory_store_provider!(ffactory::AlphaFactoryStoreProviderProxy, realm);
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }
    {
        let dir_proxy = connect_to_factory_store_provider!(
            ffactory::CastCredentialsFactoryStoreProviderProxy,
            realm
        );
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }
    {
        let dir_proxy =
            connect_to_factory_store_provider!(ffactory::MiscFactoryStoreProviderProxy, realm);
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }
    {
        let dir_proxy =
            connect_to_factory_store_provider!(ffactory::PlayReadyFactoryStoreProviderProxy, realm);
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }
    {
        let dir_proxy =
            connect_to_factory_store_provider!(ffactory::WeaveFactoryStoreProviderProxy, realm);
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }
    {
        let dir_proxy =
            connect_to_factory_store_provider!(ffactory::WidevineFactoryStoreProviderProxy, realm);
        assert!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap().is_empty());
    }

    Ok(())
}
