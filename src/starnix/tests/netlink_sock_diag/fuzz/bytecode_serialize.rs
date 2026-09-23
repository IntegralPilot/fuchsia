// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuzz::fuzz;
use netlink_packet_sock_diag::inet::bytecode::{Bytecode, SerializationError};

#[fuzz]
fn fuzz_bytecode_serialize(input: Bytecode) {
    let mut serialized_buf_1 = vec![0; input.serialized_len()];
    match input.clone().serialize(&mut serialized_buf_1) {
        Ok(()) => (),
        Err(SerializationError::BufferTooSmall) => panic!("buffer somehow too small"),
        Err(SerializationError::InvalidInstruction { .. }) => return,
    }

    let parsed = match Bytecode::parse(&serialized_buf_1) {
        Ok(parsed) => parsed,
        Err(e) => panic!("serialized program failed to parse with error {e:?}, program={input:#?}"),
    };

    assert_eq!(input, parsed, "serialize should round trip");
}
