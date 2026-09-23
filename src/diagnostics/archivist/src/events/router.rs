// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::types::*;
use crate::identity::ComponentIdentity;
use fuchsia_inspect::{self as inspect, NumericProperty};
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use futures::channel::{mpsc, oneshot};
use futures::task::{Context, Poll};
use futures::{Future, Stream, StreamExt};
use log::{debug, error};
use pin_project::pin_project;
use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::{Arc, Weak};
use thiserror::Error;

const RECENT_EVENT_LIMIT: usize = 200;

/// Core archivist internal event router that supports multiple event producers and multiple event
/// consumers.
pub struct EventRouter {
    // All the consumers that have been registered for an event.
    consumers: BTreeMap<EventType, Vec<Weak<dyn EventConsumer + Send + Sync>>>,
    // The types of all events that can be produced. Used only for validation.
    producers_registered: BTreeSet<EventType>,

    // Ends of the channel used by event producers.
    sender: mpsc::UnboundedSender<Event>,
    receiver: mpsc::UnboundedReceiver<Event>,

    inspect_logger: EventStreamLogger,
}

impl EventRouter {
    /// Creates a new empty event router.
    pub fn new(node: inspect::Node) -> Self {
        let (sender, receiver) = mpsc::unbounded();
        Self {
            consumers: BTreeMap::new(),
            sender,
            receiver,
            producers_registered: BTreeSet::new(),
            inspect_logger: EventStreamLogger::new(node),
        }
    }

    /// Registers an event producer with the given configuration specifying the types of events the
    /// given producer is allowed to emit.
    pub fn add_producer<T>(&mut self, config: ProducerConfig<'_, T>)
    where
        T: EventProducer,
    {
        let events: BTreeSet<_> = config.events.into_iter().collect();
        self.producers_registered.append(&mut events.clone());
        let dispatcher = Dispatcher::new(events, self.sender.clone());
        config.producer.set_dispatcher(dispatcher);
    }

    /// Registers an event consumer with the given configuration specifying the types of events the
    /// given consumer will receive.
    pub fn add_consumer<T>(&mut self, config: ConsumerConfig<'_, T>)
    where
        T: EventConsumer + Send + Sync + 'static,
    {
        let subscriber_weak = Arc::downgrade(config.consumer);
        for event_type in config.events {
            self.consumers
                .entry(event_type)
                .or_default()
                .push(Weak::clone(&subscriber_weak) as Weak<dyn EventConsumer + Send + Sync>);
        }
    }

    /// Starts listening for events emitted by the registered producers and dispatching them to
    /// registered consumers.
    ///
    /// First, validates that for every event type that will be dispatched, there exists at least
    /// one consumer. And that for every event that will be consumed, there exists at least one
    /// producer.
    ///
    /// Afterwards, listens to events emitted by producers. When an event arrives it sends it to
    /// all consumers of the event. Since all events are singletons, the first consumer that was
    /// registered will get the singleton data and the rest won't.
    pub fn start(mut self) -> Result<(TerminateHandle, impl Future<Output = ()>), RouterError> {
        self.validate_routing()?;

        let (terminate_handle, mut stream) = EventStream::new(self.receiver);
        let mut consumers = self.consumers;
        let mut inspect_logger = self.inspect_logger;

        let fut = async move {
            loop {
                match stream.next().await {
                    None => {
                        debug!("Event ingestion finished");
                        break;
                    }
                    Some(event) => {
                        inspect_logger.log(&event);

                        let event_type = event.ty();
                        let weak_consumers = match consumers.remove(&event_type) {
                            Some(c) => c,
                            None => continue,
                        };

                        let mut singleton_event = Some(event);

                        // Consumers which weak reference could be upgraded will be stored here.
                        let mut active_consumers = vec![];
                        for weak_consumer in weak_consumers {
                            if let Some(consumer) = weak_consumer.upgrade() {
                                active_consumers.push(weak_consumer);
                                if let Some(e) = singleton_event.take() {
                                    consumer.handle(e);
                                };
                            }
                        }

                        // We insert the list of active consumers back in the map.
                        consumers.insert(event_type, active_consumers);
                    }
                }
            }
        };
        Ok((terminate_handle, fut))
    }

    fn validate_routing(&mut self) -> Result<(), RouterError> {
        for consumed_event in self.consumers.keys() {
            if !self.producers_registered.contains(consumed_event) {
                return Err(RouterError::MissingProducer(consumed_event.clone()));
            }
        }
        for produced_event in &self.producers_registered {
            if !self.consumers.contains_key(produced_event) {
                return Err(RouterError::MissingConsumer(produced_event.clone()));
            }
        }
        Ok(())
    }
}

/// Stream of events that  provides the mechanisms used to notify when the events have
/// been drained.
#[pin_project]
struct EventStream {
    /// The stream containing events.
    #[pin]
    receiver: mpsc::UnboundedReceiver<Event>,

    /// When this future is ready, the stream will be closed. Messages still in the buffer
    /// will be drained.
    #[pin]
    on_terminate: oneshot::Receiver<()>,

    /// When the stream has been drained a notification will be sent through this channel.
    on_drained: Option<oneshot::Sender<()>>,
}

impl EventStream {
    fn new(receiver: mpsc::UnboundedReceiver<Event>) -> (TerminateHandle, Self) {
        let (snd, rcv) = oneshot::channel();
        let (drain_snd, drain_rcv) = oneshot::channel();
        (
            TerminateHandle { snd, drained: drain_rcv },
            Self { receiver, on_terminate: rcv, on_drained: Some(drain_snd) },
        )
    }
}

impl Stream for EventStream {
    type Item = Event;

    /// This stream implementation merges two streams into a single one polling from each of them
    /// in a round robin fashion. When one stream finishes, this will keep polling from the
    /// remaining one.
    ///
    /// When receiving a request for termination, the event stream will be
    /// closed so that no new messages can be sent through that channel, but it'll still be drained.
    ///
    /// When the stream has been drained, a message is sent through the appropriate
    /// channel.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // First check if request to terminate the event ingestion has been requested, if
        // it has, then close the channel to which events are sent. This will prevent
        // further messages to be sent, but it remains possible to drain the channel
        // buffer.
        //
        // IMPORTANT: If we ever use this event stream for events emitted internally, then we
        // should bring back the changed undone in https://fxrev.dev/744413 as internal event
        // streams shouldn't be closed on termination.
        match this.on_terminate.poll(cx) {
            Poll::Pending => {}
            Poll::Ready(_) => {
                this.receiver.close();
            }
        }
        // Poll the stream and track whether it's drained or not.
        match this.receiver.poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                // Notify once that it has been drained.
                if let Some(snd) = this.on_drained.take() {
                    snd.send(()).unwrap_or_else(|err| {
                        error!(err:?; "Failed to notify the events have been drained.");
                    });
                };
                Poll::Ready(None)
            }
            res @ Poll::Ready(Some(_)) => res,
        }
    }
}

/// Allows to termiante event ingestion.
pub struct TerminateHandle {
    snd: oneshot::Sender<()>,
    drained: oneshot::Receiver<()>,
}

impl TerminateHandle {
    /// Terminates event ingestion. Buffered events will be drained. The returned future
    /// will complete once all buffered events have been drained.
    pub async fn terminate(self) {
        self.snd.send(()).unwrap_or_else(|err| {
            error!(err:?; "Failed to terminate the event ingestion.");
        });
        self.drained
            .await
            .unwrap_or_else(|err| error!(err:?; "Error waiting for events to be drained."));
    }
}

/// Allows to emit events of a restricted set of types.
///
/// Event producers will receive a `Dispatcher` instance that will allow them to emit events of
/// restricted set of types.
pub struct Dispatcher {
    allowed_events: BTreeSet<EventType>,
    sender: Option<mpsc::UnboundedSender<Event>>,
}

/// Returns a no-op dispatcher.
impl Default for Dispatcher {
    fn default() -> Self {
        Self { allowed_events: BTreeSet::new(), sender: None }
    }
}

impl Dispatcher {
    fn new(allowed_events: BTreeSet<EventType>, sender: mpsc::UnboundedSender<Event>) -> Self {
        Self { allowed_events, sender: Some(sender) }
    }

    /// Emits an event. If the event isn't in the restricted set of allowed types, this operation
    /// is a no-op. An error is returned when sending the event into the channel fails.
    pub fn emit(&mut self, event: Event) -> Result<(), mpsc::TrySendError<Event>> {
        if let Some(sender) = &mut self.sender
            && self.allowed_events.contains(&event.ty())
        {
            sender.unbounded_send(event)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn new_for_test(
        allowed_events: BTreeSet<EventType>,
    ) -> (mpsc::UnboundedReceiver<Event>, Self) {
        let (sender, receiver) = mpsc::unbounded();
        (receiver, Self::new(allowed_events, sender))
    }
}

struct EventStreamLogger {
    counters: BTreeMap<EventType, inspect::UintProperty>,
    component_log_node: BoundedListNode,
    counters_node: inspect::Node,
    _node: inspect::Node,
}

impl EventStreamLogger {
    /// Creates a new event logger. All inspect data will be written as children of `parent`.
    pub fn new(node: inspect::Node) -> Self {
        let counters_node = node.create_child("event_counts");
        let recent_events_node = node.create_child("recent_events");
        Self {
            _node: node,
            counters: BTreeMap::new(),
            counters_node,
            component_log_node: BoundedListNode::new(recent_events_node, RECENT_EVENT_LIMIT),
        }
    }

    /// Log a new component event to inspect.
    pub fn log(&mut self, event: &Event) {
        let ty = event.ty();
        if self.counters.contains_key(&ty) {
            self.counters.get_mut(&ty).unwrap().add(1);
        } else {
            let counter = self.counters_node.create_uint(ty.as_ref(), 1);
            self.counters.insert(ty.clone(), counter);
        }
        // TODO(https://fxbug.dev/42174041): leverage string references for the payload.
        match &event.payload {
            EventPayload::LogSinkRequested(LogSinkRequestedPayload { component, .. })
            | EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                component, ..
            }) => {
                self.log_inspect(ty.as_ref(), component);
            }
        }
    }

    fn log_inspect(&mut self, event_name: &str, identity: &ComponentIdentity) {
        // TODO(https://fxbug.dev/42174041): leverage string references for the `event_name`.
        inspect_log!(self.component_log_node,
            "event" => event_name,
            "moniker" => identity.moniker.to_string(),
        );
    }
}

/// Set of errors that can happen when setting up an event router and executing its dispatching loop.
#[derive(Debug, Error)]
pub enum RouterError {
    #[error("Missing consumer for event type {0:?}")]
    MissingConsumer(EventType),

    #[error("Missing producer for event type {0:?}")]
    MissingProducer(EventType),
}

/// Configuration for an event producer.
pub struct ProducerConfig<'a, T> {
    /// The event producer that will receive a `Dispatcher`
    pub producer: &'a mut T,

    /// The set of events that the `producer` will be allowed to emit.
    pub events: Vec<EventType>,
}

/// Configuration for an event consumer.
pub struct ConsumerConfig<'a, T> {
    /// The event consumer that will receive events when they are emitted by producers.
    pub consumer: &'a Arc<T>,

    /// The set of event types that the `consumer` will receive.
    pub events: Vec<EventType>,
}

/// Trait implemented by data types which receive events.
pub trait EventConsumer {
    /// Event consumers will receive a call on this method when an event they are interested on
    /// happens.
    fn handle(self: Arc<Self>, event: Event);
}

/// Trait implemented by data types which emit events.
pub trait EventProducer {
    /// Whent registered, event producers will receive a call on this method with the `dispatcher`
    /// they can use to emit events.
    fn set_dispatcher(&mut self, dispatcher: Dispatcher);
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl::encoding::ProxyChannelBox;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_inspect::InspectSinkMarker;
    use fidl_fuchsia_logger::{LogSinkMarker, LogSinkRequestStream};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::FutureExt;
    use moniker::ExtendedMoniker;
    use std::sync::LazyLock;
    use zx::AsHandleRef;

    const TEST_URL: &str = "NO-OP URL";
    const FAKE_TIMESTAMP: i64 = 5;
    static IDENTITY: LazyLock<Arc<ComponentIdentity>> = LazyLock::new(|| {
        Arc::new(ComponentIdentity::new(ExtendedMoniker::parse_str("./a/b").unwrap(), TEST_URL))
    });

    #[derive(Default)]
    struct TestEventProducer {
        dispatcher: Dispatcher,
    }

    impl TestEventProducer {
        fn emit(&mut self, event_type: EventType, identity: Arc<ComponentIdentity>) {
            let event = match event_type {
                EventType::LogSinkRequested => {
                    let (_, request_stream) =
                        fidl::endpoints::create_proxy_and_stream::<LogSinkMarker>();
                    Event {
                        timestamp: zx::BootInstant::from_nanos(FAKE_TIMESTAMP),
                        payload: EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                            component: identity,
                            request_stream,
                        }),
                    }
                }
                EventType::InspectSinkRequested => {
                    let (_, request_stream) =
                        fidl::endpoints::create_proxy_and_stream::<InspectSinkMarker>();
                    Event {
                        timestamp: zx::BootInstant::from_nanos(FAKE_TIMESTAMP),
                        payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                            component: identity,
                            request_stream,
                        }),
                    }
                }
            };
            let _ = self.dispatcher.emit(event);
        }
    }

    impl EventProducer for TestEventProducer {
        fn set_dispatcher(&mut self, dispatcher: Dispatcher) {
            self.dispatcher = dispatcher;
        }
    }

    struct TestEventConsumer {
        event_sender: Mutex<mpsc::UnboundedSender<Event>>,
    }

    impl TestEventConsumer {
        fn new() -> (mpsc::UnboundedReceiver<Event>, Arc<Self>) {
            let (event_sender, event_receiver) = mpsc::unbounded();
            (event_receiver, Arc::new(Self { event_sender: Mutex::new(event_sender) }))
        }
    }

    impl EventConsumer for TestEventConsumer {
        fn handle(self: Arc<Self>, event: Event) {
            self.event_sender.lock().unbounded_send(event).unwrap();
        }
    }

    #[fuchsia::test]
    fn invalid_routing() {
        let mut producer = TestEventProducer::default();
        let (_receiver, consumer) = TestEventConsumer::new();
        let mut router = EventRouter::new(inspect::Node::default());
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &consumer,
            events: vec![EventType::InspectSinkRequested, EventType::LogSinkRequested],
        });

        // An explicit match is needed here since unwrap_err requires Debug implemented for both T
        // and E in Result<T, E> and T is a pair which second element is `impl Future` which
        // doesn't implement Debug.
        match router.start() {
            Err(err) => {
                assert_matches!(err, RouterError::MissingProducer(EventType::LogSinkRequested));
            }
            Ok(_) => panic!("expected an error from routing events"),
        }

        let mut producer = TestEventProducer::default();
        let (_receiver, consumer) = TestEventConsumer::new();
        let mut router = EventRouter::new(inspect::Node::default());
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &consumer,
            events: vec![EventType::LogSinkRequested],
        });

        match router.start() {
            Err(err) => {
                assert_matches!(
                    err,
                    RouterError::MissingConsumer(EventType::InspectSinkRequested)
                        | RouterError::MissingProducer(EventType::LogSinkRequested)
                );
            }
            Ok(_) => panic!("expected an error from routing events"),
        }
    }

    #[fuchsia::test]
    async fn event_subscription() {
        let mut producer = TestEventProducer::default();
        let (mut first_receiver, first_consumer) = TestEventConsumer::new();
        let (mut second_receiver, second_consumer) = TestEventConsumer::new();
        let mut router = EventRouter::new(inspect::Node::default());
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::LogSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &first_consumer,
            events: vec![EventType::LogSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &second_consumer,
            events: vec![EventType::LogSinkRequested],
        });

        let (_terminate_handle, fut) = router.start().unwrap();
        let _router_task = fasync::Task::spawn(fut);

        // Emit an event
        let (_, server_end) = fidl::endpoints::create_proxy::<LogSinkMarker>();
        let request_stream_koid = server_end.as_handle_ref().koid().unwrap();
        let request_stream = LogSinkRequestStream::from_channel(fidl::AsyncChannel::from_channel(
            server_end.into_channel(),
        ));
        let timestamp = zx::BootInstant::get();
        producer
            .dispatcher
            .emit(Event {
                timestamp,
                payload: EventPayload::LogSinkRequested(LogSinkRequestedPayload {
                    component: IDENTITY.clone(),
                    request_stream,
                }),
            })
            .unwrap();

        // The first consumer that was registered receives the request stream. The second one
        // receives nothing.
        let first_event = first_receiver.next().await.unwrap();
        assert_matches!(first_event, Event {
            payload: EventPayload::LogSinkRequested(payload),
            ..
        } => {
            assert_eq!(payload.component, *IDENTITY);
            let actual_koid = payload.request_stream
                .into_inner().0.channel().as_channel().as_handle_ref().koid().unwrap();
            assert_eq!(actual_koid, request_stream_koid);
        });
        assert!(second_receiver.next().now_or_never().is_none());
    }

    #[fuchsia::test]
    async fn consumers_cleanup() {
        let mut producer = TestEventProducer::default();
        let (mut first_receiver, first_consumer) = TestEventConsumer::new();
        let (mut second_receiver, second_consumer) = TestEventConsumer::new();
        let (mut third_receiver, third_consumer) = TestEventConsumer::new();
        let mut router = EventRouter::new(inspect::Node::default());
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &first_consumer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &second_consumer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_consumer(ConsumerConfig {
            consumer: &third_consumer,
            events: vec![EventType::InspectSinkRequested],
        });

        drop(first_consumer);
        drop(third_consumer);

        let (_terminate_handle, fut) = router.start().unwrap();
        let _router_task = fasync::Task::spawn(fut);

        // Emit an event
        let (_, request_stream) = fidl::endpoints::create_proxy_and_stream::<InspectSinkMarker>();
        producer
            .dispatcher
            .emit(Event {
                timestamp: zx::BootInstant::get(),
                payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                    component: IDENTITY.clone(),
                    request_stream,
                }),
            })
            .unwrap();

        // We see the event only in the receiver which consumer wasn't dropped.
        let event = second_receiver.next().await.unwrap();
        assert_matches!(event.payload, EventPayload::InspectSinkRequested(_));
        assert!(first_receiver.next().now_or_never().unwrap().is_none());
        assert!(third_receiver.next().now_or_never().unwrap().is_none());

        // We see additional events in the second receiver which remains alive.
        let (_, request_stream) = fidl::endpoints::create_proxy_and_stream::<InspectSinkMarker>();
        producer
            .dispatcher
            .emit(Event {
                timestamp: zx::BootInstant::get(),
                payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                    component: IDENTITY.clone(),
                    request_stream,
                }),
            })
            .unwrap();
        let event = second_receiver.next().await.unwrap();
        assert_matches!(event.payload, EventPayload::InspectSinkRequested(_));
        assert!(first_receiver.next().now_or_never().unwrap().is_none());
        assert!(third_receiver.next().now_or_never().unwrap().is_none());
    }

    #[fuchsia::test]
    async fn inspect_log() {
        let inspector = inspect::Inspector::default();
        let mut router = EventRouter::new(inspector.root().create_child("events"));
        let mut producer1 = TestEventProducer::default();
        let mut producer2 = TestEventProducer::default();
        let (receiver, consumer) = TestEventConsumer::new();
        router.add_consumer(ConsumerConfig {
            consumer: &consumer,
            events: vec![EventType::InspectSinkRequested, EventType::LogSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer1,
            events: vec![EventType::LogSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer2,
            events: vec![EventType::InspectSinkRequested],
        });

        producer1.emit(EventType::LogSinkRequested, IDENTITY.clone());
        producer2.emit(EventType::InspectSinkRequested, IDENTITY.clone());

        // Consume the events.
        let (_terminate_handle, fut) = router.start().unwrap();
        let _router_task = fasync::Task::spawn(fut);
        receiver.take(2).collect::<Vec<_>>().await;

        assert_data_tree!(inspector, root: {
            events: {
                event_counts: {
                    log_sink_requested: 1u64,
                    inspect_sink_requested: 1u64,
                },
                recent_events: {
                    "0": {
                        "@time": AnyProperty,
                        event: "log_sink_requested",
                        moniker: "a/b"
                    },
                    "1": {
                        "@time": AnyProperty,
                        event: "inspect_sink_requested",
                        moniker: "a/b"
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    async fn event_stream_semantics() {
        let inspector = inspect::Inspector::default();
        let mut router = EventRouter::new(inspector.root().create_child("events"));
        let mut producer1 = TestEventProducer::default();
        let mut producer2 = TestEventProducer::default();
        let (receiver, consumer) = TestEventConsumer::new();
        router.add_consumer(ConsumerConfig {
            consumer: &consumer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer1,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer2,
            events: vec![EventType::InspectSinkRequested],
        });

        let identity = |moniker| {
            Arc::new(ComponentIdentity::new(ExtendedMoniker::parse_str(moniker).unwrap(), TEST_URL))
        };

        producer1.emit(EventType::InspectSinkRequested, identity("./a"));
        producer2.emit(EventType::InspectSinkRequested, identity("./b"));
        producer1.emit(EventType::InspectSinkRequested, identity("./c"));
        producer2.emit(EventType::InspectSinkRequested, identity("./d"));

        // We should see the events in order of emission.
        let (_terminate_handle, fut) = router.start().unwrap();
        let _router_task = fasync::Task::spawn(fut);
        let events = receiver.take(4).collect::<Vec<_>>().await;

        let expected_events = vec![
            inspect_sink_requested(identity("./a")),
            inspect_sink_requested(identity("./b")),
            inspect_sink_requested(identity("./c")),
            inspect_sink_requested(identity("./d")),
        ];
        assert_eq!(events.len(), expected_events.len());
        for (event, expected_event) in std::iter::zip(events, expected_events) {
            assert_event(event, expected_event);
        }
    }

    #[fuchsia::test]
    async fn stream_draining() {
        let inspector = inspect::Inspector::default();
        let mut router = EventRouter::new(inspector.root().create_child("events"));
        let mut producer = TestEventProducer::default();
        let (mut receiver, consumer) = TestEventConsumer::new();
        router.add_consumer(ConsumerConfig {
            consumer: &consumer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::InspectSinkRequested],
        });
        router.add_producer(ProducerConfig {
            producer: &mut producer,
            events: vec![EventType::InspectSinkRequested],
        });

        producer.emit(EventType::InspectSinkRequested, IDENTITY.clone());

        let (terminate_handle, fut) = router.start().unwrap();
        let _router_task = fasync::Task::spawn(fut);
        let on_drained = terminate_handle.terminate();
        let drain_finished = fasync::Task::spawn(on_drained);

        assert_event(receiver.next().await.unwrap(), inspect_sink_requested(IDENTITY.clone()));

        // This future must be complete now.
        drain_finished.await;

        // We must never see any new event emitted by the producer.
        producer.emit(EventType::InspectSinkRequested, IDENTITY.clone());
        assert!(receiver.next().now_or_never().is_none());
    }

    fn assert_event(event: Event, other: Event) {
        assert_eq!(event.timestamp, other.timestamp);
        match (event.payload, other.payload) {
            (
                EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                    component: this_identity,
                    ..
                }),
                EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                    component: other_identity,
                    ..
                }),
            ) => {
                assert_eq!(this_identity, other_identity);
            }
            _ => unimplemented!("no other combinations are expected in these tests"),
        }
    }

    fn inspect_sink_requested(identity: Arc<ComponentIdentity>) -> Event {
        let (_proxy, request_stream) =
            fidl::endpoints::create_proxy_and_stream::<InspectSinkMarker>();
        Event {
            timestamp: zx::BootInstant::from_nanos(FAKE_TIMESTAMP),
            payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                component: identity,
                request_stream,
            }),
        }
    }
}
