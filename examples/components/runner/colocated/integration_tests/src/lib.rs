// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use attribution_testing::PrincipalIdentifier;
use fidl_fuchsia_process::HandleInfo;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use fuchsia_runtime::HandleType;
use futures_util::{FutureExt, StreamExt};
use std::sync::Arc;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_examples_colocated as fcolocated, fidl_fuchsia_memory_attribution as fattribution,
    fuchsia_async as fasync,
};

#[fuchsia::test]
async fn test_attribute_memory() {
    // Starts a component manager and obtain its root job, so that we can simulate
    // traversing the root job of the system, the kind done in `memory_monitor`.
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_realm.cm"),
    )
    .await
    .expect("Failed to create test realm builder");

    // Add a child to receive these capabilities so that we can use them in this test.
    // - fuchsia.memory.attribution.Provider
    // - fuchsia.component.Realm
    struct Capabilities {
        attribution_provider: fattribution::ProviderProxy,
        introspector: fcomponent::IntrospectorProxy,
        realm: fcomponent::RealmProxy,
    }
    let (capabilities_sender, capabilities_receiver) = async_channel::unbounded::<Capabilities>();
    let capabilities_sender = Arc::new(capabilities_sender);
    let receiver = builder
        .add_local_child(
            "receiver",
            move |handles| {
                let capabilities_sender = capabilities_sender.clone();
                async move {
                    capabilities_sender
                        .send(Capabilities {
                            attribution_provider: handles.connect_to_protocol().unwrap(),
                            introspector: handles.connect_to_protocol().unwrap(),
                            realm: handles.connect_to_protocol().unwrap(),
                        })
                        .await
                        .unwrap();

                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .expect("Failed to add child");

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.memory.attribution.Provider"))
                .from(Ref::child("elf_runner"))
                .to(&receiver),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.component.Realm"))
                .from(Ref::framework())
                .to(&receiver),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.component.Introspector"))
                .from(Ref::framework())
                .to(&receiver),
        )
        .await
        .unwrap();

    let _realm =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    let capabilities = capabilities_receiver.recv().await.unwrap();

    // Start a colocated component.
    let collection = fdecl::CollectionRef { name: "collection".to_string() };
    let decl = fdecl::Child {
        name: Some("colocated-component".to_string()),
        url: Some("#meta/colocated-component.cm".to_string()),
        startup: Some(fdecl::StartupMode::Lazy),
        ..Default::default()
    };
    let (user0, user0_peer) = zx::Channel::create();
    let args = fcomponent::CreateChildArgs {
        numbered_handles: Some(vec![HandleInfo {
            handle: user0_peer.into(),
            id: fuchsia_runtime::HandleInfo::new(HandleType::User0, 0).as_raw(),
        }]),
        ..Default::default()
    };
    capabilities.realm.create_child(&collection, &decl, args).await.unwrap().unwrap();

    let colocated_component_vmos =
        fcolocated::ColocatedProxy::new(fasync::Channel::from_channel(user0))
            .get_vmos()
            .await
            .unwrap();

    assert!(!colocated_component_vmos.is_empty());

    // Starting from the ELF runner, ask about the resource usage.
    let mut mem = attribution_testing::attribute_memory(
        PrincipalIdentifier(0),
        "elf_runner.cm".to_owned(),
        capabilities.attribution_provider,
        capabilities.introspector,
    );

    // Stream memory attribution data until the full tree shows up.
    let mut elf_runner: attribution_testing::Principal;
    loop {
        elf_runner = mem.next().await.unwrap();
        if elf_runner.children.len() == 1 && elf_runner.children[0].children.len() == 1 {
            break;
        }
    }

    // We should get the following tree:
    //
    // - elf_runner.cm
    //     - colocated_runner.cm
    //         - colocated_component.cm
    //             - Some VMO
    //         - overhead
    //     - overhead
    eprintln!("{:?}", elf_runner);
    assert_eq!(elf_runner.children.len(), 1);
    assert!(elf_runner.children[0].name.contains("colocated-runner"));
    assert_eq!(elf_runner.children[0].children.len(), 1usize);
    // Name of the colocated component
    assert_eq!(&elf_runner.children[0].children[0].name, "collection:colocated-component");
    let resource = &elf_runner.children[0].children[0].resources;
    for vmo_koid in colocated_component_vmos {
        assert!(
            resource.contains(&attribution_testing::Resource::KernelObject(zx::Koid::from_raw(
                vmo_koid
            )))
        );
    }
}
