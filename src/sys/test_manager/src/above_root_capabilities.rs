// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants::{
    HERMETIC_RESOLVER_REALM_NAME, TEST_ROOT_COLLECTION, TEST_TYPE_REALM_MAP, WRAPPER_REALM_NAME,
};
use anyhow::{format_err, Error};
use fuchsia_component_test::error::Error as RealmBuilderError;
use fuchsia_component_test::{Capability, RealmBuilder, Ref, RefContext, Route, SubRealmBuilder};
use std::collections::HashMap;
use {fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_component_test as ftest};

#[derive(Default)]
struct CollectionData {
    capabilities: Vec<ftest::Capability>,
    required_event_streams: RequiredEventStreams,
}

#[derive(Default)]
struct RequiredEventStreams {
    capability_requested: bool,
}

impl RequiredEventStreams {
    fn validate(&self) -> bool {
        self.capability_requested
    }
}

pub struct AboveRootCapabilitiesForTest {
    collection_data: HashMap<&'static str, CollectionData>,
}

impl AboveRootCapabilitiesForTest {
    pub async fn new(manifest_name: &str) -> Result<Self, Error> {
        let path = format!("/pkg/meta/{}", manifest_name);
        let file_proxy = fuchsia_fs::file::open_in_namespace(&path, fuchsia_fs::PERM_READABLE)?;
        let component_decl = fuchsia_fs::file::read_fidl::<fdecl::Component>(&file_proxy).await?;
        let collection_data = Self::load(component_decl);
        Ok(Self { collection_data })
    }

    #[cfg(test)]
    pub fn new_empty_for_tests() -> Self {
        let empty_collection_data = HashMap::new();
        Self { collection_data: empty_collection_data }
    }

    pub fn validate(&self, collection: &str) -> Result<(), Error> {
        match self.collection_data.get(collection) {
            Some(c) if !c.required_event_streams.validate() => {
                return Err(format_err!(
                    "The collection `{collection}` must be routed the events \
                `capability_requested` from `parent` scoped \
                to it"
                ));
            }
            _ => Ok(()),
        }
    }

    pub async fn apply(
        &self,
        collection: &str,
        builder: &RealmBuilder,
        wrapper_realm: &SubRealmBuilder,
    ) -> Result<(), RealmBuilderError> {
        if !self.collection_data.contains_key(collection) {
            return Ok(());
        }
        for capability in &self.collection_data[collection].capabilities {
            let (capability_for_test_wrapper, capability_for_test_root) =
                if let ftest::Capability::EventStream(event_stream) = &capability {
                    let mut test_wrapper_event_stream = event_stream.clone();
                    let (wrapper_realm, _) =
                        Ref::child(WRAPPER_REALM_NAME).into_fidl(RefContext::Source);
                    let (hermetic_resolver_realm, _) =
                        Ref::child(HERMETIC_RESOLVER_REALM_NAME).into_fidl(RefContext::Source);
                    let (test_root_collection, _) =
                        Ref::collection(TEST_ROOT_COLLECTION).into_fidl(RefContext::Source);
                    test_wrapper_event_stream.scope = Some(vec![wrapper_realm]);
                    let mut test_root_event_stream = event_stream.clone();
                    test_root_event_stream.scope =
                        Some(vec![test_root_collection, hermetic_resolver_realm]);
                    (
                        ftest::Capability::EventStream(test_wrapper_event_stream),
                        ftest::Capability::EventStream(test_root_event_stream),
                    )
                } else {
                    (capability.clone(), capability.clone())
                };
            builder
                .add_route(
                    Route::new()
                        .capability(capability_for_test_wrapper.clone())
                        .from(Ref::parent())
                        .to(wrapper_realm),
                )
                .await?;
            wrapper_realm
                .add_route(
                    Route::new()
                        .capability(capability_for_test_root.clone())
                        .from(Ref::parent())
                        .to(Ref::collection(TEST_ROOT_COLLECTION)),
                )
                .await?;
        }
        Ok(())
    }

    fn load(decl: fdecl::Component) -> HashMap<&'static str, CollectionData> {
        let mut collection_data: HashMap<_, _> =
            TEST_TYPE_REALM_MAP.values().map(|v| (*v, CollectionData::default())).collect();
        for offer_decl in decl.offers.unwrap_or(vec![]) {
            match offer_decl {
                fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    target_name: Some(target_name),
                    ..
                }) if collection_data.contains_key(name.as_str())
                    && target_name != "fuchsia.logger.LogSink"
                    && target_name != "fuchsia.inspect.InspectSink" =>
                {
                    collection_data.get_mut(name.as_str()).unwrap().capabilities.push(
                        Capability::protocol_by_name(target_name)
                            .availability_same_as_target()
                            .into(),
                    );
                }
                fdecl::Offer::Directory(fdecl::OfferDirectory {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    target_name: Some(target_name),
                    ..
                }) if collection_data.contains_key(name.as_str()) => {
                    collection_data.get_mut(name.as_str()).unwrap().capabilities.push(
                        Capability::directory(target_name).availability_same_as_target().into(),
                    );
                }
                fdecl::Offer::Storage(fdecl::OfferStorage {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    target_name: Some(target_name),
                    ..
                }) if collection_data.contains_key(name.as_str()) => {
                    let use_path = format!("/{}", target_name);
                    collection_data.get_mut(name.as_str()).unwrap().capabilities.push(
                        Capability::storage(target_name)
                            .path(use_path)
                            .availability_same_as_target()
                            .into(),
                    );
                }
                fdecl::Offer::EventStream(fdecl::OfferEventStream {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    target_name: Some(target_name),
                    source: Some(source),
                    scope,
                    ..
                }) if collection_data.contains_key(name.as_str()) => {
                    collection_data
                        .get_mut(name.as_str())
                        .unwrap()
                        .capabilities
                        .push(Capability::event_stream(target_name.clone()).into());

                    // Keep track of relevant event streams being offered from parent to the
                    // collection scoped to it.
                    let coll_ref =
                        fdecl::Ref::Collection(fdecl::CollectionRef { name: name.clone() });
                    if target_name == "capability_requested"
                        && matches!(source, fdecl::Ref::Parent(_))
                        && scope.map(|s| s.contains(&coll_ref)).unwrap_or(false)
                    {
                        let entry = collection_data.get_mut(name.as_str()).unwrap();
                        entry.required_event_streams.capability_requested =
                            entry.required_event_streams.capability_requested
                                || target_name == "capability_requested";
                    }
                }
                fdecl::Offer::Service(fdecl::OfferService {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    target_name: Some(target_name),
                    ..
                }) if collection_data.contains_key(name.as_str()) => {
                    collection_data.get_mut(name.as_str()).unwrap().capabilities.push(
                        Capability::service_by_name(target_name)
                            .availability_same_as_target()
                            .into(),
                    );
                }
                fdecl::Offer::Runner(fdecl::OfferRunner {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    ..
                })
                | fdecl::Offer::Resolver(fdecl::OfferResolver {
                    target: Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })),
                    ..
                }) if collection_data.contains_key(name.as_str()) => {
                    unimplemented!("Runners and resolvers are not supported by realm builder");
                }
                _ => {
                    // Ignore anything else that is not routed to test collections
                }
            }
        }
        collection_data
    }
}
