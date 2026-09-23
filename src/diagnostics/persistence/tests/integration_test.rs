// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_data::InspectData;
use diagnostics_hierarchy::SelectResult;
use fidl::endpoints::Proxy;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_diagnostics_persistence as fdiagnostics_persistence;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger as flogger;
use fidl_fuchsia_power_battery as fbattery;
use fidl_fuchsia_sys2 as fsys2;
use fidl_fuchsia_update as fupdate;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt, TryStreamExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

fn extract_token(content: &str) -> Option<u64> {
    let data_vec: Vec<InspectData> = serde_json::from_str(content).ok()?;
    let selector = selectors::parse_verbose("*:root:token").ok()?;

    for data in data_vec {
        if data.moniker.to_string().contains("publisher") {
            let prop = diagnostics_hierarchy::select_from_hierarchy(
                data.payload.as_ref().unwrap(),
                &selector,
            )
            .unwrap();
            let SelectResult::Properties(props) = prop else {
                panic!("malformed/unexpected test data")
            };
            assert_eq!(props.len(), 1);
            return props[0].uint();
        }
    }
    None
}

struct TestRealm {
    instance: RealmInstance,
    current_token: Arc<AtomicU64>,
    temp_dir: tempfile::TempDir,
}

async fn make_realm(
    interval: i64,
    mock_battery_tx: Option<mpsc::Sender<fbattery::BatteryInfoWatcherProxy>>,
    mock_update_tx: Option<mpsc::Sender<fupdate::NotifierProxy>>,
) -> Result<TestRealm, Error> {
    let skip_update_check = mock_update_tx.is_none();
    let builder = RealmBuilder::new().await?;

    let temp_dir = tempfile::TempDir::new_in("/tmp")?;
    let temp_path = temp_dir.path().to_path_buf();

    // Create a local child component that serves a temporary directory (`temp_dir`)
    // containing a `"cache"` subdirectory. This directory will act as the backing
    // storage for the Persistence component under test, allowing the test to directly
    // inspect persisted Inspect snapshot files on the host filesystem.
    let storage_provider = builder
        .add_local_child(
            "storage-provider",
            move |handles| {
                let temp_path = temp_path.clone();
                Box::pin(async move {
                    let dir_proxy = fuchsia_fs::directory::open_in_namespace(
                        temp_path.to_str().unwrap(),
                        fio::PERM_READABLE | fio::PERM_WRITABLE,
                    )?;
                    let _ = fuchsia_fs::directory::create_directory_recursive(
                        &dir_proxy,
                        "cache",
                        fio::PERM_READABLE | fio::PERM_WRITABLE,
                    )
                    .await?;
                    // Serve temp_path (which contains the "cache" subdir) as the component's outgoing directory.
                    fuchsia_fs::directory::clone_onto(&dir_proxy, handles.outgoing_dir)?;
                    // Stay pending so the component continues serving the directory for the duration of the test realm.
                    futures::future::pending::<()>().await;
                    Ok(())
                })
            },
            ChildOptions::new().eager(),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("cache").rights(fio::RW_STAR_DIR).path("/cache"))
                .from(&storage_provider)
                .to(Ref::parent()),
        )
        .await?;

    let current_token = Arc::new(AtomicU64::new(0));

    let publisher_token = current_token.clone();
    let archivist = builder
        .add_child("archivist", "#meta/archivist-for-embedding.cm", ChildOptions::new().eager())
        .await?;

    let publisher = builder
        .add_local_child(
            "publisher",
            move |handles| {
                let token = publisher_token.clone();
                Box::pin(async move {
                    let inspector = fuchsia_inspect::Inspector::default();
                    inspector.root().record_lazy_values("", move || {
                        let token_val = token.load(Ordering::SeqCst);
                        let inspector = fuchsia_inspect::Inspector::default();
                        inspector.root().record_uint("token", token_val);
                        async move { Ok(inspector) }.boxed()
                    });
                    let mut options = inspect_runtime::PublishOptions::default();
                    if let Ok(proxy) =
                        handles.connect_to_protocol::<fidl_fuchsia_inspect::InspectSinkProxy>()
                    {
                        if let Ok(client_end) = proxy.into_client_end() {
                            options = options.on_inspect_sink_client(client_end);
                        }
                    }
                    let _inspect_server = inspect_runtime::publish(&inspector, options);
                    let mut fs = fuchsia_component::server::ServiceFs::new();
                    fs.serve_connection(handles.outgoing_dir)?;
                    fs.collect::<()>().await;
                    Ok(())
                })
            },
            ChildOptions::new().eager(),
        )
        .await?;

    let persistence = builder
        .add_child("persistence", "#meta/diagnostics-persistence.cm", ChildOptions::new())
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.diagnostics.persist.PersistencePeriodSeconds".parse().unwrap(),
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Int64(interval)),
        }))
        .await?;
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.diagnostics.persist.SkipUpdateCheck".parse().unwrap(),
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(
                skip_update_check,
            )),
        }))
        .await?;
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.diagnostics.persist.StopOnIdleTimeoutMillis".parse().unwrap(),
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Int64(-1)),
        }))
        .await?;
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.diagnostics.persist.LowBatteryThresholdPercent".parse().unwrap(),
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Uint64(10)),
        }))
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration(
                    "fuchsia.diagnostics.persist.PersistencePeriodSeconds",
                ))
                .capability(Capability::configuration(
                    "fuchsia.diagnostics.persist.SkipUpdateCheck",
                ))
                .capability(Capability::configuration(
                    "fuchsia.diagnostics.persist.StopOnIdleTimeoutMillis",
                ))
                .capability(Capability::configuration(
                    "fuchsia.diagnostics.persist.LowBatteryThresholdPercent",
                ))
                .from(Ref::self_())
                .to(&persistence),
        )
        .await?;

    if let Some(tx) = mock_battery_tx {
        let battery_manager = builder
            .add_local_child(
                "battery-manager",
                move |handles| {
                    let tx = tx.clone();
                    Box::pin(async move {
                        let mut fs = fuchsia_component::server::ServiceFs::new();
                        fs.dir("svc").add_fidl_service(
                            |stream: fbattery::BatteryManagerRequestStream| stream,
                        );
                        fs.serve_connection(handles.outgoing_dir)?;
                        fs.for_each_concurrent(None, move |mut stream| {
                            let mut tx = tx.clone();
                            async move {
                                while let Ok(Some(req)) = stream.try_next().await {
                                    match req {
                                        fbattery::BatteryManagerRequest::Watch {
                                            watcher,
                                            control_handle: _,
                                        } => {
                                            let proxy = watcher.into_proxy();
                                            let _ = tx.send(proxy).await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        })
                        .await;
                        Ok(())
                    })
                },
                ChildOptions::new().eager(),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fbattery::BatteryManagerMarker>())
                    .from(&battery_manager)
                    .to(&persistence),
            )
            .await?;
    }

    if let Some(tx) = mock_update_tx {
        let update_listener = builder
            .add_local_child(
                "update-listener",
                move |handles| {
                    let tx = tx.clone();
                    Box::pin(async move {
                        let mut fs = fuchsia_component::server::ServiceFs::new();
                        fs.dir("svc")
                            .add_fidl_service(|stream: fupdate::ListenerRequestStream| stream);
                        fs.serve_connection(handles.outgoing_dir)?;
                        fs.for_each_concurrent(None, move |mut stream| {
                            let mut tx = tx.clone();
                            async move {
                                while let Ok(Some(req)) = stream.try_next().await {
                                    match req {
                                        fupdate::ListenerRequest::NotifyOnFirstUpdateCheck {
                                            payload,
                                            control_handle: _,
                                        } => {
                                            if let Some(notifier) = payload.notifier {
                                                let proxy = notifier.into_proxy();
                                                let _ = tx.send(proxy).await;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        })
                        .await;
                        Ok(())
                    })
                },
                ChildOptions::new().eager(),
            )
            .await?;

        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fupdate::ListenerMarker>())
                    .from(&update_listener)
                    .to(&persistence),
            )
            .await?;
    }

    // Route capability_requested event stream to archivist
    builder
        .add_route(
            Route::new()
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&archivist),
        )
        .await?;

    // Route LogSink from parent to archivist, publisher, and persistence
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<flogger::LogSinkMarker>())
                .from(Ref::parent())
                .to(&archivist)
                .to(&publisher)
                .to(&persistence),
        )
        .await?;

    // Route InspectSink from archivist to publisher, persistence
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fidl_fuchsia_inspect::InspectSinkMarker>())
                .from(&archivist)
                .to(&publisher)
                .to(&persistence),
        )
        .await?;

    // Route diagnostics-accessors dictionary from archivist as ArchiveAccessor.previous_boot to persistence
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>()
                        .as_("fuchsia.diagnostics.ArchiveAccessor.previous_boot"),
                )
                .from(Ref::dictionary(&archivist, "diagnostics-accessors"))
                .to(&persistence),
        )
        .await?;

    // Route PreviousBootDataProvider from persistence to parent
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<
                    fdiagnostics_persistence::PreviousBootDataProviderMarker,
                >())
                .from(&persistence)
                .to(Ref::parent()),
        )
        .await?;

    // Route LifecycleController from framework to parent
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys2::LifecycleControllerMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;

    // Diagnostics Persistence consumes a `storage: "cache"` capability.
    // Because RealmBuilder's route API does not support defining a new Storage capability
    // backed by a child directory capability, we manually update the root realm's decl:
    //   1. Declare a Storage capability "cache" backed by child `storage-provider`'s "cache" directory.
    //   2. Offer the "cache" Storage capability from `self` to the `persistence` child.
    let mut realm_decl = builder.get_realm_decl().await?;
    let mut capabilities = Vec::from(realm_decl.capabilities);
    capabilities.push(cm_rust::CapabilityDecl::Storage(cm_rust::StorageDecl {
        name: "cache".parse().unwrap(),
        source: cm_rust::StorageDirectorySource::Child("storage-provider".to_string()),
        backing_dir: "cache".parse().unwrap(),
        subdir: Default::default(),
        storage_id: fidl_fuchsia_component_decl::StorageId::StaticInstanceIdOrMoniker,
    }));
    realm_decl.capabilities = capabilities.into_boxed_slice();

    let mut offers = Vec::from(realm_decl.offers);
    offers.push(cm_rust::OfferDecl::Storage(cm_rust::OfferStorageDecl {
        source: cm_rust::OfferSource::Self_,
        source_name: "cache".parse().unwrap(),
        target: cm_rust::OfferTarget::Child(cm_rust::ChildRef {
            name: "persistence".parse().unwrap(),
            collection: None,
        }),
        target_name: "cache".parse().unwrap(),
        availability: cm_rust::Availability::Required,
    }));
    realm_decl.offers = offers.into_boxed_slice();
    builder.replace_realm_decl(realm_decl).await?;

    Ok(TestRealm { instance: builder.build().await?, current_token, temp_dir })
}

fn find_active_dir(dir: &Path) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name() == "active" {
                    return Some(path);
                }
                if let Some(found) = find_active_dir(&path) {
                    return Some(found);
                }
            }
        }
    }
    None
}

async fn wait_for_active_token(temp_dir: &tempfile::TempDir, expected: u64) -> Result<(), Error> {
    loop {
        if let Some(active_dir) = find_active_dir(temp_dir.path()) {
            if let Ok(entries) = std::fs::read_dir(&active_dir) {
                let file_names: Vec<String> =
                    entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();

                if file_names.len() == 2
                    && file_names.iter().any(|f| f == "active.json")
                    && file_names.iter().any(|f| f == "metadata.json")
                {
                    let active_path = active_dir.join("active.json");
                    let meta_path = active_dir.join("metadata.json");
                    if let (Ok(content), Ok(meta_content)) =
                        (std::fs::read_to_string(&active_path), std::fs::read_to_string(&meta_path))
                    {
                        if !meta_content.is_empty()
                            && serde_json::from_str::<serde_json::Value>(&meta_content).is_ok()
                        {
                            if let Some(token) = extract_token(&content) {
                                if token == expected {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
        fuchsia_async::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
            5,
        )))
        .await;
    }
}

async fn restart_persistence(lifecycle: &fsys2::LifecycleControllerProxy) -> Result<(), Error> {
    lifecycle
        .stop_instance("./persistence")
        .await?
        .map_err(|e| anyhow::anyhow!("stop_instance error: {e:?}"))?;

    let (_, binder_server) = fidl::endpoints::create_endpoints();
    lifecycle
        .start_instance("./persistence", binder_server)
        .await?
        .map_err(|e| anyhow::anyhow!("start_instance error: {e:?}"))?;
    Ok(())
}

async fn wait_for_snapshot(
    instance: &RealmInstance,
) -> Result<fdiagnostics_persistence::PreviousBootData, Error> {
    loop {
        let provider: fdiagnostics_persistence::PreviousBootDataProviderProxy =
            instance.root.connect_to_protocol_at_exposed_dir()?;
        let data = provider
            .watch_previous_boot_data(
                &fdiagnostics_persistence::PreviousBootDataProviderOptions::default(),
            )
            .await?;
        if data.inspect.is_some() {
            return Ok(data);
        }
        fuchsia_async::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_millis(
            10,
        )))
        .await;
    }
}

impl TestRealm {
    async fn destroy(self) -> Result<(), Error> {
        let res = self.instance.destroy().await;
        drop(self.temp_dir);
        Ok(res?)
    }
}

async fn read_snapshot_token(instance: &RealmInstance) -> Result<u64, Error> {
    let data = wait_for_snapshot(instance).await?;
    let inspect_file = data.inspect.expect("Expected inspect file in PreviousBootData");
    let file_proxy = inspect_file.into_proxy();
    let content = fuchsia_fs::file::read_to_string(&file_proxy).await?;
    extract_token(&content)
        .ok_or_else(|| anyhow::anyhow!("Failed to extract token from JSON:\n{content}"))
}

#[fuchsia::test]
async fn test_persistence_rotation() -> Result<(), Error> {
    const INTERVAL: i64 = 1;
    let realm = make_realm(INTERVAL, None, None).await?;

    // Boot 1 Check: Connect to PreviousBootDataProvider, verify data.inspect is None
    let provider: fdiagnostics_persistence::PreviousBootDataProviderProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;
    let data = provider.watch_previous_boot_data(&Default::default()).await?;
    assert!(data.inspect.is_none(), "Expected no previous boot data on initial boot");

    // Set Token 1 and wait until Persistence captures it in active snapshot on disk
    const TOKEN_1: u64 = 0xAAAA_1111_2222;
    realm.current_token.store(TOKEN_1, Ordering::SeqCst);
    wait_for_active_token(&realm.temp_dir, TOKEN_1).await?;

    let lifecycle: fsys2::LifecycleControllerProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;

    // Boot 2 (First Rotation): Restart persistence and verify previous boot data contains Token 1
    restart_persistence(&lifecycle).await?;
    let token = read_snapshot_token(&realm.instance).await?;
    assert_eq!(token, TOKEN_1, "Expected previous boot data to contain Token 1");

    // Set Token 2 and wait until Persistence captures it
    const TOKEN_2: u64 = 0xBBBB_3333_4444;
    realm.current_token.store(TOKEN_2, Ordering::SeqCst);
    wait_for_active_token(&realm.temp_dir, TOKEN_2).await?;

    // Boot 3 (Second Rotation): Restart persistence and verify previous boot data contains Token 2
    restart_persistence(&lifecycle).await?;
    let token = read_snapshot_token(&realm.instance).await?;
    assert_eq!(token, TOKEN_2, "Expected previous boot data to contain Token 2");

    realm.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_low_battery_trigger() -> Result<(), Error> {
    const INTERVAL: i64 = 300;
    let (tx, mut rx) = mpsc::channel(1);
    let realm = make_realm(INTERVAL, Some(tx), None).await?;

    // Boot 1 Check: Connect to PreviousBootDataProvider, verify data.inspect is None
    let provider: fdiagnostics_persistence::PreviousBootDataProviderProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;
    let data = provider.watch_previous_boot_data(&Default::default()).await?;
    assert!(data.inspect.is_none(), "Expected no previous boot data on initial boot");

    // Set distinctive low-battery token
    const LOW_BATTERY_TOKEN: u64 = 0xCAFE_BABE_BEEF;
    realm.current_token.store(LOW_BATTERY_TOKEN, Ordering::SeqCst);

    // Wait for persistence to connect to mock BatteryManager and send watcher proxy
    let watcher_proxy = rx
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to receive BatteryInfoWatcherProxy"))?;

    // Emit a low battery event (8.0% <= threshold 10.0%)
    let low_battery_info = fbattery::BatteryInfo {
        level_percent: Some(8.0),
        level_status: Some(fbattery::LevelStatus::Low),
        ..Default::default()
    };
    watcher_proxy.on_change_battery_info(&low_battery_info, None).await?;

    // Wait until Persistence captures LOW_BATTERY_TOKEN on disk
    wait_for_active_token(&realm.temp_dir, LOW_BATTERY_TOKEN).await?;

    let lifecycle: fsys2::LifecycleControllerProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;

    // Restart persistence to rotate active snapshot to previous_boot
    restart_persistence(&lifecycle).await?;

    // Verify inspect snapshot generated by low-battery trigger is present with LOW_BATTERY_TOKEN
    let token = read_snapshot_token(&realm.instance).await?;
    assert_eq!(token, LOW_BATTERY_TOKEN, "Expected low battery snapshot to contain token");

    realm.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_update_check_gating() -> Result<(), Error> {
    const INTERVAL: i64 = 5;
    let (tx, mut rx) = mpsc::channel(1);
    let realm = make_realm(INTERVAL, None, Some(tx)).await?;

    let provider: fdiagnostics_persistence::PreviousBootDataProviderProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;

    // Wait for persistence to connect to mock update Listener and send notifier proxy
    let notifier_proxy =
        rx.next().await.ok_or_else(|| anyhow::anyhow!("Failed to receive NotifierProxy"))?;

    const UPDATE_TOKEN: u64 = 0x1234_5678_9ABC;
    realm.current_token.store(UPDATE_TOKEN, Ordering::SeqCst);

    // Signal that the post-boot update check is complete
    notifier_proxy.notify()?;

    let data = provider.watch_previous_boot_data(&Default::default()).await?;
    assert!(data.inspect.is_none(), "Expected no previous boot data on initial boot");

    // Wait deterministically for persistence to collect an active snapshot with UPDATE_TOKEN on disk
    wait_for_active_token(&realm.temp_dir, UPDATE_TOKEN).await?;

    let lifecycle: fsys2::LifecycleControllerProxy =
        realm.instance.root.connect_to_protocol_at_exposed_dir()?;
    restart_persistence(&lifecycle).await?;

    // On Boot 2 restart, persistence connects to update Listener again
    let notifier_proxy_2 = rx
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("Failed to receive NotifierProxy on Boot 2"))?;
    notifier_proxy_2.notify()?;

    // Verify inspect snapshot is served by PreviousBootDataProvider with UPDATE_TOKEN
    let token = read_snapshot_token(&realm.instance).await?;
    assert_eq!(token, UPDATE_TOKEN, "Expected previous boot data to contain UPDATE_TOKEN");

    realm.destroy().await?;
    Ok(())
}
