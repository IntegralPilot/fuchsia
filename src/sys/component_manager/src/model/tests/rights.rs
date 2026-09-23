// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::testing::routing_test_helpers::*;
use ::routing::rights::validate_rights;
use ::routing_test_helpers::RoutingTestModel;
use ::routing_test_helpers::rights::CommonRightsTest;
use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_rust::offer::*;
use cm_rust_testing::*;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_io as fio;
use router_error::RouterError;
use runtime_capabilities::{DirConnector, Routable, Router, WeakInstanceToken};
use std::sync::Arc;

#[fuchsia::test]
async fn offer_increasing_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_offer_increasing_rights().await
}

#[fuchsia::test]
async fn offer_incompatible_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_offer_incompatible_rights().await
}

#[fuchsia::test]
async fn expose_increasing_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_expose_increasing_rights().await
}

#[fuchsia::test]
async fn expose_incompatible_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_expose_incompatible_rights().await
}

#[fuchsia::test]
async fn capability_increasing_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_capability_increasing_rights().await
}

#[fuchsia::test]
async fn capability_incompatible_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new().test_capability_incompatible_rights().await
}

#[fuchsia::test]
async fn offer_from_component_manager_namespace_directory_incompatible_rights() {
    CommonRightsTest::<RoutingTestBuilder>::new()
        .test_offer_from_component_manager_namespace_directory_incompatible_rights()
        .await
}

#[fuchsia::test]
async fn framework_directory_rights() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::directory()
                        .name("foo_data")
                        .source(OfferSource::Framework)
                        .target_static_child("b")
                        .subdir("foo"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::directory().name("foo_data").path("/data/hippo"))
                .build(),
        ),
    ];
    let test = RoutingTest::new("a", components).await;
    let foo_dir_proxy = fuchsia_fs::directory::open_directory_async(
        &test.test_dir_proxy,
        "foo",
        fio::PERM_READABLE,
    )
    .unwrap();
    test.model
        .context()
        .add_framework_capability(
            "foo_data",
            Router::<DirConnector>::new_ok(DirConnector::from_proxy(
                foo_dir_proxy,
                cm_types::RelativePath::dot(),
                fio::PERM_READABLE,
            )),
        )
        .await;
    test.check_use("b".try_into().unwrap(), CheckUse::default_directory(ExpectedResult::Ok)).await;
}

#[fuchsia::test]
async fn framework_directory_incompatible_rights() {
    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::directory()
                        .name("foo_data")
                        .source(OfferSource::Framework)
                        .target_static_child("b")
                        .subdir("foo"),
                )
                .child_default("b")
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(
                    UseBuilder::directory()
                        .name("foo_data")
                        .path("/data/hippo")
                        .rights(fio::X_STAR_DIR),
                )
                .build(),
        ),
    ];
    let test = RoutingTest::new("a", components).await;
    struct RightsCheckingRouter {}
    #[async_trait]
    impl Routable<DirConnector> for RightsCheckingRouter {
        async fn route(
            &self,
            mut request: RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<Option<Arc<DirConnector>>, RouterError> {
            validate_rights("a".parse().unwrap(), fio::R_STAR_DIR, &mut request)?;
            panic!("routing should have failed before we get here")
        }

        async fn route_debug(
            &self,
            _request: RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<CapabilitySource, RouterError> {
            panic!("test shouldn't do debug routing")
        }
    }
    test.model
        .context()
        .add_framework_capability("foo_data", Router::new(RightsCheckingRouter {}))
        .await;
    test.check_use(
        "b".try_into().unwrap(),
        CheckUse::default_directory(ExpectedResult::Err(zx::Status::ACCESS_DENIED)),
    )
    .await;
}
