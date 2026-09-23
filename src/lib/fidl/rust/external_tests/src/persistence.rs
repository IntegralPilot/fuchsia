// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file tests FIDL's persistent encoding/decoding API.

use assert_matches::assert_matches;
use fidl::{persist, standalone_decode_value, standalone_encode_value, unpersist};
use fidl_test_external::{Coordinate, FlexibleValueThing, ValueRecord};

#[test]
fn persist_unpersist() {
    let body = Coordinate { x: 1, y: 2 };
    let buf = persist(&body).expect("encoding failed");
    let body_out = unpersist(&buf).expect("decoding failed");
    assert_eq!(body, body_out);
}

#[test]
fn persist_unpersist_presence() {
    let body = ValueRecord { name: Some("testing".to_string()), ..Default::default() };
    let buf = persist(&body).expect("encoding failed");
    let body_out = unpersist(&buf).expect("decoding failed");
    assert_eq!(body, body_out);
}

#[test]
fn unpersist_with_old_16_byte_header_fails() {
    let bytes = &[
        0, 0, 0, 0, // txid (unused)
        0, 2, // at rest flags (0x0002 = v2 wire format)
        0, // dynamic flags
        1, // magic number
        0, 0, 0, 0, 0, 0, 0, 0, // ordinal (unused)
    ];
    assert_eq!(bytes.len(), 16);
    // Reading an old header under the new format leads to a magic number of 0.
    // This is good because it fails rather than misinterpreting the message.
    assert_matches!(unpersist::<Coordinate>(bytes), Err(fidl::Error::IncompatibleMagicNumber(0)));
}

#[test]
fn standalone_encode_decode_value() {
    let value = FlexibleValueThing::Name("foo".to_string());
    let (bytes, metadata) = standalone_encode_value(&value).expect("encoding failed");
    let value_out = standalone_decode_value(&bytes, &metadata).expect("decoding failed");
    assert_eq!(value, value_out);
}

#[cfg(target_os = "fuchsia")]
mod zx {
    use fidl::{
        convert_handle_dispositions_to_infos, standalone_decode_resource,
        standalone_encode_resource,
    };
    use fidl_test_external::StructWithHandles;

    #[test]
    fn standalone_encode_decode_resource() {
        let (c1, c2) = zx::Channel::create();
        let c1_koid = c1.koid();
        let c2_koid = c2.koid();

        let value = StructWithHandles { v: vec![c1, c2] };

        let (bytes, handle_dispositions, metadata) =
            standalone_encode_resource(value).expect("encoding failed");
        assert_eq!(handle_dispositions.len(), 2);

        let mut handle_infos = convert_handle_dispositions_to_infos(handle_dispositions)
            .expect("converting to handle infos failed");
        let value_out: StructWithHandles =
            standalone_decode_resource(&bytes, &mut handle_infos, &metadata)
                .expect("decoding failed");

        assert_eq!(value_out.v[0].koid(), c1_koid);
        assert_eq!(value_out.v[1].koid(), c2_koid);
    }
}
