// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

use crate::PublishError;
use diagnostics_log_encoding::Metatag;
use diagnostics_log_encoding::encode::{
    Encoder, EncoderOpts, EncodingError, LogEvent, MutableBuffer, TestRecord, WriteEventParams,
};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_logger::{LogSinkMarker, LogSinkSynchronousProxy, MAX_DATAGRAM_LEN_BYTES};
use fuchsia_runtime as rt;
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub(crate) struct SinkConfig {
    pub(crate) metatags: HashSet<Metatag>,
    pub(crate) retry_on_buffer_full: bool,
    pub(crate) tags: Vec<String>,
    pub(crate) always_log_file_line: bool,
}

thread_local! {
    static PROCESS_ID: zx::Koid =
        rt::process_self().koid().unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX));
    static THREAD_ID: zx::Koid = rt::with_thread_self(|thread| {
        thread.koid().unwrap_or_else(|_| zx::Koid::from_raw(zx::sys::zx_koid_t::MAX))
    });
}

pub(crate) struct Sink {
    socket: zx::Socket,
    num_events_dropped: AtomicU32,
    config: SinkConfig,
}

impl Sink {
    pub fn new(
        log_sink: &ClientEnd<LogSinkMarker>,
        config: SinkConfig,
    ) -> Result<Self, PublishError> {
        let (socket, remote_socket) = zx::Socket::create_datagram();
        let log_sink = zx::Unowned::<LogSinkSynchronousProxy>::new(log_sink.channel());
        log_sink.connect_structured(remote_socket).map_err(PublishError::SendSocket)?;
        Ok(Self { socket, config, num_events_dropped: AtomicU32::new(0) })
    }
}

impl Sink {
    #[inline]
    fn encode_and_send(
        &self,
        encode: impl FnOnce(&mut Encoder<Cursor<&mut [u8]>>, u32) -> Result<(), EncodingError>,
    ) {
        let ordering = Ordering::Relaxed;
        let previously_dropped = self.num_events_dropped.swap(0, ordering);
        let restore_and_increment_dropped_count = || {
            self.num_events_dropped.fetch_add(previously_dropped + 1, ordering);
        };

        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut encoder = Encoder::new(
            Cursor::new(&mut buf[..]),
            EncoderOpts { always_log_file_line: self.config.always_log_file_line },
        );
        if encode(&mut encoder, previously_dropped).is_err() {
            restore_and_increment_dropped_count();
            return;
        }

        let end = encoder.inner().cursor();
        let packet = &encoder.inner().get_ref()[..end];
        self.send(packet, restore_and_increment_dropped_count);
    }

    fn send(&self, packet: &[u8], on_error: impl Fn()) {
        while let Err(status) = self.socket.write(packet) {
            if status != zx::Status::SHOULD_WAIT || !self.config.retry_on_buffer_full {
                on_error();
                break;
            }
            let Ok(signals) = self
                .socket
                .wait_one(
                    zx::Signals::SOCKET_PEER_CLOSED | zx::Signals::SOCKET_WRITABLE,
                    zx::MonotonicInstant::INFINITE,
                )
                .to_result()
            else {
                on_error();
                break;
            };
            if signals.contains(zx::Signals::SOCKET_PEER_CLOSED) {
                on_error();
                break;
            }
        }
    }

    pub(crate) fn record_log(&self, record: &log::Record<'_>) {
        self.encode_and_send(|encoder, previously_dropped| {
            encoder.write_event(WriteEventParams {
                event: LogEvent::new(record),
                tags: &self.config.tags,
                metatags: self.config.metatags.iter(),
                pid: PROCESS_ID.with(|p| *p),
                tid: THREAD_ID.with(|t| *t),
                dropped: previously_dropped.into(),
            })
        });
    }

    pub fn event_for_testing(&self, record: TestRecord<'_>) {
        self.encode_and_send(move |encoder, previously_dropped| {
            encoder.write_event(WriteEventParams {
                event: record,
                tags: &self.config.tags,
                metatags: std::iter::empty(),
                pid: PROCESS_ID.with(|p| *p),
                tid: THREAD_ID.with(|t| *t),
                dropped: previously_dropped.into(),
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{increment_clock, log_every_n_seconds};
    use diagnostics_log_encoding::parse::parse_record;
    use diagnostics_log_encoding::{Argument, Record};
    use diagnostics_log_types::Severity;
    use fidl::endpoints::create_request_stream;
    use fidl_fuchsia_logger::{LogSinkMarker, LogSinkRequest};
    use fuchsia_sync::Mutex;
    use futures::AsyncReadExt;
    use futures::stream::StreamExt;
    use log::{debug, error, info, trace, warn};
    use std::sync::Arc;
    use std::time::Duration;
    use test_util::assert_gt;
    use zx::Status;

    const TARGET: &str = "diagnostics_log_lib_test::fuchsia::sink::tests";

    struct TestLogger {
        sink: Sink,
    }

    impl TestLogger {
        fn new(sink: Sink) -> Self {
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

    async fn init_sink(config: SinkConfig) -> fidl::Socket {
        let (client, mut requests) = create_request_stream::<LogSinkMarker>();
        let sink = Sink::new(&client, config).unwrap();
        log::set_boxed_logger(Box::new(TestLogger::new(sink))).expect("set logger");
        log::set_max_level(log::LevelFilter::Info);

        match requests.next().await.unwrap().unwrap() {
            LogSinkRequest::ConnectStructured { socket, .. } => socket,
            _ => panic!("sink ctor sent the wrong message"),
        }
    }

    fn arg_prefix() -> Vec<Argument<'static>> {
        vec![Argument::pid(PROCESS_ID.with(|p| *p)), Argument::tid(THREAD_ID.with(|t| *t))]
    }

    #[fuchsia::test(logging = false)]
    async fn wait_and_retry_is_possible() {
        // 160 writes so we write 5 MB given that we write 32K each write. Without enabling
        // retrying, this would lead to dropped logs.
        const TOTAL_WRITES: usize = 32 * 5;
        let (client, mut requests) = create_request_stream::<LogSinkMarker>();
        // Writes a megabyte of data to the Sink.
        std::thread::spawn(move || {
            let sink = Sink::new(
                &client,
                SinkConfig { retry_on_buffer_full: true, ..SinkConfig::default() },
            )
            .unwrap();
            for i in 0..TOTAL_WRITES {
                let buf = [i as u8; MAX_DATAGRAM_LEN_BYTES as _];
                sink.send(&buf, || unreachable!("We should never drop a log in this test"));
            }
        });
        let socket = match requests.next().await.unwrap().unwrap() {
            LogSinkRequest::ConnectStructured { socket, .. } => socket,
            _ => panic!("sink ctor sent the wrong message"),
        };
        let mut socket = fuchsia_async::Socket::from_socket(socket);
        // Ensure we are able to read all of the data written to the socket and we didn't drop
        // anything.
        for i in 0..TOTAL_WRITES {
            let mut buf = vec![0u8; MAX_DATAGRAM_LEN_BYTES as _];
            let len = socket.read(&mut buf).await.unwrap();
            assert_eq!(len, MAX_DATAGRAM_LEN_BYTES as usize);
            assert_eq!(buf, vec![i as u8; MAX_DATAGRAM_LEN_BYTES as _]);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[fuchsia::test(logging = false)]
    async fn packets_are_sent() {
        let socket = init_sink(SinkConfig {
            metatags: HashSet::from([Metatag::Target]),
            ..SinkConfig::default()
        })
        .await;
        log::set_max_level(log::LevelFilter::Trace);
        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut next_message = || {
            let len = socket.read(&mut buf).unwrap();
            let (record, _) = parse_record(&buf[..len]).unwrap();
            assert_eq!(socket.outstanding_read_bytes().unwrap(), 0, "socket must be empty");
            record.into_owned()
        };

        // emit some expected messages and then we'll retrieve them for parsing
        trace!(count = 123; "whoa this is noisy");
        let observed_trace = next_message();
        debug!(maybe = true; "don't try this at home");
        let observed_debug = next_message();
        info!("this is a message");
        let observed_info = next_message();
        warn!(reason = "just cuz"; "this is a warning");
        let observed_warn = next_message();
        error!(e = "something went pretty wrong"; "this is an error");
        let error_line = line!() - 1;
        let metatag = Argument::tag(TARGET);
        let observed_error = next_message();

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
        let socket = init_sink(SinkConfig {
            tags: vec!["tags_are_sent".to_string()],
            ..SinkConfig::default()
        })
        .await;
        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let mut next_message = || {
            let len = socket.read(&mut buf).unwrap();
            let (record, _) = parse_record(&buf[..len]).unwrap();
            assert_eq!(socket.outstanding_read_bytes().unwrap(), 0, "socket must be empty");
            record.into_owned()
        };

        info!("this should have a tag");
        let observed = next_message();

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
        let socket = init_sink(SinkConfig { ..SinkConfig::default() }).await;
        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        let next_message = |buf: &mut [u8]| {
            let len = socket.read(buf).unwrap();
            let (record, _) = parse_record(&buf[..len]).unwrap();
            assert_eq!(socket.outstanding_read_bytes().unwrap(), 0, "socket must be empty");
            record.into_owned()
        };

        let log_fn = || {
            log_every_n_seconds!(5, INFO, "test message");
        };

        let expect_message = |buf: &mut [u8]| {
            let observed = next_message(buf);

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
        expect_message(&mut buf);
        log_fn();
        // Subsequent log call in less than 5 seconds should NOT
        // result in a message.
        assert_eq!(socket.read(&mut buf), Err(Status::SHOULD_WAIT));
        increment_clock(Duration::from_secs(5));

        // Calling log_fn after 5 seconds should result in a message.
        log_fn();
        expect_message(&mut buf);
    }

    #[fuchsia::test(logging = false)]
    async fn drop_count_is_tracked() {
        let socket = init_sink(SinkConfig::default()).await;
        let mut buf = [0u8; MAX_DATAGRAM_LEN_BYTES as _];
        const MESSAGE_SIZE: usize = 104;
        const MESSAGE_SIZE_WITH_DROPS: usize = 136;
        const NUM_DROPPED: usize = 100;

        let socket_capacity = || {
            let info = socket.info().unwrap();
            info.rx_buf_max - info.rx_buf_size
        };
        let emit_message = || info!("it's-a-me, a message-o");
        let mut drain_message = |with_drops| {
            let len = socket.read(&mut buf).unwrap();

            let expected_len = if with_drops { MESSAGE_SIZE_WITH_DROPS } else { MESSAGE_SIZE };
            assert_eq!(len, expected_len, "constant message size is used to calculate thresholds");

            let (record, _) = parse_record(&buf[..len]).unwrap();
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

        // fill up the socket
        let mut num_emitted = 0;
        while socket_capacity() > MESSAGE_SIZE {
            emit_message();
            num_emitted += 1;
            assert_eq!(
                socket.info().unwrap().rx_buf_size,
                num_emitted * MESSAGE_SIZE,
                "incorrect bytes stored after {} messages sent",
                num_emitted
            );
        }

        // drop messages
        for _ in 0..NUM_DROPPED {
            emit_message();
        }

        // make space for a message to convey the drop count
        // we drain two messages here because emitting the drop count adds to the size of the packet
        // if we only drain one message then we're relying on the kernel's buffer size to satisfy
        //   (rx_buf_max_size % MESSAGE_SIZE) > (MESSAGE_SIZE_WITH_DROPS - MESSAGE_SIZE)
        // this is true at the time of writing of this test but we don't know whether that's a
        // guarantee.
        drain_message(false);
        drain_message(false);
        // we use this count below to drain the rest of the messages
        num_emitted -= 2;
        // convey the drop count, it's now at the tail of the socket
        emit_message();
        // drain remaining "normal" messages ahead of the drop count
        for _ in 0..num_emitted {
            drain_message(false);
        }
        // verify that messages were dropped
        drain_message(true);

        // check that we return to normal after reporting the drops
        emit_message();
        drain_message(false);
        assert_eq!(socket.outstanding_read_bytes().unwrap(), 0, "must drain all messages");
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
}
