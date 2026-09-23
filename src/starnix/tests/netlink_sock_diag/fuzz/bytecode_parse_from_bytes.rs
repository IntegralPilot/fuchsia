// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuzz::fuzz;
use netlink_packet_sock_diag::inet::bytecode::Bytecode;

#[fuzz]
fn fuzz_bytecode_parse_from_bytes(buf: &[u8]) {
    let parsed = if let Ok(parsed) = Bytecode::parse(buf) { parsed } else { return };

    let mut serialized_buf_1 = vec![0; parsed.serialized_len()];
    if parsed.clone().serialize(&mut serialized_buf_1).is_err() {
        return;
    }

    // We can't check that the same bytes come out because round-tripping is
    // lossy (and this is a good thing). But we CAN check that parsing the
    // serialized bytes gives the same AST.
    let parsed2 =
        Bytecode::parse(&serialized_buf_1).expect("serialized program should be parsable");
    assert_eq!(parsed, parsed2, "serialize should round trip");
}
