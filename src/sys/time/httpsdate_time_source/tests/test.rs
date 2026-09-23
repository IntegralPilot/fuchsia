// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::{ArchiveReader, Data, Inspect};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_time_external as ftime_external;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component_test::new::{
    Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route,
};
use log::warn;
use zx;

async fn connect_accessor(realm: &RealmInstance) -> fdiagnostics::ArchiveAccessorProxy {
    realm
        .root
        .connect_to_named_protocol_at_exposed_dir::<fdiagnostics::ArchiveAccessorMarker>(
            "diagnostics-accessors/fuchsia.diagnostics.ArchiveAccessor",
        )
        .expect("connect to ArchiveAccessor")
}

const TIME_SOURCE_COLLECTION: &str = "time_source_collection";
const TIME_SOURCE_URL: &str = "#meta/httpsdate_time_source.cm";
const TIME_SOURCE_NAME: &str = "httpsdate_time_source";
const ARCHIVIST_URL: &str = "#meta/archivist-for-embedding.cm";

async fn build_test_realm() -> Result<(RealmInstance, fcomponent::RealmProxy), Error> {
    let builder = RealmBuilder::new().await?;

    // 1. Add a collection to the realm declaration
    let mut realm_decl = builder.get_realm_decl().await?;
    cm_rust::push_box(
        &mut realm_decl.collections,
        cm_rust::CollectionDecl {
            name: TIME_SOURCE_COLLECTION.parse().unwrap(),
            durability: fdecl::Durability::Transient,
            environment: None,
            allowed_offers: cm_types::AllowedOffers::StaticAndDynamic,
            allow_long_names: false,
            persistent_storage: None,
        },
    );
    builder.replace_realm_decl(realm_decl).await?;

    // Route other necessary capabilities
    let archivist =
        builder.add_child("archivist", ARCHIVIST_URL, ChildOptions::new().eager()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&archivist),
        )
        .await
        .expect("added routes from parent to archivist");

    builder
        .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: "diagnostics".parse().unwrap(),
            source_path: None,
        }))
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics-accessors"))
                .from(&archivist)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.inspect.InspectSink"))
                .from(&archivist)
                .to(Ref::dictionary(Ref::self_(), "diagnostics")),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("diagnostics"))
                .from(Ref::self_())
                .to(Ref::collection(TIME_SOURCE_COLLECTION)),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(Ref::collection(TIME_SOURCE_COLLECTION)),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.component.Realm"))
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;

    let realm_instance = builder.build().await?;
    let realm_proxy =
        realm_instance.root.connect_to_named_protocol_at_exposed_dir::<fcomponent::RealmMarker>(
            "fuchsia.component.Realm",
        )?;

    Ok((realm_instance, realm_proxy))
}

async fn start_and_connect_to_pull_source(
    realm_proxy: &fcomponent::RealmProxy,
) -> Result<ftime_external::PullSourceProxy, Error> {
    // Create time source component collection
    let collection_ref = fdecl::CollectionRef { name: TIME_SOURCE_COLLECTION.into() };
    let child_decl = fdecl::Child {
        name: Some(TIME_SOURCE_NAME.into()),
        url: Some(TIME_SOURCE_URL.into()),
        startup: Some(fdecl::StartupMode::Lazy),
        ..Default::default()
    };

    realm_proxy
        .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
        .await?
        .map_err(|e| format_err!("failed to create child: {:?}", e))?;

    let child_ref = fdecl::ChildRef {
        name: TIME_SOURCE_NAME.into(),
        collection: Some(TIME_SOURCE_COLLECTION.into()),
    };
    let (exposed_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    realm_proxy
        .open_exposed_dir(&child_ref, server_end)
        .await?
        .map_err(|e| format_err!("failed to open exposed dir: {:?}", e))?;

    let pull_source =
        client::connect_to_protocol_at_dir_root::<ftime_external::PullSourceMarker>(&exposed_dir)?;
    Ok(pull_source)
}

async fn get_inspect_data(
    accessor: &fdiagnostics::ArchiveAccessorProxy,
    moniker: &str,
) -> Result<Vec<Data<Inspect>>, Error> {
    ArchiveReader::inspect()
        .with_archive(accessor.clone())
        .select_all_for_component(moniker)
        .snapshot()
        .await
        .map_err(|e| format_err!("failed to get inspect data: {:?}", e))
}

#[fuchsia::test]
async fn test_time_source_start_twice() -> Result<(), Error> {
    let (realm_instance, realm_proxy) = build_test_realm().await?;

    // Connect to PullSource to start it
    let pull_source = start_and_connect_to_pull_source(&realm_proxy).await?;

    // Call sample() to ensure the component is started and has some inspect data
    let _ = pull_source.sample(ftime_external::Urgency::Low).await?;

    // Verify inspect data
    let moniker = format!(
        "realm_builder:{}/time_source_collection:httpsdate_time_source",
        realm_instance.root.child_name()
    );
    let accessor = connect_accessor(&realm_instance).await;
    let results = get_inspect_data(&accessor, &moniker).await?;
    assert_eq!(results.len(), 1);
    let time_0 = results[0]
        .payload
        .as_ref()
        .unwrap()
        .get_property("phase_update_time")
        .unwrap()
        .uint()
        .unwrap();

    // not escrowed
    assert_eq!(results[0].metadata.escrowed, false);

    warn!("Initial inspect data: {:?}", results[0].payload.as_ref().unwrap());
    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        phase: "Initial",
        phase_update_time: diagnostics_assertions::AnyProperty,
        failures: {},
        last_failure_time: 0u64,
        config: contains {},
        sample_0: contains {},
        sample_1: contains {},
        sample_2: contains {},
        sample_3: contains {},
        sample_4: contains {},
    });

    // Shutdown and verify escrow
    warn!("Shutting down and verifying escrow");
    let token =
        pull_source.shutdown().await?.map_err(|e| format_err!("shutdown failed: {:?}", e))?;

    // Stop component
    warn!("Stopping component");
    let child_ref = fdecl::ChildRef {
        name: TIME_SOURCE_NAME.into(),
        collection: Some(TIME_SOURCE_COLLECTION.into()),
    };
    realm_proxy
        .destroy_child(&child_ref)
        .await?
        .map_err(|e| format_err!("failed to destroy child: {:?}", e))?;

    // Wait a bit for Archivist to process the escrow
    fasync::Timer::new(zx::MonotonicDuration::from_seconds(5)).await;

    warn!("Getting inspect data after shutdown");
    let results = get_inspect_data(&accessor, &moniker).await?;

    warn!("results after shutdown: {:?}", results);
    assert_eq!(results.len(), 1);
    let time_1 = results[0]
        .payload
        .as_ref()
        .unwrap()
        .get_property("phase_update_time")
        .unwrap()
        .uint()
        .unwrap();

    // escrowed
    assert_eq!(results[0].metadata.escrowed, true);

    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        phase: "Initial",
        phase_update_time: diagnostics_assertions::AnyProperty,
        failures: {},
        last_failure_time: 0u64,
        config: contains {},
        sample_0: contains {},
        sample_1: contains {},
        sample_2: contains {},
        sample_3: contains {},
        sample_4: contains {},
    });

    drop(token);

    // Start again and verify it still has the old data
    let pull_source = start_and_connect_to_pull_source(&realm_proxy).await?;
    let _ = pull_source.sample(ftime_external::Urgency::Low).await?;

    let results = get_inspect_data(&accessor, &moniker).await?;
    warn!("results after second start: {:?}", results);
    assert_eq!(results.len(), 1);
    let time_2 = results[0]
        .payload
        .as_ref()
        .unwrap()
        .get_property("phase_update_time")
        .unwrap()
        .uint()
        .unwrap();

    // not escrowed
    assert_eq!(results[0].metadata.escrowed, false);

    assert_data_tree!(results[0].payload.as_ref().unwrap(), root: {
        phase: "Initial",
        phase_update_time: diagnostics_assertions::AnyProperty,
        failures: {},
        last_failure_time: 0u64,
        config: contains {},
        sample_0: contains {},
        sample_1: contains {},
        sample_2: contains {},
        sample_3: contains {},
        sample_4: contains {},
    });

    assert_eq!(time_0, time_1);
    assert_ne!(time_1, time_2);
    Ok(())
}
