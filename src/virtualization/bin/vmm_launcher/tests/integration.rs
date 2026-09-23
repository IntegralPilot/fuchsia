// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Error};
use fidl_fuchsia_component::{ChildIteratorMarker, RealmProxy};
use fidl_fuchsia_component_decl as cdecl;
use fidl_fuchsia_virtualization::{GuestError, GuestLifecycleProxy};
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};

static VMM_URL: &'static str = "#meta/vmm.cm";

async fn build_test_realm(vmm_url: &'static str) -> Result<RealmInstance, Error> {
    let builder = RealmBuilder::new().await?;
    let vmm_launcher = builder
        .add_child("vmm_launcher", "#meta/vmm_launcher.cm", ChildOptions::new().eager())
        .await
        .expect("Failed to add vmm_launcher child");

    // Override the component_url for the vmm so that we can test both the C++ and Rust vmm
    // binaries.
    builder.init_mutable_config_from_package(&vmm_launcher).await.unwrap();
    builder.set_config_value(&vmm_launcher, "vmm_component_url", vmm_url.into()).await.unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.virtualization.GuestLifecycle"))
                .from(&vmm_launcher)
                .to(Ref::parent()),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.kernel.HypervisorResource"))
                .from(Ref::parent())
                .to(&vmm_launcher),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.kernel.VmexResource"))
                .from(Ref::parent())
                .to(&vmm_launcher),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.component.Realm"))
                .from(&vmm_launcher)
                .to(Ref::parent()),
        )
        .await?;
    builder.build().await.map_err(|e| anyhow!("Failed to build realm: {}", e))
}

async fn list_realm_children(test_realm: &RealmInstance) -> Result<Vec<cdecl::ChildRef>, Error> {
    let realm: RealmProxy = test_realm.root.connect_to_protocol_at_exposed_dir()?;
    let collection_ref = cdecl::CollectionRef { name: "virtual_machine_managers".to_string() };
    let (child_iterator, child_iterator_server_end) =
        fidl::endpoints::create_proxy::<ChildIteratorMarker>();
    realm.list_children(&collection_ref, child_iterator_server_end).await.unwrap().unwrap();

    let mut result: Vec<cdecl::ChildRef> = Vec::new();
    while let Ok(mut children) = child_iterator.next().await {
        if children.is_empty() {
            break;
        }
        result.append(&mut children);
    }
    Ok(result)
}

async fn start_vmm(realm: &RealmInstance) -> Result<GuestLifecycleProxy, Error> {
    let lifecycle: GuestLifecycleProxy = realm.root.connect_to_protocol_at_exposed_dir()?;
    // Send a run RPC to the guest. This is to ensure that the component is completely started.
    let response = lifecycle.run().await.expect("Failed to connect to vmm");
    assert_eq!(Err(GuestError::NotCreated), response);
    Ok(lifecycle)
}

async fn vmm_launcher_launches_vmm(url: &'static str) -> Result<(), Error> {
    let realm = build_test_realm(url).await.unwrap();
    let _lifecycle = start_vmm(&realm).await.expect("Failed to start VMM");
    Ok(())
}

#[fuchsia::test]
async fn test_vmm_launcher_launches_vmm() -> Result<(), Error> {
    vmm_launcher_launches_vmm(VMM_URL).await
}

async fn spawn_component_per_connection(url: &'static str) -> Result<(), Error> {
    let test_realm = build_test_realm(url).await.unwrap();

    // Expect no children to start.
    let children = list_realm_children(&test_realm).await.unwrap();
    assert_eq!(children.len(), 0);

    // Connect to lifecycle, expect a new realm child.
    let _lifecycle1 = start_vmm(&test_realm).await.expect("Failed to start first vmm");
    let children = list_realm_children(&test_realm).await.unwrap();
    assert_eq!(children.len(), 1);

    // Make a second connection, expect a new child to be created.
    let _lifecycle2 = start_vmm(&test_realm).await.expect("Failed to start second vmm");
    let children = list_realm_children(&test_realm).await.unwrap();
    assert_eq!(children.len(), 2);

    Ok(())
}

#[fuchsia::test]
async fn test_component_per_connection_vmm() -> Result<(), Error> {
    spawn_component_per_connection(VMM_URL).await
}

async fn drop_guest_lifecycle_terminates_component(url: &'static str) -> Result<(), Error> {
    let test_realm = build_test_realm(url).await.unwrap();

    // Expect no children to start.
    let children = list_realm_children(&test_realm).await.unwrap();
    assert_eq!(children.len(), 0);

    // Connect to lifecycle, expect a new realm child.
    let lifecycle = start_vmm(&test_realm).await.expect("Failed to start first vmm");
    let children = list_realm_children(&test_realm).await.unwrap();
    assert_eq!(children.len(), 1);

    // Drop the lifecycle channel and expect the vmm component to eventually terminate.
    std::mem::drop(lifecycle);
    loop {
        let children = list_realm_children(&test_realm).await.unwrap();
        if children.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    Ok(())
}

#[fuchsia::test]
async fn test_drop_guest_lifecycle_terminates_vmm() -> Result<(), Error> {
    drop_guest_lifecycle_terminates_component(VMM_URL).await
}
