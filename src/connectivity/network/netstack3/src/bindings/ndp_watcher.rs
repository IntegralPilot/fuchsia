// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;
use std::fmt::Debug;
use std::task::{Poll, ready};

use derivative::Derivative;
use fidl::endpoints::Responder as _;
use futures::channel::mpsc;
use futures::{Future, SinkExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use log::{debug, error, info, warn};
use packet_formats::icmp::ndp::options::option_types as ndp_option_types;
use thiserror::Error;

use crate::bindings::devices::BindingId;
use crate::bindings::util::{ErrorLogExt, ResultExt as _};

use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_ndp::OptionType;
use fidl_fuchsia_net_ndp_ext as fnet_ndp_ext;

/// Possible errors when serving
/// `fuchsia.net.ndp/RouterAdvertisementOptionWatcherProvider`.
#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("worker is closed")]
    WorkerClosed(#[from] WorkerClosedError),
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
}

impl ErrorLogExt for Error {
    fn log_level(&self) -> log::Level {
        match self {
            Self::WorkerClosed(WorkerClosedError) => log::Level::Error,
            Self::Fidl(fidl) => fidl.log_level(),
        }
    }
}

pub(crate) async fn serve(
    stream: fnet_ndp::RouterAdvertisementOptionWatcherProviderRequestStream,
    sink: WorkerWatcherSink,
) -> Result<(), Error> {
    stream
        .err_into()
        .try_fold(sink, |mut sink, req| async move {
            let fnet_ndp::RouterAdvertisementOptionWatcherProviderRequest::
            NewRouterAdvertisementOptionWatcher {
                option_watcher, params, control_handle: _
            }  = req;
            let interest = match Interest::try_from(params) {
                Ok(interest) => interest,
                Err(InterfaceIdMustBeNonZeroError) => {
                    option_watcher.close_with_epitaph(zx::Status::INVALID_ARGS).unwrap_or_log(
                        "failed to write InvalidArgs epitaph for NDP option watcher",
                    );
                    return Ok(sink);
                }
            };
            sink.add_watcher(option_watcher.into_stream(), interest).await?;
            Ok(sink)
        })
        .map_ok(|_: WorkerWatcherSink| ())
        .await
}

/// The maximum events to buffer at server side before the client consumes them.
const MAX_EVENTS: usize = 3 * fnet_ndp::MAX_OPTION_BATCH_SIZE as usize;

mod batch {
    use super::*;

    /// A batch of events taken from the queue.
    ///
    /// Guaranteed to be non-empty.
    /// When dropped, removes a batch's worth of events from the queue.
    pub(crate) struct Batch<'a> {
        inner_events: &'a mut VecDeque<fnet_ndp::OptionWatchEntry>,
    }

    impl<'a> Batch<'a> {
        pub(crate) fn new(
            inner_events: &'a mut VecDeque<fnet_ndp::OptionWatchEntry>,
        ) -> Option<Self> {
            if inner_events.is_empty() { None } else { Some(Self { inner_events }) }
        }

        fn batch_len(&self) -> usize {
            self.inner_events.len().min(usize::from(fnet_ndp::MAX_OPTION_BATCH_SIZE))
        }

        pub(crate) fn as_slice(&mut self) -> &[fnet_ndp::OptionWatchEntry] {
            let len = self.batch_len();
            assert!(len > 0);
            &self.inner_events.make_contiguous()[..len]
        }
    }

    impl Drop for Batch<'_> {
        fn drop(&mut self) {
            let len = self.batch_len();
            let Self { inner_events } = self;
            let _ = inner_events.drain(..len);
        }
    }
}
use batch::Batch;

mod event_queue {
    use super::*;

    fn format_events_queue(
        events: &VecDeque<fnet_ndp::OptionWatchEntry>,
        formatter: &mut std::fmt::Formatter<'_>,
    ) -> Result<(), std::fmt::Error> {
        formatter.write_fmt(format_args!("<queue with {} watch entries>", events.len()))
    }

    /// A bounded queue of [`fnet_ndp_ext::OptionWatchEntry`] for
    /// `fuchsia.net.ndp/OptionWatcher` protocol.
    #[derive(Derivative)]
    #[derivative(Debug)]
    pub(crate) struct EventQueue {
        // NB: we must avoid deriving the standard Debug impl here since the
        // OptionWatchEntry can contain PII.
        #[derivative(Debug(format_with = "format_events_queue"))]
        events: VecDeque<fnet_ndp::OptionWatchEntry>,
        dropped: usize,
    }

    impl EventQueue {
        pub(crate) fn new() -> Self {
            Self { events: VecDeque::new(), dropped: 0 }
        }

        /// Adds a [`fnet_ndp_ext::OptionWatchEntry`] to the back of the queue.
        pub(crate) fn push(&mut self, event: fnet_ndp_ext::OptionWatchEntry) {
            let Self { events, dropped } = self;
            if events.len() >= MAX_EVENTS {
                *dropped += 1;
                let _ = events.pop_front();
            }
            events.push_back(event.into());
        }

        /// Removes a batch of entries from the queue.
        ///
        /// Returns the number of entries that were dropped due to queue fullness
        /// since the last time entries were taken from the queue.
        pub(crate) fn take_batch(&mut self) -> Option<(usize, Batch<'_>)> {
            let Self { events, dropped } = self;
            let batch = Batch::new(events)?;
            Some((std::mem::take(dropped), batch))
        }
    }
}
use event_queue::EventQueue;

/// Error returned when 0 was specified as an interface ID.
#[derive(Debug)]
pub(crate) struct InterfaceIdMustBeNonZeroError;

/// A watcher's interest in NDP options.
#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct Interest {
    #[derivative(Debug(format_with = "format_option_types_vec"))]
    types: Option<bit_vec::BitVec>,
    interface_id: Option<BindingId>,
}

impl Interest {
    fn new(
        option_types: Option<impl IntoIterator<Item = OptionType>>,
        interface_id: Option<BindingId>,
    ) -> Self {
        let types = option_types.map(|interest_types| {
            let mut types = bit_vec::BitVec::from_elem(usize::from(OptionType::MAX) + 1, false);
            types.shrink_to_fit();
            for interest_type in interest_types {
                types.set(interest_type.into(), true);
            }
            types
        });
        Interest { types, interface_id }
    }

    pub(crate) fn try_from(
        params: fnet_ndp::RouterAdvertisementOptionWatcherParams,
    ) -> Result<Self, InterfaceIdMustBeNonZeroError> {
        let fnet_ndp::RouterAdvertisementOptionWatcherParams {
            interest_types,
            interest_interface_id,
            __source_breaking: _,
        } = params;
        let interface_id = match interest_interface_id {
            None => None,
            Some(id) => Some(BindingId::new(id).ok_or(InterfaceIdMustBeNonZeroError)?),
        };
        Ok(Self::new(interest_types, interface_id))
    }

    fn interested(&self, interface_id: BindingId, option_type: OptionType) -> bool {
        let Self { types: interest_types, interface_id: interest_interface_id } = self;
        interest_interface_id.map(|id| id == interface_id).unwrap_or(true)
            && interest_types.as_ref().map(|types| types[option_type.into()]).unwrap_or(true)
    }
}

fn format_option_types_vec(
    types: &Option<bit_vec::BitVec>,
    formatter: &mut std::fmt::Formatter<'_>,
) -> Result<(), std::fmt::Error> {
    match types {
        None => {
            write!(formatter, "All")
        }
        Some(types) => {
            let mut list = &mut formatter.debug_list();
            for option_type in types
                .iter()
                .enumerate()
                .filter_map(|(option_type, is_set)| is_set.then_some(option_type))
            {
                if let Some(debug_name) =
                    u8::try_from(option_type).ok().and_then(ndp_option_types::debug_name)
                {
                    list = list.entry(&format_args!("{option_type} ({debug_name})"));
                } else {
                    list = list.entry(&option_type);
                }
            }
            list.finish()
        }
    }
}

fn respond_with_batch(
    responder: fnet_ndp::OptionWatcherWatchOptionsResponder,
    dropped: usize,
    mut events: Batch<'_>,
) {
    responder
        .send(events.as_slice(), u32::try_from(dropped).unwrap_or(u32::MAX))
        .unwrap_or_log("failed to respond");
}

mod watcher {
    use super::*;

    /// A watcher of NDP options.
    pub(crate) struct Watcher {
        stream: fnet_ndp::OptionWatcherRequestStream,
        interest: Interest,
        inner_events: EventQueue,
        responder: Option<fnet_ndp::OptionWatcherWatchOptionsResponder>,
    }

    impl Future for Watcher {
        type Output = Result<(), fidl::Error>;

        fn poll(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            loop {
                let next_request = self.as_mut().stream.poll_next_unpin(cx)?;
                match ready!(next_request) {
                    Some(fnet_ndp::OptionWatcherRequest::WatchOptions { responder }) => {
                        if let Some(existing) = self.responder.take() {
                            existing
                                .control_handle()
                                .shutdown_with_epitaph(zx::Status::ALREADY_EXISTS);
                            return Poll::Ready(Ok(()));
                        }
                        if let Some((dropped, events)) = self.inner_events.take_batch() {
                            respond_with_batch(responder, dropped, events);
                            continue;
                        }
                        self.responder = Some(responder);
                    }
                    Some(fnet_ndp::OptionWatcherRequest::Probe { responder }) => {
                        responder.send().unwrap_or_log("failed to respond");
                    }
                    None => return Poll::Ready(Ok(())),
                }
            }
        }
    }

    impl Watcher {
        pub(crate) fn new(NewWatcher { stream, interest }: NewWatcher) -> Self {
            Self { stream, interest, inner_events: EventQueue::new(), responder: None }
        }

        /// Responds with a batch of watch entries, if there are any queued to
        /// send and if there is currently a pending watch request to respond
        /// to.
        pub(crate) fn flush_batch(&mut self) {
            let Self { stream: _, inner_events, responder, interest: _ } = self;
            // NB: We must take care not to take and drop the responder if
            // there's no batch of events ready.
            if responder.is_some() {
                if let Some((dropped, events)) = inner_events.take_batch() {
                    respond_with_batch(responder.take().expect("must be present"), dropped, events);
                }
            }
        }

        /// Adds an entry to the watcher's [`EventQueue`] after checking that
        /// the `interface_id` and `option_type` match the watcher's configured
        /// interest.
        ///
        /// Returns true if the watcher was interested in the entry.
        ///
        /// The watcher will not have a response sent unless
        /// [`Watcher::flush_batch`] is subsequently called.
        pub(crate) fn maybe_enqueue_without_flushing(
            &mut self,
            interface_id: BindingId,
            option_type: OptionType,
            source_address: net_types::ip::Ipv6Addr,
            body: &fnet_ndp_ext::OptionBodyRef<'_>,
        ) -> bool {
            if !self.interest.interested(interface_id, option_type) {
                return false;
            }
            self.inner_events.push(fnet_ndp_ext::OptionWatchEntry {
                interface_id,
                option_type,
                source_address,
                body: body.to_owned(),
            });
            true
        }
    }
}
use watcher::Watcher;

#[derive(Derivative)]
#[derivative(Debug)]
struct NewWatcher {
    #[derivative(Debug = "ignore")]
    stream: fnet_ndp::OptionWatcherRequestStream,
    interest: Interest,
}

/// An NDP Router Advertisement message from the netstack.
#[derive(Derivative)]
#[derivative(Debug)]
struct RouterAdvertisement {
    /// The raw bytes of the router advertisement message's options.
    // NB: avoid deriving Debug since `option_bytes` could include PII.
    #[cfg_attr(not(test), derivative(Debug = "ignore"))]
    options_bytes: Box<[u8]>,
    /// The source address of the RA message.
    source: net_types::ip::Ipv6Addr,
    /// The interface on which the message was received.
    interface_id: BindingId,
}

#[derive(Error, Debug)]
#[error("connection to NDP options worker closed")]
pub(crate) struct WorkerClosedError;

#[derive(Clone)]
pub(crate) struct WorkerWatcherSink {
    sender: mpsc::Sender<NewWatcher>,
}

impl WorkerWatcherSink {
    /// Adds a new NDP option watcher to be operated on by [`Worker`].
    pub(crate) async fn add_watcher(
        &mut self,
        stream: fnet_ndp::OptionWatcherRequestStream,
        interest: Interest,
    ) -> Result<(), WorkerClosedError> {
        self.sender
            .send(NewWatcher { stream, interest })
            .await
            .map_err(|_: mpsc::SendError| WorkerClosedError)
    }
}

/// Errors observed while trying to send a Router Advertisement from the
/// netstack to the worker.
#[derive(Error, Debug)]
pub(crate) enum RouterAdvertisementSinkError {
    /// The worker is closed.
    #[error(transparent)]
    WorkerClosed(#[from] WorkerClosedError),
    /// The sink is full.
    #[error("sink is full")]
    SinkFull,
}

#[derive(Clone)]
pub(crate) struct WorkerRouterAdvertisementSink {
    sender: mpsc::Sender<RouterAdvertisement>,
}

impl WorkerRouterAdvertisementSink {
    /// Try to send a router advertisement to the worker.
    pub(crate) fn try_send_router_advertisement(
        &self,
        options_bytes: Box<[u8]>,
        source: net_types::ip::Ipv6Addr,
        interface_id: BindingId,
    ) -> Result<(), RouterAdvertisementSinkError> {
        let ra = RouterAdvertisement { options_bytes, source, interface_id };
        self.sender.clone().try_send(ra).map_err(|err| {
            if err.is_full() {
                RouterAdvertisementSinkError::SinkFull
            } else {
                RouterAdvertisementSinkError::WorkerClosed(WorkerClosedError)
            }
        })
    }
}

pub(crate) struct Worker {
    router_advertisements: mpsc::Receiver<RouterAdvertisement>,
    watchers: mpsc::Receiver<NewWatcher>,
}

const NEW_WATCHER_SINK_DEPTH: usize = 4;

impl Worker {
    pub(crate) fn new() -> (Worker, WorkerWatcherSink, WorkerRouterAdvertisementSink) {
        let (ra_sender, ra_receiver) = mpsc::channel(MAX_EVENTS);
        let (watcher_sender, watcher_receiver) = mpsc::channel(NEW_WATCHER_SINK_DEPTH);

        (
            Worker { router_advertisements: ra_receiver, watchers: watcher_receiver },
            WorkerWatcherSink { sender: watcher_sender },
            WorkerRouterAdvertisementSink { sender: ra_sender },
        )
    }

    /// Runs the worker until all [`WorkerWatcherSink`]s and
    /// [`WorkerRouterAdvertisementSink`]s are closed.
    ///
    /// On success, returns the set of currently opened [`Watcher`]s that the
    /// `Worker` was polling on when all its sinks were closed.
    pub(crate) async fn run(self) {
        info!("NDP watcher worker starting");
        let Self { mut router_advertisements, mut watchers } = self;
        let mut current_watchers = futures::stream::FuturesUnordered::<Watcher>::new();

        let current_watchers = loop {
            #[derive(Debug)]
            enum Action {
                WatcherEnded(Result<(), fidl::Error>),
                NewWatcher(Option<NewWatcher>),
                RouterAdvertisement(Option<RouterAdvertisement>),
            }
            let action = futures::select_biased! {
                r = current_watchers.by_ref().select_next_some() => Action::WatcherEnded(r),
                // Always consume router advertisements before watchers to avoid
                // dropping RAs due to the channel being full.
                ra = router_advertisements.next() => Action::RouterAdvertisement(ra),
                nw = watchers.next() => Action::NewWatcher(nw),
            };
            debug!("NDP watcher action: {action:?}");

            match action {
                Action::WatcherEnded(r) => match r {
                    Ok(()) => {}
                    Err(e) => {
                        if !e.is_closed() {
                            error!("error operating ndp watcher {e:?}");
                        }
                    }
                },
                Action::NewWatcher(Some(new_watcher)) => {
                    current_watchers.push(Watcher::new(new_watcher));
                }
                Action::RouterAdvertisement(Some(ra)) => {
                    Self::consume_router_advertisement(ra, &mut current_watchers);
                }
                Action::NewWatcher(None) | Action::RouterAdvertisement(None) => {
                    break current_watchers;
                }
            }
        };

        if !current_watchers.is_empty() {
            warn!("NDP watcher shutting down, dropped {} watchers", current_watchers.len());
        }
    }

    fn consume_router_advertisement(
        router_advertisement: RouterAdvertisement,
        watchers: &mut futures::stream::FuturesUnordered<Watcher>,
    ) {
        let RouterAdvertisement { options_bytes, source, interface_id } = router_advertisement;
        let parse_result = packet::records::Records::<
            _,
            packet_formats::icmp::ndp::options::NdpOptionsImpl,
        >::parse(options_bytes.as_ref());
        let records = match parse_result {
            Ok(records) => records,
            Err(packet::records::options::OptionParseErr) => {
                // Normally we wouldn't log an error due to network traffic,
                // but in theory core should already have validated this.
                // Avoid panicking in case there's a bug in packet-formats.
                error!(
                    "ndp watcher discarding router advertisement options \
                     due to parsing failure"
                );
                return;
            }
        };
        for bytes in records.iter_bytes() {
            let [option_type, _length_byte, body @ ..] = bytes else {
                error!("Records::iter_bytes() yielded a malformed NDP option");
                return;
            };
            let option_type = *option_type;
            let body = match fnet_ndp_ext::OptionBodyRef::new(body) {
                Ok(body) => body,
                Err(e) => {
                    // Logging an error because in theory packet-formats
                    // should never yield an invalid body here.
                    error!("observed NDP option with invalid body length: {e:?}");
                    continue;
                }
            };

            let mut num_interested_watchers = 0usize;
            for watcher in watchers.iter_mut() {
                let interested = watcher.maybe_enqueue_without_flushing(
                    interface_id,
                    option_type,
                    source,
                    &body,
                );
                if interested {
                    num_interested_watchers += 1;
                }
            }
            debug!(
                "enqueued NDP option to {num_interested_watchers} / {} watchers",
                watchers.len()
            );
        }
        for watcher in watchers.iter_mut() {
            watcher.flush_batch();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;
    use fixture::fixture;
    use fuchsia_async as fasync;
    use futures::{FutureExt, Stream};
    use itertools::Itertools;
    use packet_formats::icmp::ndp::options as packet_formats_ndp;
    use test_case::test_case;

    impl WorkerWatcherSink {
        fn create_watcher(
            &mut self,
            option_types: Option<impl IntoIterator<Item = OptionType>>,
            interface_id: Option<BindingId>,
        ) -> fnet_ndp::OptionWatcherProxy {
            let (watcher, stream) =
                fidl::endpoints::create_proxy_and_stream::<fnet_ndp::OptionWatcherMarker>();
            self.add_watcher(stream, Interest::new(option_types, interface_id))
                .now_or_never()
                .expect("unexpected backpressure on sink")
                .expect("failed to send watcher to worker");
            watcher
        }

        /// Resolves once the initial watcher probe successfully returns.
        async fn create_watcher_event_stream<O: IntoIterator<Item = OptionType>>(
            &mut self,
            option_types: Option<O>,
            interface_id: Option<BindingId>,
        ) -> impl Stream<Item = (Vec<fnet_ndp::OptionWatchEntry>, u32)> + use<O> {
            let watcher = self.create_watcher(option_types, interface_id);
            watcher.probe().await.expect("should successfully probe");
            futures::stream::unfold(watcher, |watcher| async move {
                match watcher.watch_options().await {
                    Err(e) => {
                        if e.is_closed() {
                            None
                        } else {
                            panic!("error fetching next event on watcher: {e:?}")
                        }
                    }
                    Ok(event) => Some((event, watcher)),
                }
            })
        }
    }

    async fn with_worker<
        Fut: Future<Output = ()>,
        F: FnOnce(WorkerWatcherSink, WorkerRouterAdvertisementSink) -> Fut,
    >(
        _name: &str,
        f: F,
    ) {
        let (worker, watcher_sink, ra_sink) = Worker::new();
        let ((), ()) = futures::future::join(worker.run(), f(watcher_sink, ra_sink)).await;
    }

    impl WorkerRouterAdvertisementSink {
        fn send_ra(&self, ra: RouterAdvertisement) {
            self.sender.clone().try_send(ra).expect("should succeed")
        }
    }

    const INTERESTED_INTERFACE_ID: BindingId = BindingId::new(1).unwrap();
    const UNINTERESTED_INTERFACE_ID: BindingId = BindingId::new(2).unwrap();
    const SOURCE_ADDR_NET_TYPES: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("fe80::1");

    /// Returns a `RouterAdvertisement` containing a Nonce option and the `OptionWatchEntry` it
    /// corresponds to.
    fn nonce_ra_and_watch_entry(
        interface_id: BindingId,
    ) -> (RouterAdvertisement, fnet_ndp_ext::OptionWatchEntry) {
        let nonce = [1, 2, 3, 4, 5, 6];

        let builder =
            packet_formats_ndp::NdpOptionBuilder::Nonce(packet_formats_ndp::NdpNonce::from(&nonce));
        let len = packet::records::RecordBuilder::serialized_len(&builder);
        let mut data = vec![0u8; len];
        packet::records::RecordBuilder::serialize_into(&builder, &mut data);

        (
            RouterAdvertisement {
                options_bytes: data.clone().into(),
                source: SOURCE_ADDR_NET_TYPES,
                interface_id,
            },
            fnet_ndp_ext::OptionWatchEntry {
                interface_id,
                source_address: SOURCE_ADDR_NET_TYPES,
                option_type: packet_formats::icmp::ndp::options::NdpOptionType::Nonce.into(),
                body: fnet_ndp_ext::OptionBody::new(data[2..].to_vec())
                    .expect("should be valid option body"),
            },
        )
    }

    #[fixture(with_worker)]
    #[fuchsia::test(allow_stalls = false)]
    async fn interface_interest_filtering(
        mut watcher_sink: WorkerWatcherSink,
        ra_sink: WorkerRouterAdvertisementSink,
    ) {
        let mut interest_all_watcher =
            std::pin::pin!(watcher_sink.create_watcher_event_stream(None::<[_; 0]>, None).await);
        let mut interest_specific_interface_watcher = std::pin::pin!(
            watcher_sink
                .create_watcher_event_stream(None::<[_; 0]>, Some(INTERESTED_INTERFACE_ID))
                .await
        );

        // Both receive the option on the interested interface.
        {
            let (ra, entry) = nonce_ra_and_watch_entry(INTERESTED_INTERFACE_ID);
            ra_sink.send_ra(ra);
            let (batch, dropped) = interest_all_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.clone().into()]);
            assert_eq!(dropped, 0);

            let (batch, dropped) =
                interest_specific_interface_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.into()]);
            assert_eq!(dropped, 0);
        }

        // Only the broadly-interested watcher receives the option on the other
        // interface.
        {
            let (ra, entry) = nonce_ra_and_watch_entry(UNINTERESTED_INTERFACE_ID);
            ra_sink.send_ra(ra);
            let (batch, dropped) = interest_all_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.clone().into()]);
            assert_eq!(dropped, 0);

            let mut next_fut = interest_specific_interface_watcher.next();
            let polled = fasync::TestExecutor::poll_until_stalled(&mut next_fut).await;
            match polled {
                Poll::Pending => (),
                Poll::Ready(result) => {
                    panic!("should not complete: {result:?}");
                }
            }
        }
    }

    fn recursive_dns_server_option_bytes() -> Vec<u8> {
        const ADDRESSES: [net_types::ip::Ipv6Addr; 2] =
            [net_declare::net_ip_v6!("2001:db8::1"), net_declare::net_ip_v6!("2001:db8::2")];
        let option = packet_formats_ndp::RecursiveDnsServer::new(u32::MAX, &ADDRESSES);
        let builder = packet_formats_ndp::NdpOptionBuilder::RecursiveDnsServer(option.clone());
        let len = packet::records::RecordBuilder::serialized_len(&builder);
        let mut data = vec![0u8; len];
        packet::records::RecordBuilder::serialize_into(&builder, &mut data);
        data
    }

    fn rdnss_ra_and_watch_entry() -> (RouterAdvertisement, fnet_ndp_ext::OptionWatchEntry) {
        let data = recursive_dns_server_option_bytes();
        (
            RouterAdvertisement {
                options_bytes: data.clone().into(),
                source: SOURCE_ADDR_NET_TYPES,
                interface_id: INTERESTED_INTERFACE_ID,
            },
            fnet_ndp_ext::OptionWatchEntry {
                interface_id: INTERESTED_INTERFACE_ID,
                source_address: SOURCE_ADDR_NET_TYPES,
                option_type: packet_formats_ndp::NdpOptionType::RecursiveDnsServer.into(),
                body: fnet_ndp_ext::OptionBody::new(data[2..].to_vec())
                    .expect("should be valid option body"),
            },
        )
    }

    #[fixture(with_worker)]
    #[fuchsia::test(allow_stalls = false)]
    async fn option_type_interest_filtering(
        mut watcher_sink: WorkerWatcherSink,
        ra_sink: WorkerRouterAdvertisementSink,
    ) {
        let mut interest_all_watcher =
            std::pin::pin!(watcher_sink.create_watcher_event_stream(None::<[_; 0]>, None).await);
        let mut type_specific_interface_watcher = std::pin::pin!(
            watcher_sink
                .create_watcher_event_stream(
                    Some([packet_formats_ndp::NdpOptionType::RecursiveDnsServer.into()]),
                    None
                )
                .await
        );

        // Both receive the RDNSS option.
        {
            let (ra, entry) = rdnss_ra_and_watch_entry();
            ra_sink.send_ra(ra);
            let (batch, dropped) = interest_all_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.clone().into()]);
            assert_eq!(dropped, 0);

            let (batch, dropped) =
                type_specific_interface_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.into()]);
            assert_eq!(dropped, 0);
        }

        // Only the broadly-interested watcher receives the nonce option.
        {
            let mut next_fut = type_specific_interface_watcher.next();
            let polled = fasync::TestExecutor::poll_until_stalled(&mut next_fut).await;
            match polled {
                Poll::Pending => (),
                Poll::Ready(result) => {
                    panic!("should not complete: {result:?}");
                }
            }

            let (ra, entry) = nonce_ra_and_watch_entry(INTERESTED_INTERFACE_ID);
            ra_sink.send_ra(ra);
            let (batch, dropped) = interest_all_watcher.next().await.expect("should yield");
            assert_eq!(batch, vec![entry.clone().into()]);
            assert_eq!(dropped, 0);

            let polled = fasync::TestExecutor::poll_until_stalled(&mut next_fut).await;
            match polled {
                Poll::Pending => (),
                Poll::Ready(result) => {
                    panic!("should not complete: {result:?}");
                }
            }
        }
    }

    #[fixture(with_worker)]
    #[fuchsia::test(allow_stalls = false)]
    async fn watcher_batches_events(
        mut watcher_sink: WorkerWatcherSink,
        ra_sink: WorkerRouterAdvertisementSink,
    ) {
        let watcher1 =
            std::pin::pin!(watcher_sink.create_watcher_event_stream(None::<[_; 0]>, None).await);
        let mut watcher2 =
            std::pin::pin!(watcher_sink.create_watcher_event_stream(None::<[_; 0]>, None).await);

        const NUM_EXPECTED_DROPPED: usize = 10;
        let interface_ids = 1u64..=u64::try_from(MAX_EVENTS + NUM_EXPECTED_DROPPED).unwrap();
        let ra_sink = &ra_sink;

        // Drive the events through watcher 1 one-by-one to show that they can
        // be eagerly yielded by the watcher.
        let (got_batches, want_batches) = watcher1
            .zip(futures::stream::iter(interface_ids.clone().map(|i| {
                let i = BindingId::new(i).unwrap();
                let (ra, entry) = nonce_ra_and_watch_entry(i);
                ra_sink.send_ra(ra);
                entry
            })))
            .fold((Vec::new(), Vec::new()), |(mut got, mut want), ((batch, dropped), entry)| {
                got.push((batch, dropped));
                want.push((vec![entry.into()], 0));
                futures::future::ready((got, want))
            })
            .await;
        assert_eq!(got_batches, want_batches);

        // Now expect to see the same entries on watcher 2, except in batches of
        // the max batch size.

        // Need a second let-binding because `.chunks` returns IntoIter, not
        // Iter.
        let want_batches = interface_ids
            .dropping(NUM_EXPECTED_DROPPED)
            .chunks(usize::from(fnet_ndp::MAX_OPTION_BATCH_SIZE));
        let want_batches = want_batches
            .into_iter()
            .zip(std::iter::once(NUM_EXPECTED_DROPPED).chain(std::iter::repeat(0usize)))
            .map(|(batch, dropped)| {
                let batch = batch
                    .map(|i| {
                        let i = BindingId::new(i).unwrap();
                        let (_, entry) = nonce_ra_and_watch_entry(i);
                        fnet_ndp::OptionWatchEntry::from(entry)
                    })
                    .collect::<Vec<_>>();
                (batch, u32::try_from(dropped).expect("should fit in u32"))
            })
            .collect::<Vec<_>>();

        let got_batches = watcher2.by_ref().take(want_batches.len()).collect::<Vec<_>>().await;
        assert_eq!(got_batches, want_batches);

        // There shouldn't be any more events on the watcher.
        match fasync::TestExecutor::poll_until_stalled(&mut watcher2.next()).await {
            Poll::Ready(_) => panic!("should not observe event"),
            Poll::Pending => (),
        }
    }

    /// Tests that the worker can handle watchers coming and going.
    #[fixture(with_worker)]
    #[fuchsia::test(allow_stalls = false)]
    async fn watcher_turnaround(
        watcher_sink: WorkerWatcherSink,
        ra_sink: WorkerRouterAdvertisementSink,
    ) {
        let watcher_sink = &watcher_sink;
        let ra_sink = &ra_sink;

        #[derive(Copy, Clone, Debug)]
        enum WatcherBehavior {
            WatchOnce,
            ProbeOnly,
            RequestThenDropWithoutWaiting,
        }
        let spawn_watcher_and_watch_once =
        // comment to force rustfmt to keep the line break
        |interface_id: BindingId, behavior: WatcherBehavior| async move {
            let mut watcher = std::pin::pin!(
                watcher_sink
                    .clone()
                    .create_watcher_event_stream(None::<[_; 0]>, Some(interface_id))
                    .await
            );
            let (ra, entry) = nonce_ra_and_watch_entry(interface_id);
            ra_sink.send_ra(ra);

            match behavior {
                WatcherBehavior::WatchOnce => {
                    let (batch, dropped) = watcher.next().await.expect("should yield");
                    assert_eq!(batch, vec![entry.into()]);
                    assert_eq!(dropped, 0);
                }
                WatcherBehavior::ProbeOnly => {}
                WatcherBehavior::RequestThenDropWithoutWaiting => {
                    let _fut = watcher.next();
                }
            }
        };
        for (i, behavior) in (1u64..=100u64).zip(
            [
                WatcherBehavior::WatchOnce,
                WatcherBehavior::ProbeOnly,
                WatcherBehavior::RequestThenDropWithoutWaiting,
            ]
            .into_iter()
            .cycle(),
        ) {
            let interface_id = BindingId::new(i).unwrap();
            spawn_watcher_and_watch_once(interface_id, behavior).await;
        }
    }

    #[fixture(with_worker)]
    #[fuchsia::test(allow_stalls = false)]
    async fn watcher_disallows_double_get(
        mut watcher_sink: WorkerWatcherSink,
        ra_sink: WorkerRouterAdvertisementSink,
    ) {
        let watcher = watcher_sink.create_watcher(None::<[_; 0]>, None);
        let (r1, r2) =
            futures::future::join(watcher.watch_options(), watcher.watch_options()).await;
        for r in [r1, r2] {
            assert_matches!(
                r,
                Err(fidl::Error::ClientChannelClosed { epitaph, .. })
                    if epitaph == zx::Status::ALREADY_EXISTS
            );
        }
        // Need to keep the router advertisement sink alive in order for the
        // worker to continue running until the end of the test.
        drop(ra_sink);
    }

    #[test_case(None => "Interest { types: All, interface_id: None }")]
    #[test_case(Some(vec![]) => "Interest { types: [], interface_id: None }")]
    #[test_case(Some(vec![packet_formats_ndp::NdpOptionType::RecursiveDnsServer.into()])
        => "Interest { types: [25 (RECURSIVE_DNS_SERVER)], interface_id: None }")]
    #[test_case(Some(vec![1, packet_formats_ndp::NdpOptionType::RecursiveDnsServer.into()])
        => "Interest { types: [1, 25 (RECURSIVE_DNS_SERVER)], interface_id: None }")]
    fn format_interest(types: Option<Vec<OptionType>>) -> String {
        format!("{:?}", Interest::new(types, None))
    }
}
