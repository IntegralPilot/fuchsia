// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use diagnostics_log_encoding::encode::{
    Encoder, EncoderOpts, EncodingError, LogEvent, MutableBuffer, TestRecord, WriteEventParams,
};
use diagnostics_log_encoding::{Header, Metatag};
use fidl_fuchsia_logger::MAX_DATAGRAM_LEN_BYTES;
use fuchsia_runtime as rt;
use fuchsia_sync::RwLock;
use std::cell::UnsafeCell;
use std::collections::HashSet;
use std::io::Cursor;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// This is the amount of data that can be buffered by the BufferedPublisher before messages are
// dropped.
const QUEUE_SIZE: usize = 256 * 1024;

#[derive(Default)]
pub(crate) struct SinkConfig {
    pub(crate) metatags: HashSet<Metatag>,
    pub(crate) tags: Vec<String>,
    pub(crate) always_log_file_line: bool,
}

thread_local! {
    static PROCESS_ID: zx::Koid = rt::process_self()
        .koid()
        .unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX));
    static THREAD_ID: zx::Koid = rt::with_thread_self(|thread| {
        thread.koid().unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX))
    });
}

pub trait Sink {
    fn num_events_dropped(&self) -> &AtomicU32;
    fn config(&self) -> &SinkConfig;
    fn send(&self, packet: &[u8]) -> Result<(), zx::Status>;

    fn event_for_testing(&self, record: TestRecord<'_>) {
        self.encode_and_send(move |encoder, previously_dropped| {
            encoder.write_event(WriteEventParams {
                event: record,
                tags: &self.config().tags,
                metatags: std::iter::empty(),
                pid: PROCESS_ID.with(|p| *p),
                tid: THREAD_ID.with(|t| *t),
                dropped: previously_dropped.into(),
            })
        });
    }

    fn record_log(&self, record: &log::Record<'_>) {
        self.encode_and_send(|encoder, previously_dropped| {
            encoder.write_event(WriteEventParams {
                event: LogEvent::new(record),
                tags: &self.config().tags,
                metatags: self.config().metatags.iter(),
                pid: PROCESS_ID.with(|p| *p),
                tid: THREAD_ID.with(|t| *t),
                dropped: previously_dropped.into(),
            })
        });
    }

    #[inline]
    fn encode_and_send(
        &self,
        encode: impl FnOnce(&mut Encoder<Cursor<&mut [u8]>>, u32) -> Result<(), EncodingError>,
    ) {
        let ordering = Ordering::Relaxed;
        let num_events_dropped = self.num_events_dropped();
        let previously_dropped = num_events_dropped.swap(0, ordering);
        let restore_and_increment_dropped_count = || {
            num_events_dropped.fetch_add(previously_dropped + 1, ordering);
        };

        // TODO(https://fxbug.dev/466294903): Explore optimizations:
        // 1) Use uninitialized buffer to avoid zeroing.
        // 2) Potentially use stack allocation for smaller messages by first measuring
        //    the encoded size.

        let mut buf = vec![0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut encoder = Encoder::new(
            Cursor::new(&mut buf[..]),
            EncoderOpts { always_log_file_line: self.config().always_log_file_line },
        );
        if encode(&mut encoder, previously_dropped).is_err() {
            restore_and_increment_dropped_count();
            return;
        }

        let end = encoder.inner().cursor();
        let packet = &encoder.inner().get_ref()[..end];

        if packet.is_empty() || self.send(packet).is_err() {
            restore_and_increment_dropped_count();
        }
    }
}

pub struct IoBufferSink {
    iob: zx::Iob,
    num_events_dropped: AtomicU32,
    config: SinkConfig,
}

impl Sink for IoBufferSink {
    fn num_events_dropped(&self) -> &AtomicU32 {
        &self.num_events_dropped
    }

    fn config(&self) -> &SinkConfig {
        &self.config
    }

    fn send(&self, packet: &[u8]) -> Result<(), zx::Status> {
        self.iob.write(Default::default(), 0, packet)
    }
}

impl IoBufferSink {
    pub fn new(iob: zx::Iob, config: SinkConfig) -> Self {
        Self { iob, num_events_dropped: AtomicU32::new(0), config }
    }
}

pub struct BufferedSink {
    iob: OnceLock<zx::Iob>,
    buffer: RwLock<Option<Buffer>>,
    num_events_dropped: AtomicU32,
    config: SinkConfig,
}

struct Buffer {
    queue: Queue,
}

impl BufferedSink {
    pub fn new(config: SinkConfig) -> Self {
        Self {
            iob: OnceLock::new(),
            buffer: RwLock::new(Some(Buffer { queue: Queue::new() })),
            num_events_dropped: AtomicU32::new(0),
            config,
        }
    }

    /// Spawns a thread that is responsible for setting the buffer.
    pub fn set_buffer(&self, io_buffer: zx::Iob) {
        // Forward the outstanding messages in two phases. In the first phase, we forward the queue
        // without blocking other loggers.  In the second phase, we forward the queue whilst
        // blocking other loggers, which should hopefully be for a small amount of time.
        let queue = {
            let mut buffer = self.buffer.write();
            if let Some(Buffer { queue, .. }) = &mut *buffer {
                Some(std::mem::replace(queue, Queue::new()))
            } else {
                None
            }
        };

        let mut dropped = 0;
        if let Some(mut queue) = queue {
            self.forward_queue(&mut queue, &io_buffer, &mut dropped);
        }

        // This time, hold the lock until we've finished.
        let mut buffer = self.buffer.write();
        if let Some(Buffer { queue, .. }) = &mut *buffer {
            self.forward_queue(queue, &io_buffer, &mut dropped);
        }

        self.num_events_dropped.fetch_add(dropped, Ordering::Relaxed);

        self.iob.set(io_buffer).unwrap();
        *buffer = None;
    }

    fn forward_queue(&self, queue: &mut Queue, iob: &zx::Iob, dropped: &mut u32) {
        let mut slice = queue.as_slice();
        while !slice.is_empty() {
            let header = Header(u64::from_le_bytes(slice[..8].try_into().unwrap()));
            let message_len = header.size_words() as usize * 8;
            assert!(message_len > 0);
            if *dropped > 0 || iob.write(Default::default(), 0, &slice[..message_len]).is_err() {
                *dropped += 1;
            }
            slice = &slice[message_len..];
        }
    }
}

impl Sink for BufferedSink {
    fn num_events_dropped(&self) -> &AtomicU32 {
        &self.num_events_dropped
    }

    fn config(&self) -> &SinkConfig {
        &self.config
    }

    fn send(&self, packet: &[u8]) -> Result<(), zx::Status> {
        loop {
            if let Some(iob) = self.iob.get() {
                return iob.write(Default::default(), 0, packet);
            }

            let buffer = self.buffer.read();
            let Some(Buffer { queue, .. }) = &*buffer else {
                // We lost a race, loop and write with the IOBuffer.
                continue;
            };

            return if queue.push(packet) { Ok(()) } else { Err(zx::Status::NO_SPACE) };
        }
    }
}

struct Queue {
    buf: UnsafeCell<Box<[MaybeUninit<u8>]>>,
    len: AtomicUsize,
}

// SAFETY: `Queue` is made safe below.
unsafe impl Send for BufferedSink {}
unsafe impl Sync for BufferedSink {}

impl Queue {
    fn new() -> Self {
        Self { buf: UnsafeCell::new(Box::new_uninit_slice(QUEUE_SIZE)), len: AtomicUsize::new(0) }
    }

    fn capacity(&self) -> usize {
        // SAFETY: The length is immutable.
        unsafe { (&*self.buf.get()).len() }
    }

    /// Returns false if there is no capacity.
    fn push(&self, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        let mut len = self.len.load(Ordering::Relaxed);
        loop {
            if len + data.len() > self.capacity() {
                return false;
            }
            match self.len.compare_exchange_weak(
                len,
                len + data.len(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // SAFETY: We check bounds above, and thanks to the atomic update of len
                    // we can be sure there are no other concurrent writes to the same part
                    // of the buffer.
                    unsafe {
                        (*self.buf.get())
                            .as_mut_ptr()
                            .cast::<u8>()
                            .add(len)
                            .copy_from_nonoverlapping(data.as_ptr(), data.len());
                    }
                    return true;
                }
                Err(old) => len = old,
            }
        }
    }

    fn as_slice(&mut self) -> &[u8] {
        // SAFETY: This is safe because `len` is only updated above, but we have exclusive access
        // here, so we can be certain `len` bytes have been written to above.
        unsafe {
            std::slice::from_raw_parts(self.buf.get_mut().as_ptr().cast(), *self.len.get_mut())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{increment_clock, log_every_n_seconds};
    use diagnostics_log_encoding::parse::parse_record;
    use diagnostics_log_encoding::{Argument, Record};
    use diagnostics_log_types::Severity;
    use fuchsia_sync::Mutex;
    use futures::FutureExt;
    use log::{debug, error, info, trace, warn};
    use ring_buffer::{self, RING_BUFFER_MESSAGE_HEADER_SIZE, RingBuffer};
    use std::sync::Arc;
    use std::time::Duration;
    use test_util::assert_gt;

    const TARGET: &str = "diagnostics_log_lib_test::fuchsia::sink::tests";

    struct TestLogger {
        sink: IoBufferSink,
    }

    impl TestLogger {
        fn new(sink: IoBufferSink) -> Self {
            Self { sink }
        }
    }

    impl log::Log for TestLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                self.sink.record_log(record);
            }
        }

        fn flush(&self) {}
    }

    async fn init_sink(config: SinkConfig) -> ring_buffer::Reader {
        let ring_buffer = RingBuffer::create(32 * zx::system_get_page_size() as usize);
        let (iob, _) = ring_buffer.new_iob_writer(0).unwrap();

        let sink = IoBufferSink::new(iob, config);
        log::set_boxed_logger(Box::new(TestLogger::new(sink))).expect("set logger");
        log::set_max_level(log::LevelFilter::Info);

        ring_buffer
    }

    fn arg_prefix() -> Vec<Argument<'static>> {
        vec![Argument::pid(PROCESS_ID.with(|p| *p)), Argument::tid(THREAD_ID.with(|t| *t))]
    }

    #[fuchsia::test(logging = false)]
    async fn packets_are_sent() {
        let mut ring_buffer = init_sink(SinkConfig {
            metatags: HashSet::from([Metatag::Target]),
            ..SinkConfig::default()
        })
        .await;
        log::set_max_level(log::LevelFilter::Trace);

        let mut next_message = async move || {
            let (_tag, buf) = ring_buffer.read_message().await.unwrap();
            let (record, _) = parse_record(&buf).unwrap();
            assert_eq!(ring_buffer.head(), ring_buffer.tail(), "buffer must be empty");
            record.into_owned()
        };

        // emit some expected messages and then we'll retrieve them for parsing
        trace!(count = 123; "whoa this is noisy");
        let observed_trace = next_message().await;
        debug!(maybe = true; "don't try this at home");
        let observed_debug = next_message().await;
        info!("this is a message");
        let observed_info = next_message().await;
        warn!(reason = "just cuz"; "this is a warning");
        let observed_warn = next_message().await;
        error!(e = "something went pretty wrong"; "this is an error");
        let error_line = line!() - 1;
        let metatag = Argument::tag(TARGET);
        let observed_error = next_message().await;

        // TRACE
        {
            let mut expected_trace = Record {
                timestamp: observed_trace.timestamp,
                severity: Severity::Trace as u8,
                arguments: arg_prefix(),
            };
            expected_trace.arguments.push(metatag.clone());
            expected_trace.arguments.push(Argument::message("whoa this is noisy"));
            expected_trace.arguments.push(Argument::new("count", 123));
            assert_eq!(observed_trace, expected_trace);
        }

        // DEBUG
        {
            let mut expected_debug = Record {
                timestamp: observed_debug.timestamp,
                severity: Severity::Debug as u8,
                arguments: arg_prefix(),
            };
            expected_debug.arguments.push(metatag.clone());
            expected_debug.arguments.push(Argument::message("don't try this at home"));
            expected_debug.arguments.push(Argument::new("maybe", true));
            assert_eq!(observed_debug, expected_debug);
        }

        // INFO
        {
            let mut expected_info = Record {
                timestamp: observed_info.timestamp,
                severity: Severity::Info as u8,
                arguments: arg_prefix(),
            };
            expected_info.arguments.push(metatag.clone());
            expected_info.arguments.push(Argument::message("this is a message"));
            assert_eq!(observed_info, expected_info);
        }

        // WARN
        {
            let mut expected_warn = Record {
                timestamp: observed_warn.timestamp,
                severity: Severity::Warn as u8,
                arguments: arg_prefix(),
            };
            expected_warn.arguments.push(metatag.clone());
            expected_warn.arguments.push(Argument::message("this is a warning"));
            expected_warn.arguments.push(Argument::new("reason", "just cuz"));
            assert_eq!(observed_warn, expected_warn);
        }

        // ERROR
        {
            let mut expected_error = Record {
                timestamp: observed_error.timestamp,
                severity: Severity::Error as u8,
                arguments: arg_prefix(),
            };
            expected_error
                .arguments
                .push(Argument::file("src/lib/diagnostics/log/rust/src/fuchsia/sink.rs"));
            expected_error.arguments.push(Argument::line(error_line as u64));
            expected_error.arguments.push(metatag);
            expected_error.arguments.push(Argument::message("this is an error"));
            expected_error.arguments.push(Argument::new("e", "something went pretty wrong"));
            assert_eq!(observed_error, expected_error);
        }
    }

    #[fuchsia::test(logging = false)]
    async fn tags_are_sent() {
        let mut ring_buffer = init_sink(SinkConfig {
            tags: vec!["tags_are_sent".to_string()],
            ..SinkConfig::default()
        })
        .await;

        let mut next_message = async move || {
            let (_tag, buf) = ring_buffer.read_message().await.unwrap();
            let (record, _) = parse_record(&buf).unwrap();
            assert_eq!(ring_buffer.head(), ring_buffer.tail(), "buffer must be empty");
            record.into_owned()
        };

        info!("this should have a tag");
        let observed = next_message().await;

        let mut expected = Record {
            timestamp: observed.timestamp,
            severity: Severity::Info as u8,
            arguments: arg_prefix(),
        };
        expected.arguments.push(Argument::message("this should have a tag"));
        expected.arguments.push(Argument::tag("tags_are_sent"));
        assert_eq!(observed, expected);
    }

    #[fuchsia::test(logging = false)]
    async fn log_every_n_seconds_test() {
        let mut ring_buffer = init_sink(SinkConfig { ..SinkConfig::default() }).await;
        let mut next_message = async move || {
            let (_tag, buf) = ring_buffer.read_message().await.unwrap();
            let (record, _) = parse_record(&buf).unwrap();
            assert_eq!(ring_buffer.head(), ring_buffer.tail(), "buffer must be empty");
            record.into_owned()
        };

        let log_fn = || {
            log_every_n_seconds!(5, INFO, "test message");
        };

        let mut expect_message = async move || {
            let observed = next_message().await;

            let mut expected = Record {
                timestamp: observed.timestamp,
                severity: Severity::Info as u8,
                arguments: arg_prefix(),
            };
            expected.arguments.push(Argument::message("test message"));
            assert_eq!(observed, expected);
        };

        log_fn();
        // First log call should result in a message.
        expect_message().await;
        log_fn();
        // Subsequent log call in less than 5 seconds should NOT
        // result in a message.
        assert!(expect_message().now_or_never().is_none());
        increment_clock(Duration::from_secs(5));

        // Calling log_fn after 5 seconds should result in a message.
        log_fn();
        expect_message().await;
    }

    #[fuchsia::test(logging = false)]
    async fn drop_count_is_tracked() {
        let mut ring_buffer = init_sink(SinkConfig::default()).await;
        const MESSAGE_SIZE: usize = 104;
        const MESSAGE_SIZE_WITH_DROPS: usize = 136;
        const NUM_DROPPED: usize = 100;

        let emit_message = || info!("it's-a-me, a message-o");

        // Post one message and wait for it to appear.
        emit_message();
        ring_buffer.read_message().await.unwrap();

        // From now on, messages should get posted immediately.  Fill up the buffer.
        let mut num_emitted = 0;
        let buffer_space =
            || ring_buffer.capacity() - (ring_buffer.head() - ring_buffer.tail()) as usize;
        while buffer_space() >= RING_BUFFER_MESSAGE_HEADER_SIZE + MESSAGE_SIZE {
            emit_message();
            num_emitted += 1;
            assert_eq!(
                (ring_buffer.head() - ring_buffer.tail()) as usize,
                num_emitted * (RING_BUFFER_MESSAGE_HEADER_SIZE + MESSAGE_SIZE),
                "incorrect bytes stored after {} messages sent",
                num_emitted
            );
        }

        // drop messages
        for _ in 0..NUM_DROPPED {
            emit_message();
        }

        let mut drain_message = async |with_drops| {
            let (_tag, buf) = ring_buffer.read_message().await.unwrap();

            let expected_len = if with_drops { MESSAGE_SIZE_WITH_DROPS } else { MESSAGE_SIZE };
            assert_eq!(
                buf.len(),
                expected_len,
                "constant message size is used to calculate thresholds"
            );

            let (record, _) = parse_record(&buf).unwrap();
            let mut expected_args = arg_prefix();

            if with_drops {
                expected_args.push(Argument::dropped(NUM_DROPPED as u64));
            }

            expected_args.push(Argument::message("it's-a-me, a message-o"));

            assert_eq!(
                record,
                Record {
                    timestamp: record.timestamp,
                    severity: Severity::Info as u8,
                    arguments: expected_args
                }
            );
        };

        // make space for a message to convey the drop count
        // we drain two messages here because emitting the drop count adds to the size of the packet
        // if we only drain one message then we're relying on the kernel's buffer size to satisfy
        //   (rx_buf_max_size % MESSAGE_SIZE) > (MESSAGE_SIZE_WITH_DROPS - MESSAGE_SIZE)
        // this is true at the time of writing of this test but we don't know whether that's a
        // guarantee.
        drain_message(false).await;
        drain_message(false).await;
        // we use this count below to drain the rest of the messages
        num_emitted -= 2;
        // convey the drop count, it's now at the tail of the socket
        emit_message();
        // drain remaining "normal" messages ahead of the drop count
        for _ in 0..num_emitted {
            drain_message(false).await;
        }
        // verify that messages were dropped
        drain_message(true).await;

        // check that we return to normal after reporting the drops
        emit_message();
        drain_message(false).await;
        assert_eq!(ring_buffer.head(), ring_buffer.tail(), "must drain all messages");
    }

    #[fuchsia::test(logging = false)]
    async fn build_record_from_log_event() {
        let before_timestamp = zx::BootInstant::get();
        let last_record = Arc::new(Mutex::new(None));
        let logger = TrackerLogger::new(last_record.clone());
        log::set_boxed_logger(Box::new(logger)).expect("set logger");
        log::set_max_level(log::LevelFilter::Info);
        log::info!(
            is_a_str = "hahaha",
            is_debug:? = PrintMe(5),
            is_signed = -500,
            is_unsigned = 1000u64,
            is_bool = false;
            "blarg this is a message"
        );

        let guard = last_record.lock();
        let encoder = guard.as_ref().unwrap();
        let (record, _) = parse_record(encoder.inner().get_ref()).expect("wrote valid record");
        assert_gt!(record.timestamp, before_timestamp);
        assert_eq!(
            record,
            Record {
                timestamp: record.timestamp,
                severity: Severity::Info as u8,
                arguments: vec![
                    Argument::pid(PROCESS_ID.with(|p| *p)),
                    Argument::tid(THREAD_ID.with(|p| *p)),
                    Argument::tag("diagnostics_log_lib_test::fuchsia::sink::tests"),
                    Argument::message("blarg this is a message"),
                    Argument::other("is_a_str", "hahaha"),
                    Argument::other("is_debug", "PrintMe(5)"),
                    Argument::other("is_signed", -500),
                    Argument::other("is_unsigned", 1000u64),
                    Argument::other("is_bool", false),
                    Argument::tag("a-tag"),
                ]
            }
        );
    }

    // Note the inner u32 is used in the debug implementation.
    #[derive(Debug)]
    struct PrintMe(#[allow(unused)] u32);

    type ByteEncoder = Encoder<Cursor<[u8; 1024]>>;

    struct TrackerLogger {
        last_record: Arc<Mutex<Option<ByteEncoder>>>,
    }

    impl TrackerLogger {
        fn new(last_record: Arc<Mutex<Option<ByteEncoder>>>) -> Self {
            Self { last_record }
        }
    }

    impl log::Log for TrackerLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            let mut encoder = Encoder::new(Cursor::new([0u8; 1024]), EncoderOpts::default());
            encoder
                .write_event(WriteEventParams {
                    event: LogEvent::new(record),
                    tags: &["a-tag"],
                    metatags: [Metatag::Target].iter(),
                    pid: PROCESS_ID.with(|p| *p),
                    tid: THREAD_ID.with(|t| *t),
                    dropped: 0,
                })
                .expect("wrote event");
            let mut last_record = self.last_record.lock();
            last_record.replace(encoder);
        }

        fn flush(&self) {}
    }

    #[fuchsia::test]
    async fn buffered_sink() {
        const TAG: &str = "foo";
        let sink = Arc::new(BufferedSink::new(SinkConfig {
            tags: vec![TAG.to_string()],
            ..Default::default()
        }));
        const MSG: &str = "The quick brown fox jumped over the lazy dog.";
        const COUNT: usize = 1000;
        // Log from a different thread to test races.
        {
            let sink = Arc::clone(&sink);
            std::thread::spawn(move || {
                for i in 0..COUNT {
                    sink.record_log(
                        &log::Record::builder()
                            .level(log::Level::Warn)
                            .args(format_args!("{i}: {MSG}"))
                            .build(),
                    );
                }
            });
        }
        const MAX_RECORD_SIZE: usize = 176;
        // Include a 25% buffer.
        let mut ring_buffer = RingBuffer::create(
            (MAX_RECORD_SIZE * COUNT * 5 / 4).next_multiple_of(zx::system_get_page_size() as usize),
        );
        sink.set_buffer(ring_buffer.new_iob_writer(0).unwrap().0);
        // Now check that all the messages got written.
        for i in 0..COUNT {
            let (_tag, msg) = ring_buffer.read_message().await.unwrap();
            let (record, _) = parse_record(&msg).unwrap();
            assert_eq!(record.severity, Severity::Warn as u8);
            let mut found = 0;
            for arg in record.arguments {
                match arg {
                    Argument::Message(msg) => {
                        assert_eq!(msg, format!("{i}: {MSG}"));
                        assert_eq!(found & 1, 0);
                        found |= 1;
                    }
                    Argument::Tag(tag) => {
                        assert_eq!(tag, TAG);
                        assert_eq!(found & 2, 0);
                        found |= 2;
                    }
                    _ => {}
                }
            }
            assert_eq!(found, 3);
        }
    }
}
