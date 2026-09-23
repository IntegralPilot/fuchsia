// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg_attr(
    target_arch = "aarch64",
    allow(clippy::unnecessary_cast, reason = "mass allow for https://fxbug.dev/381896734")
)]

use super::*;
use assert_matches::assert_matches;
use diagnostics_log_encoding::Record;
use diagnostics_log_encoding::encode::{Encoder, EncoderOpts};
use fidl_fuchsia_diagnostics_types::Severity as StreamSeverity;
use fidl_fuchsia_logger::{LogLevelFilter, LogMessage};
use std::io::Cursor;
use std::sync::{Arc, LazyLock};

static TEST_IDENTITY: LazyLock<Arc<MonikerWithUrl>> = LazyLock::new(|| {
    Arc::new(MonikerWithUrl {
        moniker: "fake-test-env/test-component".try_into().unwrap(),
        url: "fuchsia-pkg://fuchsia.com/testing123#test-component.cm".into(),
    })
});

#[repr(C, packed)]
struct fx_log_metadata_t_packed {
    pid: zx::sys::zx_koid_t,
    tid: zx::sys::zx_koid_t,
    time: zx::sys::zx_time_t,
    severity: fx_log_severity_t,
    dropped_logs: u32,
}

#[repr(C, packed)]
struct fx_log_packet_t_packed {
    metadata: fx_log_metadata_t_packed,
    /// Contains concatenated tags and message and a null terminating character at the end.
    /// `char(tag_len) + "tag1" + char(tag_len) + "tag2\0msg\0"`
    data: [c_char; MAX_DATAGRAM_LEN - METADATA_SIZE],
}

#[fuchsia::test]
fn abi_test() {
    assert_eq!(METADATA_SIZE, 32);
    assert_eq!(MAX_TAGS, 5);
    assert_eq!(MAX_TAG_LEN, 64);
    assert_eq!(mem::size_of::<fx_log_metadata_t>(), METADATA_SIZE);
    assert_eq!(mem::size_of::<fx_log_packet_t>(), MAX_DATAGRAM_LEN);

    // Test that there is no padding
    assert_eq!(mem::size_of::<fx_log_packet_t>(), mem::size_of::<fx_log_packet_t_packed>());

    assert_eq!(mem::size_of::<fx_log_metadata_t>(), mem::size_of::<fx_log_metadata_t_packed>());
}
fn test_packet() -> fx_log_packet_t {
    let mut packet: fx_log_packet_t = Default::default();
    packet.metadata.pid = 1;
    packet.metadata.tid = 2;
    packet.metadata.time = 3;
    packet.metadata.severity = LogLevelFilter::Debug as i32;
    packet.metadata.dropped_logs = 10;
    packet
}

fn get_test_identity() -> MonikerWithUrl {
    (**TEST_IDENTITY).clone()
}

#[fuchsia::test]
fn short_reads() {
    let packet = test_packet();
    let one_short = &packet.as_bytes()[..METADATA_SIZE];
    let two_short = &packet.as_bytes()[..METADATA_SIZE - 1];

    assert_eq!(LoggerMessage::try_from(one_short), Err(MessageError::ShortRead { len: 32 }));

    assert_eq!(LoggerMessage::try_from(two_short), Err(MessageError::ShortRead { len: 31 }));
}

#[fuchsia::test]
fn invalid_utf8_is_handled() {
    let mut packet = test_packet();
    // No tags
    packet.data[0] = 0;
    // Start message
    let bytes = b"This is \xF0\x90\x80 invalid";
    for i in 1..=bytes.len() {
        packet.data[i] = bytes[i - 1] as c_char;
    }
    packet.data[bytes.len() + 1] = 0;
    let buffer = &packet.as_bytes()[..METADATA_SIZE + bytes.len() + 2];
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    let parsed = crate::from_logger(get_test_identity(), logger_message);
    let expected = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Debug,
    })
    .set_dropped(packet.metadata.dropped_logs.into())
    .set_pid(packet.metadata.pid)
    .set_message("This is � invalid".to_string())
    .set_tid(packet.metadata.tid)
    .build();
    assert_eq!(parsed, expected);
}

#[fuchsia::test]
fn unterminated() {
    let mut packet = test_packet();
    let end = 9;
    packet.data[end] = 1;

    let buffer = &packet.as_bytes()[..MIN_PACKET_SIZE + end];
    let parsed = LoggerMessage::try_from(buffer);

    assert_eq!(parsed, Err(MessageError::NotNullTerminated { terminator: 1 }));
}

#[fuchsia::test]
fn tags_no_message() {
    let mut packet = test_packet();
    let end = 12;
    packet.data[0] = end as c_char - 1;
    packet.fill_data(1..end, b'A' as c_char);
    packet.data[end] = 0;

    let buffer = &packet.as_bytes()[..MIN_PACKET_SIZE + end]; // omit null-terminated
    let parsed = LoggerMessage::try_from(buffer);

    assert_eq!(parsed, Err(MessageError::OutOfBounds));
}

#[fuchsia::test]
fn tags_with_message() {
    let mut packet = test_packet();
    let a_start = 1;
    let a_count = 11;
    let a_end = a_start + a_count;

    packet.data[0] = a_count as c_char;
    packet.fill_data(a_start..a_end, b'A' as c_char);
    packet.data[a_end] = 0; // terminate tags

    let b_start = a_start + a_count + 1;
    let b_count = 5;
    let b_end = b_start + b_count;
    packet.fill_data(b_start..b_end, b'B' as c_char);

    let data_size = b_start + b_count;

    let buffer = &packet.as_bytes()[..METADATA_SIZE + data_size + 1]; // null-terminate message
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + b_end);
    let parsed = crate::from_logger(get_test_identity(), logger_message);
    let expected = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Debug,
    })
    .set_dropped(packet.metadata.dropped_logs.into())
    .set_pid(packet.metadata.pid)
    .set_message("BBBBB".to_string())
    .add_tag("AAAAAAAAAAA")
    .set_tid(packet.metadata.tid)
    .build();

    assert_eq!(parsed, expected);
}

#[fuchsia::test]
fn two_tags_no_message() {
    let mut packet = test_packet();
    let a_start = 1;
    let a_count = 11;
    let a_end = a_start + a_count;

    packet.data[0] = a_count as c_char;
    packet.fill_data(a_start..a_end, b'A' as c_char);

    let b_start = a_end + 1;
    let b_count = 5;
    let b_end = b_start + b_count;

    packet.data[a_end] = b_count as c_char;
    packet.fill_data(b_start..b_end, b'B' as c_char);

    let buffer = &packet.as_bytes()[..MIN_PACKET_SIZE + b_end];
    let parsed = LoggerMessage::try_from(buffer);

    assert_eq!(parsed, Err(MessageError::OutOfBounds));
}

#[fuchsia::test]
fn two_tags_with_message() {
    let mut packet = test_packet();
    let a_start = 1;
    let a_count = 11;
    let a_end = a_start + a_count;

    packet.data[0] = a_count as c_char;
    packet.fill_data(a_start..a_end, b'A' as c_char);

    let b_start = a_end + 1;
    let b_count = 5;
    let b_end = b_start + b_count;

    packet.data[a_end] = b_count as c_char;
    packet.fill_data(b_start..b_end, b'B' as c_char);

    let c_start = b_end + 1;
    let c_count = 5;
    let c_end = c_start + c_count;
    packet.fill_data(c_start..c_end, b'C' as c_char);

    let data_size = c_start + c_count;

    let buffer = &packet.as_bytes()[..METADATA_SIZE + data_size + 1]; // null-terminated
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + data_size);
    let parsed = crate::from_logger(get_test_identity(), logger_message);
    let expected = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Debug,
    })
    .set_dropped(packet.metadata.dropped_logs.into())
    .set_pid(packet.metadata.pid)
    .set_message("CCCCC".to_string())
    .add_tag("AAAAAAAAAAA")
    .add_tag("BBBBB")
    .set_tid(packet.metadata.tid)
    .build();

    assert_eq!(parsed, expected);
}

#[fuchsia::test]
fn max_tags_with_message() {
    let mut packet = test_packet();

    let tags_start = 1;
    let tag_len = 2;
    let tag_size = tag_len + 1; // the length-prefix byte
    for tag_num in 0..MAX_TAGS {
        let start = tags_start + (tag_size * tag_num);
        let end = start + tag_len;

        packet.data[start - 1] = tag_len as c_char;
        let ascii = b'A' as c_char + tag_num as c_char;
        packet.fill_data(start..end, ascii);
    }

    let msg_start = tags_start + (tag_size * MAX_TAGS);
    let msg_len = 5;
    let msg_end = msg_start + msg_len;
    let msg_ascii = b'A' as c_char + MAX_TAGS as c_char;
    packet.fill_data(msg_start..msg_end, msg_ascii);

    let min_buffer = &packet.as_bytes()[..METADATA_SIZE + msg_end + 1]; // null-terminated
    let full_buffer = packet.as_bytes();

    let logger_message = LoggerMessage::try_from(min_buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + msg_end);
    let min_parsed = crate::from_logger(get_test_identity(), logger_message);

    let logger_message = LoggerMessage::try_from(full_buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + msg_end);
    let full_parsed =
        crate::from_logger(get_test_identity(), LoggerMessage::try_from(full_buffer).unwrap());

    let tag_properties = (0..MAX_TAGS as _)
        .map(|tag_num| String::from_utf8(vec![(b'A' as c_char + tag_num) as u8; tag_len]).unwrap())
        .collect::<Vec<_>>();
    let mut builder = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Debug,
    })
    .set_dropped(packet.metadata.dropped_logs.into())
    .set_pid(packet.metadata.pid)
    .set_message(String::from_utf8(vec![msg_ascii as u8; msg_len]).unwrap())
    .set_tid(packet.metadata.tid);
    for tag in tag_properties {
        builder = builder.add_tag(tag);
    }
    let expected_message = builder.build();

    assert_eq!(min_parsed, expected_message);
    assert_eq!(full_parsed, expected_message);
}

#[fuchsia::test]
fn max_tags() {
    let mut packet = test_packet();
    let tags_start = 1;
    let tag_len = 2;
    let tag_size = tag_len + 1; // the length-prefix byte
    for tag_num in 0..MAX_TAGS {
        let start = tags_start + (tag_size * tag_num);
        let end = start + tag_len;

        packet.data[start - 1] = tag_len as c_char;
        let ascii = b'A' as c_char + tag_num as c_char;
        packet.fill_data(start..end, ascii);
    }

    let msg_start = tags_start + (tag_size * MAX_TAGS);

    let buffer_missing_terminator = &packet.as_bytes()[..METADATA_SIZE + msg_start];
    assert_eq!(
        LoggerMessage::try_from(buffer_missing_terminator),
        Err(MessageError::OutOfBounds),
        "can't parse an empty message without a nul terminator"
    );

    let buffer = &packet.as_bytes()[..METADATA_SIZE + msg_start + 1]; // null-terminated
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, 48);
    let parsed = crate::from_logger(get_test_identity(), logger_message);
    let mut builder = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Debug,
    })
    .set_dropped(packet.metadata.dropped_logs as u64)
    .set_pid(packet.metadata.pid)
    .set_tid(packet.metadata.tid)
    .set_message("".to_string());
    for tag_num in 0..MAX_TAGS as _ {
        builder =
            builder.add_tag(String::from_utf8(vec![(b'A' as c_char + tag_num) as u8; 2]).unwrap());
    }
    assert_eq!(parsed, builder.build());
}

#[fuchsia::test]
fn no_tags_with_message() {
    let mut packet = test_packet();
    packet.data[0] = 0;
    packet.data[1] = b'A' as c_char;
    packet.data[2] = b'A' as c_char; // measured size ends here
    packet.data[3] = 0;

    let buffer = &packet.as_bytes()[..METADATA_SIZE + 4]; // 0 tag size + 2 byte message + null
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + 3);
    let parsed = crate::from_logger(get_test_identity(), logger_message);

    assert_eq!(
        parsed,
        LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(3),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity: Severity::Debug,
        })
        .set_dropped(packet.metadata.dropped_logs as u64)
        .set_pid(packet.metadata.pid)
        .set_tid(packet.metadata.tid)
        .set_message("AA".to_string())
        .build()
    );
}

#[fuchsia::test]
fn message_severity() {
    let mut packet = test_packet();
    packet.metadata.severity = LogLevelFilter::Info as i32;
    packet.data[0] = 0; // tag size
    packet.data[1] = 0; // null terminated

    let mut buffer = &packet.as_bytes()[..METADATA_SIZE + 2]; // tag size + null
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + 1);
    let mut parsed = crate::from_logger(get_test_identity(), logger_message);

    let mut expected_message = LogsDataBuilder::new(BuilderArgs {
        timestamp: Timestamp::from_nanos(packet.metadata.time),
        component_url: Some(TEST_IDENTITY.url.clone()),
        moniker: TEST_IDENTITY.moniker.clone(),
        severity: Severity::Info,
    })
    .set_pid(packet.metadata.pid)
    .set_message("".to_string())
    .set_dropped(10)
    .set_tid(packet.metadata.tid)
    .build();

    assert_eq!(parsed, expected_message);

    packet.metadata.severity = LogLevelFilter::Trace as i32;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    expected_message.metadata.severity = Severity::Trace;

    assert_eq!(parsed, expected_message);

    packet.metadata.severity = LogLevelFilter::Debug as i32;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    expected_message.metadata.severity = Severity::Debug;

    assert_eq!(parsed, expected_message);

    packet.metadata.severity = LogLevelFilter::Warn as i32;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    expected_message.metadata.severity = Severity::Warn;

    assert_eq!(parsed, expected_message);

    packet.metadata.severity = LogLevelFilter::Error as i32;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    expected_message.metadata.severity = Severity::Error;

    assert_eq!(parsed, expected_message);
}

#[fuchsia::test]
fn legacy_message_severity() {
    let mut packet = test_packet();
    packet.data[0] = 0; // tag size
    packet.data[1] = 0; // null terminated

    let expected_message = |severity: Severity, raw_severity: Option<u8>| {
        let mut expected_message = LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(3),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity,
        })
        .set_dropped(packet.metadata.dropped_logs as u64)
        .set_pid(1)
        .set_tid(2)
        .set_message("".to_string());

        if let Some(raw_severity) = raw_severity {
            expected_message = expected_message.set_raw_severity(raw_severity);
        }

        expected_message.build()
    };

    packet.metadata.severity = LogLevelFilter::Info as i32 - 1;
    let mut buffer = &packet.as_bytes()[..METADATA_SIZE + 2]; // tag size + null
    let logger_message = LoggerMessage::try_from(buffer).unwrap();
    assert_eq!(logger_message.size_bytes, METADATA_SIZE + 1);
    let mut parsed = crate::from_logger(get_test_identity(), logger_message);
    let expected = expected_message(Severity::Debug, Some(0x30 - 1));
    assert_eq!(parsed, expected);

    packet.metadata.severity = LogLevelFilter::Info as i32 - 2;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    let expected = expected_message(Severity::Debug, Some(0x30 - 2));
    assert_eq!(parsed, expected);

    packet.metadata.severity = LogLevelFilter::Info as i32 + 1;
    buffer = &packet.as_bytes()[..METADATA_SIZE + 2];
    parsed = crate::from_logger(get_test_identity(), LoggerMessage::try_from(buffer).unwrap());
    let expected = expected_message(Severity::Warn, Some(0x30 + 1));
    assert_eq!(parsed, expected);
}

#[fuchsia::test]
fn test_raw_severity_parsing_and_conversions() {
    let raw_severity: u8 = 0x30 - 2; // INFO-2=DEBUG
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(72),
        severity: raw_severity,
        arguments: vec![
            Argument::file("some_file.cc"),
            Argument::line(420),
            Argument::new("arg1", -23),
            Argument::new("arg2", true),
            Argument::pid(zx::Koid::from_raw(43)),
            Argument::tid(zx::Koid::from_raw(912)),
            Argument::dropped(2),
            Argument::tag("tag"),
            Argument::message("msg"),
        ],
    };

    let mut buffer = Cursor::new(vec![0u8; MAX_DATAGRAM_LEN]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize];
    let mut parsed = crate::from_structured(get_test_identity(), encoded).unwrap();
    parsed.sort_payload();
    assert_eq!(
        parsed,
        LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(72),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity: Severity::Debug,
        })
        .set_raw_severity(raw_severity)
        .set_dropped(2)
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .set_pid(43u64)
        .set_tid(912u64)
        .add_tag("tag")
        .set_message("msg".to_string())
        .add_key(LogsProperty::Int(LogsField::Other("arg1".to_string()), -23i64))
        .add_key(LogsProperty::Bool(LogsField::Other("arg2".to_string()), true))
        .build()
    );

    let message: LogMessage = parsed.into();
    assert_eq!(
        message,
        LogMessage {
            severity: raw_severity as i32,
            time: Timestamp::from_nanos(72),
            dropped_logs: 2,
            pid: 43,
            tid: 912,
            msg: "[some_file.cc(420)] msg arg1=-23 arg2=true".into(),
            tags: vec!["tag".into()]
        }
    );
}

#[fuchsia::test]
fn test_from_structured() {
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(72),
        severity: Severity::Error as u8,
        arguments: vec![
            Argument::file("some_file.cc"),
            Argument::line(420),
            Argument::new("arg1", -23),
            Argument::new("arg2", true),
            Argument::pid(zx::Koid::from_raw(43)),
            Argument::tid(zx::Koid::from_raw(912)),
            Argument::dropped(2),
            Argument::tag("tag"),
            Argument::message("msg"),
        ],
    };

    let mut buffer = Cursor::new(vec![0u8; MAX_DATAGRAM_LEN]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize];
    let parsed = crate::from_structured(get_test_identity(), encoded).unwrap();
    assert_eq!(
        parsed,
        LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(72),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity: Severity::Error,
        })
        .set_dropped(2)
        .set_file("some_file.cc".to_string())
        .set_line(420)
        .set_pid(43u64)
        .set_tid(912u64)
        .add_tag("tag")
        .set_message("msg".to_string())
        .add_key(LogsProperty::Int(LogsField::Other("arg1".to_string()), -23i64))
        .add_key(LogsProperty::Bool(LogsField::Other("arg2".to_string()), true))
        .build()
    );
    let severity = i32::from(Severity::Error as u8);
    let message: LogMessage = parsed.into();
    assert_eq!(
        message,
        LogMessage {
            severity,
            time: Timestamp::from_nanos(72),
            dropped_logs: 2,
            pid: 43,
            tid: 912,
            msg: "[some_file.cc(420)] msg arg1=-23 arg2=true".into(),
            tags: vec!["tag".into()]
        }
    );

    // multiple tags
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(72),
        severity: StreamSeverity::Error.into_primitive(),
        arguments: vec![Argument::tag("tag1"), Argument::tag("tag2"), Argument::tag("tag3")],
    };
    let mut buffer = Cursor::new(vec![0u8; MAX_DATAGRAM_LEN]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize];
    let parsed = crate::from_structured(get_test_identity(), encoded).unwrap();
    assert_eq!(
        parsed,
        LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(72),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity: Severity::Error,
        })
        .add_tag("tag1")
        .add_tag("tag2")
        .add_tag("tag3")
        .build()
    );

    // empty record
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(72),
        severity: Severity::Error as u8,
        arguments: vec![],
    };
    let mut buffer = Cursor::new(vec![0u8; MAX_DATAGRAM_LEN]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let encoded = &buffer.get_ref().as_slice()[..buffer.position() as usize];
    let parsed = crate::from_structured(get_test_identity(), encoded).unwrap();
    assert_eq!(
        parsed,
        LogsDataBuilder::new(BuilderArgs {
            timestamp: Timestamp::from_nanos(72),
            component_url: Some(TEST_IDENTITY.url.clone()),
            moniker: TEST_IDENTITY.moniker.clone(),
            severity: Severity::Error,
        })
        .build()
    );

    // parse error
    assert_matches!(
        crate::from_structured(get_test_identity(), &[]).unwrap_err(),
        MessageError::ParseError { .. }
    );
}

#[fuchsia::test]
fn from_extended_record_out_of_bounds() {
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(72),
        severity: Severity::Error as u8,
        arguments: vec![],
    };
    let mut buffer = Cursor::new(vec![0u8; MAX_DATAGRAM_LEN]);
    let mut encoder = Encoder::new(&mut buffer, EncoderOpts::default());
    encoder.write_record(record).unwrap();
    let pos = buffer.position() as usize;

    let mut extended = buffer.get_ref()[..pos].to_vec();
    extended.extend_from_slice(&1000u32.to_le_bytes());
    extended.extend_from_slice(&1000u32.to_le_bytes());
    extended.extend_from_slice(&0u64.to_le_bytes());
    extended.extend_from_slice(b"short");

    assert_matches!(crate::from_extended_record(&extended).unwrap_err(), MessageError::OutOfBounds);
}
