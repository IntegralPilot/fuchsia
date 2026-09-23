// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines a thread safe accept queue to be shared between listener and
//! connected sockets.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
#[cfg(test)]
use alloc::vec::Vec;
use core::fmt::Debug;
use core::hash::Hash;
use core::ops::DerefMut;

use assert_matches::assert_matches;
use derivative::Derivative;
use netstack3_base::Inspector;
use netstack3_base::sync::Mutex;
use netstack3_hashmap::HashMap;

/// A notifier used to tell Bindings about new pending connections for a single
/// socket.
pub trait ListenerNotifier {
    /// When the ready queue length has changed, signal to the Bindings.
    fn new_incoming_connections(&mut self, num_ready: usize);
}

/// A thread-safe shareable accept queue.
///
/// [`AcceptQueue`] implements all necessary locking internally and can be
/// safely shared between a listener and the connected sockets generated from
/// it.
#[derive(Debug, Derivative)]
#[derivative(Clone(bound = ""))]
pub struct AcceptQueue<S, R, N>(Arc<Mutex<AcceptQueueInner<S, R, N>>>);

#[cfg(test)]
impl<S, R, N> PartialEq for AcceptQueue<S, R, N>
where
    AcceptQueueInner<S, R, N>: PartialEq,
{
    fn eq(&self, Self(other): &Self) -> bool {
        let Self(inner) = self;
        if Arc::ptr_eq(other, inner) {
            return true;
        }
        let guard = inner.lock();
        let other_guard = other.lock();
        (&*guard).eq(&*other_guard)
    }
}

#[cfg(test)]
impl<S, R, N> Eq for AcceptQueue<S, R, N> where Self: PartialEq {}

#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
enum EntryState {
    Pending,
    Ready,
}

#[derive(Debug)]
#[cfg_attr(test, derive(Derivative))]
#[cfg_attr(
    test,
    derivative(PartialEq(bound = "S: Hash + Clone + Eq + PartialEq, R: PartialEq, N: PartialEq"))
)]
struct AcceptQueueInner<S, R, N> {
    ready_queue: VecDeque<(S, R)>,
    all_sockets: HashMap<S, EntryState>,
    notifier: Option<N>,
}

impl<S, R, N> AcceptQueue<S, R, N>
where
    S: Hash + Clone + Eq + PartialEq + Debug,
    N: ListenerNotifier,
{
    /// Creates a new [`AcceptQueue`] with `notifier`.
    pub(crate) fn new(notifier: N) -> Self {
        Self(Arc::new(Mutex::new(AcceptQueueInner::new(notifier))))
    }

    fn lock(&self) -> impl DerefMut<Target = AcceptQueueInner<S, R, N>> + '_ {
        let Self(inner) = self;
        inner.lock()
    }

    /// Pops a ready entry from the queue.
    ///
    /// Returns the socket and its associated ready state if the queue was not
    /// empty.
    ///
    /// `pop_ready` also hits the notifier with the new ready item count.
    pub(crate) fn pop_ready(&self) -> Option<(S, R)> {
        self.lock().pop_ready()
    }

    /// Collects all currently pending sockets.
    #[cfg(test)]
    pub(crate) fn collect_pending(&self) -> Vec<S> {
        self.lock().collect_pending()
    }

    /// Pushes a new `pending` connection.
    ///
    /// Panics if `pending` is already in the queue or if the queue is already
    /// closed.
    pub(crate) fn push_pending(&self, pending: S) {
        self.lock().push_pending(pending)
    }

    /// Returns the total number of entries in the queue (pending and ready).
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    /// Returns the number of ready entries in the queue.
    #[cfg(test)]
    pub(crate) fn ready_len(&self) -> usize {
        self.lock().ready_len()
    }

    /// Returns the number of pending entries in the queue.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.lock().pending_len()
    }

    /// Notifies that [`newly_ready`] is now ready to be accepted.
    ///
    /// Does nothing if the queue is closed.
    ///
    /// Panics if the queue is not closed and `newly_ready` was not in the queue
    /// as a pending entry.
    pub(crate) fn notify_ready(&self, newly_ready: &S, ready_state: R) {
        self.lock().notify_ready(newly_ready, ready_state)
    }

    /// Removes an entry from the queue.
    ///
    /// No-op if the queue is closed or if the entry was not in the queue.
    pub(crate) fn remove(&self, entry: &S) {
        self.lock().remove(entry)
    }

    /// Closes the queue and returns all entries that were in the queue and the
    /// notifier.
    ///
    /// Panics if the queue is already closed.
    pub(crate) fn close(&self) -> (impl Iterator<Item = S> + use<S, R, N>, N) {
        self.lock().close()
    }

    /// Returns true if the queue is closed.
    pub(crate) fn is_closed(&self) -> bool {
        self.lock().is_closed()
    }

    /// Inspects the queue.
    pub(crate) fn inspect<I: Inspector>(&self, inspector: &mut I) {
        let inner = self.lock();
        inspector.record_usize("NumReady", inner.ready_len());
        inspector.record_usize("NumPending", inner.pending_len());
        inspector.record_debug("Contents", &inner.all_sockets);
    }
}

impl<S, R, N> AcceptQueueInner<S, R, N>
where
    S: Hash + Clone + Eq + PartialEq + Debug,
    N: ListenerNotifier,
{
    fn new(notifier: N) -> Self {
        Self {
            ready_queue: Default::default(),
            all_sockets: Default::default(),
            notifier: Some(notifier),
        }
    }

    fn pop_ready(&mut self) -> Option<(S, R)> {
        let AcceptQueueInner { ready_queue, all_sockets, notifier } = self;
        let (socket, ready_state) = ready_queue.pop_front()?;
        // Assert internal consistency on state.
        assert_matches!(all_sockets.remove(&socket), Some(EntryState::Ready));
        // Invariant: queue must be empty if notifier is none.
        let notifier = notifier.as_mut().unwrap();
        notifier.new_incoming_connections(ready_queue.len());
        Some((socket, ready_state))
    }

    #[cfg(test)]
    pub(crate) fn collect_pending(&self) -> Vec<S> {
        let AcceptQueueInner { all_sockets, .. } = self;
        all_sockets
            .iter()
            .filter_map(|(socket, state)| match state {
                EntryState::Ready => None,
                EntryState::Pending => Some(socket.clone()),
            })
            .collect()
    }

    fn push_pending(&mut self, pending: S) {
        let AcceptQueueInner { all_sockets, notifier, .. } = self;
        // Protect against the listener attempting to push pending
        // connections if the queue is closed.
        assert!(notifier.is_some());
        assert_matches!(all_sockets.insert(pending, EntryState::Pending), None);
    }

    fn len(&self) -> usize {
        let AcceptQueueInner { all_sockets, .. } = self;
        all_sockets.len()
    }

    fn ready_len(&self) -> usize {
        let AcceptQueueInner { ready_queue, .. } = self;
        ready_queue.len()
    }

    fn pending_len(&self) -> usize {
        let AcceptQueueInner { ready_queue, all_sockets, .. } = self;
        all_sockets.len() - ready_queue.len()
    }

    fn notify_ready(&mut self, newly_ready: &S, ready_state: R) {
        let AcceptQueueInner { ready_queue, all_sockets, notifier } = self;
        let notifier = match notifier {
            Some(notifier) => notifier,

            None => {
                // Queue is closed, listener must be shutting down.
                // Maintain the invariant that a closed queue has cleared all
                // its entries.
                debug_assert!(ready_queue.is_empty());
                debug_assert!(all_sockets.is_empty());
                return;
            }
        };
        let entry = all_sockets
            .get_mut(newly_ready)
            .expect("attempted to notify ready entry that was not in queue");
        let prev_state = core::mem::replace(entry, EntryState::Ready);
        assert_matches!(prev_state, EntryState::Pending);
        ready_queue.push_back((newly_ready.clone(), ready_state));
        notifier.new_incoming_connections(ready_queue.len());
    }

    fn remove(&mut self, entry: &S) {
        let AcceptQueueInner { ready_queue, all_sockets, notifier } = self;
        // Avoid a HashMap lookup if the queue is already closed.
        let notifier = match notifier.as_mut() {
            Some(notifier) => notifier,
            None => {
                // Maintain the invariant that a closed queue has cleared all
                // its entries.
                debug_assert!(ready_queue.is_empty());
                debug_assert!(all_sockets.is_empty());
                return;
            }
        };

        match all_sockets.remove(entry) {
            Some(EntryState::Pending) | None => (),
            Some(EntryState::Ready) => {
                let before_len = ready_queue.len();
                ready_queue.retain(|(s, _ready_data)| s != entry);
                let after_len = ready_queue.len();
                // Item must only be in ready queue once.
                assert_eq!(after_len, before_len - 1);
                notifier.new_incoming_connections(after_len);
            }
        }
    }

    fn close(&mut self) -> (impl Iterator<Item = S> + use<S, R, N>, N) {
        let AcceptQueueInner { ready_queue, all_sockets, notifier } = self;
        // Remove the notifier, this signals that the queue is closed.
        let notifier = notifier.take().expect("queue is already closed");
        // Remove all items from the ready queue by replacing it with an empty
        // VecDeque. This ensures that it'll immediately free all of its memory
        // as opposed to just calling clear.
        *ready_queue = Default::default();
        // Take all the entries so we can return from the caller, leaving a
        // fresh empty HashMap in its place.
        let entries = core::mem::take(all_sockets);
        (entries.into_keys(), notifier)
    }

    fn is_closed(&self) -> bool {
        let AcceptQueueInner { notifier, .. } = self;
        notifier.is_none()
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use netstack3_hashmap::HashSet;

    #[test]
    fn push_ready_pop() {
        let mut queue = AcceptQueueInner::new(Notifier::default());
        assert_eq!(queue.pop_ready(), None);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.clear_notifier(), None);

        queue.push_pending(Socket(0));
        assert_eq!(queue.pop_ready(), None);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.clear_notifier(), None);

        queue.notify_ready(&Socket(0), Ready(2));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.ready_len(), 1);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.clear_notifier(), Some(1));

        assert_eq!(queue.pop_ready(), Some((Socket(0), Ready(2))));
        assert_eq!(queue.clear_notifier(), Some(0));
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.pop_ready(), None);
    }

    #[test]
    fn close() {
        let mut queue = AcceptQueueInner::new(Notifier::default());
        let mut expect = HashSet::new();
        for i in 0..3 {
            let s = Socket(i);
            queue.push_pending(s.clone());
            assert!(expect.insert(s));
        }
        let (socks, _notifier) = queue.close();
        let got = socks.collect::<HashSet<_>>();
        assert_eq!(got, expect);

        assert!(queue.is_closed());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn remove() {
        let mut queue = AcceptQueueInner::new(Notifier::default());
        let s1 = Socket(1);
        let s2 = Socket(2);
        queue.push_pending(s1.clone());
        queue.push_pending(s2.clone());
        queue.notify_ready(&s2, Ready(2));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.ready_len(), 1);
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.clear_notifier(), Some(1));

        queue.remove(&s1);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.ready_len(), 1);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.clear_notifier(), None);

        queue.remove(&s2);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.clear_notifier(), Some(0));

        // Removing already removed entry is a no-op.
        queue.remove(&s1);
        queue.remove(&s2);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.ready_len(), 0);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.clear_notifier(), None);
    }

    #[derive(Default, Eq, PartialEq, Debug, Hash, Clone)]
    struct Socket(usize);
    #[derive(Default, Eq, PartialEq, Debug)]
    struct Ready(usize);

    #[derive(Default, Eq, PartialEq, Debug)]
    struct Notifier(Option<usize>);

    type AcceptQueueInner = super::AcceptQueueInner<Socket, Ready, Notifier>;

    impl AcceptQueueInner {
        fn clear_notifier(&mut self) -> Option<usize> {
            let Self { notifier, .. } = self;
            let Notifier(v) = notifier.as_mut().unwrap();
            v.take()
        }
    }

    impl super::ListenerNotifier for Notifier {
        fn new_incoming_connections(&mut self, num_ready: usize) {
            let Self(n) = self;
            assert_matches!(n.replace(num_ready), None);
        }
    }
}
