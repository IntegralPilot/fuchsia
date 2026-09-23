// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CheckUse, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder};
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use fidl_fuchsia_io as fio;
use std::marker::PhantomData;
use zx_status;

pub struct CommonRightsTest<T: RoutingTestModelBuilder> {
    builder: PhantomData<T>,
}

impl<T: RoutingTestModelBuilder> CommonRightsTest<T> {
    pub fn new() -> Self {
        Self { builder: PhantomData }
    }

    pub async fn test_offer_increasing_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::RW_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(["c"].try_into().unwrap(), CheckUse::default_directory(ExpectedResult::Ok))
            .await;
    }

    pub async fn test_offer_incompatible_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::W_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    // Default rights on `UseBuilder::directory` are `fio::R_STAR_DIR`
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::default_directory(ExpectedResult::Err(zx_status::Status::ACCESS_DENIED)),
            )
            .await;
    }

    pub async fn test_expose_increasing_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::R_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(["c"].try_into().unwrap(), CheckUse::default_directory(ExpectedResult::Ok))
            .await;
    }

    pub async fn test_expose_incompatible_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::RW_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::W_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::default_directory(ExpectedResult::Err(zx_status::Status::ACCESS_DENIED)),
            )
            .await;
    }

    pub async fn test_capability_increasing_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::R_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::R_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(["c"].try_into().unwrap(), CheckUse::default_directory(ExpectedResult::Ok))
            .await;
    }

    pub async fn test_capability_incompatible_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("baz_data")
                            .source_static_child("b")
                            .target_static_child("c")
                            .rights(fio::RW_STAR_DIR),
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
                            .name("foo_data")
                            .path("/data/foo")
                            .rights(fio::W_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("foo_data")
                            .source(ExposeSource::Self_)
                            .target_name("bar_data")
                            .rights(fio::RW_STAR_DIR),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("baz_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let model = T::new("a", components).build().await;
        model
            .check_use(
                ["c"].try_into().unwrap(),
                CheckUse::default_directory(ExpectedResult::Err(zx_status::Status::ACCESS_DENIED)),
            )
            .await;
    }

    ///  component manager's namespace
    ///   |
    ///   a
    ///    \
    ///     b
    ///
    /// a: offers directory /offer_from_cm_namespace/data/foo from realm as bar_data
    /// b: uses directory bar_data as /data/hippo, but the rights don't match
    pub async fn test_offer_from_component_manager_namespace_directory_incompatible_rights(&self) {
        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("foo_data")
                            .target_name("bar_data")
                            .source(OfferSource::Parent)
                            .target_static_child("b"),
                    )
                    .child_default("b")
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().name("bar_data").path("/data/hippo"))
                    .build(),
            ),
        ];
        let namespace_capabilities = vec![
            CapabilityBuilder::directory()
                .name("foo_data")
                .path("/offer_from_cm_namespace/data/foo")
                .rights(fio::W_STAR_DIR)
                .build(),
        ];
        let mut builder = T::new("a", components);
        builder.set_namespace_capabilities(namespace_capabilities);
        let model = builder.build().await;

        model.install_namespace_directory("/offer_from_cm_namespace");
        model
            .check_use(
                ["b"].try_into().unwrap(),
                CheckUse::default_directory(ExpectedResult::Err(zx_status::Status::ACCESS_DENIED)),
            )
            .await;
    }
}
