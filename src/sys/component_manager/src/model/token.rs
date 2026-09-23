// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use moniker::Moniker;
use std::collections::HashMap;
use std::sync::Arc;
use zx::Koid;

use super::context::ModelContext;

const KOID_ERROR: &str = "basic info should not require any rights";

pub struct InstanceToken(zx::Event);

impl Clone for InstanceToken {
    fn clone(&self) -> Self {
        Self(self.0.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap())
    }
}

impl From<InstanceToken> for zx::Event {
    fn from(value: InstanceToken) -> Self {
        value.0
    }
}

impl From<zx::Event> for InstanceToken {
    fn from(value: zx::Event) -> Self {
        InstanceToken(value)
    }
}

/// [`InstanceRegistry`] maintains mapping from [`InstanceToken`] KOIDs to the
/// moniker of those component instances.
pub struct InstanceRegistry {
    koid_to_moniker: Mutex<HashMap<Koid, Arc<Moniker>>>,
}

impl InstanceRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { koid_to_moniker: Mutex::new(HashMap::new()) })
    }

    fn add(&self, moniker: Moniker) -> InstanceToken {
        let event = zx::Event::create();
        let koid = event.koid().expect(KOID_ERROR);
        self.koid_to_moniker.lock().insert(koid, Arc::new(moniker));
        InstanceToken(event)
    }

    #[cfg(all(test, not(feature = "src_model_tests")))]
    pub fn add_for_tests(&self, moniker: Moniker) -> InstanceToken {
        self.add(moniker)
    }

    /// Looks up the moniker of a component given an [`InstanceToken`].
    ///
    /// If this method returns `None`, then either the component instance has
    /// been destroyed, or the token was not minted by component_manager.
    pub fn get(&self, token: &InstanceToken) -> Option<Arc<Moniker>> {
        let koid = token.0.koid().expect(KOID_ERROR);
        self.koid_to_moniker.lock().get(&koid).cloned()
    }

    fn remove(&self, koid: Koid) {
        self.koid_to_moniker.lock().remove(&koid);
    }
}

/// [`InstanceTokenState`] caches a minted [`InstanceToken`] and remembers to
/// unregister it from the registry when the state is dropped.
pub enum InstanceTokenState {
    Unset,
    Set { token: InstanceToken, context: Arc<ModelContext> },
}

impl InstanceTokenState {
    /// Gets a token corresponding to the provided component instance.
    pub fn set(&mut self, moniker: &Moniker, context: &Arc<ModelContext>) -> InstanceToken {
        match self {
            InstanceTokenState::Unset => {
                let token = context.instance_registry().add(moniker.clone());
                *self = InstanceTokenState::Set { token: token.clone(), context: context.clone() };
                token
            }
            InstanceTokenState::Set { token, .. } => token.clone(),
        }
    }
}

impl Default for InstanceTokenState {
    fn default() -> Self {
        InstanceTokenState::Unset
    }
}

impl Drop for InstanceTokenState {
    fn drop(&mut self) {
        match self {
            InstanceTokenState::Unset => {}
            InstanceTokenState::Set { token, context } => {
                context.instance_registry().remove(token.0.koid().expect(KOID_ERROR));
            }
        }
    }
}
