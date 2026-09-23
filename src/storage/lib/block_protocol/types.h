// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_PROTOCOL_TYPES_H_
#define SRC_STORAGE_LIB_BLOCK_PROTOCOL_TYPES_H_

#include <stdint.h>
#include <zircon/types.h>

// LINT.IfChange

using reqid_t = uint32_t;
using groupid_t = uint16_t;
using vmoid_t = uint16_t;

struct BlockFifoCommand {
  uint8_t opcode = 0;
  uint8_t padding_to_satisfy_zerocopy[3] = {0, 0, 0};
  uint32_t flags = 0;
};

// * Reads with Decompression *
//
// For a non-fragmented read with decompression (`flags` has `DECOMPRESS_WITH_ZSTD` set)
//  `dev_offset` and `length` should specify the compressed blocks.  `total_compressed_bytes` should
//  be the total number of compressed bytes.  `uncompresed_bytes` is the total number of
//  uncompressed bytes.  `compressed_prefix_bytes` specifies the padding at the beginning before the
//  start of compressed data.
//
// For a fragmented read with decompression:
//
// For the first request in the group: `dev_offset` and `length` should specify the compressed
// blocks for the first fragment.  `total_compressed_bytes` should be set to the total number of
// compressed bytes across all requests, and likewise for `uncompressed_bytes` and
// `compressed_prefix_bytes`.
//
// For subsequent requests, `dev_offset` and `length` should specify the compressed blocks.  All
// other fields should be zero.
//
// The group may only contain read requests applicable to the decompressed read.
//
// There is a 128 MiB limit on the total compressed amount.
struct BlockFifoRequest {
  BlockFifoCommand command = {};
  reqid_t reqid = 0;
  groupid_t group = 0;
  vmoid_t vmoid = 0;
  uint32_t length = 0;  // In blocks.

  // The total number of compressed bytes across all requests in the group (only applicable for the
  // first request in a group). This does *not* include any padding at either the beginning or end.
  uint32_t total_compressed_bytes = 0;

  uint64_t vmo_offset = 0;
  uint64_t dev_offset = 0;
  uint64_t trace_flow_id = 0;
  // The data unit number used as an inline crypto tweak. Only used if the request flags include
  // `INLINE_ENCRYPTION_ENABLED`.
  uint64_t dun = 0;
  // The keyslot for the key used to encrypt/decrypt the request's data if the request flags include
  // `INLINE_ENCRYPTION_ENABLED`.
  uint8_t slot = 0;
  uint8_t padding = 0;

  // The number of bytes to skip at the beginning (only applicable for the first request in a
  // group).
  uint16_t compressed_prefix_bytes = 0;

  // The total number of uncompressed bytes for this request (only applicable for the first request
  // in a group).
  uint32_t uncompressed_bytes = 0;
};

struct BlockFifoResponse {
  zx_status_t status = 0;
  reqid_t reqid = 0;
  groupid_t group = 0;
  uint16_t padding_to_satisfy_zerocopy = 0;
  uint32_t count = 0;
  uint64_t padding_to_match_request_size_and_alignment[6] = {0, 0, 0, 0, 0, 0};
};

// Notify humans to update Rust bindings because there's no bindgen automation.
// TODO(https://fxbug.dev/42153476): Remove lint when no longer necessary.
// LINT.ThenChange(//src/storage/lib/block_protocol/src/fifo.rs)

#endif  // SRC_STORAGE_LIB_BLOCK_PROTOCOL_TYPES_H_
