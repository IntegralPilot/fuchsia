// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fidl::endpoints::DiscoverableProtocolMarker;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use futures::StreamExt;
use test_case::test_case;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_data as fdata,
};

const EXIT_WITH_CODE: &str = "exit_with_code";

async fn build_program(exit_code: i64) -> RealmInstance {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_root.cm"),
    )
    .await
    .unwrap();

    let program = builder
        .add_child_from_decl(
            EXIT_WITH_CODE,
            cm_rust::ComponentDecl {
                program: Some(cm_rust::ProgramDecl {
                    runner: Some("elf".parse().unwrap()),
                    info: fdata::Dictionary {
                        entries: Some(vec![
                            fdata::DictionaryEntry {
                                key: "binary".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::Str(
                                    "bin/exit_with_code".to_string(),
                                ))),
                            },
                            fdata::DictionaryEntry {
                                key: "args".to_string(),
                                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
                                    format!("{}", exit_code),
                                ]))),
                            },
                        ]),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                exposes: Box::from([cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
                    source: cm_rust::ExposeSource::Framework,
                    source_name: fcomponent::BinderMarker::PROTOCOL_NAME.parse().unwrap(),
                    source_dictionary: Default::default(),
                    target: cm_rust::ExposeTarget::Parent,
                    target_name: "exit_with_code_binder".parse().unwrap(),
                    availability: Default::default(),
                })]),
                ..Default::default()
            },
            ChildOptions::new().environment("elf-env"),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("exit_with_code_binder"))
                .from(&program)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap()
}

fn exit_code_to_status(exit_code: i64) -> i32 {
    if exit_code == 0 {
        0
    } else {
        fcomponent::Error::InstanceDied.into_primitive().try_into().unwrap()
    }
}

#[test_case(0)]
#[test_case(1)]
#[test_case(255)]
#[fuchsia::test]
async fn exit_code_event_stream(exit_code: i64) {
    let realm = build_program(exit_code).await;
    let proxy: fcomponent::EventStreamProxy =
        realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    proxy.wait_for_ready().await.unwrap();
    let mut event_stream = EventStream::new(proxy);
    realm.start_component_tree().await.unwrap();

    _ = realm
        .root
        .connect_to_named_protocol_at_exposed_dir::<fcomponent::BinderMarker>(
            "exit_with_code_binder",
        )
        .unwrap();

    let stopped = EventMatcher::ok()
        .moniker(EXIT_WITH_CODE)
        .wait::<Stopped>(&mut event_stream)
        .await
        .expect("failed to observe Stopped event");

    let expected_status = exit_code_to_status(exit_code).into();
    assert_eq!(stopped.result().unwrap().status, expected_status);
    assert_eq!(stopped.result().unwrap().exit_code, Some(exit_code));
}

#[test_case(0)]
#[test_case(1)]
#[test_case(255)]
#[fuchsia::test]
async fn exit_code_execution_controller(exit_code: i64) {
    let realm = build_program(exit_code).await;
    realm.start_component_tree().await.unwrap();

    let realm_proxy: fcomponent::RealmProxy =
        realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    let (controller, controller_server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>();
    let () = realm_proxy
        .open_controller(
            &fdecl::ChildRef { name: EXIT_WITH_CODE.to_string(), collection: None },
            controller_server_end,
        )
        .await
        .unwrap()
        .unwrap();

    let (execution_controller, execution_controller_server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ExecutionControllerMarker>();
    let () = controller
        .start(fcomponent::StartChildArgs::default(), execution_controller_server_end)
        .await
        .unwrap()
        .unwrap();

    let mut event_stream = execution_controller.take_event_stream();
    let event = event_stream.next().await.unwrap().unwrap();
    match event {
        fcomponent::ExecutionControllerEvent::OnStop { stopped_payload } => {
            let expected_status = exit_code_to_status(exit_code);
            assert_eq!(stopped_payload.status, Some(expected_status));
            assert_eq!(stopped_payload.exit_code, Some(exit_code));
        }
        _ => unreachable!(),
    }
}
