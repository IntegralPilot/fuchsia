// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use perfetto_protos::perfetto::protos::ReadBuffersResponse;
use perfetto_trace_protos::perfetto::protos::Trace;
use prost::{DecodeError, Message};

/// Decodes a Perfetto trace from a protobuf blob.
///
/// This function is intentionally isolated in its own crate to contain the
/// build-time cost of monomorphizing the `Trace::decode` function.
pub fn decode_trace(protobuf_blob: &[u8]) -> Result<Trace, DecodeError> {
    Trace::decode(protobuf_blob)
}

/// Encodes a Perfetto trace into a protobuf blob.
///
/// This function is intentionally isolated in its own crate to contain the
/// build-time cost of monomorphizing the `Trace::encode_to_vec` function.
pub fn encode_trace(trace: &Trace) -> Vec<u8> {
    trace.encode_to_vec()
}

/// Decodes a Perfetto read buffers response from a protobuf blob.
///
/// This function is intentionally isolated in its own crate to contain the
/// build-time cost of monomorphizing the `ReadBuffersResponse::decode` function.
pub fn decode_read_buffers_response(
    protobuf_blob: &[u8],
) -> Result<ReadBuffersResponse, DecodeError> {
    ReadBuffersResponse::decode(protobuf_blob)
}
