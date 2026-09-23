// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl_fuchsia_examples as fex;

#[test]
fn persist_unpersist() -> Result<(), fidl::Error> {
    // [START persist]
    let original_value = fex::Color { id: 0, name: "red".to_string() };
    let bytes = fidl::persist(&original_value)?;
    // [END persist]

    // [START unpersist]
    let decoded_value = fidl::unpersist(&bytes)?;
    assert_eq!(original_value, decoded_value);
    // [END unpersist]

    Ok(())
}

#[test]
fn standalone_encode_decode_value() -> Result<(), fidl::Error> {
    // [START standalone_encode_value]
    let original_value = fex::JsonValue::StringValue("hello".to_string());
    let (bytes, wire_metadata) = fidl::standalone_encode_value(&original_value)?;
    // [END standalone_encode_value]

    // [START standalone_decode_value]
    let decoded_value = fidl::standalone_decode_value(&bytes, &wire_metadata)?;
    assert_eq!(original_value, decoded_value);
    // [END standalone_decode_value]

    Ok(())
}

#[test]
fn standalone_encode_decode_resource() -> Result<(), fidl::Error> {
    // [START standalone_encode_resource]
    let original_value = fex::EventStruct { event: Some(fidl::Event::create()) };
    let (bytes, handle_dispositions, wire_metadata) =
        fidl::standalone_encode_resource(original_value)?;
    // [END standalone_encode_resource]

    // [START standalone_decode_resource]
    let mut handle_infos = fidl::convert_handle_dispositions_to_infos(handle_dispositions)?;
    let decoded_value: fex::EventStruct =
        fidl::standalone_decode_resource(&bytes, &mut handle_infos, &wire_metadata)?;
    assert!(decoded_value.event.is_some());
    // [END standalone_decode_resource]

    Ok(())
}
