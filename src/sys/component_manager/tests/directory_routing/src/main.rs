// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust::push_box;
use cm_types::{LongName, Name, Path, RelativePath};
use fidl_fuchsia_io as fio;
use fuchsia_component_test::new::{ChildOptions, RealmBuilder, RealmInstance};
use futures::channel::mpsc;
use futures::{FutureExt, StreamExt};

/// Sets up a new realm that looks like:
///
///          .
///         / \
/// provider   consumer
///
/// The provider component when launched will host a read-write outgoing directory that looks like:
/// { data: { example_file: <text file>, example_subdir: { example_subdir_file: <text file>}} }.
/// The consumer component when launched will connect to "/data" in its namespace.
///
/// This function will set up these components, then add the capability and expose declarations to
/// the provider's manifest, the offer to the root's manifest, and the use to the consumer's
/// manifest, launch the realm in a nested component manager, and then return the directory proxy
/// grabbed back to the caller (along with the `RealmInstance` that we should keep alive while
/// poking at that proxy).
///
/// This way the caller can specify whatever specific configurations for the full capability route
/// they prefer, and then exercise the connected directory from the client end to confirm that
/// things are working as expected.
async fn set_up_realm_builder_and_get_proxy(
    capability_decl: cm_rust::DirectoryDecl,
    expose_decl: cm_rust::ExposeDirectoryDecl,
    offer_decl: cm_rust::offer::OfferDirectoryDecl,
    use_decl: cm_rust::UseDirectoryDecl,
) -> (RealmInstance, fio::DirectoryProxy) {
    let (proxy_sender, mut proxy_receiver) = mpsc::unbounded();

    let builder = RealmBuilder::new().await.unwrap();
    let provider = builder
        .add_local_child(
            "provider",
            move |h| {
                async move {
                    let id: u64 = rand::random();
                    let outgoing_root = fuchsia_fs::directory::open_in_namespace(
                        &format!("/tmp/directory-routing-{:x}", id),
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
                    )
                    .unwrap();
                    let data_dir = fuchsia_fs::directory::open_directory(
                        &outgoing_root,
                        "data",
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
                    )
                    .await
                    .unwrap();
                    let example_file = fuchsia_fs::directory::open_file(
                        &data_dir,
                        "example_file",
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
                    )
                    .await
                    .unwrap();
                    fuchsia_fs::file::write(&example_file, "Hello, world!").await.unwrap();
                    let subdir = fuchsia_fs::directory::open_directory(
                        &data_dir,
                        "example_subdir",
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
                    )
                    .await
                    .unwrap();
                    let example_subdir_file = fuchsia_fs::directory::open_file(
                        &subdir,
                        "example_subdir_file",
                        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
                    )
                    .await
                    .unwrap();
                    fuchsia_fs::file::write(&example_subdir_file, "Hello, hippos!").await.unwrap();
                    outgoing_root
                        .clone(fidl::endpoints::ServerEnd::new(h.outgoing_dir.into()))
                        .unwrap();
                    futures::future::pending().await
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    let consumer = builder
        .add_local_child(
            "consumer",
            move |h| {
                let dir_proxy = h.clone_from_namespace("data").unwrap();
                proxy_sender.unbounded_send(dir_proxy).unwrap();
                futures::future::ok(()).boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    let mut provider_manifest = builder.get_component_decl(&provider).await.unwrap();
    push_box(
        &mut provider_manifest.capabilities,
        cm_rust::CapabilityDecl::Directory(capability_decl),
    );
    push_box(&mut provider_manifest.exposes, cm_rust::ExposeDecl::Directory(expose_decl));
    builder.replace_component_decl(&provider, provider_manifest).await.unwrap();

    let mut realm_decl = builder.get_realm_decl().await.unwrap();
    push_box(&mut realm_decl.offers, cm_rust::offer::OfferDecl::Directory(Box::new(offer_decl)));
    builder.replace_realm_decl(realm_decl).await.unwrap();

    let mut consumer_manifest = builder.get_component_decl(&consumer).await.unwrap();
    push_box(&mut consumer_manifest.uses, cm_rust::UseDecl::Directory(use_decl));
    builder.replace_component_decl(&consumer, consumer_manifest).await.unwrap();

    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let dir_proxy = proxy_receiver.next().await.unwrap();
    (instance, dir_proxy)
}

#[fuchsia::test]
async fn read_only_test() {
    let capability_name = Name::new("data").unwrap();
    let (_instance, dir_proxy) = set_up_realm_builder_and_get_proxy(
        cm_rust::DirectoryDecl {
            name: capability_name.clone(),
            source_path: Some(Path::new("/data").unwrap()),
            rights: fio::R_STAR_DIR,
        },
        cm_rust::ExposeDirectoryDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: capability_name.clone(),
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::offer::OfferDirectoryDecl {
            source: cm_rust::offer::OfferSource::Child(cm_rust::ChildRef {
                name: LongName::new("provider").unwrap(),
                collection: None,
            }),
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("consumer").unwrap(),
                collection: None,
            }),
            target_name: capability_name.clone(),
            dependency_type: cm_rust::DependencyType::Strong,
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::UseDirectoryDecl {
            source: cm_rust::UseSource::Parent,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target_path: Path::new("/data").unwrap(),
            rights: fio::R_STAR_DIR,
            subdir: RelativePath::dot(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        },
    )
    .await;

    let file_proxy =
        fuchsia_fs::directory::open_file(&dir_proxy, "example_file", fio::PERM_READABLE)
            .await
            .unwrap();
    let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
    assert_eq!(file_contents, "Hello, world!");

    fuchsia_fs::directory::open_file(&dir_proxy, "example_file_2", fio::PERM_WRITABLE)
        .await
        .expect_err("we shouldn't be allowed to open a writable file in a read-only directory");
}

#[fuchsia::test]
async fn read_write_test() {
    let capability_name = Name::new("data").unwrap();
    let (_instance, dir_proxy) = set_up_realm_builder_and_get_proxy(
        cm_rust::DirectoryDecl {
            name: capability_name.clone(),
            source_path: Some(Path::new("/data").unwrap()),
            rights: fio::RW_STAR_DIR,
        },
        cm_rust::ExposeDirectoryDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: capability_name.clone(),
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::offer::OfferDirectoryDecl {
            source: cm_rust::offer::OfferSource::Child(cm_rust::ChildRef {
                name: LongName::new("provider").unwrap(),
                collection: None,
            }),
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("consumer").unwrap(),
                collection: None,
            }),
            target_name: capability_name.clone(),
            dependency_type: cm_rust::DependencyType::Strong,
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::UseDirectoryDecl {
            source: cm_rust::UseSource::Parent,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target_path: Path::new("/data").unwrap(),
            rights: fio::RW_STAR_DIR,
            subdir: RelativePath::dot(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        },
    )
    .await;

    {
        let file_proxy =
            fuchsia_fs::directory::open_file(&dir_proxy, "example_file", fio::PERM_READABLE)
                .await
                .unwrap();
        let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
        assert_eq!(file_contents, "Hello, world!");
    }

    {
        let file_proxy = fuchsia_fs::directory::open_file(
            &dir_proxy,
            "example_file_2",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FLAG_MUST_CREATE,
        )
        .await
        .unwrap();
        fuchsia_fs::file::write(&file_proxy, "Hello, hippos!").await.unwrap();
    }

    {
        let file_proxy =
            fuchsia_fs::directory::open_file(&dir_proxy, "example_file_2", fio::PERM_READABLE)
                .await
                .unwrap();
        let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
        assert_eq!(file_contents, "Hello, hippos!");
    }
}

#[fuchsia::test]
async fn read_write_to_read_only_test() {
    let capability_name = Name::new("data").unwrap();
    let (_instance, dir_proxy) = set_up_realm_builder_and_get_proxy(
        cm_rust::DirectoryDecl {
            name: capability_name.clone(),
            source_path: Some(Path::new("/data").unwrap()),
            rights: fio::RW_STAR_DIR,
        },
        cm_rust::ExposeDirectoryDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: capability_name.clone(),
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::offer::OfferDirectoryDecl {
            source: cm_rust::offer::OfferSource::Child(cm_rust::ChildRef {
                name: LongName::new("provider").unwrap(),
                collection: None,
            }),
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("consumer").unwrap(),
                collection: None,
            }),
            target_name: capability_name.clone(),
            dependency_type: cm_rust::DependencyType::Strong,
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::UseDirectoryDecl {
            source: cm_rust::UseSource::Parent,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target_path: Path::new("/data").unwrap(),
            rights: fio::R_STAR_DIR,
            subdir: RelativePath::dot(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        },
    )
    .await;

    let file_proxy =
        fuchsia_fs::directory::open_file(&dir_proxy, "example_file", fio::PERM_READABLE)
            .await
            .unwrap();
    let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
    assert_eq!(file_contents, "Hello, world!");

    fuchsia_fs::directory::open_file(&dir_proxy, "example_file_2", fio::PERM_WRITABLE)
        .await
        .expect_err("we shouldn't be allowed to open a writable file in a read-only directory");
}

#[fuchsia::test]
async fn read_only_subdir_test() {
    let capability_name = Name::new("data").unwrap();
    let (_instance, dir_proxy) = set_up_realm_builder_and_get_proxy(
        cm_rust::DirectoryDecl {
            name: capability_name.clone(),
            source_path: Some(Path::new("/data").unwrap()),
            rights: fio::R_STAR_DIR,
        },
        cm_rust::ExposeDirectoryDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: capability_name.clone(),
            rights: None,
            subdir: RelativePath::dot(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::offer::OfferDirectoryDecl {
            source: cm_rust::offer::OfferSource::Child(cm_rust::ChildRef {
                name: LongName::new("provider").unwrap(),
                collection: None,
            }),
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("consumer").unwrap(),
                collection: None,
            }),
            target_name: capability_name.clone(),
            dependency_type: cm_rust::DependencyType::Strong,
            rights: None,
            subdir: RelativePath::new("example_subdir").unwrap(),
            availability: cm_rust::Availability::Required,
        },
        cm_rust::UseDirectoryDecl {
            source: cm_rust::UseSource::Parent,
            source_name: capability_name.clone(),
            source_dictionary: RelativePath::dot(),
            target_path: Path::new("/data").unwrap(),
            rights: fio::R_STAR_DIR,
            subdir: RelativePath::dot(),
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        },
    )
    .await;

    let file_proxy =
        fuchsia_fs::directory::open_file(&dir_proxy, "example_subdir_file", fio::PERM_READABLE)
            .await
            .unwrap();
    let file_contents = fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap();
    assert_eq!(file_contents, "Hello, hippos!");

    fuchsia_fs::directory::open_file(&dir_proxy, "example_subdir_file_2", fio::PERM_WRITABLE)
        .await
        .expect_err("we shouldn't be allowed to open a writable file in a read-only directory");
}
