// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for managing individual clients (aka sockets) of Netlink.

use fuchsia_sync::Mutex;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::num::NonZeroU32;
use std::ops::DerefMut as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_matches::assert_matches;
use derivative::Derivative;
use netlink_packet_core::NetlinkMessage;

use crate::logging::{log_debug, log_warn};
use crate::messaging::Sender;
use crate::multicast_groups::{
    self, InvalidLegacyGroupsError, InvalidModernGroupError, LegacyGroups, ModernGroup,
    MulticastGroupMemberships,
};
use crate::protocol_family::ProtocolFamily;

/// A unique identifier for a client.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ClientId(u64);

/// The aspects of a client shared by [`InternalClient`] and [`ExternalClient`].
struct InnerClient<F: ProtocolFamily> {
    /// The unique ID for this client.
    id: ClientId,
    /// State relating to the client's multicast group memberships.
    group_memberships: Mutex<MulticastGroupState<F>>,
    /// The client's assigned port number.
    port_number: Mutex<Option<NonZeroU32>>,
}

impl<F: ProtocolFamily> Drop for InnerClient<F> {
    fn drop(&mut self) {
        let mut group_memberships_lock_guard = self.group_memberships.lock();
        let memberships = std::mem::replace(
            &mut group_memberships_lock_guard.deref_mut().memberships,
            MulticastGroupMemberships::new(),
        );
        let mut groups = None;

        for group in memberships.iter_groups() {
            if let Some(group) = F::should_notify_on_group_membership_change(group) {
                push_one_or_many(&mut groups, group);
            }
        }
        if let Some(groups) = groups {
            group_memberships_lock_guard
                .async_work_sink
                .unbounded_send(AsyncWorkItem::OnLeaveMulticastGroup(groups))
                .unwrap_or_else(|_: futures::channel::mpsc::TrySendError<AsyncWorkItem<F>>| {
                    log_warn!("async work receiver was dropped")
                });
        }
    }
}

struct MulticastGroupState<F: ProtocolFamily> {
    /// The client's current multicast group memberships.
    memberships: MulticastGroupMemberships<F>,
    /// Channel for sending async work to the worker thread.
    async_work_sink: futures::channel::mpsc::UnboundedSender<AsyncWorkItem<F>>,
}

impl<F: ProtocolFamily> MulticastGroupState<F> {
    fn new(async_work_sink: futures::channel::mpsc::UnboundedSender<AsyncWorkItem<F>>) -> Self {
        Self { memberships: MulticastGroupMemberships::new(), async_work_sink }
    }
}

impl<F: ProtocolFamily> Display for InnerClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let InnerClient { id: ClientId(id), group_memberships: _, port_number } = self;
        write!(f, "Client[{}/{:?}, {}]", id, *port_number.lock(), F::NAME)
    }
}

/// The internal half of a Netlink client, with the external half being provided
/// by ['ExternalClient'].
#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub(crate) struct InternalClient<F: ProtocolFamily, S: Sender<F::Response>> {
    /// The inner client.
    inner: Arc<InnerClient<F>>,
    /// The [`Sender`] of messages from Netlink to the Client.
    sender: S,
}

impl<F: ProtocolFamily, S: Sender<F::Response>> Debug for InternalClient<F, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

impl<F: ProtocolFamily, S: Sender<F::Response>> Display for InternalClient<F, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let InternalClient { inner, sender: _ } = self;
        write!(f, "{}", inner)
    }
}

impl<F: ProtocolFamily, S: Sender<F::Response>> InternalClient<F, S> {
    /// Returns true if this client is a member of the provided group.
    pub(crate) fn member_of_group(&self, group: ModernGroup) -> bool {
        self.inner.group_memberships.lock().memberships.member_of_group(group)
    }

    /// Sends the given unicast message to the external half of this client.
    pub(crate) fn send_unicast(&mut self, message: NetlinkMessage<F::Response>) {
        self.send(message, None)
    }

    /// Sends the given multicast message to the external half of this client.
    fn send_multicast(&mut self, message: NetlinkMessage<F::Response>, group: ModernGroup) {
        self.send(message, Some(group))
    }

    fn send(&mut self, mut message: NetlinkMessage<F::Response>, group: Option<ModernGroup>) {
        if let Some(port_number) = *self.inner.port_number.lock() {
            message.header.port_number = port_number.into();
        }
        self.sender.send(message, group)
    }

    fn get_id(&self) -> ClientId {
        self.inner.id.clone()
    }
}

#[derive(Debug)]
pub(crate) enum OneOrMany<T> {
    One(T),
    Many(HashSet<T>),
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;

    type IntoIter = itertools::Either<std::iter::Once<T>, std::collections::hash_set::IntoIter<T>>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            OneOrMany::One(one) => itertools::Either::Left(std::iter::once(one)),
            OneOrMany::Many(hash_set) => itertools::Either::Right(hash_set.into_iter()),
        }
    }
}

fn push_one_or_many<T: Hash + PartialEq + Eq + Copy>(
    mut one_or_many: &mut Option<OneOrMany<T>>,
    item: T,
) {
    match &mut one_or_many {
        None => {
            *one_or_many = Some(OneOrMany::One(item));
        }
        Some(OneOrMany::One(existing)) => {
            if *existing == item {
                return;
            }
            *one_or_many = Some(OneOrMany::Many(HashSet::from_iter([*existing, item])));
        }
        Some(OneOrMany::Many(set)) => {
            let _: bool = set.insert(item);
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) enum AsyncWorkItem<F: ProtocolFamily> {
    OnJoinMulticastGroup(F::NotifiedMulticastGroup, oneshot_sync::Sender<()>),
    // No synchronization is actually needed on this, it's just a cleanup signal.
    OnLeaveMulticastGroup(OneOrMany<F::NotifiedMulticastGroup>),
    OnSetMulticastGroups {
        joined: Option<OneOrMany<F::NotifiedMulticastGroup>>,
        left: Option<OneOrMany<F::NotifiedMulticastGroup>>,
        complete: Option<oneshot_sync::Sender<()>>,
    },
    Inner(F::AsyncWorkItem),
}

/// Blocks until asynchronous work is completed.
#[derive(Debug)]
pub enum AsyncWorkCompletionWaiter {
    /// Blocks until a signal is observed on the receiver.
    Blocking(oneshot_sync::Receiver<()>),
    Noop,
}

impl AsyncWorkCompletionWaiter {
    /// Blocks until asynchronous work is completed.
    pub fn wait_until_complete(self) {
        match self {
            Self::Blocking(receiver) => receiver.receive().expect("signaler should not be dropped"),
            Self::Noop => (),
        }
    }

    /// Returns true if [`Self::wait_until_complete`] will not produce any
    /// blocking work.
    pub fn is_noop(&self) -> bool {
        matches!(self, AsyncWorkCompletionWaiter::Noop)
    }

    /// Asserts that blocking work is required, and blocks until asynchronous work is completed.
    pub fn assert_blocking_and_wait_until_complete(self) {
        assert_matches!(&self, Self::Blocking(_));
        self.wait_until_complete()
    }
}

/// The external half of a Netlink client, with the internal half being provided
/// by ['InternalClient'].
pub(crate) struct ExternalClient<F: ProtocolFamily> {
    /// The inner client.
    inner: Arc<InnerClient<F>>,
}

impl<F: ProtocolFamily> Display for ExternalClient<F> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ExternalClient { inner } = self;
        write!(f, "{}", inner)
    }
}

impl<F: ProtocolFamily> ExternalClient<F> {
    pub(crate) fn set_port_number(&self, v: NonZeroU32) {
        *self.inner.port_number.lock() = Some(v);
    }

    /// Adds the given multicast group membership.
    pub(crate) fn add_membership(
        &self,
        group: ModernGroup,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidModernGroupError> {
        let mut group_memberships_lock_guard = self.inner.group_memberships.lock();
        let newly_added =
            group_memberships_lock_guard.memberships.add_membership(group).inspect_err(|err| {
                log_warn!("{self} failed to join invalid multicast group {group:?}: {err:?}")
            })?;
        log_debug!("{self} joined multicast group (newly_added = {newly_added:?}): {group:?}");
        if !newly_added {
            return Ok(AsyncWorkCompletionWaiter::Noop);
        }
        if let Some(group) = F::should_notify_on_group_membership_change(group) {
            let (sender, receiver) = oneshot_sync::channel();

            // NB: it's important that this is done under lock to avoid
            // reordering of setup and teardown events on the channel.
            group_memberships_lock_guard
                .async_work_sink
                .unbounded_send(AsyncWorkItem::OnJoinMulticastGroup(group, sender))
                .expect("netlink worker should not have dropped async work receiver");
            Ok(AsyncWorkCompletionWaiter::Blocking(receiver))
        } else {
            Ok(AsyncWorkCompletionWaiter::Noop)
        }
    }

    /// Deletes the given multicast group membership.
    pub(crate) fn del_membership(&self, group: ModernGroup) -> Result<(), InvalidModernGroupError> {
        let mut group_memberships_lock_guard = self.inner.group_memberships.lock();
        let was_present =
            group_memberships_lock_guard.memberships.del_membership(group).inspect_err(|e| {
                log_warn!("{self} failed to leave invalid multicast group {group:?}: {e:?}");
            })?;
        log_debug!("{self} left multicast group (was_present = {was_present:?}): {group:?}",);
        if !was_present {
            return Ok(());
        }
        if let Some(group) = F::should_notify_on_group_membership_change(group) {
            // NB: it's important that this is done under lock to avoid
            // reordering of setup and teardown events on the channel.
            group_memberships_lock_guard
                .async_work_sink
                .unbounded_send(AsyncWorkItem::OnLeaveMulticastGroup(OneOrMany::One(group)))
                .expect("netlink worker should not have dropped async work receiver");
        }
        Ok(())
    }

    /// Sets the legacy multicast group memberships.
    pub(crate) fn set_legacy_memberships(
        &self,
        legacy_memberships: LegacyGroups,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidLegacyGroupsError> {
        let mut group_memberships_lock_guard = self.inner.group_memberships.lock();
        let mutations = group_memberships_lock_guard
            .memberships
            .set_legacy_memberships(legacy_memberships)
            .inspect_err(|e| {
                log_warn!("{self} failed to update multicast groups {legacy_memberships:?}: {e:?}")
            })?;
        log_debug!("{self} updated multicast groups: {legacy_memberships:?}");
        let mut joined = None;
        let mut left = None;
        for mutation in mutations {
            match mutation {
                multicast_groups::Mutation::None => {}
                multicast_groups::Mutation::Add(group) => {
                    if let Some(group) = F::should_notify_on_group_membership_change(group) {
                        push_one_or_many(&mut joined, group);
                    }
                }
                multicast_groups::Mutation::Del(group) => {
                    if let Some(group) = F::should_notify_on_group_membership_change(group) {
                        push_one_or_many(&mut left, group);
                    }
                }
            }
        }

        let waiter = if joined.is_some() || left.is_some() {
            let (sender, receiver) = oneshot_sync::channel();

            group_memberships_lock_guard
                .async_work_sink
                .unbounded_send(AsyncWorkItem::OnSetMulticastGroups {
                    joined,
                    left,
                    complete: Some(sender),
                })
                .expect("netlink worker should not have dropped async work receiver");
            AsyncWorkCompletionWaiter::Blocking(receiver)
        } else {
            AsyncWorkCompletionWaiter::Noop
        };

        Ok(waiter)
    }
}

// Instantiate a new client pair.
pub(crate) fn new_client_pair<F: ProtocolFamily, S: Sender<F::Response>>(
    id: ClientId,
    sender: S,
    async_work_sink: futures::channel::mpsc::UnboundedSender<AsyncWorkItem<F>>,
) -> (ExternalClient<F>, InternalClient<F, S>) {
    let inner = Arc::new(InnerClient {
        id,
        group_memberships: Mutex::new(MulticastGroupState::new(async_work_sink)),
        port_number: Mutex::default(),
    });
    (ExternalClient { inner: inner.clone() }, InternalClient { inner, sender })
}

/// A generator of [`ClientId`].
#[derive(Default)]
pub(crate) struct ClientIdGenerator(AtomicU64);

impl ClientIdGenerator {
    /// Returns a unique [`ClientId`].
    pub(crate) fn new_id(&self) -> ClientId {
        let ClientIdGenerator(next_id) = self;
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, u64::MAX, "exhausted client IDs");
        ClientId(id)
    }
}

/// The table of connected clients for a given ProtocolFamily.
#[derive(Derivative)]
#[derivative(Clone(bound = ""), Default(bound = ""))]
pub(crate) struct ClientTable<F: ProtocolFamily, S: Sender<F::Response>> {
    clients: Arc<Mutex<HashMap<ClientId, InternalClient<F, S>>>>,
}

impl<F: ProtocolFamily, S: Sender<F::Response>> ClientTable<F, S> {
    /// Adds the given client to this [`ClientTable`].
    ///
    /// # Panics
    ///
    /// Panics if the [`ClientTable`] already contains an entry with the same
    /// [`ClientId`].
    pub(crate) fn add_client(&self, client: InternalClient<F, S>) {
        let id = client.get_id();
        let prev_entry = self.clients.lock().insert(id.clone(), client);
        assert_matches!(prev_entry, None, "{id:?} unexpectedly already existed in the table");
    }

    /// Removes the given client from this ['ClientTable'].
    ///
    /// # Panics
    ///
    /// Panics if the [`ClientTable`] does not contain an entry with the same
    /// [`ClientId`].
    pub(crate) fn remove_client(&self, client: InternalClient<F, S>) {
        let id = client.get_id();
        let prev_entry = self.clients.lock().remove(&id);
        assert_matches!(prev_entry, Some(_), "{id:?} unexpectedly did not exist in the table");
    }

    /// Sends the message to all clients who are members of the multicast group.
    pub(crate) fn send_message_to_group(
        &self,
        message: NetlinkMessage<F::Response>,
        group: ModernGroup,
    ) {
        let count = self.clients.lock().iter_mut().fold(0, |count, (_client_id, client)| {
            if client.member_of_group(group) {
                client.send_multicast(message.clone(), group);
                count + 1
            } else {
                count
            }
        });
        log_debug!(
            "Notified {} {} clients of message for group {:?}: {:?}",
            count,
            F::NAME,
            group,
            message
        );
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    use futures::{Future, StreamExt as _};

    use crate::messaging::testutil::{FakeSender, FakeSenderSink};

    pub(crate) const CLIENT_ID_1: ClientId = ClientId(1);
    pub(crate) const CLIENT_ID_2: ClientId = ClientId(2);
    pub(crate) const CLIENT_ID_3: ClientId = ClientId(3);
    pub(crate) const CLIENT_ID_4: ClientId = ClientId(4);
    pub(crate) const CLIENT_ID_5: ClientId = ClientId(5);

    /// Creates a new client with memberships to the given groups.
    pub(crate) fn new_fake_client<F: ProtocolFamily>(
        id: ClientId,
        group_memberships: impl IntoIterator<Item = ModernGroup>,
    ) -> (
        FakeSenderSink<F::Response>,
        InternalClient<F, FakeSender<F::Response>>,
        impl Future<Output = ()>,
    ) {
        let (sender, sender_sink) = crate::messaging::testutil::fake_sender_with_sink();
        let (async_work_sink, mut async_work_receiver) = futures::channel::mpsc::unbounded();

        let (external_client, internal_client) = new_client_pair(id, sender, async_work_sink);
        for group in group_memberships {
            let waiter = external_client.add_membership(group).expect("add group membership");
            assert!(waiter.is_noop(), "should not use this helper for incurring blocking work");
        }

        let async_work_drain_task = async move {
            let work = async_work_receiver.next().await;
            assert_matches!(work, None, "should not receive any async work: {work:?}");
        };

        (sender_sink, internal_client, async_work_drain_task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use test_case::test_case;

    use crate::messaging::testutil::{SentMessage, fake_sender_with_sink};
    use crate::protocol_family::testutil::{
        FakeProtocolFamily, MODERN_GROUP_NEEDS_BLOCKING, MODERN_GROUP1, MODERN_GROUP2,
        new_fake_netlink_message,
    };

    impl<F: ProtocolFamily, S: Sender<F::Response>> ClientTable<F, S> {
        /// Test helper to return the IDs of clients in this [`ClientTable`].
        pub(crate) fn client_ids(&self) -> Vec<ClientId> {
            let mut ids = self.clients.lock().keys().cloned().collect::<Vec<_>>();
            ids.sort();
            ids
        }
    }

    fn closed_async_work_sink<F: ProtocolFamily>()
    -> futures::channel::mpsc::UnboundedSender<AsyncWorkItem<F>> {
        let (sender, _) = futures::channel::mpsc::unbounded();
        sender.close_channel();
        sender
    }

    // Verify that multicast group membership changes applied to the external
    // client are observed on the internal client.
    #[fuchsia::test(logging = false)]
    async fn test_group_memberships() {
        let (sender, _sink) = fake_sender_with_sink();
        let (async_work_sink, mut async_work_receiver) = futures::channel::mpsc::unbounded();
        let (external_client, internal_client) = new_client_pair::<FakeProtocolFamily, _>(
            testutil::CLIENT_ID_1,
            sender,
            async_work_sink,
        );

        assert!(!internal_client.member_of_group(MODERN_GROUP1));
        assert!(!internal_client.member_of_group(MODERN_GROUP2));

        // Add one membership and verify the other is unaffected.
        let waiter =
            external_client.add_membership(MODERN_GROUP1).expect("failed to add membership");
        assert!(waiter.is_noop(), "should not need to block on async work");
        assert!(internal_client.member_of_group(MODERN_GROUP1));
        assert!(!internal_client.member_of_group(MODERN_GROUP2));
        // Add the second membership.
        let waiter =
            external_client.add_membership(MODERN_GROUP2).expect("failed to add membership");
        assert!(waiter.is_noop(), "should not need to block on async work");
        assert!(internal_client.member_of_group(MODERN_GROUP1));
        assert!(internal_client.member_of_group(MODERN_GROUP2));
        // Delete the first membership and verify the other is unaffected.
        external_client.del_membership(MODERN_GROUP1).expect("failed to del membership");
        assert!(!internal_client.member_of_group(MODERN_GROUP1));
        assert!(internal_client.member_of_group(MODERN_GROUP2));
        // Delete the second membership.
        external_client.del_membership(MODERN_GROUP2).expect("failed to del membership");
        assert!(!internal_client.member_of_group(MODERN_GROUP1));
        assert!(!internal_client.member_of_group(MODERN_GROUP2));

        let waiter = external_client
            .add_membership(MODERN_GROUP_NEEDS_BLOCKING)
            .expect("add should succeed");
        let work = async_work_receiver.next().await.expect("should yield work");
        match work {
            AsyncWorkItem::OnJoinMulticastGroup((), sender) => {
                sender.send(());
            }
            item @ _ => panic!("should be join, not {item:?}"),
        }
        waiter.assert_blocking_and_wait_until_complete();

        assert!(
            external_client
                .add_membership(MODERN_GROUP_NEEDS_BLOCKING)
                .expect("add should succeed")
                .is_noop(),
            "should be no-op"
        );

        external_client.del_membership(MODERN_GROUP_NEEDS_BLOCKING).expect("add should succeed");
        let work = async_work_receiver.next().await.expect("should yield work");
        match work {
            AsyncWorkItem::OnLeaveMulticastGroup(groups) => {
                assert_matches!(groups, OneOrMany::One(()));
            }
            item @ _ => panic!("should be leave, not {item:?}"),
        }

        drop(async_work_receiver);

        // Deleting the membership again shouldn't panic even though the async
        // work receiver has been dropped, as we're already not in the group (so
        // no additional work needs to be done).
        external_client.del_membership(MODERN_GROUP_NEEDS_BLOCKING).expect("add should succeed");
    }

    #[fuchsia::test(logging = false)]
    async fn test_send_message_to_group() {
        let clients = ClientTable::default();
        let scope = fasync::Scope::new();
        let (mut sink_group1, client_group1, async_work_drain_task) =
            testutil::new_fake_client::<FakeProtocolFamily>(testutil::CLIENT_ID_1, [MODERN_GROUP1]);
        let _join_handle = scope.spawn(async_work_drain_task);
        let (mut sink_group2, client_group2, async_work_drain_task) =
            testutil::new_fake_client::<FakeProtocolFamily>(testutil::CLIENT_ID_2, [MODERN_GROUP2]);
        let _join_handle = scope.spawn(async_work_drain_task);
        let (mut sink_both_groups, client_both_groups, async_work_drain_task) =
            testutil::new_fake_client::<FakeProtocolFamily>(
                testutil::CLIENT_ID_3,
                [MODERN_GROUP1, MODERN_GROUP2],
            );
        let _join_handle = scope.spawn(async_work_drain_task);
        clients.add_client(client_group1);
        clients.add_client(client_group2);
        clients.add_client(client_both_groups);

        assert_eq!(&sink_group1.take_messages()[..], &[]);
        assert_eq!(&sink_group2.take_messages()[..], &[]);
        assert_eq!(&sink_both_groups.take_messages()[..], &[]);

        clients.send_message_to_group(new_fake_netlink_message(), MODERN_GROUP1);
        assert_eq!(
            &sink_group1.take_messages()[..],
            &[SentMessage::multicast(new_fake_netlink_message(), MODERN_GROUP1)]
        );
        assert_eq!(&sink_group2.take_messages()[..], &[]);
        assert_eq!(
            &sink_both_groups.take_messages()[..],
            &[SentMessage::multicast(new_fake_netlink_message(), MODERN_GROUP1)]
        );

        clients.send_message_to_group(new_fake_netlink_message(), MODERN_GROUP2);
        assert_eq!(&sink_group1.take_messages()[..], &[]);
        assert_eq!(
            &sink_group2.take_messages()[..],
            &[SentMessage::multicast(new_fake_netlink_message(), MODERN_GROUP2)]
        );
        assert_eq!(
            &sink_both_groups.take_messages()[..],
            &[SentMessage::multicast(new_fake_netlink_message(), MODERN_GROUP2)]
        );
        drop(clients);
        scope.join().await;
    }

    #[test]
    fn test_client_id_generator() {
        let generator = ClientIdGenerator::default();
        const NUM_IDS: u32 = 1000;
        let mut ids = HashSet::new();
        for _ in 0..NUM_IDS {
            // `insert` returns false if the ID is already present in the set,
            // indicating that the generator returned a non-unique ID.
            assert!(ids.insert(generator.new_id()));
        }
    }

    #[test_case(0; "pid_0")]
    #[test_case(1; "pid_1")]
    fn test_set_port_number(port_number: u32) {
        let (sender, mut sink) = fake_sender_with_sink();
        let (external_client, mut internal_client) = new_client_pair::<FakeProtocolFamily, _>(
            testutil::CLIENT_ID_1,
            sender,
            closed_async_work_sink(),
        );

        if let Some(port_number) = NonZeroU32::new(port_number) {
            external_client.set_port_number(port_number)
        }

        let message = new_fake_netlink_message();
        assert_eq!(message.header.port_number, 0);
        internal_client.send_unicast(message);

        assert_matches!(
            &sink.take_messages()[..],
            [SentMessage { message, group }] => {
                assert_eq!(message.header.port_number, port_number);
                assert_eq!(*group, None);
            }
        )
    }
}
