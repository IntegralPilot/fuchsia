// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod container;
pub mod debuglog;
pub mod error;
pub mod listener;
pub mod repository;
pub mod serial;
pub mod servers;
pub mod shared_buffer;
pub mod stats;
pub mod stored_message;
#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests {
    use crate::identity::ComponentIdentity;
    use crate::logs::testing::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use diagnostics_log_encoding::{Argument, Record};
    use diagnostics_log_types::Severity;
    use fidl_fuchsia_logger::{LogFilterOptions, LogLevelFilter, LogMessage};
    use fuchsia_async as fasync;
    use moniker::ExtendedMoniker;
    use std::sync::Arc;

    #[fuchsia::test]
    async fn test_log_manager() {
        Box::pin(TestHarness::default().manager_test()).await;
    }

    #[fuchsia::test]
    async fn unfiltered_stats() {
        let first_message = LogMessage {
            pid: 1,
            tid: 2,
            time: zx::BootInstant::get(),
            dropped_logs: 3,
            severity: Severity::Warn as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let first_packet = to_record(&first_message);

        let mut second_message = first_message.clone();
        second_message.pid = 0;
        let second_packet = to_record(&second_message);

        let mut third_message = second_message.clone();
        third_message.severity = Severity::Info as i32;
        let third_packet = to_record(&third_message);

        let (fourth_packet, fourth_message) = (third_packet.clone(), third_message.clone());

        let mut fifth_message = fourth_message.clone();
        fifth_message.severity = Severity::Error as i32;
        let fifth_packet = to_record(&fifth_message);

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![
            first_packet,
            second_packet,
            third_packet,
            fourth_packet,
            fifth_packet,
        ]);
        drop(stream);

        let log_stats_tree = harness
            .filter_test(
                vec![first_message, second_message, third_message, fourth_message, fifth_message],
                None,
            )
            .await;

        assert_data_tree!(
            log_stats_tree,
            root: contains {
                log_sources: {
                    "UNKNOWN": {
                        url: "fuchsia-pkg://UNKNOWN",
                        last_timestamp: AnyProperty,
                        sockets_closed: 1u64,
                        sockets_opened: 1u64,
                        invalid: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        total: {
                            number: 5u64,
                            bytes: AnyProperty,
                        },
                        rolled_out: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        trace: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        debug: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        info: {
                            number: 2u64,
                            bytes: AnyProperty,
                        },
                        warn: {
                            number: 2u64,
                            bytes: AnyProperty,
                        },
                        error: {
                            number: 1u64,
                            bytes: AnyProperty,
                        },
                        fatal: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                    },
                }
            }
        );
    }

    macro_rules! attributed_inspect_two_streams_different_identities_by_reader {
        (
            $harness:ident,
            $log_reader1:ident @ $foo_moniker:literal,
            $log_reader2:ident @ $bar_moniker:literal,
        ) => {{
            let message = LogMessage {
                pid: 1,
                tid: 2,
                time: zx::BootInstant::get(),
                dropped_logs: 3,
                severity: Severity::Warn as i32,
                msg: String::from("BBBBB"),
                tags: vec![String::from("AAAAA")],
            };
            let packet = to_record(&message);

            let mut message2 = message.clone();
            message2.severity = Severity::Error as i32;
            let packet2 = to_record(&message2);

            let mut foo_stream = $harness.create_stream_from_log_reader($log_reader1);
            foo_stream.write_packet(packet);

            let mut bar_stream = $harness.create_stream_from_log_reader($log_reader2);
            bar_stream.write_packet(packet2);
            drop((foo_stream, bar_stream));

            let log_stats_tree = $harness.filter_test(vec![message, message2], None).await;

            assert_data_tree!(
                log_stats_tree,
                root: contains {
                    log_sources: {
                        $foo_moniker: {
                            url: "http://foo.com",
                            last_timestamp: AnyProperty,
                            sockets_closed: 1u64,
                            sockets_opened: 1u64,
                            invalid: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            total: {
                                number: 1u64,
                                bytes: AnyProperty,
                            },
                            rolled_out: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            trace: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            debug: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            info: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            warn: {
                                number: 1u64,
                                bytes: AnyProperty,
                            },
                            error: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            fatal: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                        },
                        $bar_moniker: {
                            url: "http://bar.com",
                            last_timestamp: AnyProperty,
                            sockets_closed: 1u64,
                            sockets_opened: 1u64,
                            invalid: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            total: {
                                number: 1u64,
                                bytes: AnyProperty,
                            },
                            rolled_out: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            trace: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            debug: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            info: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            warn: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                            error: {
                                number: 1u64,
                                bytes: AnyProperty,
                            },
                            fatal: {
                                number: 0u64,
                                bytes: 0u64,
                            },
                        },
                    },
                }
            );
        }}
    }

    #[fuchsia::test]
    async fn attributed_inspect_two_streams_different_identities() {
        let mut harness = TestHarness::with_retained_sinks();

        let log_reader1 = harness.create_default_reader(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./foo").unwrap(),
            "http://foo.com",
        ));

        let log_reader2 = harness.create_default_reader(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "http://bar.com",
        ));

        attributed_inspect_two_streams_different_identities_by_reader!(
            harness,
            log_reader1 @ "foo",
            log_reader2 @ "bar",
        );
    }

    #[fuchsia::test]
    async fn attributed_inspect_two_v2_streams_different_identities() {
        let mut harness = TestHarness::with_retained_sinks();
        let log_reader1 = harness.create_event_stream_reader("./foo", "http://foo.com");
        let log_reader2 = harness.create_event_stream_reader("./bar", "http://bar.com");

        attributed_inspect_two_streams_different_identities_by_reader!(
            harness,
            log_reader1 @ "foo",
            log_reader2 @ "bar",
        );
    }

    #[fuchsia::test]
    async fn attributed_inspect_two_mixed_streams_different_identities() {
        let mut harness = TestHarness::with_retained_sinks();
        let log_reader1 = harness.create_event_stream_reader("./foo", "http://foo.com");
        let log_reader2 = harness.create_default_reader(ComponentIdentity::new(
            ExtendedMoniker::parse_str("./bar").unwrap(),
            "http://bar.com",
        ));

        attributed_inspect_two_streams_different_identities_by_reader!(
            harness,
            log_reader1 @ "foo",
            log_reader2 @ "bar",
        );
    }

    #[fuchsia::test]
    async fn test_filter_by_pid() {
        let lm = LogMessage {
            pid: 1,
            tid: 2,
            time: zx::BootInstant::get(),
            dropped_logs: 3,
            severity: Severity::Info as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let p = to_record(&lm);
        let lm2 = LogMessage { pid: 11, ..lm.clone() };
        let p2 = to_record(&lm2);
        let options = LogFilterOptions {
            filter_by_pid: true,
            pid: 1,
            filter_by_tid: false,
            tid: 0,
            min_severity: LogLevelFilter::None,
            verbosity: 0,
            tags: vec![],
        };

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![p, p2]);
        drop(stream);
        harness.filter_test(vec![lm], Some(options)).await;
    }

    #[fuchsia::test]
    async fn test_filter_by_tid() {
        let lm = LogMessage {
            pid: 1,
            tid: 2,
            time: zx::BootInstant::get(),
            dropped_logs: 3,
            severity: Severity::Info as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let p = to_record(&lm);
        let lm2 = LogMessage { tid: 12, ..lm.clone() };
        let p2 = to_record(&lm2);
        let options = LogFilterOptions {
            filter_by_pid: false,
            pid: 1,
            filter_by_tid: true,
            tid: 2,
            min_severity: LogLevelFilter::None,
            verbosity: 0,
            tags: vec![],
        };

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![p, p2]);
        drop(stream);
        harness.filter_test(vec![lm], Some(options)).await;
    }

    #[fuchsia::test]
    async fn test_filter_by_min_severity() {
        let lm = LogMessage {
            pid: 0,
            tid: 0,
            time: zx::BootInstant::get(),
            dropped_logs: 2,
            severity: Severity::Error as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let p = to_record(&LogMessage { severity: Severity::Warn as i32, ..lm.clone() });
        let p2 = to_record(&lm);
        let p3 = to_record(&LogMessage { severity: Severity::Info as i32, ..lm.clone() });
        let p4 = to_record(&LogMessage { severity: 0x70, ..lm.clone() });
        let p5 = to_record(&LogMessage { severity: Severity::Fatal as i32, ..lm.clone() });

        let options = LogFilterOptions {
            filter_by_pid: false,
            pid: 1,
            filter_by_tid: false,
            tid: 1,
            min_severity: LogLevelFilter::Error,
            verbosity: 0,
            tags: vec![],
        };

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![p, p2, p3, p4, p5]);
        drop(stream);
        harness.filter_test(vec![lm], Some(options)).await;
    }

    #[fuchsia::test]
    async fn test_filter_by_combination() {
        let lm = LogMessage {
            pid: 0,
            tid: 0,
            time: zx::BootInstant::get(),
            dropped_logs: 2,
            severity: Severity::Error as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("AAAAA")],
        };
        let p = to_record(&LogMessage { severity: Severity::Warn as i32, ..lm.clone() });
        let p2 = to_record(&lm);
        let p3 = to_record(&LogMessage { pid: 1, ..lm.clone() });
        let options = LogFilterOptions {
            filter_by_pid: true,
            pid: 0,
            filter_by_tid: false,
            tid: 1,
            min_severity: LogLevelFilter::Error,
            verbosity: 0,
            tags: vec![],
        };

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![p, p2, p3]);
        drop(stream);
        harness.filter_test(vec![lm], Some(options)).await;
    }

    #[fuchsia::test]
    async fn test_filter_by_tags() {
        let lm1 = LogMessage {
            pid: 1,
            tid: 1,
            time: zx::BootInstant::get(),
            dropped_logs: 2,
            severity: Severity::Warn as i32,
            msg: String::from("BBBBB"),
            tags: vec![String::from("DDDDD")],
        };
        let p = to_record(&lm1);

        let lm2 = LogMessage {
            pid: 0,
            tid: 0,
            time: zx::BootInstant::get(),
            dropped_logs: 2,
            severity: Severity::Warn as i32,
            msg: String::from("CCCCC"),
            tags: vec![String::from("AAAAA"), String::from("BBBBB")],
        };
        let p2 = to_record(&lm2);

        let options = LogFilterOptions {
            filter_by_pid: false,
            pid: 1,
            filter_by_tid: false,
            tid: 1,
            min_severity: LogLevelFilter::None,
            verbosity: 0,
            tags: vec![String::from("BBBBB"), String::from("DDDDD")],
        };

        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(vec![p, p2]);
        drop(stream);
        harness.filter_test(vec![lm1, lm2], Some(options)).await;
    }

    #[fuchsia::test]
    async fn test_structured_log() {
        let logs = vec![
            Record {
                timestamp: zx::BootInstant::from_nanos(6),
                severity: Severity::Info as u8,
                arguments: vec![Argument::message("hi")],
            },
            Record {
                timestamp: zx::BootInstant::from_nanos(13),
                severity: Severity::Error as u8,
                arguments: vec![],
            },
            Record {
                timestamp: zx::BootInstant::from_nanos(19),
                severity: Severity::Warn as u8,
                arguments: vec![
                    Argument::pid(zx::Koid::from_raw(0x1d1)),
                    Argument::tid(zx::Koid::from_raw(0x1d2)),
                    Argument::dropped(23),
                    Argument::tag("tag"),
                    Argument::message("message"),
                ],
            },
            Record {
                timestamp: zx::BootInstant::from_nanos(21),
                severity: Severity::Warn as u8,
                arguments: vec![Argument::tag("tag-1"), Argument::tag("tag-2")],
            },
        ];

        let expected_logs = vec![
            LogMessage {
                pid: zx::sys::ZX_KOID_INVALID,
                tid: zx::sys::ZX_KOID_INVALID,
                time: zx::BootInstant::from_nanos(6),
                severity: LogLevelFilter::Info as i32,
                dropped_logs: 0,
                msg: String::from("hi"),
                tags: vec!["UNKNOWN".to_owned()],
            },
            LogMessage {
                pid: zx::sys::ZX_KOID_INVALID,
                tid: zx::sys::ZX_KOID_INVALID,
                time: zx::BootInstant::from_nanos(14),
                severity: LogLevelFilter::Error as i32,
                dropped_logs: 0,
                msg: String::from(""),
                tags: vec!["UNKNOWN".to_owned()],
            },
            LogMessage {
                pid: 0x1d1,
                tid: 0x1d2,
                time: zx::BootInstant::from_nanos(19),
                severity: LogLevelFilter::Warn as i32,
                dropped_logs: 23,
                msg: String::from("message"),
                tags: vec![String::from("tag")],
            },
            LogMessage {
                pid: zx::sys::ZX_KOID_INVALID,
                tid: zx::sys::ZX_KOID_INVALID,
                time: zx::BootInstant::from_nanos(21),
                severity: LogLevelFilter::Warn as i32,
                dropped_logs: 0,
                msg: String::from(""),
                tags: vec![String::from("tag-1"), String::from("tag-2")],
            },
        ];
        let mut harness = TestHarness::default();
        let mut stream = harness.create_structured_stream(Arc::new(ComponentIdentity::unknown()));
        stream.write_packets(logs);
        drop(stream);
        harness.filter_test(expected_logs, None).await;
    }

    #[fuchsia::test]
    async fn test_debuglog_drainer() {
        let log1 = TestDebugEntry::new("log1".as_bytes());
        let log2 = TestDebugEntry::new("log2".as_bytes());
        let log3 = TestDebugEntry::new("log3".as_bytes());

        let klog_reader = TestDebugLog::default();
        klog_reader.enqueue_read_entry(&log1);
        klog_reader.enqueue_read_entry(&log2);
        // logs received after kernel indicates no logs should be read
        klog_reader.enqueue_read_fail(zx::Status::SHOULD_WAIT);
        klog_reader.enqueue_read_entry(&log3);
        klog_reader.enqueue_read_fail(zx::Status::SHOULD_WAIT);

        let expected_logs = vec![
            LogMessage {
                pid: log1.record.pid.raw_koid(),
                tid: log1.record.tid.raw_koid(),
                time: log1.record.timestamp,
                dropped_logs: 0,
                severity: LogLevelFilter::Info as i32,
                msg: String::from("log1"),
                tags: vec![String::from("klog")],
            },
            LogMessage {
                pid: log2.record.pid.raw_koid(),
                tid: log2.record.tid.raw_koid(),
                time: log2.record.timestamp,
                dropped_logs: 0,
                severity: LogLevelFilter::Info as i32,
                msg: String::from("log2"),
                tags: vec![String::from("klog")],
            },
            LogMessage {
                pid: log3.record.pid.raw_koid(),
                tid: log3.record.tid.raw_koid(),
                time: log3.record.timestamp,
                dropped_logs: 0,
                severity: LogLevelFilter::Info as i32,
                msg: String::from("log3"),
                tags: vec![String::from("klog")],
            },
        ];

        let scope = fasync::Scope::new();
        let klog_stats_tree = debuglog_test(expected_logs, klog_reader, scope.new_child()).await;
        assert_data_tree!(
            klog_stats_tree,
            root: contains {
                log_sources: {
                    "klog": {
                        url: "fuchsia-boot://kernel",
                        last_timestamp: AnyProperty,
                        sockets_closed: 0u64,
                        sockets_opened: 0u64,
                        invalid: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        total: {
                            number: 3u64,
                            bytes: AnyProperty,
                        },
                        rolled_out: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        trace: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        debug: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        info: {
                            number: 3u64,
                            bytes: AnyProperty,
                        },
                        warn: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        error: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                        fatal: {
                            number: 0u64,
                            bytes: 0u64,
                        },
                    },
                }
            }
        );
    }
}
