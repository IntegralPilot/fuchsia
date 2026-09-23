// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CheckUse, ExpectedResult, RoutingTestModel, RoutingTestModelBuilder};
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use fidl_fuchsia_io as fio;
use moniker::Moniker;
use routing::debug_route_sandbox_path;
use std::marker::PhantomData;
use std::path::PathBuf;
use zx_status::Status;

pub struct CommonDictionaryTest<T: RoutingTestModelBuilder> {
    builder: PhantomData<T>,
}

impl<T: RoutingTestModelBuilder> CommonDictionaryTest<T> {
    pub fn new() -> Self {
        Self { builder: PhantomData }
    }

    pub async fn test_use_protocol_from_dictionary(&self) {
        // Test extracting a protocol from a dictionary with source parent, self and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Self_)
                            .name("A")
                            .from_dictionary("self_dict"),
                    )
                    .use_(UseBuilder::protocol().name("B").from_dictionary("parent_dict"))
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("leaf")
                            .name("C")
                            .from_dictionary("child_dict"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B", "/svc/C"] {
            test.check_use(
                "mid".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_use_protocol_from_dictionary_not_a_dictionary(&self) {
        // Test extracting a protocol from a dictionary, but the dictionary is not actually a
        // dictionary so it should fail with a type error.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("parent_not_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("parent_not_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    // We don't test "self_not_dict" here because the manifest schema forbids such a
                    // manifest from existing.
                    .use_(UseBuilder::protocol().name("A").from_dictionary("parent_not_dict"))
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("leaf")
                            .name("B")
                            .from_dictionary("child_not_dict"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("child_not_dict")
                    .expose(
                        ExposeBuilder::protocol()
                            .name("child_not_dict")
                            .source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B"] {
            test.check_use(
                "mid".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Err(Status::NOT_FOUND),
                },
            )
            .await;
        }
    }

    pub async fn test_use_protocol_from_dictionary_not_used(&self) {
        // Create a dictionary with two protocols. `use` one of the protocols, but not the other.
        // Only the protocol that is `use`d should be accessible.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").from_dictionary("parent_dict"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_use_protocol_from_dictionary_not_found(&self) {
        // Test extracting a protocol from a dictionary, but the protocol is missing from the
        // dictionary.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").from_dictionary("dict"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;

        // Test extracting a protocol from a dictionary, but the dictionary is not routed to the
        // target.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .target_name("other_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").from_dictionary("dict"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_use_directory_from_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::directory().name("bar_data").path("/data/foo"))
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap()))
                            .rights(fio::R_STAR_DIR),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::directory().name("foo_data").path("/data/foo"))
                    .dictionary_default("self_dict")
                    .offer(
                        OfferBuilder::directory()
                            .name("foo_data")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap()))
                            .rights(fio::R_STAR_DIR),
                    )
                    .use_(
                        UseBuilder::directory()
                            .source(UseSource::Self_)
                            .name("A")
                            .from_dictionary("self_dict")
                            .path("/A"),
                    )
                    .use_(
                        UseBuilder::directory().name("B").from_dictionary("parent_dict").path("/B"),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Directory {
                path: "/A".parse().unwrap(),
                file: PathBuf::from("hippo"),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Directory {
                path: "/B".parse().unwrap(),
                file: PathBuf::from("hippo"),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_expose_directory_from_dictionary(&self) {
        // Routing a directory into a dictionary isn't supported yet, it should fail.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::directory().source_static_child("mid").name("A").path("/A"))
                    .use_(UseBuilder::directory().source_static_child("mid").name("B").path("/B"))
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::directory().name("foo_data").path("/data/foo"))
                    .dictionary_default("self_dict")
                    .offer(
                        OfferBuilder::directory()
                            .name("foo_data")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap()))
                            .rights(fio::R_STAR_DIR),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("A")
                            .source(ExposeSource::Self_)
                            .from_dictionary("self_dict"),
                    )
                    .expose(
                        ExposeBuilder::directory()
                            .name("B")
                            .source_static_child("leaf")
                            .from_dictionary("child_dict"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::directory().name("bar_data").path("/data/foo"))
                    .dictionary_default("child_dict")
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .offer(
                        OfferBuilder::directory()
                            .name("bar_data")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap()))
                            .rights(fio::R_STAR_DIR),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Directory {
                path: "/A".parse().unwrap(),
                file: PathBuf::from("hippo"),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Directory {
                path: "/B".parse().unwrap(),
                file: PathBuf::from("hippo"),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_use_protocol_from_nested_dictionary(&self) {
        // Test extracting a protocol from a dictionary nested in another dictionary with source
        // parent, self and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("nested")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("nested")
                    .dictionary_default("self_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Self_)
                            .name("A")
                            .from_dictionary("self_dict/nested"),
                    )
                    .use_(UseBuilder::protocol().name("B").from_dictionary("parent_dict/nested"))
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("leaf")
                            .name("C")
                            .from_dictionary("child_dict/nested"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("nested")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B", "/svc/C"] {
            test.check_use(
                "mid".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_offer_protocol_from_dictionary(&self) {
        // Test extracting a protocol from a dictionary with source parent, self, and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf")
                            .from_dictionary("self_dict"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("B")
                            .target_name("B_svc")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("parent_dict"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("C")
                            .target_name("C_svc")
                            .source_static_child("provider")
                            .target_static_child("leaf")
                            .from_dictionary("child_dict"),
                    )
                    .child_default("provider")
                    .child_default("leaf")
                    .build(),
            ),
            (
                "provider",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A_svc").path("/svc/A"))
                    .use_(UseBuilder::protocol().name("B_svc").path("/svc/B"))
                    .use_(UseBuilder::protocol().name("C_svc").path("/svc/C"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B", "/svc/C"] {
            test.check_use(
                "mid/leaf".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_offer_protocol_from_dictionary_not_found(&self) {
        // Test extracting a protocol from a dictionary, but the protocol is missing from the
        // dictionary.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("dict"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A_svc").path("/svc/A"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "mid/leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_offer_protocol_from_dictionary_to_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .dictionary_default("dict1")
                    .dictionary_default("dict2")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source_static_child("provider")
                            .target(OfferTarget::Capability("dict1".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .target_name("A")
                            .from_dictionary("dict1")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict2".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict2")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("provider")
                    .child_default("leaf")
                    .build(),
            ),
            (
                "provider",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .expose(ExposeBuilder::protocol().name("foo").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").from_dictionary("dict2").path("/svc/A"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_offer_protocol_from_nested_dictionary(&self) {
        // Test extracting a protocol from a dictionary nested in another dictionary with source
        // parent, self, and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("nested")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .dictionary_default("nested")
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf")
                            .from_dictionary("self_dict/nested"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("B")
                            .target_name("B_svc")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("parent_dict/nested"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("C")
                            .target_name("C_svc")
                            .source_static_child("provider")
                            .target_static_child("leaf")
                            .from_dictionary("child_dict/nested"),
                    )
                    .child_default("provider")
                    .child_default("leaf")
                    .build(),
            ),
            (
                "provider",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .dictionary_default("nested")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A_svc").path("/svc/A"))
                    .use_(UseBuilder::protocol().name("B_svc").path("/svc/B"))
                    .use_(UseBuilder::protocol().name("C_svc").path("/svc/C"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B", "/svc/C"] {
            test.check_use(
                "mid/leaf".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_expose_protocol_from_dictionary(&self) {
        // Test extracting a protocol from a dictionary with source self and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("mid")
                            .name("A_svc")
                            .path("/svc/A"),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("mid")
                            .name("B_svc")
                            .path("/svc/B"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .expose(
                        ExposeBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .from_dictionary("self_dict")
                            .source(ExposeSource::Self_),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("B")
                            .target_name("B_svc")
                            .from_dictionary("child_dict")
                            .source_static_child("leaf"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B"] {
            test.check_use(
                ".".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_expose_protocol_from_dictionary_not_found(&self) {
        // Test extracting a protocol from a dictionary, but the protocol is missing.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("mid")
                            .name("A_svc")
                            .path("/svc/A"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .expose(
                        ExposeBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .source(ExposeSource::Self_)
                            .from_dictionary("dict")
                            .build(),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .expose(ExposeBuilder::dictionary().name("dict").source(ExposeSource::Self_))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_expose_protocol_from_nested_dictionary(&self) {
        // Test extracting a protocol from a dictionary nested in a dictionary with source self and
        // child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("mid")
                            .name("A_svc")
                            .path("/svc/A"),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("mid")
                            .name("B_svc")
                            .path("/svc/B"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .dictionary_default("nested")
                    .expose(
                        ExposeBuilder::protocol()
                            .name("A")
                            .target_name("A_svc")
                            .from_dictionary("self_dict/nested")
                            .source(ExposeSource::Self_),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("B")
                            .target_name("B_svc")
                            .from_dictionary("child_dict/nested")
                            .source_static_child("leaf"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .dictionary_default("nested")
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B"] {
            test.check_use(
                ".".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_dictionary_in_exposed_dir(&self) {
        // Test extracting a protocol from a dictionary with source self and child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .expose(
                        ExposeBuilder::dictionary().name("self_dict").source(ExposeSource::Self_),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source_static_child("leaf"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("nested")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        // The dictionaries in the exposed dir will be converted to subdirectories.
        for path in ["/self_dict/A", "/child_dict/nested/B"] {
            test.check_use_exposed_dir(
                ".".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_offer_dictionary_to_dictionary(&self) {
        // Tests dictionary nesting when the nested dictionary comes from parent, self, or child.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("parent_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("parent_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("self_dict")
                    .dictionary_default("root_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("self_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("self_dict")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("root_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("parent_dict")
                            .source(OfferSource::Parent)
                            .target(OfferTarget::Capability("root_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("child_dict")
                            .source_static_child("leaf")
                            .target(OfferTarget::Capability("root_dict".parse().unwrap())),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Self_)
                            .name("A")
                            .from_dictionary("root_dict/self_dict"),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Self_)
                            .name("B")
                            .from_dictionary("root_dict/parent_dict"),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Self_)
                            .name("C")
                            .from_dictionary("root_dict/child_dict"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .dictionary_default("child_dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("child_dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::dictionary().name("child_dict").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        for path in ["/svc/A", "/svc/B", "/svc/C"] {
            test.check_use(
                "mid".try_into().unwrap(),
                CheckUse::Protocol {
                    path: path.parse().unwrap(),
                    expected_res: ExpectedResult::Ok,
                },
            )
            .await;
        }
    }

    pub async fn test_use_from_dictionary_availability_attenuated(&self) {
        // required -> optional downgrade allowed, of:
        // - a capability in a dictionary
        // - a capability in a dictionary in a dictionary
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .dictionary_default("nested")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .name("A")
                            .from_dictionary("dict")
                            .availability(Availability::Optional),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .name("B")
                            .from_dictionary("dict/nested")
                            .availability(Availability::Optional),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_use_from_dictionary_availability_invalid(&self) {
        // attempted optional -> required upgrade, disallowed, of:
        // - an optional capability in a dictionary.
        // - a capability in an optional dictionary.
        // - a capability in a dictionary in an optional dictionary.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .protocol_default("qux")
                    .dictionary_default("required_dict")
                    .dictionary_default("optional_dict")
                    .dictionary_default("nested")
                    .dictionary_default("dict_with_optional_nested")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("required_dict".parse().unwrap()))
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("optional_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("qux")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability(
                                "dict_with_optional_nested".parse().unwrap(),
                            ))
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("required_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("optional_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf")
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict_with_optional_nested")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").from_dictionary("required_dict"))
                    .use_(UseBuilder::protocol().name("B").from_dictionary("optional_dict"))
                    .use_(
                        UseBuilder::protocol()
                            .name("C")
                            .from_dictionary("dict_with_optional_nested/nested"),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/C".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_offer_from_dictionary_availability_attenuated(&self) {
        // required -> optional downgrade allowed, of:
        // - a capability in a dictionary
        // - a capability in a dictionary in a dictionary
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .dictionary_default("nested")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf")
                            .from_dictionary("dict"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("B")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf")
                            .from_dictionary("dict/nested"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A").availability(Availability::Optional))
                    .use_(UseBuilder::protocol().name("B").availability(Availability::Optional))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_offer_from_dictionary_availability_invalid(&self) {
        // attempted optional -> required upgrade, disallowed, of:
        // - an optional capability in a dictionary.
        // - a capability in an optional dictionary.
        // - a capability in a dictionary in an optional dictionary.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .protocol_default("qux")
                    .dictionary_default("required_dict")
                    .dictionary_default("optional_dict")
                    .dictionary_default("nested")
                    .dictionary_default("dict_with_optional_nested")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("required_dict".parse().unwrap()))
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("optional_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("qux")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability(
                                "dict_with_optional_nested".parse().unwrap(),
                            ))
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("required_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("optional_dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid")
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict_with_optional_nested")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::protocol()
                            .name("A")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("required_dict"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("B")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("optional_dict"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("C")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf")
                            .from_dictionary("dict_with_optional_nested/nested"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("A"))
                    .use_(UseBuilder::protocol().name("B"))
                    .use_(UseBuilder::protocol().name("C"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            "mid/leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            "mid/leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            "mid/leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/C".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_expose_from_dictionary_availability_attenuated(&self) {
        // required -> optional downgrade allowed, of:
        // - a capability in a dictionary
        // - a capability in a dictionary in a dictionary
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("leaf")
                            .name("A")
                            .availability(Availability::Optional),
                    )
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("leaf")
                            .name("B")
                            .availability(Availability::Optional),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .dictionary_default("nested")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("A")
                            .from_dictionary("dict")
                            .source(ExposeSource::Self_),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("B")
                            .from_dictionary("dict/nested")
                            .source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    pub async fn test_expose_from_dictionary_availability_invalid(&self) {
        // attempted optional -> required upgrade, disallowed, of:
        // - an optional capability in a dictionary.
        // - a capability in an optional dictionary.
        // - a capability in a dictionary in an optional dictionary.
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().source_static_child("mid").name("A"))
                    .use_(UseBuilder::protocol().source_static_child("mid").name("B"))
                    .use_(UseBuilder::protocol().source_static_child("mid").name("C"))
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .expose(
                        ExposeBuilder::protocol()
                            .name("A")
                            .from_dictionary("required_dict")
                            .source_static_child("leaf"),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("B")
                            .from_dictionary("optional_dict")
                            .source_static_child("leaf"),
                    )
                    .expose(
                        ExposeBuilder::protocol()
                            .name("C")
                            .from_dictionary("dict_with_optional_nested/nested")
                            .source_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .protocol_default("foo")
                    .protocol_default("bar")
                    .protocol_default("qux")
                    .dictionary_default("required_dict")
                    .dictionary_default("optional_dict")
                    .dictionary_default("nested")
                    .dictionary_default("dict")
                    .dictionary_default("dict_with_optional_nested")
                    .offer(
                        OfferBuilder::protocol()
                            .name("foo")
                            .target_name("A")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap()))
                            .availability(Availability::Optional),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("bar")
                            .target_name("B")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("optional_dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("qux")
                            .target_name("C")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("nested".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("nested")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability(
                                "dict_with_optional_nested".parse().unwrap(),
                            ))
                            .availability(Availability::Optional),
                    )
                    .expose(
                        ExposeBuilder::dictionary()
                            .name("required_dict")
                            .source(ExposeSource::Self_),
                    )
                    .expose(
                        ExposeBuilder::dictionary()
                            .name("optional_dict")
                            .source(ExposeSource::Self_)
                            .availability(Availability::Optional),
                    )
                    .expose(
                        ExposeBuilder::dictionary()
                            .name("dict_with_optional_nested")
                            .source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/B".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
        test.check_use(
            ".".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/C".parse().unwrap(),
                expected_res: ExpectedResult::Err(Status::NOT_FOUND),
            },
        )
        .await;
    }

    pub async fn test_use_runner_from_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .runner_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::runner()
                            .name("foo")
                            .target_name("bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new_empty_component()
                    .use_(UseBuilder::runner().name("bar").from_dictionary("dict"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        let mid_component =
            test.look_up_instance(&["mid"].try_into().unwrap()).await.expect("mid instance");
        let source = debug_route_sandbox_path(
            &mid_component,
            &UseDecl::Runner(UseRunnerDecl {
                source: UseSource::Parent,
                source_name: "bar".parse().unwrap(),
                source_dictionary: "dict".parse().unwrap(),
            }),
        )
        .await
        .expect("failed to route runner");

        match source {
            capability_source::CapabilitySource::Component(
                capability_source::ComponentSource {
                    capability:
                        capability_source::ComponentCapability::Runner(RunnerDecl { name, .. }),
                    moniker,
                },
            ) => {
                assert_eq!(name, "foo");
                assert_eq!(moniker, Moniker::root());
            }
            _ => panic!("unexpected source: {:?}", source),
        }
    }

    pub async fn test_offer_runner_from_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .runner_default("foo")
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::runner()
                            .name("foo")
                            .target_name("bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::runner()
                            .name("bar")
                            .target_name("baz")
                            .from_dictionary("dict")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new_empty_component()
                    .use_(UseBuilder::runner().name("baz"))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        let leaf_component = test
            .look_up_instance(&["mid", "leaf"].try_into().unwrap())
            .await
            .expect("leaf instance");
        let source = debug_route_sandbox_path(
            &leaf_component,
            &UseDecl::Runner(UseRunnerDecl {
                source: UseSource::Parent,
                source_name: "baz".parse().unwrap(),
                source_dictionary: Default::default(),
            }),
        )
        .await
        .expect("failed to route runner");

        match source {
            capability_source::CapabilitySource::Component(
                capability_source::ComponentSource {
                    capability:
                        capability_source::ComponentCapability::Runner(RunnerDecl { name, .. }),
                    moniker,
                },
            ) => {
                assert_eq!(name, "foo");
                assert_eq!(moniker, Moniker::root());
            }
            _ => panic!("unexpected source: {:?}", source),
        }
    }

    pub async fn test_offer_directory_from_dictionary(&self) {
        let use_decl =
            UseBuilder::directory().name("baz").rights(fio::R_STAR_DIR).path("/foo").build();
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .capability(
                        CapabilityBuilder::directory()
                            .name("foo")
                            .rights(fio::R_STAR_DIR)
                            .path("/foo"),
                    )
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::directory()
                            .name("foo")
                            .target_name("bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::directory()
                            .name("bar")
                            .target_name("baz")
                            .from_dictionary("dict")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            ("leaf", ComponentDeclBuilder::new().use_(use_decl.clone()).build()),
        ];

        let test = T::new("root", components).build().await;
        let leaf_component = test
            .look_up_instance(&["mid", "leaf"].try_into().unwrap())
            .await
            .expect("leaf instance");

        let source = debug_route_sandbox_path(&leaf_component, &use_decl)
            .await
            .expect("failed to route resolver");

        match source {
            capability_source::CapabilitySource::StorageBackingDirectory(
                capability_source::StorageBackingDirectorySource {
                    capability:
                        capability_source::ComponentCapability::Directory(DirectoryDecl {
                            name, ..
                        }),
                    moniker,
                    ..
                },
            ) => {
                assert_eq!(name, "foo");
                assert_eq!(moniker, Moniker::root());
            }
            _ => panic!("unexpected source: {:?}", source),
        }
    }

    pub async fn test_use_config_from_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::config().name("foo").value(10_i32.into()))
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::config()
                            .name("foo")
                            .target_name("bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::config()
                            .name("bar")
                            .config_type(ConfigValueType::Int32)
                            .from_dictionary("dict"),
                    )
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        let leaf_component =
            test.look_up_instance(&["leaf"].try_into().unwrap()).await.expect("leaf instance");
        let source = debug_route_sandbox_path(
            &leaf_component,
            &UseDecl::Config(Box::new(UseConfigurationDecl {
                source: UseSource::Parent,
                source_name: "bar".parse().unwrap(),
                target_name: "bar".parse().unwrap(),
                availability: Availability::Required,
                type_: ConfigValueType::Int32,
                default: None,
                source_dictionary: "dict".parse().unwrap(),
            })),
        )
        .await
        .expect("failed to route config");

        match source {
            capability_source::CapabilitySource::Component(
                capability_source::ComponentSource {
                    capability: capability_source::ComponentCapability::Config(_),
                    moniker,
                },
            ) => {
                assert_eq!(moniker, moniker::Moniker::root());
            }
            _ => panic!("unexpected source: {:?}", source),
        }
    }

    pub async fn test_offer_config_from_dictionary(&self) {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::config().name("foo").value(10_i32.into()))
                    .dictionary_default("dict")
                    .offer(
                        OfferBuilder::config()
                            .name("foo")
                            .target_name("bar")
                            .source(OfferSource::Self_)
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::dictionary()
                            .name("dict")
                            .source(OfferSource::Self_)
                            .target_static_child("mid"),
                    )
                    .child_default("mid")
                    .build(),
            ),
            (
                "mid",
                ComponentDeclBuilder::new()
                    .offer(
                        OfferBuilder::config()
                            .name("bar")
                            .target_name("baz")
                            .from_dictionary("dict")
                            .source(OfferSource::Parent)
                            .target_static_child("leaf"),
                    )
                    .child_default("leaf")
                    .build(),
            ),
            (
                "leaf",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::config().name("baz").config_type(ConfigValueType::Int32))
                    .build(),
            ),
        ];

        let test = T::new("root", components).build().await;
        let leaf_component = test
            .look_up_instance(&["mid", "leaf"].try_into().unwrap())
            .await
            .expect("leaf instance");
        let source = debug_route_sandbox_path(
            &leaf_component,
            &UseDecl::Config(Box::new(UseConfigurationDecl {
                source: UseSource::Parent,
                source_name: "baz".parse().unwrap(),
                target_name: "baz".parse().unwrap(),
                availability: Availability::Required,
                type_: ConfigValueType::Int32,
                default: None,
                source_dictionary: Default::default(),
            })),
        )
        .await
        .expect("failed to route config");

        match source {
            capability_source::CapabilitySource::Component(
                capability_source::ComponentSource {
                    capability: capability_source::ComponentCapability::Config(_),
                    moniker,
                },
            ) => {
                assert_eq!(moniker, moniker::Moniker::root());
            }
            _ => panic!("unexpected source: {:?}", source),
        }
    }
}
