// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_diagnostics_types::Severity;
use fidl_fuchsia_validate_logs::{
    Argument, EncodingPuppetMarker, EncodingValidatorRequest, EncodingValidatorRequestStream,
    Record, TestFailure, TestSuccess, ValidateResult, ValidateResultsIteratorGetNextResponse,
    ValidateResultsIteratorRequest, ValidateResultsIteratorRequestStream, Value,
};
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::*;

type TestCase = (&'static str, Record, Vec<u8>);

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new();

    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(serve_requests(stream)).detach();
    });
    fs.take_and_serve_directory_handle().expect("serev dir");
    fs.collect::<()>().await;
}

async fn serve_requests(mut stream: EncodingValidatorRequestStream) {
    while let Some(Ok(request)) = stream.next().await {
        match request {
            EncodingValidatorRequest::Validate { results, control_handle: _ } => {
                let stream = results.into_stream();
                fasync::Task::spawn(async move {
                    serve_results(stream).await;
                })
                .detach();
            }
        }
    }
}

async fn serve_results(mut stream: ValidateResultsIteratorRequestStream) {
    let proxy = client::connect_to_protocol::<EncodingPuppetMarker>().unwrap();

    info!("Testing encoding.");
    let mut test_cases: Vec<TestCase> = vec![
        test_signed_int_positive(),
        test_signed_int_negative(),
        test_unsigned_int(),
        test_float(),
        test_string(),
        test_multiword_string(),
        test_empty_string(),
        test_multiword_arg_name(),
        test_word_size_arg_name(),
        test_unsigned_int_max(),
        test_no_args(),
        test_multiple_args(),
        test_boolean(),
    ];
    while let Some(Ok(request)) = stream.next().await {
        match request {
            ValidateResultsIteratorRequest::GetNext { responder } => {
                let (test_name, record_to_encode, expected) = {
                    let Some(test_case) = test_cases.pop() else {
                        responder.send(ValidateResultsIteratorGetNextResponse::default()).ok();
                        continue;
                    };
                    test_case
                };
                let result = proxy
                    .encode(&record_to_encode)
                    .await
                    .expect("encode")
                    .expect("Unable to get Record");
                let size = result.size;
                let vmo = result.vmo;
                let mut buffer = vec![0; size.try_into().expect("Unable to convert size")];
                vmo.read(&mut buffer, 0).expect("Read vmo");
                if expected != buffer {
                    responder
                        .send(ValidateResultsIteratorGetNextResponse {
                            result: Some(ValidateResult::Failure(TestFailure {
                                test_name: test_name.to_string(),
                                reason: format!(
                                    "Expected: {expected:#04X?}, actual: {buffer:#04X?}"
                                ),
                            })),
                            ..ValidateResultsIteratorGetNextResponse::default()
                        })
                        .ok();
                } else {
                    responder
                        .send(ValidateResultsIteratorGetNextResponse {
                            result: Some(ValidateResult::Success(TestSuccess {
                                test_name: test_name.to_string(),
                            })),
                            ..ValidateResultsIteratorGetNextResponse::default()
                        })
                        .ok();
                }
            }
        }
    }
}

fn test_string() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(12);
    let arg = Argument { name: String::from("hello"), value: Value::Text("world".to_string()) };
    let record = Record { timestamp, severity: Severity::Info, arguments: vec![arg] };
    // 5: represents the size of the record
    // 9: represents the type of Record (Log record)
    // 0x30: represents the INFO severity
    let mut expected_record_header = vec![0x59, 0, 0, 0, 0, 0, 0, 0x30];
    let mut expected_time_stamp = vec![0xC, 0, 0, 0, 0, 0, 0, 0];
    // 3: represents the size of argument
    // 6: represents the value type
    // 5, 0x80: string ref for NameRef
    // second 5, 0x80: string ref for ValueRef
    let mut expected_arg_header = vec![0x36, 0, 0x5, 0x80, 0x5, 0x80, 0, 0];
    // Representation of "hello"
    let mut expected_arg_name = vec![0x68, 0x65, 0x6C, 0x6C, 0x6F, 0, 0, 0];
    // Representation of "world"
    let mut expected_test_value = vec![0x77, 0x6F, 0x72, 0x6C, 0x64, 0, 0, 0];

    let mut expected_result = vec![];
    expected_result.append(&mut expected_record_header);
    expected_result.append(&mut expected_time_stamp);
    expected_result.append(&mut expected_arg_header);
    expected_result.append(&mut expected_arg_name);
    expected_result.append(&mut expected_test_value);
    ("test_string", record, expected_result)
}

fn test_multiword_string() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x24);
    let arg =
        Argument { name: String::from("name"), value: Value::Text(String::from("aaaaaaabbb")) };
    let record = Record { timestamp, severity: Severity::Warn, arguments: vec![arg] };

    #[rustfmt::skip]
    let expected_result = vec![
        // Record header - 9 for log type, 6 for log size, 0x40 for WARN severity
        0x69, 0, 0, 0, 0, 0, 0, 0x40,
        // timestamp
        0x24, 0, 0, 0, 0, 0, 0, 0,
        // argument header - 6 for value type, 4 for arg size, 0x4/0x80 for NameRef
        // 0xa/0x80 for ValueRef
        0x46, 0, 0x4, 0x80, 0xa, 0x80, 0, 0,
        // "name" representation with padding
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0,
        // "aaaaaaabbb" with padding
        0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x61, 0x62,
        0x62, 0x62, 0, 0, 0, 0, 0, 0,
    ];
    ("test_multiword_string", record, expected_result)
}

fn test_empty_string() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x24);
    let arg = Argument { name: String::from("name"), value: Value::Text(String::from("")) };
    let record = Record { timestamp, severity: Severity::Warn, arguments: vec![arg] };

    #[rustfmt::skip]
    let expected_result = vec![
        // Record header - 9 for log type, 4 for log size, 0x40 for WARN severity
        0x49, 0, 0, 0, 0, 0, 0, 0x40,
        // timestamp
        0x24, 0, 0, 0, 0, 0, 0, 0,
        // argument header - 6 for value type, 2 for arg size, 0x4/0x80 for NameRef
        // ValueRef is 0 for empty string
        0x26, 0, 0x4, 0x80, 0, 0, 0, 0,
        // "name" representation with padding
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0,
        // (empty string, no bytes)
    ];
    ("test_empty_string", record, expected_result)
}

fn test_boolean() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x24);
    let arg = Argument { name: String::from("name"), value: Value::Boolean(true) };
    let record = Record { timestamp, severity: Severity::Warn, arguments: vec![arg] };
    #[rustfmt::skip]
    let expected_result = vec![
        // Record header - 4 for the size of the record, 9 for the type of Record (Log record), 0x40 for WARN severity
        0x49, 0, 0, 0, 0, 0, 0, 0x40,
        // Timestamp
        0x24, 0, 0, 0, 0, 0, 0, 0,
        // Argument Header - 2 for the size of argument, 9 for the value type, 0x4/0x80 for string ref for NameRef
        // Boolean value is 0x1
        0x29, 0, 0x4, 0x80, 0x1, 0, 0, 0,
        // Representation of "name" with padding
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0,
    ];
    ("test_boolean", record, expected_result)
}

fn test_float() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(6);
    let arg = Argument { name: String::from("name"), value: Value::Floating(3.25) };
    let record = Record { timestamp, severity: Severity::Warn, arguments: vec![arg] };
    // Record header = 0x59, 0, 0, 0, 0, 0, 0, 0x40
    // Timestamp = 0x6, 0, 0, 0, 0, 0, 0, 0
    // Arg Header = 0x35, 0, 0x4, 0x80, 0, 0, 0, 0
    // Arg name = 0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0
    // Arg value = 0x6F, 0x12, 0x83, 0xC0, 0xCA, 0x21, 0x09, 0x40
    let expected_result = vec![
        0x59, 0, 0, 0, 0, 0, 0, 0x40, 0x6, 0, 0, 0, 0, 0, 0, 0, 0x35, 0, 0x4, 0x80, 0, 0, 0, 0,
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0a, 0x40,
    ];
    ("test_float", record, expected_result)
}

fn test_unsigned_int() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(6);
    let arg = Argument { name: String::from("name"), value: Value::UnsignedInt(3) };
    let record = Record { timestamp, severity: Severity::Debug, arguments: vec![arg] };
    // Record header = 0x59, 0, 0, 0, 0, 0, 0, 0x20
    // Timestamp = 0x6, 0, 0, 0, 0, 0, 0, 0
    // Arg Header = 0x34, 0, 0x4, 0x80, 0, 0, 0, 0
    // Arg name = 0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0
    // Arg value = 0x3, 0, 0, 0, 0, 0, 0, 0
    let expected_result = vec![
        0x59, 0, 0, 0, 0, 0, 0, 0x20, 0x6, 0, 0, 0, 0, 0, 0, 0, 0x34, 0, 0x4, 0x80, 0, 0, 0, 0,
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0, 0x3, 0, 0, 0, 0, 0, 0, 0,
    ];
    ("test_unsigned_int", record, expected_result)
}

fn test_unsigned_int_max() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(6);
    let arg = Argument { name: String::from("name"), value: Value::UnsignedInt(u64::MAX) };
    let record = Record { timestamp, severity: Severity::Debug, arguments: vec![arg] };
    // Record header = 0x59, 0, 0, 0, 0, 0, 0, 0x20
    // Timestamp = 0x6, 0, 0, 0, 0, 0, 0, 0
    // Arg Header = 0x34, 0, 0x4, 0x80, 0, 0, 0, 0
    // Arg name = 0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0
    // Arg value = 0xff, 0xff, ...
    let expected_result = vec![
        0x59, 0, 0, 0, 0, 0, 0, 0x20, 0x6, 0, 0, 0, 0, 0, 0, 0, 0x34, 0, 0x4, 0x80, 0, 0, 0, 0,
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ];
    ("test_unsigned_int", record, expected_result)
}

fn test_signed_int_negative() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(9);
    let arg = Argument { name: String::from("name"), value: Value::SignedInt(-7) };
    let record = Record { timestamp, severity: Severity::Error, arguments: vec![arg] };
    // Record header = 0x59, 0, 0, 0, 0, 0, 0, 0x50
    // Timestamp = 0x9, 0, 0, 0, 0, 0, 0, 0
    // Arg Header = 0x33, 0, 0x4, 0x80, 0, 0, 0, 0
    // Arg name = 0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0
    // Arg value = 0xF9, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
    let expected_result = vec![
        0x59, 0, 0, 0, 0, 0, 0, 0x50, 0x9, 0, 0, 0, 0, 0, 0, 0, 0x33, 0, 0x4, 0x80, 0, 0, 0, 0,
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0, 0xF9, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    ("test_signed_int_negative", record, expected_result)
}

fn test_signed_int_positive() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(9);
    let arg = Argument { name: String::from("name"), value: Value::SignedInt(4) };
    let record = Record { timestamp, severity: Severity::Warn, arguments: vec![arg] };
    // Record header = 0x59, 0, 0, 0, 0, 0, 0, 0x40
    // Timestamp = 0x9, 0, 0, 0, 0, 0, 0, 0
    // Arg Header = 0x33, 0, 0x4, 0x80, 0, 0, 0, 0
    // Arg name = 0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0
    // Arg value = 0x4, 0, 0, 0, 0, 0, 0, 0
    let expected_result = vec![
        0x59, 0, 0, 0, 0, 0, 0, 0x40, 0x9, 0, 0, 0, 0, 0, 0, 0, 0x33, 0, 0x4, 0x80, 0, 0, 0, 0,
        0x6E, 0x61, 0x6D, 0x65, 0, 0, 0, 0, 0x4, 0, 0, 0, 0, 0, 0, 0,
    ];
    ("test_signed_int_positive", record, expected_result)
}

fn test_multiword_arg_name() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x4523);
    let arg = Argument { name: String::from("abcdabcdabcd"), value: Value::SignedInt(9) };
    let record = Record { timestamp, severity: Severity::Error, arguments: vec![arg] };
    #[rustfmt::skip]
    let expected_result = vec![
        // record header
        0x69, 0, 0, 0, 0, 0, 0, 0x50,
        // timestamp
        0x23, 0x45, 0, 0, 0, 0, 0, 0,
        // arg header - record size 4, NameRef 0xC/0x80
        0x43, 0, 0xC, 0x80, 0, 0, 0, 0,
        // arg name "abcdabcdabcd"
        0x61, 0x62, 0x63, 0x64, 0x61, 0x62, 0x63, 0x64,
        0x61, 0x62, 0x63, 0x64, 0, 0, 0, 0,
        // arg value
        0x9, 0, 0, 0, 0, 0, 0, 0,
    ];
    ("test_multiword_arg_name", record, expected_result)
}

fn test_word_size_arg_name() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x4523);
    let arg = Argument { name: String::from("abcdabcd"), value: Value::SignedInt(9) };
    let record = Record { timestamp, severity: Severity::Error, arguments: vec![arg] };
    #[rustfmt::skip]
    let expected_result = vec![
        // record header
        0x59, 0, 0, 0, 0, 0, 0, 0x50,
        // timestamp
        0x23, 0x45, 0, 0, 0, 0, 0, 0,
        // arg header - record size 4, NameRef 0x8/0x80
        0x33, 0, 0x8, 0x80, 0, 0, 0, 0,
        // arg name "abcdabcd" (no padding - already word-aligned)
        0x61, 0x62, 0x63, 0x64, 0x61, 0x62, 0x63, 0x64,
        // arg value
        0x9, 0, 0, 0, 0, 0, 0, 0,
    ];
    ("test_word_size_arg_name", record, expected_result)
}

fn test_no_args() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0x1234);
    let record = Record { timestamp, severity: Severity::Error, arguments: vec![] };
    #[rustfmt::skip]
    let expected_result = vec![
        // record header
        0x29, 0, 0, 0, 0, 0, 0, 0x50,
        // timestamp
        0x34, 0x12, 0, 0, 0, 0, 0, 0,
    ];
    ("test_no_args", record, expected_result)
}

fn test_multiple_args() -> TestCase {
    let timestamp = zx::BootInstant::from_nanos(0xabcd);
    let arguments = vec![
        Argument { name: String::from("aa"), value: Value::SignedInt(3) },
        Argument { name: String::from("bbb"), value: Value::UnsignedInt(0x90) },
    ];
    let record = Record { timestamp, severity: Severity::Error, arguments };
    #[rustfmt::skip]
    let expected_result = vec![
        // record header
        0x89, 0, 0, 0, 0, 0, 0, 0x50,
        // timestamp
        0xcd, 0xab, 0, 0, 0, 0, 0, 0,
        // arg 1 header
        0x33, 0, 0x2, 0x80, 0, 0, 0, 0,
        // arg name "aa"
        0x61, 0x61, 0, 0, 0, 0, 0, 0,
        // arg value
        0x3, 0, 0, 0, 0, 0, 0, 0,
        // arg 2 header
        0x34, 0, 0x3, 0x80, 0, 0, 0, 0,
        // arg name "bbb"
        0x62, 0x62, 0x62, 0, 0, 0, 0, 0,
        // arg value
        0x90, 0, 0, 0, 0, 0, 0, 0,
    ];
    ("test_multiple_args", record, expected_result)
}
