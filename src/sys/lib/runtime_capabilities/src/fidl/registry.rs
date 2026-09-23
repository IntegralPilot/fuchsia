// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Capability, RemoteError};
use fidl::HandleRef;
use fidl::handle::{EventPair, Signals};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use zx::Koid;

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

/// Given a reference to a handle, returns a copy of a capability from the registry that was added
/// with the handle's koid.
///
/// Returns [RemoteError::Unregistered] if the capability is not in the registry.
pub(crate) fn try_from_handle_in_registry<'a>(
    handle_ref: HandleRef<'_>,
) -> Result<Capability, RemoteError> {
    let koid = handle_ref.koid().unwrap();
    let capability = get(koid).ok_or(RemoteError::Unregistered)?;
    Ok(capability)
}

/// Registers a capability with a task.
///
/// After this call, the registry will retain `capability` and recognize `koid`.
/// One may use [`get`] to get another clone of the capability given the `koid`.
///
/// When `fut` completes:
///
/// - The `capability` will be dropped.
/// - The `koid` will no longer be recognized by the registry.
///
pub(crate) fn insert(
    capability: Capability,
    koid: Koid,
    fut: impl Future<Output = ()> + Send + 'static,
) {
    let mut registry = REGISTRY.lock();
    let guard = scopeguard::guard((), move |_| {
        REGISTRY.lock().remove(koid);
    });
    let task = fasync::Task::spawn(async move {
        let _guard = guard;
        fut.await;
    });
    let existing = registry.insert(koid, Entry { capability, task: Some(task) });
    assert!(existing.is_none());
}

/// Inserts a capability into the registry and returns a token that can be
/// used to lookup the same capability later. Short form of [`insert`] when
/// the capability is represented externally as a token.
pub(crate) fn insert_token(capability: Capability) -> EventPair {
    let (watcher, token) = EventPair::create();
    insert(
        capability,
        token.basic_info().unwrap().koid,
        fasync::OnSignals::new(watcher, Signals::OBJECT_PEER_CLOSED).map(|_| ()),
    );
    token
}

/// Get a capability from the global registry and returns it, if it exists.
pub(crate) fn get(koid: Koid) -> Option<Capability> {
    let registry = REGISTRY.lock();
    registry.get(koid).map(|entry| entry.capability.clone())
}

pub struct Entry {
    pub capability: Capability,
    // TODO(https://fxbug.dev/332389972): Remove or explain #[allow(dead_code)].
    #[allow(dead_code)]
    pub task: Option<fasync::Task<()>>,
}

/// The [Registry] stores capabilities that have been converted to FIDL, providing a way to get
/// the original Rust object back from a FIDL representation of a capability.
///
/// There should only be a single Registry, outside of unit tests.
#[derive(Default)]
pub struct Registry {
    entries: HashMap<Koid, Entry>,
}

impl Registry {
    /// Inserts an entry into the registry.
    ///
    /// If an entry with the same koid already exists, replaces the entry with the new one
    /// and returns the old one.
    ///
    /// Returns None if the entry with the given koid did not previously exist.
    pub(crate) fn insert(&mut self, koid: Koid, entry: Entry) -> Option<Entry> {
        self.entries.insert(koid, entry)
    }

    /// Removes an entry from the registry, if one with a matching koid exists.
    pub(crate) fn remove(&mut self, koid: Koid) -> Option<Entry> {
        self.entries.remove(&koid)
    }

    /// Gets an entry from the registry, if one with a matching koid exists.
    pub(crate) fn get(&self, koid: Koid) -> Option<&Entry> {
        self.entries.get(&koid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Data;
    use assert_matches::assert_matches;
    use futures::channel::oneshot;

    /// Tests that a capability can be inserted and retrieved from a Registry.
    #[test]
    fn insert_get_remove() {
        let mut registry = Registry::default();

        // Insert a Data capability into the registry.
        let koid = Koid::from_raw(123);
        let data = Data::Int64(0);
        assert!(registry.insert(koid, Entry { capability: data.into(), task: None }).is_none());

        // Get a capability with the same koid. It should be a Data.
        let entry = registry.get(koid).unwrap();
        let got_unit = entry.capability.clone();
        assert_matches!(got_unit, Capability::Data(_));

        // Remove a capability with the same koid. It should be a Data.
        let entry = registry.remove(koid).unwrap();
        let got_unit = entry.capability;
        assert_matches!(got_unit, Capability::Data(_));

        assert!(registry.remove(koid).is_none());
    }

    /// Tests that a capability added with a [remove_when_done] task is removed
    /// when the wrapped task completes.
    #[fuchsia::test(allow_stalls = false)]
    async fn insert_with_task_remove_when_done() {
        let (sender, receiver) = oneshot::channel::<()>();
        // This task completes when the sender is dropped.
        let task = fasync::Task::spawn(async move {
            let _ = receiver.await;
        });

        let koid = Koid::from_raw(123);
        let data = Data::Int64(0);

        insert(data.into(), koid, task);

        // Drop the sender so `task` completes and `remove_when_done_task` removes the entry.
        drop(sender);

        // Allow the spawned future to complete.
        let _ = fasync::TestExecutor::poll_until_stalled(std::future::pending::<()>()).await;

        // Remove a capability with the same koid. It should not exist.
        assert!(get(koid).is_none());
    }
}
