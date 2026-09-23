// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_id_index::make_index_file;
use crate::{
    CheckUse, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder, generate_storage_path,
};
use cm_config::{CapabilityAllowlistKey, CapabilityAllowlistSource};
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use component_id_index::InstanceId;
use fidl_fuchsia_io as fio;
use moniker::{ExtendedMoniker, Moniker};
use std::collections::HashSet;
use std::marker::PhantomData;
use zx_status;

pub struct CommonStorageTest<T: RoutingTestModelBuilder> {
    builder: PhantomData<T>,
}

impl<T: RoutingTestModelBuilder> CommonStorageTest<T> {
    pub fn new() -> Self {
        Self { builder: PhantomData }
    }

    ///   component manager's namespace
    ///    |
    ///    a
    ///    |
    ///    b
    ///
    /// a: has storage decl with name "mystorage" with a source of realm at path /data
    /// a: offers cache storage to b from "mystorage"
    /// b: uses cache storage as /storage.
    pub async fn test_storage_dir_from_cm_namespace(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("tmp")
                            .source(StorageDirectorySource::Parent)
                            .subdir("cache"),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let namespace_capabilities = vec![
            CapabilityBuilder::directory()
                .name("tmp")
                .path("/tmp")
                .rights(fio::RW_STAR_DIR)
                .build(),
        ];
        let mut builder = T::new("a", components);
        builder.set_namespace_capabilities(namespace_capabilities);
        let model = builder.build().await;

        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: true,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;

        model.check_namespace_subdir_contents("/tmp/cache", vec!["b:0".to_string()]).await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: offers cache storage to b from "void"
    /// b: uses cache storage as /storage
    pub async fn test_storage_from_void(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Void)
                            .target_static_child("b")
                            .availability(Availability::Optional),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /data
    /// a: offers cache storage to b from "mystorage"
    /// b: uses cache storage as /storage
    pub async fn test_storage_and_dir_from_parent(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model.check_test_subdir_contents(".", vec!["b:0".to_string(), "foo".to_string()]).await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /data, with subdir
    ///    "cache"
    /// a: offers cache storage to b from "mystorage"
    /// b: uses cache storage as /storage
    pub async fn test_storage_and_dir_from_parent_with_subdir(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_)
                            .subdir("cache"),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model.check_test_subdir_contents(".", vec!["cache".to_string(), "foo".to_string()]).await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /data, but /data
    ///    has only read rights
    /// a: offers cache storage to b from "mystorage"
    /// b: uses cache storage as /storage
    pub async fn test_storage_and_dir_from_parent_rights_invalid(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::directory().name("data").path("/data"))
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::ACCESS_DENIED),
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///      \
    ///       c
    ///
    /// a: offers directory /data to b as /minfs
    /// b: has storage decl with name "mystorage" with a source of realm at path /minfs
    /// b: offers data storage to c from "mystorage"
    /// c: uses data storage as /storage
    pub async fn test_storage_from_parent_dir_from_grandparent(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("data")
                            .target_name("minfs")
                            .source(OfferSource::Self_)
                            .target_static_child("b")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Parent),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///      \
    ///       c
    ///
    /// a: offers directory /data to b as /minfs with subdir "subdir_1"
    /// b: has storage decl with name "mystorage" with a source of realm at path /minfs with subdir
    ///    "subdir_2"
    /// b: offers data storage to c from "mystorage"
    /// c: uses data storage as /storage
    pub async fn test_storage_from_parent_dir_from_grandparent_with_subdirs(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("data")
                            .target_name("minfs")
                            .source(OfferSource::Self_)
                            .target_static_child("b")
                            .rights(fio::RW_STAR_DIR)
                            .subdir("subdir_1"),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Parent)
                            .subdir("subdir_2"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model.add_subdir_to_data_directory("subdir_1");
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("subdir_1/subdir_2".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;

        model
            .check_test_subdir_contents(".", vec!["foo".to_string(), "subdir_1".to_string()])
            .await;
        model.check_test_subdir_contents("subdir_1", vec!["subdir_2".to_string()]).await;
        model.check_test_subdir_contents("subdir_1/subdir_2", vec!["c:0".to_string()]).await;
    }

    ///   a
    ///    \
    ///     b
    ///      \
    ///       c
    ///
    /// a: offers directory /data to b as /minfs
    /// b: has storage decl with name "mystorage" with a source of realm at path /minfs, subdir "bar"
    /// b: offers data storage to c from "mystorage"
    /// c: uses data storage as /storage
    pub async fn test_storage_from_parent_dir_from_grandparent_with_subdir(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR)
                            .build(),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("data")
                            .target_name("minfs")
                            .source(OfferSource::Self_)
                            .target_static_child("b")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Parent)
                            .subdir("bar"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("bar".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model.check_test_subdir_contents(".", vec!["bar".to_string(), "foo".to_string()]).await;
    }

    ///   a
    ///    \
    ///     b
    ///      \
    ///       c
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /data
    /// a: offers data storage to b from "mystorage"
    /// b: offers data storage to c from realm
    /// c: uses data storage as /storage
    pub async fn test_storage_and_dir_from_grandparent(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data-root")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("data-root")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Parent)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b", "c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
    }

    ///   a
    ///  / \
    /// b   c
    ///
    /// b: exposes directory /data as /minfs
    /// a: has storage decl with name "mystorage" with a source of child b at path /minfs
    /// a: offers cache storage to c from "mystorage"
    /// c: uses cache storage as /storage
    pub async fn test_storage_from_parent_dir_from_sibling(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Child("b".into())),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("b")
                    .child_default("c")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .target_name("minfs")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
    }

    ///   a
    ///  / \
    /// b   c
    ///
    /// b: exposes directory /data as /minfs with subdir "subdir_1"
    /// a: has storage decl with name "mystorage" with a source of child b at path /minfs and subdir
    ///    "subdir_2"
    /// a: offers cache storage to c from "mystorage"
    /// c: uses cache storage as /storage
    pub async fn test_storage_from_parent_dir_from_sibling_with_subdir(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Child("b".into()))
                            .subdir("subdir_2"),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("b")
                    .child_default("c")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .target_name("minfs")
                            .rights(fio::RW_STAR_DIR)
                            .subdir("subdir_1"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model.add_subdir_to_data_directory("subdir_1");
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("subdir_1/subdir_2".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model
            .check_test_subdir_contents(".", vec!["foo".to_string(), "subdir_1".to_string()])
            .await;
        model.check_test_subdir_contents("subdir_1", vec!["subdir_2".to_string()]).await;
        model.check_test_subdir_contents("subdir_1/subdir_2", vec!["c:0".to_string()]).await;
    }

    ///   a
    ///  / \
    /// b   c
    ///      \
    ///       d
    ///
    /// b: exposes directory /data as /minfs
    /// a: has storage decl with name "mystorage" with a source of child b at path /minfs
    /// a: offers data, cache, and meta storage to c from "mystorage"
    /// c: uses cache and meta storage as /storage
    /// c: offers data and meta storage to d
    /// d: uses data and meta storage
    pub async fn test_storage_multiple_types(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Child("b".into()))
                            .subdir("data"),
                    )
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Child("b".into()))
                            .subdir("cache"),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("b")
                    .child_default("c")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("data")
                            .source(ExposeSource::Self_)
                            .target_name("minfs")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Parent)
                            .target_static_child("d"),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Parent)
                            .target_static_child("d"),
                    )
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .use_(UseBuilder::storage().name("cache").path("/cache"))
                    .child_default("d")
                    .build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .use_(UseBuilder::storage().name("cache").path("/cache"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("data".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/cache".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model
            .check_use(
                ["c", "d"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c", "d"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("data".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model
            .check_use(
                ["c", "d"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/cache".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c", "d"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /storage
    /// a: offers cache storage to b from "mystorage"
    /// b: uses data storage as /storage, fails to since data != cache
    /// b: uses meta storage, fails to since meta != cache
    pub async fn test_use_the_wrong_type_of_storage(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: offers directory from self at path "/data"
    /// b: uses data storage as /storage, fails to since data storage != "/data" directories
    pub async fn test_directories_are_not_storage(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("b")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///
    /// a: has storage decl with name "mystorage" with a source of self at path /data
    /// a: does not offer any storage to b
    /// b: uses meta storage and data storage as /storage, fails to since it was not offered either
    pub async fn test_use_storage_when_not_offered(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::directory()
                            .name("minfs")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    ///   a
    ///    \
    ///     b
    ///      \
    ///       c
    ///
    /// a: offers directory /data to b as /minfs, but a is non-executable
    /// b: has storage decl with name "mystorage" with a source of realm at path /minfs
    /// b: offers data and meta storage to b from "mystorage"
    /// c: uses meta and data storage as /storage, fails to since a is non-executable
    pub async fn test_dir_offered_from_nonexecutable(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new_empty_component()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("data")
                            .target_name("minfs")
                            .source(OfferSource::Self_)
                            .target_static_child("b")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("data")
                            .source(OfferSource::Self_)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("data")
                            .backing_dir("minfs")
                            .source(StorageDirectorySource::Parent),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("data").path("/storage"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            )
            .await;
    }

    ///   component manager's namespace
    ///    |
    ///    a
    ///    |
    ///    b
    ///
    /// a: has storage decl with name "mystorage" with a source of parent at path /data
    /// a: offers cache storage to b from "mystorage"
    /// b: uses cache storage as /storage.
    /// Policy prevents b from using storage.
    pub async fn test_storage_dir_from_cm_namespace_prevented_by_policy(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("tmp")
                            .source(StorageDirectorySource::Parent)
                            .subdir("cache"),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let namespace_capabilities = vec![
            CapabilityBuilder::directory()
                .name("tmp")
                .path("/tmp")
                .rights(fio::RW_STAR_DIR)
                .build(),
        ];
        let mut builder = T::new("a", components);
        builder.set_namespace_capabilities(namespace_capabilities);
        builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(Moniker::root()),
                source_name: "cache".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Storage,
            },
            HashSet::new(),
        );
        let model = builder.build().await;

        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: true,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Err(zx_status::Status::ACCESS_DENIED),
                },
            )
            .await;
    }

    ///   component manager's namespace
    ///    |
    ///    a
    ///    |
    ///    b
    ///    |
    ///    c
    ///
    /// Instance IDs defined only for `b` in the component ID index.
    /// Check that the correct storage layout is used when a component has an instance ID.
    pub async fn test_instance_id_from_index(&self) {
        let b_instance_id = InstanceId::new_random(&mut rand::rng());
        let component_id_index = {
            let mut index = component_id_index::Index::default();
            index
                .insert(component_id_index::IndexEntry {
                    moniker: Moniker::parse_str("/b").unwrap(),
                    instance_id: b_instance_id.clone(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let component_id_index_path = make_index_file(component_id_index).unwrap();
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("data")
                            .path("/data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Self_)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .capability(
                        CapabilityBuilder::storage()
                            .name("cache")
                            .backing_dir("data")
                            .source(StorageDirectorySource::Self_),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .offer(
                        OfferBuilder::storage()
                            .name("cache")
                            .source(OfferSource::Parent)
                            .target_static_child("c"),
                    )
                    .child_default("c")
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::storage().name("cache").path("/storage"))
                    .build(),
            ),
        ];
        let mut builder = T::new("a", components);
        builder.set_component_id_index_path(
            component_id_index_path.path().to_owned().try_into().unwrap(),
        );
        let model = builder.build().await;

        // instance `b` uses instance-id based paths.
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["b"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        model.check_test_subdir_contains(".", b_instance_id.to_string()).await;

        // instance `c` uses moniker-based paths.
        let storage_relation = Moniker::try_from(["b", "c"]).unwrap();
        model
            .check_use(
                ["b", "c"].try_into().unwrap(),
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(storage_relation.clone()),
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;

        let expected_storage_path =
            generate_storage_path(None, &storage_relation, None).to_str().unwrap().to_string();
        model.check_test_dir_tree_contains(expected_storage_path).await;
    }
}
