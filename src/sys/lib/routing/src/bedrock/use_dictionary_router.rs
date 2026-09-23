// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::RoutingError;
use async_trait::async_trait;
use capability_source::CapabilitySource;
use fidl_fuchsia_component_runtime::RouteRequest;
use futures::StreamExt;
use futures::channel::oneshot;
use futures::stream::FuturesUnordered;
use router_error::RouterError;
use runtime_capabilities::{
    Capability, Dictionary, EntryUpdate, Routable, Router, UpdateNotifierRetention,
    WeakInstanceToken,
};
use std::sync::Arc;

/// Given an original dictionary and a handful of additional dictionary routers, produces a router
/// that when invoked will route all the additional routers, merge everything together into a
/// single dictionary, and return that.
pub struct UseDictionaryRouter {
    path: cm_types::Path,
    moniker: moniker::Moniker,
    original_dictionary: Arc<Dictionary>,
    dictionary_routers: Vec<Arc<Router<Dictionary>>>,
    capability_source: CapabilitySource,
}

impl UseDictionaryRouter {
    pub fn new(
        path: cm_types::Path,
        moniker: moniker::Moniker,
        original_dictionary: Arc<Dictionary>,
        dictionary_routers: Vec<Arc<Router<Dictionary>>>,
        capability_source: CapabilitySource,
    ) -> Arc<Router<Dictionary>> {
        Router::new(Self {
            path,
            moniker,
            original_dictionary,
            dictionary_routers,
            capability_source,
        })
    }

    /// Keeps this dictionary updated as any entries are added to or removed from other_dict. If
    /// there are conflicting entries in `self` and `other_dict` at the time this function is
    /// called, then `Some` will be returned with the name of the conflicting entry. If a
    /// conflicting entry is added to `other_dict` after this function has been returned, then a
    /// log about the conflict will be emitted. In both cases the conflicting item in `other_dict`
    /// will be ignored, and the preexisting entry in `self` will take precedence.
    async fn dictionary_follow_updates_from(
        &self,
        self_dictionary: Arc<Dictionary>,
        other_dict: Arc<Dictionary>,
    ) -> Vec<(cm_types::Name, Capability, Capability)> {
        let self_clone = self_dictionary;
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);
        let mut initial_conflicts = vec![];
        other_dict.register_update_notifier(Box::new(move |entry_update| {
            match entry_update {
                EntryUpdate::Add(key, capability) => {
                    if let Some(preexisting_value) = self_clone.get(key) {
                        // There's a conflict! Let's let the preexisting value take precedence, and
                        // note the issue.
                        initial_conflicts.push((key.into(), capability.clone(), preexisting_value));
                    } else {
                        let _ = self_clone.insert(key.into(), capability.clone());
                    }
                }
                EntryUpdate::Remove(key) => {
                    let _ = self_clone.remove(key);
                }
                EntryUpdate::Idle => {
                    if let Some(sender) = sender.take() {
                        let _ = sender.send(std::mem::take(&mut initial_conflicts));
                    }
                }
            }
            UpdateNotifierRetention::Retain
        }));

        receiver.await.expect("sender was dropped unexpectedly")
    }
}

#[async_trait]
impl Routable<Dictionary> for UseDictionaryRouter {
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        let mut futures_unordered = FuturesUnordered::new();
        for dictionary_router in self.dictionary_routers.iter() {
            futures_unordered.push(dictionary_router.route(request.clone(), target.clone()));
        }
        let resulting_dictionary = self.original_dictionary.shallow_copy();
        while let Some(route_result) = futures_unordered.next().await {
            match route_result {
                Ok(Some(other_dictionary)) => {
                    let initial_conflicts = self
                        .dictionary_follow_updates_from(
                            resulting_dictionary.clone(),
                            other_dictionary,
                        )
                        .await;
                    let mut conflicting_names = vec![];
                    for (key, capability, preexisting_value) in initial_conflicts {
                        log::warn!(
                            "{}: unable to add {key} from source {} to merged dictionary for path \
                            {} because the dictionary already contains an item with the same name \
                            from source {}",
                            self.moniker,
                            try_get_router_source(&capability, target.clone())
                                .await
                                .unwrap_or_else(|| "<unknown>".to_string()),
                            self.path,
                            try_get_router_source(&preexisting_value, target.clone())
                                .await
                                .unwrap_or_else(|| "<unknown>".to_string()),
                        );
                        conflicting_names.push(key);
                    }
                    if !conflicting_names.is_empty() {
                        return Err(RoutingError::ConflictingDictionaryEntries {
                            moniker: self.capability_source.source_moniker(),
                            conflicting_names,
                        }
                        .into());
                    }
                }
                Ok(None) => (),
                Err(_e) => {
                    // Errors are already logged by this point by the WithPorcelain router.
                    // Specifically, the routers in `dictionary_routers` are assembled by
                    // `crate::bedrock::sandbox_construction::extend_dict_with_use`, which does
                    // this wrapping.
                }
            }
        }
        Ok(resulting_dictionary.into())
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Ok(self.capability_source.clone())
    }
}

async fn try_get_router_source(
    capability: &Capability,
    target: Arc<WeakInstanceToken>,
) -> Option<String> {
    let source: CapabilitySource = match capability {
        Capability::DictionaryRouter(router) => {
            router.route_debug(RouteRequest::default(), target).await.ok()?
        }
        Capability::ConnectorRouter(router) => {
            router.route_debug(RouteRequest::default(), target).await.ok()?
        }
        Capability::DirConnectorRouter(router) => {
            router.route_debug(RouteRequest::default(), target).await.ok()?
        }
        Capability::DataRouter(router) => {
            router.route_debug(RouteRequest::default(), target).await.ok()?
        }
        _ => return None,
    };
    Some(format!("{}", source))
}
