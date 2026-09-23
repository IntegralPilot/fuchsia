// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file tests FIDL's transaction message encoding/decoding API.

use fidl_message::{
    decode_message, decode_response_flexible, decode_response_flexible_result,
    decode_response_strict_result, decode_transaction_header, encode_message,
    encode_response_flexible, encode_response_flexible_unknown, encode_response_result,
    DynamicFlags, MaybeUnknown, TransactionHeader,
};
use fidl_test_external::{Coordinate, ValueRecord};

#[test]
fn encode_decode_message_empty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let bytes = encode_message(header, ()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    decode_message::<()>(header_out, body).unwrap();
    assert_eq!(header, header_out);
}

#[test]
fn encode_decode_message_nonempty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value = Coordinate { x: 1, y: 2 };
    let bytes = encode_message(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let value_out = decode_message(header_out, body).unwrap();
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_known_empty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let bytes = encode_response_flexible(header, ()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible(header_out, body).unwrap();
    let MaybeUnknown::Known(()) = result else { panic!() };
    assert_eq!(header, header_out);
}

#[test]
fn encode_decode_response_flexible_known_nonempty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value = Coordinate { x: 1, y: 2 };
    let bytes = encode_response_flexible(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible(header_out, body).unwrap();
    let MaybeUnknown::Known(value_out) = result else { panic!() };
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_unknown() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let bytes = encode_response_flexible_unknown(header).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible::<Coordinate>(header_out, body).unwrap();
    let MaybeUnknown::Unknown = result else { panic!() };
    assert_eq!(header, header_out);
}

#[test]
fn encode_decode_response_strict_result_ok_empty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<(), ValueRecord> = Ok(());
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let value_out = decode_response_strict_result(header_out, body).unwrap();
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_strict_result_ok_nonempty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<Coordinate, ValueRecord> = Ok(Coordinate { x: 1, y: 2 });
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let value_out = decode_response_strict_result(header_out, body).unwrap();
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_strict_result_err() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<Coordinate, ValueRecord> = Err(ValueRecord::default());
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let value_out = decode_response_strict_result(header_out, body).unwrap();
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_result_ok_empty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<(), i32> = Ok(());
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible_result(header_out, body).unwrap();
    let MaybeUnknown::Known(value_out) = result else { panic!() };
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_result_ok_nonempty() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<Coordinate, i32> = Ok(Coordinate { x: 1, y: 2 });
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible_result(header_out, body).unwrap();
    let MaybeUnknown::Known(value_out) = result else { panic!() };
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_result_err() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let value: Result<Coordinate, i32> = Err(456);
    let bytes = encode_response_result(header, value.clone()).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible_result(header_out, body).unwrap();
    let MaybeUnknown::Known(value_out) = result else { panic!() };
    assert_eq!(header, header_out);
    assert_eq!(value, value_out);
}

#[test]
fn encode_decode_response_flexible_result_unknown() {
    let header = TransactionHeader::new(123, 0xabc, DynamicFlags::FLEXIBLE);
    let bytes = encode_response_flexible_unknown(header).unwrap();
    let (header_out, body) = decode_transaction_header(&bytes).unwrap();
    let result = decode_response_flexible_result::<Coordinate, i32>(header_out, body).unwrap();
    let MaybeUnknown::Unknown = result else { panic!() };
    assert_eq!(header, header_out);
}
