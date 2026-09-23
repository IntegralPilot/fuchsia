// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod tests {
    use crate::routing::RoutingTestBuilderForAnalyzer;
    use cm_rust::{Availability, ExposeSource};
    use cm_rust_testing::*;
    use moniker::Moniker;
    use routing_test_helpers::availability::CommonAvailabilityTest;
    use routing_test_helpers::{
        CheckUse, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder, ServiceInstance,
    };
    use std::path::PathBuf;

    #[fuchsia::test]
    async fn offer_availability_successful_routes() {
        CommonAvailabilityTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_availability_successful_routes()
            .await
    }

    #[fuchsia::test]
    async fn offer_availability_invalid_routes() {
        CommonAvailabilityTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_availability_invalid_routes()
            .await
    }

    #[fuchsia::test]
    async fn expose_availability_successful_routes() {
        CommonAvailabilityTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_availability_successful_routes()
            .await
    }

    #[fuchsia::test]
    async fn expose_availability_invalid_routes() {
        CommonAvailabilityTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_availability_invalid_routes()
            .await
    }

    // The following tests only run in `RoutingTestBuilderForAnalyzer` and not `RoutingTestBuilder`.
    // That's because the latter tries to check both the expose is valid and that the source of the
    // expose responds to FIDL calls. When routing from `void`, there is no FIDL server to answer
    // such calls.

    /// Creates the following topology:
    ///
    ///           a
    ///          /
    ///         /
    ///        b
    ///
    /// `a` exposes a variety of capabilities from `b`. `b` exposes those from `void`.
    #[fuchsia::test]
    pub async fn test_expose_availability_from_void() {
        struct TestCase {
            expose_availability: Availability,
            expected_res: ExpectedResult,
        }
        for test_case in &[
            TestCase {
                expose_availability: Availability::Required,
                expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
            },
            TestCase {
                expose_availability: Availability::Optional,
                expected_res: ExpectedResult::Ok,
            },
        ] {
            let components = vec![
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source_static_child("b")
                                .availability(test_case.expose_availability),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source_static_child("b")
                                .availability(test_case.expose_availability),
                        )
                        .expose(
                            ExposeBuilder::directory()
                                .name("dir")
                                .source_static_child("b")
                                .availability(test_case.expose_availability),
                        )
                        .child_default("b")
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(ExposeSource::Void)
                                .availability(Availability::Optional),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(ExposeSource::Void)
                                .availability(Availability::Optional),
                        )
                        .expose(
                            ExposeBuilder::directory()
                                .name("dir")
                                .source(ExposeSource::Void)
                                .availability(Availability::Optional),
                        )
                        .build(),
                ),
            ];
            let builder = RoutingTestBuilderForAnalyzer::new("a", components);
            let model = builder.build().await;

            for check_use in vec![
                CheckUse::Service {
                    path: "/fuchsia.examples.EchoService".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: test_case.expected_res.clone(),
                },
                CheckUse::Protocol {
                    path: "/fuchsia.examples.Echo".parse().unwrap(),
                    expected_res: test_case.expected_res.clone(),
                },
                CheckUse::Directory {
                    path: "/dir".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: test_case.expected_res.clone(),
                },
            ] {
                model.check_use_exposed_dir(Moniker::root(), check_use).await;
            }
        }
    }
}
