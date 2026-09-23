// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use cm_rust::push_box;
use fidl_fuchsia_io as fio;
use fuchsia_component_test::new::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt};

fn get_expected_config_contents() -> String {
    std::fs::read_to_string("/pkg/data/example_config")
        .expect("failed to read example config from test namespace")
}

async fn read_example_config_and_assert_contents(
    path: &str,
    handles: LocalComponentHandles,
    mut success_sender: mpsc::Sender<()>,
) -> Result<(), Error> {
    let config_dir =
        handles.clone_from_namespace("config").expect("failed to clone config from namespace");
    let example_config_file =
        fuchsia_fs::directory::open_file(&config_dir, path, fio::PERM_READABLE)
            .await
            .expect("failed to open example config file");
    let example_config_contents = fuchsia_fs::file::read_to_string(&example_config_file)
        .await
        .expect("failed to read example config file");
    assert_eq!(example_config_contents, get_expected_config_contents());
    success_sender.send(()).await.expect("failed to send success");
    Ok(())
}

#[fuchsia::test]
async fn offer_pkg_from_framework() {
    let (success_sender, mut success_receiver) = mpsc::channel(1);

    let builder = RealmBuilder::new().await.unwrap();
    let config_reader = builder
        .add_local_child(
            "config-reader",
            move |h| {
                read_example_config_and_assert_contents("example_config", h, success_sender.clone())
                    .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::directory("pkg")
                        .as_("config")
                        .subdir("data")
                        .path("/config")
                        .rights(fio::R_STAR_DIR),
                )
                .from(Ref::framework())
                .to(&config_reader),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    assert!(
        success_receiver.next().await.is_some(),
        "failed to receive success signal from local component"
    );
}

#[fuchsia::test]
async fn offer_pkg_from_framework_no_subdir() {
    let (success_sender, mut success_receiver) = mpsc::channel(1);

    let builder = RealmBuilder::new().await.unwrap();
    let config_reader = builder
        .add_local_child(
            "config-reader",
            move |h| {
                read_example_config_and_assert_contents(
                    "data/example_config",
                    h,
                    success_sender.clone(),
                )
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::directory("pkg")
                        .as_("config")
                        .path("/config")
                        .rights(fio::R_STAR_DIR),
                )
                .from(Ref::framework())
                .to(&config_reader),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    assert!(
        success_receiver.next().await.is_some(),
        "failed to receive success signal from local component"
    );
}

#[fuchsia::test]
async fn expose_pkg_from_framework() {
    let (success_sender, mut success_receiver) = mpsc::channel(1);

    let builder = RealmBuilder::new().await.unwrap();
    let config_reader = builder
        .add_local_child(
            "config-reader",
            move |h| {
                read_example_config_and_assert_contents("example_config", h, success_sender.clone())
                    .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    let config_provider = builder
        .add_child_from_decl(
            "config-provider",
            cm_rust::ComponentDecl {
                exposes: Box::from([cm_rust::ExposeDecl::Directory(
                    cm_rust::ExposeDirectoryDecl {
                        source: cm_rust::ExposeSource::Framework,
                        source_name: "pkg".parse().unwrap(),
                        source_dictionary: Default::default(),
                        target: cm_rust::ExposeTarget::Parent,
                        target_name: "config".parse().unwrap(),
                        rights: Some(fio::R_STAR_DIR),
                        subdir: "data".parse().unwrap(),
                        availability: cm_rust::Availability::Required,
                    },
                )]),
                ..cm_rust::ComponentDecl::default()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("config").path("/config").rights(fio::R_STAR_DIR))
                .from(&config_provider)
                .to(&config_reader),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    assert!(
        success_receiver.next().await.is_some(),
        "failed to receive success signal from local component"
    );
}

#[fuchsia::test]
async fn use_pkg_from_framework() {
    let (success_sender, mut success_receiver) = mpsc::channel(1);

    let builder = RealmBuilder::new().await.unwrap();
    let config_reader = builder
        .add_local_child(
            "config-reader",
            move |h| {
                read_example_config_and_assert_contents("example_config", h, success_sender.clone())
                    .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    let mut config_reader_decl = builder.get_component_decl(&config_reader).await.unwrap();
    push_box(
        &mut config_reader_decl.uses,
        cm_rust::UseDecl::Directory(cm_rust::UseDirectoryDecl {
            source: cm_rust::UseSource::Framework,
            source_name: "pkg".parse().unwrap(),
            source_dictionary: Default::default(),
            target_path: "/config".parse().unwrap(),
            rights: fio::R_STAR_DIR,
            subdir: "data".parse().unwrap(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        }),
    );
    builder.replace_component_decl(&config_reader, config_reader_decl).await.unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    assert!(
        success_receiver.next().await.is_some(),
        "failed to receive success signal from local component"
    );
}
