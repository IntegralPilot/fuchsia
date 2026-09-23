// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use arbitrary::Arbitrary;
use fuzz::fuzz;
use netlink_packet_sock_diag::inet::bytecode::{Bytecode, SerializationError};

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    bytecode: Bytecode,
    buffer_size: u16,
}

#[fuzz]
fn fuzz_bytecode_serialize_varied_buffer(input: FuzzInput) {
    let FuzzInput { bytecode, buffer_size } = input;
    let mut buf = vec![0u8; buffer_size as usize];

    let res = bytecode.clone().serialize(&mut buf);
    let expected_len = bytecode.serialized_len();

    match res {
        Ok(()) => {
            assert!(buf.len() >= expected_len);
            let written_buf = &buf[..expected_len];
            let parsed = Bytecode::parse(written_buf).expect("parsing written part should succeed");
            assert_eq!(bytecode, parsed);
        }
        Err(SerializationError::BufferTooSmall) => {
            assert!(buf.len() < expected_len);
        }
        Err(SerializationError::InvalidInstruction { .. }) => return,
    }
}
