// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Watchers handles a list of watcher connections attached to a directory.  Watchers as described
//! in fuchsia.io.

pub mod event_producers;

mod watcher;
pub use watcher::Controller;

use crate::directory::entry_container::{Directory, DirectoryWatcher};
use crate::directory::watchers::event_producers::EventProducer;
use crate::execution_scope::ExecutionScope;
use flex_fuchsia_io as fio;
use slab::Slab;
use std::sync::Arc;

/// Wraps all watcher connections observing one directory.  The directory is responsible for
/// calling [`Self::add()`] and [`Self::send_event()`] method when appropriate to make sure
/// watchers are observing a consistent view.
pub struct Watchers(Slab<Controller>);

impl Watchers {
    /// Constructs a new Watchers instance with no connected watchers.
    pub fn new() -> Self {
        Watchers(Slab::new())
    }

    /// Connects a new watcher (connected over the `channel`) to the list of watchers.  It is the
    /// responsibility of the caller to also send `WATCH_EVENT_EXISTING` and `WatchMask::IDLE`
    /// events on the returned [`Controller`] to the newly connected watcher using the
    /// [`Self::send_event`] methods.  This `mask` is the event mask this watcher has requested.
    ///
    /// Return value of `None` means the executor did not accept a new task, so the watcher has
    /// been dropped.
    ///
    /// NOTE The reason `add` can not send both events on its own by consuming an
    /// [`EventProducer`] is because a lazy directory needs async context to generate a list of
    /// it's entries.  Meaning we need a async version of the [`EventProducer`] - and that is a lot
    /// of additional managing of functions and state.  Traits do not support async methods yet, so
    /// we would need to manage futures returned by the [`EventProducer`] methods explicitly.
    /// Plus, for the [`crate::directory::immutable::Simple`] directory it is all unnecessary.
    #[must_use = "Caller of add() must send WATCH_EVENT_EXISTING and fio::WatchMask::IDLE on the \
                  returned controller"]
    pub fn add(
        &mut self,
        scope: ExecutionScope,
        directory: Arc<dyn Directory>,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> &Controller {
        let entry = self.0.vacant_entry();
        let key = entry.key();
        let done = move || directory.unregister_watcher(key);

        entry.insert(Controller::new(scope, mask, watcher, done))
    }

    /// Informs all the connected watchers about the specified event.  While `mask` and `event`
    /// carry the same information, as they are represented by `WatchMask::*` and `WATCH_EVENT_*`
    /// constants in fuchsia.io, it is easier when both forms are provided.  `mask` is used to
    /// filter out those watchers that did not request for observation of this event and `event` is
    /// used to construct the event object.  The method will operate correctly only if `mask` and
    /// `event` match.
    ///
    /// In case of a communication error with any of the watchers, connection to this watcher is
    /// closed.
    pub fn send_event(&mut self, producer: &mut dyn EventProducer) {
        while producer.prepare_for_next_buffer() {
            let mut consumed_any = false;

            for (_key, controller) in self.0.iter() {
                controller.send_buffer(producer.mask(), || {
                    consumed_any = true;
                    producer.buffer()
                });
            }

            if !consumed_any {
                break;
            }
        }
    }

    /// Disconnects a watcher with the specified key.  A directory will use this method during the
    /// `unregister_watcher` call.
    pub fn remove(&mut self, key: usize) {
        self.0.remove(key);
    }
}

#[cfg(all(test, target_os = "fuchsia"))]
mod tests {
    use super::*;
    use crate::directory::dirents_sink::Sealed;
    use crate::directory::entry::{EntryInfo, GetEntryInfo};
    use crate::directory::traversal_position::TraversalPosition;
    use crate::node::Node;
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use zx_status::Status;

    struct FakeDirectory(Mutex<Inner>);

    impl FakeDirectory {
        fn new() -> Arc<Self> {
            Arc::new(FakeDirectory(Mutex::new(Inner {
                remove_called: false,
                watchers: Watchers::new(),
            })))
        }
    }
    struct Inner {
        remove_called: bool,
        watchers: Watchers,
    }

    impl Directory for FakeDirectory {
        fn open(
            self: Arc<Self>,
            _scope: ExecutionScope,
            _path: crate::Path,
            _flags: fio::Flags,
            _object_request: crate::ObjectRequestRef<'_>,
        ) -> Result<(), Status> {
            unimplemented!()
        }

        async fn read_dirents(
            &self,
            _pos: &TraversalPosition,
            _sink: Box<dyn crate::directory::dirents_sink::Sink>,
        ) -> Result<(TraversalPosition, Box<dyn Sealed>), Status> {
            unimplemented!()
        }

        fn register_watcher(
            self: Arc<Self>,
            scope: ExecutionScope,
            mask: fio::WatchMask,
            watcher: DirectoryWatcher,
        ) -> Result<(), Status> {
            let _ = self.0.lock().watchers.add(scope, self.clone(), mask, watcher);
            Ok(())
        }

        fn unregister_watcher(self: Arc<Self>, key: usize) {
            let mut this = self.0.lock();
            this.remove_called = true;
            this.watchers.remove(key);
        }
    }

    impl Node for FakeDirectory {
        async fn get_attributes(
            &self,
            _requested_attributes: fio::NodeAttributesQuery,
        ) -> Result<fio::NodeAttributes2, Status> {
            unimplemented!()
        }
    }

    impl GetEntryInfo for FakeDirectory {
        fn entry_info(&self) -> EntryInfo {
            unimplemented!()
        }
    }

    #[fuchsia::test]
    fn test_unregister_watcher_on_peer_closed() {
        let mut executor = fasync::TestExecutor::new();
        let directory = FakeDirectory::new();
        #[cfg(feature = "fdomain")]
        let client_domain = fdomain_local::local_client_empty();

        #[cfg(feature = "fdomain")]
        let (client, server) = client_domain.create_endpoints::<fio::DirectoryWatcherMarker>();
        #[cfg(not(feature = "fdomain"))]
        let (client, server) = flex_client::fidl::create_endpoints::<fio::DirectoryWatcherMarker>();

        #[cfg(feature = "fdomain")]
        let scope = ExecutionScope::new(client_domain.clone());
        #[cfg(not(feature = "fdomain"))]
        let scope = ExecutionScope::new();
        directory
            .clone()
            .register_watcher(scope, fio::WatchMask::EXISTING, server.into())
            .expect("Failed to register watcher");
        assert!(!directory.0.lock().remove_called);
        assert_eq!(directory.0.lock().watchers.0.len(), 1);

        // Dropping the client end will signal PEER_CLOSED on the server end.
        std::mem::drop(client);
        // Wait for the Controller's task to handle the PEER_CLOSED signal.
        let _ = executor.run_until_stalled(&mut std::future::pending::<()>());
        assert!(directory.0.lock().remove_called);
        assert_eq!(directory.0.lock().watchers.0.len(), 0);
    }

    #[fuchsia::test]
    fn test_unregister_watcher_on_message() {
        let mut executor = fasync::TestExecutor::new();
        let directory = FakeDirectory::new();
        #[cfg(feature = "fdomain")]
        let client_domain = fdomain_local::local_client_empty();

        #[cfg(feature = "fdomain")]
        let (client, server) = client_domain.create_endpoints::<fio::DirectoryWatcherMarker>();
        #[cfg(not(feature = "fdomain"))]
        let (client, server) = flex_client::fidl::create_endpoints::<fio::DirectoryWatcherMarker>();

        #[cfg(feature = "fdomain")]
        let scope = ExecutionScope::new(client_domain.clone());
        #[cfg(not(feature = "fdomain"))]
        let scope = ExecutionScope::new();
        directory
            .clone()
            .register_watcher(scope, fio::WatchMask::EXISTING, server.into())
            .expect("Failed to register watcher");
        assert!(!directory.0.lock().remove_called);
        assert_eq!(directory.0.lock().watchers.0.len(), 1);

        // The client shouldn't send anything over the channel. The Controller's task will terminate
        // if it receives a message.
        client.channel().write(&[1, 2, 3], &mut []).expect("Failed to write to channel");
        // Wait for the Controller's task to handle the CHANNEL_READABLE signal.
        let _ = executor.run_until_stalled(&mut std::future::pending::<()>());
        assert!(directory.0.lock().remove_called);
        assert_eq!(directory.0.lock().watchers.0.len(), 0);
    }
}
