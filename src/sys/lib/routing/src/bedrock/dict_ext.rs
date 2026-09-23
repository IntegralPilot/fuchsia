// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_types::{IterablePath, RelativePath};
use fidl_fuchsia_component_runtime::RouteRequest;
use router_error::RouterError;
use runtime_capabilities::{Capability, Dictionary, Routable, Router, WeakInstanceToken};
use std::sync::Arc;

#[async_trait]
pub trait DictExt {
    /// Returns the capability at the path, if it exists. Returns `None` if path is empty.
    fn get_capability(&self, path: &impl IterablePath) -> Option<Capability>;

    /// Inserts the capability at the path. Intermediary dictionaries are created as needed. If
    /// there's already a capability at the path, then the preexisting value is returned.
    fn insert_capability(
        &self,
        path: &impl IterablePath,
        capability: Capability,
    ) -> Option<Capability>;

    /// Removes the capability at the path, if it exists, and returns it.
    fn remove_capability(&self, path: &impl IterablePath) -> Option<Capability>;
}

#[async_trait]
impl DictExt for Arc<Dictionary> {
    fn get_capability(&self, path: &impl IterablePath) -> Option<Capability> {
        let mut segments = path.iter_segments();
        let Some(mut current_name) = segments.next() else {
            return Some(Capability::Dictionary(self.clone()));
        };
        let mut current_dict = self.clone();
        loop {
            match segments.next() {
                Some(next_name) => {
                    let sub_dict =
                        current_dict.get(current_name).and_then(|value| value.to_dictionary())?;
                    current_dict = sub_dict;

                    current_name = next_name;
                }
                None => return current_dict.get(current_name),
            }
        }
    }

    fn insert_capability(
        &self,
        path: &impl IterablePath,
        capability: Capability,
    ) -> Option<Capability> {
        let mut segments = path.iter_segments();
        let mut current_name = segments.next().expect("path must be non-empty");
        let mut current_dict = self.clone();
        loop {
            match segments.next() {
                Some(next_name) => {
                    let sub_dict = {
                        match current_dict.get(current_name) {
                            Some(Capability::Dictionary(dict)) => dict,
                            Some(Capability::DictionaryRouter(preexisting_router)) => {
                                let mut path = vec![next_name];
                                while let Some(name) = segments.next() {
                                    path.push(name);
                                }
                                let path = RelativePath::from(path);
                                let new_router = Router::new(AdditiveDictionaryRouter {
                                    preexisting_router,
                                    path,
                                    capability,
                                });

                                // Replace the entry in current_dict.
                                return current_dict.insert(current_name.into(), new_router.into());
                            }
                            None => {
                                let dict = Dictionary::new();
                                current_dict.insert(
                                    current_name.into(),
                                    Capability::Dictionary(dict.clone()),
                                );
                                dict
                            }
                            _ => return None,
                        }
                    };
                    current_dict = sub_dict;

                    current_name = next_name;
                }
                None => {
                    return current_dict.insert(current_name.into(), capability);
                }
            }
        }
    }

    fn remove_capability(&self, path: &impl IterablePath) -> Option<Capability> {
        let mut segments = path.iter_segments();
        let mut current_name = segments.next().expect("path must be non-empty");
        let mut current_dict = self.clone();
        loop {
            match segments.next() {
                Some(next_name) => {
                    let sub_dict =
                        current_dict.get(current_name).and_then(|value| value.to_dictionary());
                    if sub_dict.is_none() {
                        // The capability doesn't exist, there's nothing to remove.
                        return None;
                    }
                    current_dict = sub_dict.unwrap();
                    current_name = next_name;
                }
                None => {
                    return current_dict.remove(current_name);
                }
            }
        }
    }
}

struct AdditiveDictionaryRouter {
    preexisting_router: Arc<Router<Dictionary>>,
    path: RelativePath,
    capability: Capability,
}

#[async_trait]
impl Routable<Dictionary> for AdditiveDictionaryRouter {
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        let dictionary = match self.preexisting_router.route(request, target).await {
            Ok(Some(dictionary)) => dictionary.shallow_copy(),
            other_response => return other_response,
        };
        let _ = dictionary.insert_capability(&self.path, self.capability.clone());
        Ok(Some(dictionary))
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        self.preexisting_router.route_debug(request, target).await
    }
}
