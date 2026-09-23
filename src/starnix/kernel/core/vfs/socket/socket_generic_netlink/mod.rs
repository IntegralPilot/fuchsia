// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use futures::FutureExt;
use futures::channel::mpsc;
use futures::future::join;
use futures::stream::{FuturesUnordered, StreamExt};
use linux_uapi::{NLM_F_ACK, NLM_F_CAPPED};
use netlink::NETLINK_LOG_TAG;
use netlink::messaging::Sender;
use netlink::multicast_groups::ModernGroup;
use netlink_packet_core::{
    ErrorMessage, NETLINK_HEADER_LEN, NetlinkHeader, NetlinkMessage, NetlinkPayload,
};
use netlink_packet_generic::constants::GENL_ID_CTRL;
use netlink_packet_generic::ctrl::nlas::{GenlCtrlAttrs, McastGrpAttrs};
use netlink_packet_generic::ctrl::{GenlCtrl, GenlCtrlCmd};
use netlink_packet_generic::{GenlHeader, GenlMessage};
use netlink_packet_utils::Emitable;
use starnix_logging::track_stub;
use starnix_sync::{GenericNetlinkServerStateLock, LockDepMutex};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;
use std::ops::DerefMut;
use std::sync::Arc;

use starnix_logging::{log_error, log_info, log_warn};
use starnix_uapi::errors::Errno;
use starnix_uapi::{ENOENT, error};

mod messages;
mod nl80211;
mod taskstats;

pub use messages::GenericMessage;

const MIN_FAMILY_ID: u16 = GENL_ID_CTRL + 1;
const NLCTRL_FAMILY: &str = "nlctrl";

#[async_trait]
pub trait GenericNetlinkFamily<S>: Send + Sync {
    /// Return the unique name for this generic netlink protocol.
    ///
    /// This name is used by the ctrl server to identify this server for clients.
    fn name(&self) -> String;

    /// Return the multicast groups that are supported by this protocol family.
    ///
    /// Each multicast group is assigned a unique ID by the ctrl server.
    fn multicast_groups(&self) -> Vec<String> {
        vec![]
    }

    /// Returns a future that pipes messages for the given multicast group into
    /// the given sink. The parent of this server is responsible for managing
    /// multicast group memberships and routing these messages appropriately.
    /// The assigned family id of this multicast group is passed in to be used
    /// appropriately when constructing messages.
    async fn stream_multicast_messages(
        &self,
        group: String,
        assigned_family_id: u16,
        message_sink: mpsc::UnboundedSender<NetlinkMessage<GenericMessage>>,
    );

    /// Handle a netlink message targeted to this server.
    ///
    /// The given payload contains the generic netlink header and all subsequent data.
    /// Protocol servers should implement their own generic netlink families and
    /// deserialize messages using `GenlMessage::<_>::deserialize`.
    async fn handle_message(&self, netlink_header: NetlinkHeader, payload: Vec<u8>, sender: &mut S);
}

fn extract_family_names(genl_ctrl: GenlCtrl) -> Vec<String> {
    genl_ctrl
        .nlas
        .into_iter()
        .filter_map(
            |attr| {
                if let GenlCtrlAttrs::FamilyName(name) = attr { Some(name) } else { None }
            },
        )
        .collect()
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct ClientId(u64);

/// All state required to the generic netlink server. This struct assumes
/// synchronous access, and should be kept inside of a GenericNetlinkServer.
struct GenericNetlinkServerState<S> {
    /// Mapping from generic family server name to its assigned ID value.
    family_ids: HashMap<String, u16>,
    /// Servers for specific generic netlink families. Servers are stored in this
    /// list by order of ID, such that protocol N is in servers[N - MIN_FAMILY_ID].
    families: Vec<Arc<dyn GenericNetlinkFamily<S>>>,
    /// Sink for new families to setup multicast group handling.
    new_family_sender: mpsc::UnboundedSender<Arc<dyn GenericNetlinkFamily<S>>>,
    /// Multicast groups, identified by (family name, group name). Multicast
    /// group IDs are assigned uniquely across all generic families.
    multicast_groups: HashMap<(String, String), ModernGroup>,
    /// Counter used to generate unique ID values for all multicast groups.
    multicast_group_id_counter: ModernGroup,
    /// Unique internal IDs used to track clients.
    client_id_counter: ClientId,
    /// Senders for passing multicast traffic to clients.
    client_senders: HashMap<ClientId, S>,
    /// Mapping from multicast group -> list of subscribed client IDs.
    multicast_group_memberships: HashMap<ModernGroup, HashSet<ClientId>>,
}

impl<S: Sender<GenericMessage>> GenericNetlinkServerState<S> {
    fn new(new_family_sender: mpsc::UnboundedSender<Arc<dyn GenericNetlinkFamily<S>>>) -> Self {
        Self {
            family_ids: HashMap::new(),
            families: vec![],
            new_family_sender,
            multicast_groups: HashMap::new(),
            multicast_group_id_counter: ModernGroup(0),
            client_id_counter: ClientId(0),
            client_senders: HashMap::new(),
            multicast_group_memberships: HashMap::new(),
        }
    }

    fn add_family(&mut self, family: Arc<dyn GenericNetlinkFamily<S>>) {
        match (self.families.len() as u16).checked_add(MIN_FAMILY_ID) {
            Some(new_family_id) => {
                self.family_ids.insert(family.name(), new_family_id);
                if let Err(e) = self.new_family_sender.unbounded_send(Arc::clone(&family)) {
                    log_error!(
                        tag = NETLINK_LOG_TAG;
                        "Failed to setup multicast group handling for new generic \
                         netlink family {}: {}",
                        family.name(),
                        e
                    );
                }
                self.families.push(family);
            }
            None => {
                log_error!(
                    tag = NETLINK_LOG_TAG;
                    "Failed to add generic netlink family: too many families"
                );
            }
        }
    }

    fn get_family(&self, family_id: u16) -> Option<Arc<dyn GenericNetlinkFamily<S>>> {
        if family_id >= MIN_FAMILY_ID
            && ((family_id - MIN_FAMILY_ID) as usize) < self.families.len()
        {
            Some(Arc::clone(&self.families[(family_id - MIN_FAMILY_ID) as usize]))
        } else {
            None
        }
    }

    fn get_multicast_group_id(&mut self, family: String, group: String) -> ModernGroup {
        *self.multicast_groups.entry((family, group)).or_insert_with(|| {
            self.multicast_group_id_counter.0 += 1;
            self.multicast_group_id_counter
        })
    }

    fn handle_ctrl_message(
        &mut self,
        mut netlink_header: NetlinkHeader,
        genl_message: GenlMessage<GenlCtrl>,
        sender: &mut S,
    ) {
        let (genl_header, genl_ctrl) = genl_message.into_parts();
        match genl_ctrl.cmd {
            GenlCtrlCmd::GetFamily => {
                let family_names = extract_family_names(genl_ctrl);
                log_info!(tag = NETLINK_LOG_TAG; "Netlink GetFamily request: {:?}", family_names);

                for family in &family_names {
                    if family == NLCTRL_FAMILY {
                        self.send_get_family_response(
                            netlink_header,
                            genl_header,
                            NLCTRL_FAMILY,
                            GENL_ID_CTRL,
                            None,
                            sender,
                        );
                    } else if let Some(id) = self.family_ids.get(family).copied() {
                        log_info!(
                            tag = NETLINK_LOG_TAG;
                            "Serving requested netlink family {}",
                            family
                        );
                        let mcast_groups = self
                            .get_family(id)
                            .expect("Known family ID should always exist")
                            .multicast_groups()
                            .into_iter()
                            .map(|name| {
                                vec![
                                    McastGrpAttrs::Name(name.clone()),
                                    McastGrpAttrs::Id(
                                        self.get_multicast_group_id(family.to_string(), name).0,
                                    ),
                                ]
                            })
                            .collect();
                        self.send_get_family_response(
                            netlink_header,
                            genl_header,
                            family,
                            id,
                            Some(mcast_groups),
                            sender,
                        );
                    } else {
                        log_warn!(
                            tag = NETLINK_LOG_TAG;
                            "Cannot serve requested netlink family {}",
                            family
                        );

                        // Send back error message
                        let mut buffer = [0; NETLINK_HEADER_LEN];
                        netlink_header.emit(&mut buffer[..NETLINK_HEADER_LEN]);
                        let mut error = ErrorMessage::default();
                        error.code = NonZero::new(-(ENOENT as i32));
                        error.header = buffer.to_vec();
                        netlink_header.flags = NLM_F_CAPPED as u16;
                        let mut netlink_message =
                            NetlinkMessage::new(netlink_header, NetlinkPayload::Error(error));
                        netlink_message.finalize();
                        sender.send(netlink_message, None);
                    }
                }
            }
            GenlCtrlCmd::NewFamily => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlNewFamily")
            }
            GenlCtrlCmd::DelFamily => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlDelFamily")
            }
            GenlCtrlCmd::NewOps => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlNewOps")
            }
            GenlCtrlCmd::DelOps => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlDelOps")
            }
            GenlCtrlCmd::GetOps => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlGetOps")
            }
            GenlCtrlCmd::NewMcastGrp => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlNewMcastGrp")
            }
            GenlCtrlCmd::DelMcastGrp => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlDelMcastGrp")
            }
            GenlCtrlCmd::GetMcastGrp => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlGetMcastGrp")
            }
            GenlCtrlCmd::GetPolicy => {
                track_stub!(TODO("https://fxbug.dev/297431602"), "NetlinkCtrlGetPolicy")
            }
        }
    }

    fn send_get_family_response(
        &mut self,
        mut netlink_header: NetlinkHeader,
        genl_header: GenlHeader,
        family: &str,
        id: u16,
        mcast_groups: Option<Vec<Vec<McastGrpAttrs>>>,
        sender: &mut S,
    ) {
        let mut nlas =
            vec![GenlCtrlAttrs::FamilyId(id), GenlCtrlAttrs::FamilyName(family.to_string())];
        mcast_groups.map(|mg| nlas.push(GenlCtrlAttrs::McastGroups(mg)));
        let resp_ctrl = GenlCtrl { cmd: GenlCtrlCmd::NewFamily, nlas };
        // Flags need to be cleared as we are sending a response back
        // to the client, not requesting that the client send us
        // data or ACKs.
        let orig_flags = netlink_header.flags;
        netlink_header.flags = 0;
        let mut genl_message = GenlMessage::from_parts(genl_header, resp_ctrl);
        genl_message.finalize();
        let mut message = NetlinkMessage::new(
            netlink_header,
            NetlinkPayload::InnerMessage(GenericMessage::Ctrl(genl_message)),
        );
        message.finalize();
        sender.send(message, None);
        // Conversion is safe because 4 < 65535.
        if orig_flags & NLM_F_ACK as u16 != 0 {
            // ACK requested, send ACK
            let mut buffer = [0; NETLINK_HEADER_LEN];
            // Conversion is safe because 256 < 65535
            netlink_header.flags = NLM_F_CAPPED as u16;
            netlink_header.emit(&mut buffer[..NETLINK_HEADER_LEN]);
            let mut ack = ErrorMessage::default();
            // Netlink uses an error payload with no error code to indicate a
            // successful ack.
            ack.code = None;
            ack.header = buffer.to_vec();
            let mut netlink_message =
                NetlinkMessage::new(netlink_header, NetlinkPayload::Error(ack));
            netlink_message.finalize();
            sender.send(netlink_message, None);
        }
    }
}

/// Coordinates all generic netlink clients and families.
#[derive(Clone)]
struct GenericNetlinkServer<S> {
    state: Arc<LockDepMutex<GenericNetlinkServerState<S>, GenericNetlinkServerStateLock>>,
}

impl<S: Sender<GenericMessage>> GenericNetlinkServer<S> {
    fn new(new_family_sender: mpsc::UnboundedSender<Arc<dyn GenericNetlinkFamily<S>>>) -> Self {
        Self { state: Arc::new(GenericNetlinkServerState::new(new_family_sender).into()) }
    }

    async fn handle_generic_message(
        &self,
        message: NetlinkMessage<GenericMessage>,
        sender: &mut S,
    ) {
        let (netlink_header, payload) = message.into_parts();
        let req = match payload {
            NetlinkPayload::InnerMessage(p) => p,
            p => {
                log_error!(tag = NETLINK_LOG_TAG; "Dropping unexpected netlink payload: {:?}", p);
                return;
            }
        };
        match req {
            GenericMessage::Ctrl(ctrl_message) => {
                self.state.lock().handle_ctrl_message(netlink_header, ctrl_message, sender)
            }
            GenericMessage::Other { family: family_id, payload } => {
                let family = self.state.lock().get_family(family_id);
                match family {
                    Some(family) => {
                        family.handle_message(netlink_header, payload, sender).await;
                    }
                    None => log_info!(
                        tag = NETLINK_LOG_TAG;
                        "Ignoring generic netlink message with unsupported family {}",
                        family_id,
                    ),
                }
            }
        }
    }

    async fn run_generic_netlink_client(self, mut client: GenericNetlinkClient<S>) {
        log_info!(tag = NETLINK_LOG_TAG; "Registered new generic netlink client");
        loop {
            match client.receiver.next().await {
                Some(message) => self.handle_generic_message(message, &mut client.sender).await,
                None => {
                    log_info!(tag = NETLINK_LOG_TAG; "Generic netlink client exited");
                    let mut state = self.state.lock();
                    for memberships in state.multicast_group_memberships.values_mut() {
                        memberships.remove(&client.client_id);
                    }
                    state.client_senders.remove(&client.client_id);
                    return;
                }
            }
        }
    }

    async fn pipe_single_multicast_group(
        &self,
        mcast_group_id: ModernGroup,
        mcast_stream: mpsc::UnboundedReceiver<NetlinkMessage<GenericMessage>>,
    ) {
        if self
            .state
            .lock()
            .multicast_group_memberships
            .insert(mcast_group_id, HashSet::new())
            .is_some()
        {
            log_error!(
                tag = NETLINK_LOG_TAG;
                "pipe_single_multicast_group called on group {} but group is already served",
                mcast_group_id.0
            );
            return;
        }
        let fut = mcast_stream.for_each(|mcast_message| {
            let mut state_lock = self.state.lock();
            let state = state_lock.deref_mut();
            for client_id in state
                .multicast_group_memberships
                .get(&mcast_group_id)
                .expect("Group memberships should always be present")
            {
                if let Some(sender) = state.client_senders.get_mut(client_id) {
                    sender.send(mcast_message.clone(), Some(mcast_group_id));
                }
            }
            async {}
        });
        fut.await;
    }

    async fn pipe_multicast_traffic_for_family(self, family: Arc<dyn GenericNetlinkFamily<S>>) {
        let unordered = FuturesUnordered::new();
        let family_name = family.name().to_string();
        let family_id =
            *self.state.lock().family_ids.get(&family_name).expect("Failed to get family id");
        for mcast_group in family.multicast_groups() {
            let mcast_group_id =
                self.state.lock().get_multicast_group_id(family_name.clone(), mcast_group.clone());
            let (sink, receiver) = mpsc::unbounded();
            unordered.push(family.stream_multicast_messages(mcast_group, family_id, sink));
            unordered.push(Box::pin(self.pipe_single_multicast_group(mcast_group_id, receiver)));
        }
        unordered.collect::<Vec<()>>().await;
    }
}

pub(crate) struct GenericNetlinkClient<S> {
    client_id: ClientId,
    sender: S,
    receiver: mpsc::UnboundedReceiver<NetlinkMessage<GenericMessage>>,
}

pub struct GenericNetlinkWorkerParams<S: Sender<GenericMessage>> {
    server: GenericNetlinkServer<S>,
    new_client_receiver: mpsc::UnboundedReceiver<GenericNetlinkClient<S>>,
    new_family_receiver: mpsc::UnboundedReceiver<Arc<dyn GenericNetlinkFamily<S>>>,
}

pub async fn run_generic_netlink_worker<S: Sender<GenericMessage>>(
    params: GenericNetlinkWorkerParams<S>,
    enable_nl80211: bool,
) {
    // Initialize supported families on the worker, so that they shares an
    // executor with the main netlink future.
    let nl80211_family = if enable_nl80211 {
        // This boolean is tied to availability of the Wlanix protocol, so this
        // operation will always succeed unless our product config is invalid.
        Some(nl80211::Nl80211Family::new().expect("Failed to connect to Nl80211 netlink family"))
    } else {
        None
    };
    let taskstats_family = taskstats::TaskstatsFamily::new();
    {
        let mut state = params.server.state.lock();
        if let Some(nl80211_family) = nl80211_family {
            state.add_family(Arc::new(nl80211_family));
        }
        state.add_family(Arc::new(taskstats_family));
    }

    run_generic_netlink_worker_internal(params).await
}

fn run_generic_netlink_worker_internal<S: Sender<GenericMessage>>(
    params: GenericNetlinkWorkerParams<S>,
) -> impl std::future::Future<Output = ()> + Send {
    let GenericNetlinkWorkerParams { server, new_client_receiver, new_family_receiver } = params;

    let server_clone = server.clone();
    let multicast_fut = new_family_receiver.for_each_concurrent(None, move |family| {
        server_clone.clone().pipe_multicast_traffic_for_family(family)
    });
    let new_client_fut = new_client_receiver
        .for_each_concurrent(None, move |client| server.clone().run_generic_netlink_client(client));

    join(new_client_fut, multicast_fut).map(|_| ())
}

pub struct GenericNetlink<S> {
    server: GenericNetlinkServer<S>,
    new_client_sender: mpsc::UnboundedSender<GenericNetlinkClient<S>>,
}

impl<S: Sender<GenericMessage>> GenericNetlink<S> {
    pub fn new() -> (Self, GenericNetlinkWorkerParams<S>) {
        let (new_client_sender, new_client_receiver) = mpsc::unbounded();
        let (new_family_sender, new_family_receiver) = mpsc::unbounded();
        let server = GenericNetlinkServer::new(new_family_sender);
        let generic_netlink = Self { server: server.clone(), new_client_sender };
        let worker_params =
            GenericNetlinkWorkerParams { server, new_client_receiver, new_family_receiver };
        (generic_netlink, worker_params)
    }

    pub fn new_generic_client(
        &self,
        sender: S,
        receiver: mpsc::UnboundedReceiver<NetlinkMessage<GenericMessage>>,
    ) -> Result<GenericNetlinkClientHandle<S>, anyhow::Error> {
        let mut state = self.server.state.lock();
        let client_id = state.client_id_counter;
        state.client_id_counter.0 += 1;
        state.client_senders.insert(client_id, sender.clone());
        let handle = GenericNetlinkClientHandle { client_id, server: self.server.clone() };
        let new_client = GenericNetlinkClient { client_id, sender, receiver };
        self.new_client_sender
            .unbounded_send(new_client)
            .map_err(|_| anyhow::anyhow!("Failed to connect a new generic netlink client"))?;
        Ok(handle)
    }
}

impl<S: Sender<GenericMessage>> GenericNetlink<S> {
    pub fn add_family(&self, family: Arc<dyn GenericNetlinkFamily<S>>) {
        self.server.state.lock().add_family(family)
    }
}

pub struct GenericNetlinkClientHandle<S> {
    client_id: ClientId,
    server: GenericNetlinkServer<S>,
}

impl<S> GenericNetlinkClientHandle<S> {
    pub(crate) fn add_membership(&self, group_id: ModernGroup) -> Result<(), Errno> {
        let mut state = self.server.state.lock();
        if let Some(memberships) = state.multicast_group_memberships.get_mut(&group_id) {
            memberships.insert(self.client_id);
            Ok(())
        } else {
            error!(EINVAL)
        }
    }
}

#[cfg(test)]
mod test_utils {
    use super::*;
    use netlink_packet_core::NetlinkSerializable;

    #[derive(Clone)]
    pub(crate) struct TestSender<M> {
        pub messages: Arc<starnix_sync::Mutex<Vec<NetlinkMessage<M>>>>,
    }

    impl<M: Clone + NetlinkSerializable + Send + Sync> Sender<M> for TestSender<M> {
        fn send(&mut self, message: NetlinkMessage<M>, _group: Option<ModernGroup>) {
            self.messages.lock().push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_utils::*;
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async::TestExecutor;
    use fuchsia_rcu::RcuReadScope;
    use futures::future::Future;
    use futures::pin_mut;
    use netlink_packet_generic::GenlHeader;
    use starnix_rcu::RcuHashMap;
    use std::task::Poll;

    const TEST_FAMILY: &str = "test_family";
    const MCAST_GROUP_1: &str = "m1";
    const MCAST_GROUP_2: &str = "m2";

    fn getfamily_request() -> NetlinkMessage<GenericMessage> {
        let getfamily_ctrl = GenlCtrl {
            cmd: GenlCtrlCmd::GetFamily,
            nlas: vec![GenlCtrlAttrs::FamilyName(TEST_FAMILY.to_string())],
        };
        let mut genl_message =
            GenlMessage::new(GenlHeader { cmd: 0, version: 0 }, getfamily_ctrl, GENL_ID_CTRL);
        genl_message.finalize();
        let mut netlink_message = NetlinkMessage::new(
            Default::default(),
            NetlinkPayload::InnerMessage(GenericMessage::Ctrl(genl_message)),
        );
        netlink_message.finalize();
        netlink_message
    }

    struct TestFamily {
        messages_to_server: starnix_sync::Mutex<Vec<Vec<u8>>>,
        multicast_message_sinks: RcuHashMap<
            String,
            mpsc::UnboundedSender<NetlinkMessage<GenericMessage>>,
            std::collections::hash_map::RandomState,
        >,
    }

    impl Default for TestFamily {
        fn default() -> Self {
            Self {
                messages_to_server: starnix_sync::Mutex::default(),
                multicast_message_sinks: RcuHashMap::with_hasher(
                    std::collections::hash_map::RandomState::new(),
                ),
            }
        }
    }

    #[async_trait]
    impl<S> GenericNetlinkFamily<S> for TestFamily {
        fn name(&self) -> String {
            TEST_FAMILY.into()
        }

        fn multicast_groups(&self) -> Vec<String> {
            vec![MCAST_GROUP_1.to_string(), MCAST_GROUP_2.to_string()]
        }

        async fn stream_multicast_messages(
            &self,
            group: String,
            _assigned_family_id: u16,
            message_sink: mpsc::UnboundedSender<NetlinkMessage<GenericMessage>>,
        ) {
            self.multicast_message_sinks.insert(group, message_sink);
        }

        async fn handle_message(
            &self,
            _netlink_header: NetlinkHeader,
            payload: Vec<u8>,
            _sender: &mut S,
        ) {
            self.messages_to_server.lock().push(payload);
        }
    }

    fn start_test_netlink()
    -> (GenericNetlink<TestSender<GenericMessage>>, impl Future<Output = ()> + Send) {
        let (netlink, worker_params) = GenericNetlink::new();
        let worker = run_generic_netlink_worker_internal(worker_params);
        (netlink, worker)
    }

    fn netlink_with_test_family() -> (
        GenericNetlink<TestSender<GenericMessage>>,
        Arc<TestFamily>,
        impl Future<Output = ()> + Send,
    ) {
        let test_family = Arc::new(TestFamily::default());
        let (netlink, worker) = start_test_netlink();
        netlink.server.state.lock().add_family(Arc::clone(&test_family) as _);
        (netlink, test_family, worker)
    }

    fn new_client(
        netlink: &GenericNetlink<TestSender<GenericMessage>>,
    ) -> (
        Arc<starnix_sync::Mutex<Vec<NetlinkMessage<GenericMessage>>>>,
        mpsc::UnboundedSender<NetlinkMessage<GenericMessage>>,
        GenericNetlinkClientHandle<TestSender<GenericMessage>>,
    ) {
        let messages_to_client = Arc::new(starnix_sync::Mutex::new(vec![]));
        let (netlink_sender, receiver) = mpsc::unbounded();
        let sender = TestSender { messages: messages_to_client.clone() };
        let client_handle =
            netlink.new_generic_client(sender, receiver).expect("Failed to add new generic client");
        (messages_to_client, netlink_sender, client_handle)
    }

    #[test]
    fn test_ctrl_getfamily_missing() {
        let mut exec = TestExecutor::new();
        let (netlink, worker) = start_test_netlink();
        pin_mut!(worker);
        let (messages_to_client, sender, _client_handle) = new_client(&netlink);

        sender.unbounded_send(getfamily_request()).expect("Failed to send getfamily request");
        assert!(exec.run_until_stalled(&mut worker) == Poll::Pending);

        // The family doesn't exist, so an error should be returned.
        assert!(messages_to_client.lock().len() == 1);

        let (_netlink_header, payload) = messages_to_client.lock().pop().unwrap().into_parts();
        let err_msg = assert_matches!(payload, NetlinkPayload::Error(m) => m);
        assert_eq!(err_msg.code, NonZero::new(-2));
    }

    #[test]
    fn test_ctrl_getfamily() {
        let mut exec = TestExecutor::new();
        let (netlink, _test_family, fut) = netlink_with_test_family();
        pin_mut!(fut);
        let (messages_to_client, sender, _client_handle) = new_client(&netlink);

        sender.unbounded_send(getfamily_request()).expect("Failed to send getfamily request");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        // Verify that we got all expected information in the response.
        assert!(messages_to_client.lock().len() == 1);
        let (_netlink_header, payload) = messages_to_client.lock().pop().unwrap().into_parts();
        let (_genl_header, ctrl_payload) = assert_matches!(
            payload,
            NetlinkPayload::InnerMessage(GenericMessage::Ctrl(m)) => m.into_parts());
        assert_eq!(ctrl_payload.cmd, GenlCtrlCmd::NewFamily);
        assert!(
            ctrl_payload
                .nlas
                .iter()
                .any(|nla| *nla == GenlCtrlAttrs::FamilyName(TEST_FAMILY.into()))
        );
        assert!(ctrl_payload.nlas.iter().any(|nla| matches!(nla, GenlCtrlAttrs::FamilyId(_))));
        let multicast_groups = ctrl_payload
            .nlas
            .iter()
            .filter_map(
                |nla| if let GenlCtrlAttrs::McastGroups(vec) = nla { Some(vec) } else { None },
            )
            .next()
            .expect("No multicast groups");
        assert_eq!(multicast_groups.len(), 2);
        assert!(multicast_groups.iter().any(|group| {
            group.iter().any(|attr| matches!(attr, McastGrpAttrs::Id(_)));
            group.iter().any(|attr| *attr == McastGrpAttrs::Name(MCAST_GROUP_1.into()))
        }));
        assert!(multicast_groups.iter().any(|group| {
            group.iter().any(|attr| matches!(attr, McastGrpAttrs::Id(_)));
            group.iter().any(|attr| *attr == McastGrpAttrs::Name(MCAST_GROUP_2.into()))
        }));
    }

    #[test]
    fn test_ctrl_getfamily_before_and_after_add_family() {
        let mut exec = TestExecutor::new();
        let (netlink, fut) = start_test_netlink();
        pin_mut!(fut);
        let (messages_to_client, sender, _client_handle) = new_client(&netlink);

        sender.unbounded_send(getfamily_request()).expect("Failed to send getfamily request");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        // The family doesn't exist, so an error should be returned.
        assert!(messages_to_client.lock().len() == 1);

        let (_netlink_header, payload) = messages_to_client.lock().pop().unwrap().into_parts();
        let err_msg = assert_matches!(payload, NetlinkPayload::Error(m) => m);
        assert_eq!(err_msg.code, NonZero::new(-2));

        // Add the test family and try again.
        let test_family = Arc::new(TestFamily::default());
        netlink.add_family(test_family);

        sender.unbounded_send(getfamily_request()).expect("Failed to send getfamily request");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        // Verify that we got all expected information in the response.
        assert!(messages_to_client.lock().len() == 1);
        let (_netlink_header, payload) = messages_to_client.lock().pop().unwrap().into_parts();
        let (_genl_header, ctrl_payload) = assert_matches!(
            payload,
            NetlinkPayload::InnerMessage(GenericMessage::Ctrl(m)) => m.into_parts());
        assert_eq!(ctrl_payload.cmd, GenlCtrlCmd::NewFamily);
        assert!(
            ctrl_payload
                .nlas
                .iter()
                .any(|nla| *nla == GenlCtrlAttrs::FamilyName(TEST_FAMILY.into()))
        );
        assert!(ctrl_payload.nlas.iter().any(|nla| matches!(nla, GenlCtrlAttrs::FamilyId(_))));
        let multicast_groups = ctrl_payload
            .nlas
            .iter()
            .filter_map(
                |nla| if let GenlCtrlAttrs::McastGroups(vec) = nla { Some(vec) } else { None },
            )
            .next()
            .expect("No multicast groups");
        assert_eq!(multicast_groups.len(), 2);
        assert!(multicast_groups.iter().any(|group| {
            group.iter().any(|attr| matches!(attr, McastGrpAttrs::Id(_)));
            group.iter().any(|attr| *attr == McastGrpAttrs::Name(MCAST_GROUP_1.into()))
        }));
        assert!(multicast_groups.iter().any(|group| {
            group.iter().any(|attr| matches!(attr, McastGrpAttrs::Id(_)));
            group.iter().any(|attr| *attr == McastGrpAttrs::Name(MCAST_GROUP_2.into()))
        }));
    }

    #[test]
    fn test_send_family_message() {
        let mut exec = TestExecutor::new();
        let (netlink, test_family, fut) = netlink_with_test_family();
        pin_mut!(fut);
        let (messages_to_client, sender, _client_handle) = new_client(&netlink);

        sender.unbounded_send(getfamily_request()).expect("Failed to send getfamily request");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        assert!(messages_to_client.lock().len() == 1);
        let (_netlink_header, payload) = messages_to_client.lock().pop().unwrap().into_parts();
        let (_genl_header, ctrl_payload) = assert_matches!(
            payload,
            NetlinkPayload::InnerMessage(GenericMessage::Ctrl(m)) => m.into_parts());
        let family_id = *ctrl_payload
            .nlas
            .iter()
            .filter_map(|nla| if let GenlCtrlAttrs::FamilyId(id) = nla { Some(id) } else { None })
            .next()
            .expect("Could not find family id");

        let mut netlink_message = NetlinkMessage::new(
            Default::default(),
            NetlinkPayload::InnerMessage(GenericMessage::Other {
                family: family_id,
                payload: vec![0, 1, 2, 3],
            }),
        );
        netlink_message.finalize();
        sender.unbounded_send(netlink_message).expect("Failed to send test family message");

        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);
        assert_eq!(test_family.messages_to_server.lock().len(), 1);
    }

    #[test]
    fn test_send_invalid_family_message() {
        let mut exec = TestExecutor::new();
        let (netlink, test_family, fut) = netlink_with_test_family();
        pin_mut!(fut);
        let (messages_to_client, sender, _client_handle) = new_client(&netlink);

        let mut netlink_message = NetlinkMessage::new(
            Default::default(),
            NetlinkPayload::InnerMessage(GenericMessage::Other {
                family: 1337,
                payload: vec![0, 1, 2, 3],
            }),
        );
        netlink_message.finalize();
        sender.unbounded_send(netlink_message).expect("Failed to send test family message");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);
        assert!(test_family.messages_to_server.lock().is_empty());
        assert!(messages_to_client.lock().is_empty());
    }

    #[test]
    fn test_server_gets_multicast_messages() {
        let mut exec = TestExecutor::new();
        let (_netlink, test_family, fut) = netlink_with_test_family();
        pin_mut!(fut);
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);
        let scope = RcuReadScope::new();
        assert_eq!(test_family.multicast_message_sinks.iter(&scope).count(), 2);
        assert!(test_family.multicast_message_sinks.get(&scope, MCAST_GROUP_1).is_some());
        assert!(test_family.multicast_message_sinks.get(&scope, MCAST_GROUP_2).is_some());
    }

    #[test]
    fn test_bad_multicast_subscription_fails() {
        let mut exec = TestExecutor::new();
        let (netlink, fut) = start_test_netlink();
        pin_mut!(fut);
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        let (_messages_to_client, _sender, client_handle) = new_client(&netlink);
        client_handle
            .add_membership(ModernGroup(1337))
            .expect_err("Should not be able to add invalid multicast membership");
    }

    #[test]
    fn test_multicast_subscriptions() {
        let mut exec = TestExecutor::new();
        let (netlink, test_family, fut) = netlink_with_test_family();
        pin_mut!(fut);
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);
        let (messages_to_client_1, _sender_1, client_handle_1) = new_client(&netlink);
        let (messages_to_client_2, _sender_2, client_handle_2) = new_client(&netlink);
        let (messages_to_client_3, _sender_3, _client_handle_3) = new_client(&netlink);

        let mcast_group_1_id = netlink
            .server
            .state
            .lock()
            .get_multicast_group_id(TEST_FAMILY.to_string(), MCAST_GROUP_1.to_string());
        client_handle_1.add_membership(mcast_group_1_id).expect("add_membership failed");
        client_handle_2.add_membership(mcast_group_1_id).expect("add_membership failed");

        assert!(messages_to_client_1.lock().is_empty());
        assert!(messages_to_client_2.lock().is_empty());
        assert!(messages_to_client_3.lock().is_empty());

        let message_sink = test_family
            .multicast_message_sinks
            .get(&RcuReadScope::new(), MCAST_GROUP_1)
            .expect("Failed to find multicast message sender")
            .clone();
        let netlink_message = NetlinkMessage::new(NetlinkHeader::default(), NetlinkPayload::Noop);
        message_sink.unbounded_send(netlink_message).expect("Failed to send message");
        assert!(exec.run_until_stalled(&mut fut) == Poll::Pending);

        // All subscribed clients receive the message.
        assert_eq!(messages_to_client_1.lock().len(), 1);
        assert_eq!(messages_to_client_2.lock().len(), 1);
        // Client 3 did not subscribe and should not receive the message.
        assert!(messages_to_client_3.lock().is_empty());
    }
}
