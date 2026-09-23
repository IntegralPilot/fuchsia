// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::events::types::{Event, EventPayload, LogSinkRequestedPayload};
use crate::identity::ComponentIdentity;
use crate::logs::repository::LogsRepository;
use crate::logs::servers::LogServer;
use crate::logs::shared_buffer::create_ring_buffer;
use crate::logs::stored_message::StoredMessage;
use diagnostics_log_encoding::encode::{
    Encoder, EncoderOpts, EncodingError, MutableBuffer, RecordEvent, WriteEventParams,
};
use diagnostics_log_encoding::{Argument, RawSeverity, Record};
use diagnostics_log_types::Severity;
use diagnostics_message::MAX_DATAGRAM_LEN;
use fidl::prelude::*;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_logger::{
    LogFilterOptions, LogMarker, LogMessage, LogProxy, LogSinkMarker, LogSinkProxy,
    MAX_DATAGRAM_LEN_BYTES,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol_at_dir_svc;
use fuchsia_inspect::Inspector;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Cursor;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use validating_log_listener::validate_log_stream;

pub struct TestHarness {
    inspector: Inspector,
    log_manager: Arc<LogsRepository>,
    _log_server: LogServer,
    log_proxy: LogProxy,
    /// weak pointers to "pending" TestStreams which haven't dropped yet
    pending_streams: Vec<Weak<()>>,
    /// LogSinks to retain for inspect attribution tests
    sinks: Option<Vec<LogSinkProxy>>,
    scope: fasync::Scope,
}

pub fn create_log_sink_requested_event(
    target_moniker: String,
    target_url: String,
    capability: zx::Channel,
) -> fcomponent::Event {
    fcomponent::Event {
        header: Some(fcomponent::EventHeader {
            event_type: Some(fcomponent::EventType::CapabilityRequested),
            moniker: Some(target_moniker),
            component_url: Some(target_url),
            timestamp: Some(zx::BootInstant::get()),
            ..Default::default()
        }),
        payload: Some(fcomponent::EventPayload::CapabilityRequested(
            fcomponent::CapabilityRequestedPayload {
                name: Some(LogSinkMarker::PROTOCOL_NAME.into()),
                capability: Some(capability),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new(false)
    }
}

impl TestHarness {
    /// Create a new test harness which will keep its LogSinks alive as long as it itself is,
    /// useful for testing inspect hierarchies for attribution.
    // TODO(https://fxbug.dev/42131398) this will be made unnecessary by historical retention of component stats
    pub fn with_retained_sinks() -> Self {
        Self::new(true)
    }

    fn new(hold_sinks: bool) -> Self {
        let scope = fasync::Scope::new();
        let inspector = Inspector::default();
        let log_manager = LogsRepository::new(
            create_ring_buffer(1_000_000),
            std::iter::empty(),
            inspector.root(),
            scope.new_child(),
        );
        let log_server = LogServer::new(Arc::clone(&log_manager), scope.new_child());

        let (log_proxy, log_stream) = fidl::endpoints::create_proxy_and_stream::<LogMarker>();
        log_server.spawn(log_stream);

        Self {
            inspector,
            log_manager,
            _log_server: log_server,
            log_proxy,
            pending_streams: vec![],
            sinks: if hold_sinks { Some(vec![]) } else { None },
            scope,
        }
    }

    pub fn create_default_reader(&self, identity: ComponentIdentity) -> Arc<dyn LogReader> {
        Arc::new(DefaultLogReader::new(
            Arc::clone(&self.log_manager),
            Arc::new(identity),
            self.scope.to_handle(),
        ))
    }

    pub fn create_event_stream_reader(
        &self,
        target_moniker: impl Into<String>,
        target_url: impl Into<String>,
    ) -> Arc<dyn LogReader> {
        Arc::new(EventStreamLogReader::new(
            Arc::clone(&self.log_manager),
            target_moniker,
            target_url,
            self.scope.to_handle(),
        ))
    }

    /// Check to make sure all `TestStream`s have been dropped. This ensures that we repeatedly test
    /// the case where a socket's write half is dropped before the socket is drained.
    fn check_pending_streams(&mut self) {
        self.pending_streams.retain(|w| w.upgrade().is_some());
        assert_eq!(
            self.pending_streams.len(),
            0,
            "drop all test streams before filter_test() to test crashed writer behavior"
        );
    }

    /// Run a filter test, returning the Inspector to check Inspect output.
    pub async fn filter_test(
        mut self,
        expected: impl IntoIterator<Item = LogMessage>,
        filter_options: Option<LogFilterOptions>,
    ) -> Inspector {
        self.check_pending_streams();
        validate_log_stream(expected, self.log_proxy, filter_options).await;
        self.inspector
    }

    pub async fn manager_test(mut self) {
        let lm1 = LogMessage {
            time: zx::BootInstant::get(),
            pid: 1,
            tid: 2,
            dropped_logs: 3,
            severity: Severity::Info as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let mut p = to_record(&lm1);
        let mut lm2 = copy_log_message(&lm1);
        let mut lm3 = copy_log_message(&lm1);
        let mut stream = self.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packet(p.clone());

        p.severity = Severity::Info as u8;
        lm2.severity = Severity::Info as i32;
        lm3.severity = Severity::Info as i32;
        stream.write_packet(p.clone());

        lm3.pid += 10;
        p = to_record(&lm3);
        stream.write_packet(p);
        drop(stream);
        self.check_pending_streams();
        validate_log_stream(vec![lm1, lm2, lm3], self.log_proxy, None).await;
    }

    /// Create a [`TestStream`] which should be dropped before calling `filter_test` or
    /// `manager_test`.
    pub fn create_stream_from_log_reader(
        &mut self,
        log_reader: Arc<dyn LogReader>,
    ) -> TestStream<StructuredMessageWriter> {
        self.make_stream(log_reader)
    }

    /// Create a [`TestStream`] which should be dropped before calling `filter_test` or
    /// `manager_test`.
    pub fn create_structured_stream(
        &mut self,
        identity: Arc<ComponentIdentity>,
    ) -> TestStream<StructuredMessageWriter> {
        self.make_stream(Arc::new(DefaultLogReader::new(
            Arc::clone(&self.log_manager),
            identity,
            self.scope.to_handle(),
        )))
    }

    fn make_stream<E, P>(&mut self, log_reader: Arc<dyn LogReader>) -> TestStream<E>
    where
        E: LogWriter<Packet = P>,
    {
        let _log_sink_proxy = log_reader.handle_request();

        let (sin, sout) = zx::Socket::create_datagram();
        E::connect(&_log_sink_proxy, sout);

        let _alive = Arc::new(());
        self.pending_streams.push(Arc::downgrade(&_alive));

        if let Some(sinks) = self.sinks.as_mut() {
            sinks.push(_log_sink_proxy.clone());
        }

        TestStream { _alive, _log_sink_proxy, sin, _encoder: PhantomData }
    }
}

/// A `LogWriter` can connect to and send `Packets` to a LogSink over a socket.
pub trait LogWriter {
    type Packet;
    fn connect(log_sink: &LogSinkProxy, sout: zx::Socket);

    fn write(sout: &zx::Socket, packet: Self::Packet);
}

/// A `LogWriter` that writes `Record` to a LogSink in the structured
/// log format.
pub struct StructuredMessageWriter;

impl LogWriter for StructuredMessageWriter {
    type Packet = Record<'static>;

    fn connect(log_sink: &LogSinkProxy, sin: zx::Socket) {
        log_sink.connect_structured(sin).expect("unable to connect out socket to log sink");
    }

    fn write(sin: &zx::Socket, record: Record<'_>) {
        let mut buffer = Cursor::new(vec![0; MAX_DATAGRAM_LEN]);
        let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
        encoder.write_record(record).unwrap();
        let slice = &buffer.get_ref()[..buffer.position() as usize];
        sin.write(slice).unwrap();
    }
}

/// A `LogReader` host a LogSink connection.
pub trait LogReader {
    fn handle_request(&self) -> LogSinkProxy;
}

// A LogReader that exercises the handle_log_sink code path.
pub struct DefaultLogReader {
    log_manager: Arc<LogsRepository>,
    identity: Arc<ComponentIdentity>,
    scope: fasync::ScopeHandle,
}

impl DefaultLogReader {
    fn new(
        log_manager: Arc<LogsRepository>,
        identity: Arc<ComponentIdentity>,
        scope: fasync::ScopeHandle,
    ) -> DefaultLogReader {
        Self { log_manager, identity, scope }
    }
}

impl LogReader for DefaultLogReader {
    fn handle_request(&self) -> LogSinkProxy {
        let (log_sink_proxy, log_sink_stream) =
            fidl::endpoints::create_proxy_and_stream::<LogSinkMarker>();
        let container = self.log_manager.get_log_container(Arc::clone(&self.identity));
        container.handle_log_sink(log_sink_stream, self.scope.clone());
        log_sink_proxy
    }
}

// A LogReader that exercises the components EventStream and CapabilityRequested event
// code path for log attribution.
pub struct EventStreamLogReader {
    log_manager: Arc<LogsRepository>,
    target_moniker: String,
    target_url: String,
    scope: fasync::ScopeHandle,
}

impl EventStreamLogReader {
    fn new(
        log_manager: Arc<LogsRepository>,
        target_moniker: impl Into<String>,
        target_url: impl Into<String>,
        scope: fasync::ScopeHandle,
    ) -> EventStreamLogReader {
        Self {
            log_manager,
            target_moniker: target_moniker.into(),
            target_url: target_url.into(),
            scope,
        }
    }

    async fn handle_event_stream(
        stream: fcomponent::EventStreamProxy,
        scope: fasync::ScopeHandle,
        log_manager: Arc<LogsRepository>,
    ) {
        while let Ok(res) = stream.get_next().await {
            for event in res {
                Self::handle_event(event, scope.clone(), Arc::clone(&log_manager))
            }
        }
    }

    fn handle_event(
        event: fcomponent::Event,
        scope: fasync::ScopeHandle,
        log_manager: Arc<LogsRepository>,
    ) {
        let LogSinkRequestedPayload { component, request_stream } =
            match event.try_into().expect("into component event") {
                Event { payload: EventPayload::LogSinkRequested(payload), .. } => payload,
                other => unreachable!("should never see {:?} here", other),
            };
        let container = log_manager.get_log_container(component);
        container.handle_log_sink(request_stream, scope);
    }
}

impl LogReader for EventStreamLogReader {
    fn handle_request(&self) -> LogSinkProxy {
        let (event_stream_proxy, mut event_stream) =
            fidl::endpoints::create_proxy_and_stream::<fcomponent::EventStreamMarker>();
        let (log_sink_proxy, log_sink_server_end) =
            fidl::endpoints::create_proxy::<LogSinkMarker>();

        let (tx, mut rx) = mpsc::unbounded();
        tx.unbounded_send(create_log_sink_requested_event(
            self.target_moniker.clone(),
            self.target_url.clone(),
            log_sink_server_end.into_channel(),
        ))
        .unwrap();
        self.scope.spawn(Self::handle_event_stream(
            event_stream_proxy,
            self.scope.clone(),
            Arc::clone(&self.log_manager),
        ));
        self.scope.spawn(async move {
            let _tx_clone = tx;
            while let Some(Ok(request)) = event_stream.next().await {
                match request {
                    fcomponent::EventStreamRequest::GetNext { responder } => {
                        responder.send(vec![rx.next().await.unwrap()]).unwrap();
                    }
                    fcomponent::EventStreamRequest::WaitForReady { responder } => {
                        responder.send().unwrap()
                    }
                }
            }
        });
        log_sink_proxy
    }
}

pub struct TestStream<E> {
    sin: zx::Socket,
    _alive: Arc<()>,
    _log_sink_proxy: LogSinkProxy,
    _encoder: PhantomData<E>,
}

impl<E, P> TestStream<E>
where
    E: LogWriter<Packet = P>,
{
    pub fn write_packets(&mut self, packets: Vec<P>) {
        for p in packets {
            self.write_packet(p);
        }
    }

    pub fn write_packet(&mut self, packet: P) {
        E::write(&self.sin, packet);
    }
}

/// Run a test on logs from klog, returning the inspector object.
pub async fn debuglog_test(
    expected: impl IntoIterator<Item = LogMessage>,
    debug_log: TestDebugLog,
    scope: fasync::Scope,
) -> Inspector {
    let inspector = Inspector::default();
    let lm = LogsRepository::new(
        create_ring_buffer(1_000_000),
        std::iter::empty(),
        inspector.root(),
        scope.new_child(),
    );
    let log_server = LogServer::new(Arc::clone(&lm), scope);
    let (log_proxy, log_stream) = fidl::endpoints::create_proxy_and_stream::<LogMarker>();
    log_server.spawn(log_stream);
    lm.drain_debuglog(debug_log);

    validate_log_stream(expected, log_proxy, None).await;
    inspector
}

pub fn to_record(
    LogMessage { pid, tid, time, severity, dropped_logs, tags, msg }: &LogMessage,
) -> Record<'static> {
    let mut arguments = vec![
        Argument::Pid(zx::Koid::from_raw(*pid)),
        Argument::Tid(zx::Koid::from_raw(*tid)),
        Argument::Dropped(*dropped_logs as u64),
        Argument::Message(Cow::Owned(msg.clone())),
    ];
    arguments.extend(tags.iter().map(|t| Argument::Tag(Cow::Owned(t.clone()))));
    Record { timestamp: *time, severity: *severity as RawSeverity, arguments }
}

pub fn copy_log_message(log_message: &LogMessage) -> LogMessage {
    LogMessage {
        pid: log_message.pid,
        tid: log_message.tid,
        time: log_message.time,
        severity: log_message.severity,
        dropped_logs: log_message.dropped_logs,
        tags: log_message.tags.clone(),
        msg: log_message.msg.clone(),
    }
}

/// A fake reader that returns enqueued responses on read.
pub struct TestDebugLog {
    read_responses: Mutex<VecDeque<ReadResponse>>,
}
type ReadResponse = Result<zx::DebugLogRecord, zx::Status>;

impl crate::logs::debuglog::DebugLog for TestDebugLog {
    fn read(&self) -> Result<zx::DebugLogRecord, zx::Status> {
        self.read_responses.lock().pop_front().expect("Got more read requests than enqueued")
    }

    async fn ready_signal(&self) -> Result<(), zx::Status> {
        if self.read_responses.lock().is_empty() {
            // ready signal should never complete if we have no logs left.
            futures::future::pending::<()>().await;
        }
        Ok(())
    }
}

impl Default for TestDebugLog {
    fn default() -> Self {
        TestDebugLog { read_responses: Mutex::new(VecDeque::new()) }
    }
}

impl TestDebugLog {
    pub fn enqueue_read(&self, response: zx::DebugLogRecord) {
        self.read_responses.lock().push_back(Ok(response));
    }

    pub fn enqueue_read_entry(&self, entry: &TestDebugEntry) {
        self.enqueue_read(entry.record);
    }

    pub fn enqueue_read_fail(&self, error: zx::Status) {
        self.read_responses.lock().push_back(Err(error))
    }
}

pub struct TestDebugEntry {
    pub record: zx::DebugLogRecord,
}

pub const TEST_KLOG_FLAGS: u8 = 47;
pub const TEST_KLOG_TIMESTAMP: i64 = 12345i64;
pub const TEST_KLOG_PID: u64 = 0xad01u64;
pub const TEST_KLOG_TID: u64 = 0xbe02u64;

impl TestDebugEntry {
    pub fn new(log_data: &[u8]) -> Self {
        static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let mut rec = zx::sys::zx_log_record_t::default();
        let len = rec.data.len().min(log_data.len());
        rec.header.sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        rec.header.datalen = len as u16;
        rec.header.flags = TEST_KLOG_FLAGS;
        rec.header.timestamp = TEST_KLOG_TIMESTAMP;
        rec.header.pid = TEST_KLOG_PID;
        rec.header.tid = TEST_KLOG_TID;
        rec.data[..len].copy_from_slice(&log_data[..len]);
        rec.header.severity = 0x30 /* info */;
        TestDebugEntry { record: zx::DebugLogRecord::from_raw(&rec).unwrap() }
    }

    pub fn new_with_severity(log_data: &[u8], severity: zx::DebugLogSeverity) -> Self {
        let mut this = Self::new(log_data);
        this.record.severity = severity;
        this
    }
}

/// Helper to connect to log sink and make it easy to write logs to socket.
pub struct LogSinkHelper {
    log_sink: Option<LogSinkProxy>,
    sock: Option<zx::Socket>,
}

impl LogSinkHelper {
    pub fn new(directory: &fio::DirectoryProxy) -> Self {
        let log_sink = connect_to_protocol_at_dir_svc::<LogSinkMarker>(directory)
            .expect("cannot connect to log sink");
        let mut s = Self { log_sink: Some(log_sink), sock: None };
        s.sock = Some(s.connect());
        s
    }

    pub fn connect(&self) -> zx::Socket {
        let (sin, sout) = zx::Socket::create_datagram();
        self.log_sink
            .as_ref()
            .unwrap()
            .connect_structured(sin)
            .expect("unable to send socket to log sink");
        sout
    }

    /// kills current sock and creates new connection.
    pub fn add_new_connection(&mut self) {
        self.kill_sock();
        self.sock = Some(self.connect());
    }

    pub fn kill_sock(&mut self) {
        self.sock.take();
    }

    pub fn write_log(&self, msg: &str) {
        Self::write_log_at(self.sock.as_ref().unwrap(), msg);
    }

    pub fn write_log_at(sock: &zx::Socket, msg: &str) {
        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut encoder = Encoder::new(Cursor::new(&mut buf[..]), EncoderOpts::default());

        struct LogEvent<'a>(&'a str);

        impl RecordEvent for LogEvent<'_> {
            fn raw_severity(&self) -> RawSeverity {
                Severity::Info as RawSeverity
            }

            fn file(&self) -> Option<&str> {
                unreachable!();
            }

            fn line(&self) -> Option<u32> {
                unreachable!();
            }

            fn target(&self) -> &str {
                unreachable!();
            }

            fn write_arguments<B: MutableBuffer>(
                self,
                encoder: &mut Encoder<B>,
            ) -> Result<(), EncodingError> {
                encoder.write_argument(Argument::message(self.0))?;
                Ok(())
            }

            fn timestamp(&self) -> zx::BootInstant {
                zx::BootInstant::get()
            }
        }

        let tags: &[&str] = &[];
        encoder
            .write_event(WriteEventParams {
                event: LogEvent(msg),
                tags,
                metatags: std::iter::empty(),
                pid: zx::Koid::from_raw(1),
                tid: zx::Koid::from_raw(1),
                dropped: 0,
            })
            .unwrap();
        let end = encoder.inner().cursor();
        let packet = &encoder.inner().get_ref()[..end];
        sock.write(packet).unwrap();
    }

    pub fn kill_log_sink(&mut self) {
        self.log_sink.take();
    }
}

pub fn make_message(msg: &str, tag: Option<&str>, timestamp: zx::BootInstant) -> StoredMessage {
    let mut record = Record {
        timestamp,
        severity: Severity::Debug as u8,
        arguments: vec![
            Argument::pid(zx::Koid::from_raw(1)),
            Argument::tid(zx::Koid::from_raw(2)),
            Argument::message(msg),
        ],
    };
    if let Some(tag) = tag {
        record.arguments.push(Argument::tag(tag));
    }
    let mut buffer = Cursor::new(vec![0u8; msg.len() + 128]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let encoded = &buffer.get_ref()[..buffer.position() as usize];
    StoredMessage::new(
        encoded.to_vec().into(),
        &Arc::new(crate::logs::stats::LogStreamStats::new(
            &fuchsia_inspect::Node::default(),
            &ComponentIdentity::unknown(),
        )),
    )
    .unwrap()
}
