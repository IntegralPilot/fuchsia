// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    CheckUse, ComponentEventRoute, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder,
    ServiceInstance,
};
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use fidl_fuchsia_io as fio;
use moniker::Moniker;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use zx_status;

pub struct CommonAvailabilityTest<T: RoutingTestModelBuilder> {
    builder: PhantomData<T>,
}

#[derive(Debug)]
struct TestCase {
    /// The availability of either an `Offer` or `Expose` declaration.
    provider_availability: Availability,
    use_availability: Availability,
}

impl<T: RoutingTestModelBuilder> CommonAvailabilityTest<T> {
    pub fn new() -> Self {
        Self { builder: PhantomData }
    }

    const VALID_AVAILABILITY_PAIRS: &'static [TestCase] = &[
        TestCase {
            provider_availability: Availability::Required,
            use_availability: Availability::Required,
        },
        TestCase {
            provider_availability: Availability::Optional,
            use_availability: Availability::Optional,
        },
        TestCase {
            provider_availability: Availability::Required,
            use_availability: Availability::Optional,
        },
        TestCase {
            provider_availability: Availability::SameAsTarget,
            use_availability: Availability::Required,
        },
        TestCase {
            provider_availability: Availability::SameAsTarget,
            use_availability: Availability::Optional,
        },
        TestCase {
            provider_availability: Availability::Required,
            use_availability: Availability::Transitional,
        },
        TestCase {
            provider_availability: Availability::Optional,
            use_availability: Availability::Transitional,
        },
        TestCase {
            provider_availability: Availability::Transitional,
            use_availability: Availability::Transitional,
        },
        TestCase {
            provider_availability: Availability::SameAsTarget,
            use_availability: Availability::Transitional,
        },
    ];

    pub async fn test_offer_availability_successful_routes(&self) {
        for test_case in Self::VALID_AVAILABILITY_PAIRS {
            let components = vec![
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .offer(
                            OfferBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source_static_child("b")
                                .target_static_child("c")
                                .availability(test_case.provider_availability),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source_static_child("b")
                                .target_static_child("c")
                                .availability(test_case.provider_availability),
                        )
                        .offer(
                            OfferBuilder::directory()
                                .name("dir")
                                .source_static_child("b")
                                .target_static_child("c")
                                .rights(fio::R_STAR_DIR)
                                .availability(test_case.provider_availability),
                        )
                        .capability(
                            CapabilityBuilder::directory()
                                .name("data")
                                .path("/data")
                                .rights(fio::RW_STAR_DIR),
                        )
                        .capability(
                            CapabilityBuilder::storage()
                                .name("cache")
                                .backing_dir("data")
                                .source(StorageDirectorySource::Self_)
                                .subdir("cache"),
                        )
                        .offer(
                            OfferBuilder::storage()
                                .name("cache")
                                .source(OfferSource::Self_)
                                .target_static_child("c")
                                .availability(test_case.provider_availability),
                        )
                        .offer(
                            OfferBuilder::event_stream()
                                .name("started")
                                .source(OfferSource::Parent)
                                .target_static_child("c")
                                .availability(test_case.provider_availability),
                        )
                        .child_default("b")
                        .child_default("c")
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .capability(
                            CapabilityBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/foo.service"),
                        )
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(ExposeSource::Self_),
                        )
                        .capability(
                            CapabilityBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .path("/svc/foo"),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(ExposeSource::Self_),
                        )
                        .capability(CapabilityBuilder::directory().name("dir").path("/data/dir"))
                        .expose(ExposeBuilder::directory().name("dir").source(ExposeSource::Self_))
                        .build(),
                ),
                (
                    "c",
                    ComponentDeclBuilder::new()
                        .use_(
                            UseBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::directory()
                                .name("dir")
                                .path("/dir")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::storage()
                                .name("cache")
                                .path("/storage")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::event_stream()
                                .name("started")
                                .path("/event/stream")
                                .availability(test_case.use_availability),
                        )
                        .build(),
                ),
            ];
            let mut builder = T::new("a", components);
            builder.set_builtin_capabilities(vec![CapabilityDecl::EventStream(EventStreamDecl {
                name: "started".parse().unwrap(),
            })]);
            let model = builder.build().await;
            model
                .create_static_file(Path::new("dir/hippo"), "hello")
                .await
                .expect("failed to create file");
            for check_use in vec![
                CheckUse::Service {
                    path: "/svc/fuchsia.examples.EchoService".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Protocol {
                    path: "/svc/fuchsia.examples.Echo".parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Directory {
                    path: "/dir".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Storage {
                    path: "/storage".parse().unwrap(),
                    storage_relation: Some(Moniker::try_from(["c"]).unwrap()),
                    from_cm_namespace: false,
                    storage_subdir: Some("cache".to_string()),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::EventStream {
                    expected_res: ExpectedResult::Ok,
                    path: "/event/stream".parse().unwrap(),
                    scope: Some(ComponentEventRoute { component: ".".to_string(), scope: None }),
                    name: "started".parse().unwrap(),
                },
            ] {
                model.check_use(["c"].try_into().unwrap(), check_use).await;
            }
        }
    }

    pub async fn test_offer_availability_invalid_routes(&self) {
        struct TestCase {
            source: OfferSource,
            storage_source: Option<OfferSource>,
            offer_availability: Availability,
            use_availability: Availability,
        }
        for test_case in &[
            TestCase {
                source: offer_source_static_child("b"),
                storage_source: Some(OfferSource::Self_),
                offer_availability: Availability::Optional,
                use_availability: Availability::Required,
            },
            TestCase {
                source: OfferSource::Void,
                storage_source: None,
                offer_availability: Availability::Optional,
                use_availability: Availability::Required,
            },
            TestCase {
                source: OfferSource::Void,
                storage_source: None,
                offer_availability: Availability::Optional,
                use_availability: Availability::Optional,
            },
            TestCase {
                source: OfferSource::Void,
                storage_source: None,
                offer_availability: Availability::Transitional,
                use_availability: Availability::Optional,
            },
            TestCase {
                source: OfferSource::Void,
                storage_source: None,
                offer_availability: Availability::Transitional,
                use_availability: Availability::Required,
            },
        ] {
            let components = vec![
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .offer(
                            OfferBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(test_case.source.clone())
                                .target_static_child("c")
                                .availability(test_case.offer_availability),
                        )
                        .offer(
                            OfferBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(test_case.source.clone())
                                .target_static_child("c")
                                .availability(test_case.offer_availability),
                        )
                        .offer(
                            OfferBuilder::directory()
                                .name("dir")
                                .source(test_case.source.clone())
                                .target_static_child("c")
                                .rights(fio::Operations::CONNECT)
                                .availability(test_case.offer_availability),
                        )
                        .offer(
                            OfferBuilder::storage()
                                .name("data")
                                .source(
                                    test_case
                                        .storage_source
                                        .as_ref()
                                        .map(Clone::clone)
                                        .unwrap_or_else(|| test_case.source.clone()),
                                )
                                .target_static_child("c")
                                .availability(test_case.offer_availability),
                        )
                        .capability(
                            CapabilityBuilder::storage()
                                .name("data")
                                .backing_dir("dir")
                                .source(StorageDirectorySource::Child("b".into())),
                        )
                        .child_default("b")
                        .child_default("c")
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .capability(
                            CapabilityBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/foo.service"),
                        )
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(ExposeSource::Self_),
                        )
                        .capability(
                            CapabilityBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .path("/svc/foo"),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(ExposeSource::Self_),
                        )
                        .capability(
                            CapabilityBuilder::directory()
                                .name("dir")
                                .path("/dir")
                                .rights(fio::Operations::CONNECT),
                        )
                        .expose(ExposeBuilder::directory().name("dir").source(ExposeSource::Self_))
                        .build(),
                ),
                (
                    "c",
                    ComponentDeclBuilder::new()
                        .use_(
                            UseBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::directory()
                                .name("dir")
                                .path("/dir")
                                .rights(fio::Operations::CONNECT)
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::storage()
                                .name("data")
                                .path("/data")
                                .availability(test_case.use_availability),
                        )
                        .build(),
                ),
            ];
            let model = T::new("a", components).build().await;
            for check_use in vec![
                CheckUse::Service {
                    path: "/svc/fuchsia.examples.EchoService".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
                CheckUse::Protocol {
                    path: "/svc/fuchsia.examples.Echo".parse().unwrap(),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
                CheckUse::Directory {
                    path: "/dir".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
                CheckUse::Storage {
                    path: "/data".parse().unwrap(),
                    storage_relation: None,
                    from_cm_namespace: false,
                    storage_subdir: None,
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            ] {
                model.check_use(["c"].try_into().unwrap(), check_use).await;
            }
        }
    }

    /// Creates the following topology:
    ///
    ///           a
    ///          /
    ///         /
    ///        b
    ///
    /// And verifies exposing a variety of capabilities from `b`, testing the combination of
    /// availability settings and capability types.
    ///
    /// Storage and event stream capabilities cannot be exposed, hence omitted.
    pub async fn test_expose_availability_successful_routes(&self) {
        for test_case in Self::VALID_AVAILABILITY_PAIRS {
            let components = vec![
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .use_(
                            UseBuilder::service()
                                .source_static_child("b")
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/fuchsia.examples.EchoService_a")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::protocol()
                                .source_static_child("b")
                                .name("fuchsia.examples.Echo")
                                .path("/svc/fuchsia.examples.Echo_a")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::directory()
                                .source_static_child("b")
                                .name("dir")
                                .path("/dir_a")
                                .availability(test_case.use_availability),
                        )
                        .child_default("b")
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .capability(
                            CapabilityBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/foo.service"),
                        )
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(ExposeSource::Self_)
                                .availability(test_case.provider_availability),
                        )
                        .capability(
                            CapabilityBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .path("/svc/foo"),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(ExposeSource::Self_)
                                .availability(test_case.provider_availability),
                        )
                        .capability(CapabilityBuilder::directory().name("dir").path("/data/dir"))
                        .expose(
                            ExposeBuilder::directory()
                                .name("dir")
                                .source(ExposeSource::Self_)
                                .availability(test_case.provider_availability),
                        )
                        .build(),
                ),
            ];
            let builder = T::new("a", components);
            let model = builder.build().await;

            // Add a file to the directory capability in the component that declared it, so "b".
            model
                .create_static_file(Path::new("dir/hippo"), "hello")
                .await
                .expect("failed to create file");

            for check_use in vec![
                CheckUse::Service {
                    path: "/svc/fuchsia.examples.EchoService_a".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Protocol {
                    path: "/svc/fuchsia.examples.Echo_a".parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Directory {
                    path: "/dir_a".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: ExpectedResult::Ok,
                },
            ] {
                model.check_use(Moniker::root(), check_use).await;
            }

            for check_use in vec![
                CheckUse::Service {
                    path: "/fuchsia.examples.EchoService".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Protocol {
                    path: "/fuchsia.examples.Echo".parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
                CheckUse::Directory {
                    path: "/dir".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: ExpectedResult::Ok,
                },
            ] {
                model.check_use_exposed_dir(["b"].try_into().unwrap(), check_use).await;
            }
        }
    }

    /// Creates the following topology:
    ///
    ///           a
    ///          /
    ///         /
    ///        b
    ///
    /// And verifies exposing a variety of capabilities from `b`. Except that either the route is
    /// broken, or the rules around availability are broken.
    pub async fn test_expose_availability_invalid_routes(&self) {
        struct TestCase {
            source: ExposeSource,
            expose_availability: Availability,
            use_availability: Availability,
        }
        for test_case in &[
            TestCase {
                source: ExposeSource::Self_,
                expose_availability: Availability::Optional,
                use_availability: Availability::Required,
            },
            TestCase {
                source: ExposeSource::Void,
                expose_availability: Availability::Optional,
                use_availability: Availability::Required,
            },
            TestCase {
                source: ExposeSource::Void,
                expose_availability: Availability::Optional,
                use_availability: Availability::Optional,
            },
            TestCase {
                source: ExposeSource::Void,
                expose_availability: Availability::Transitional,
                use_availability: Availability::Optional,
            },
            TestCase {
                source: ExposeSource::Void,
                expose_availability: Availability::Transitional,
                use_availability: Availability::Required,
            },
        ] {
            let components = vec![
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .use_(
                            UseBuilder::service()
                                .source_static_child("b")
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/fuchsia.examples.EchoService_a")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::protocol()
                                .source_static_child("b")
                                .name("fuchsia.examples.Echo")
                                .path("/svc/fuchsia.examples.Echo_a")
                                .availability(test_case.use_availability),
                        )
                        .use_(
                            UseBuilder::directory()
                                .source_static_child("b")
                                .name("dir")
                                .path("/dir_a")
                                .availability(test_case.use_availability),
                        )
                        .child_default("b")
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .capability(
                            CapabilityBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .path("/svc/foo.service"),
                        )
                        .expose(
                            ExposeBuilder::service()
                                .name("fuchsia.examples.EchoService")
                                .source(test_case.source.clone())
                                .availability(test_case.expose_availability),
                        )
                        .capability(
                            CapabilityBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .path("/svc/foo"),
                        )
                        .expose(
                            ExposeBuilder::protocol()
                                .name("fuchsia.examples.Echo")
                                .source(test_case.source.clone())
                                .availability(test_case.expose_availability),
                        )
                        .capability(CapabilityBuilder::directory().name("dir").path("/data/dir"))
                        .expose(
                            ExposeBuilder::directory()
                                .name("dir")
                                .source(test_case.source.clone())
                                .availability(test_case.expose_availability),
                        )
                        .build(),
                ),
            ];
            let builder = T::new("a", components);
            let model = builder.build().await;

            // Add a file to the directory capability in the component that declared it, so "b".
            model
                .create_static_file(Path::new("dir/hippo"), "hello")
                .await
                .expect("failed to create file");
            for check_use in vec![
                CheckUse::Service {
                    path: "/svc/fuchsia.examples.EchoService_a".parse().unwrap(),
                    instance: ServiceInstance::Named("default".to_owned()),
                    member: "echo".to_owned(),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
                CheckUse::Protocol {
                    path: "/svc/fuchsia.examples.Echo_a".parse().unwrap(),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
                CheckUse::Directory {
                    path: "/dir_a".parse().unwrap(),
                    file: PathBuf::from("hippo"),
                    expected_res: ExpectedResult::Err(zx_status::Status::NOT_FOUND),
                },
            ] {
                model.check_use(Moniker::root(), check_use).await;
            }
        }
    }
}
