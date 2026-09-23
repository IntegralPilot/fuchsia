// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Benchmarks for a reference encoder / decoder specialized to
// Table1Struct, Table16Struct and Table63Struct as defined in
// the fidl benchmark suite.

#include <lib/fit/function.h>
#include <zircon/fidl.h>

#include <algorithm>
#include <cstdint>

#include "builder.h"
#include "decode_benchmark_util.h"
#include "encode_benchmark_util.h"

#ifdef __Fuchsia__
#include <zircon/syscalls.h>
#endif

namespace {

template <size_t N>
bool EncodeUint8TableStruct(void* value, const char** error,
                            fit::function<void(const uint8_t*, size_t)> callback) {
  uint8_t out_buf[sizeof(fidl_vector_t) + sizeof(fidl_envelope_t) * N];
  fidl_vector_t* table_vec = reinterpret_cast<fidl_vector_t*>(value);
  size_t count = table_vec->count;
  void* data = table_vec->data;
  if (unlikely(data == nullptr && count != 0)) {
    *error = "table with null data had non-zero element count";
    return false;
  }
  *reinterpret_cast<fidl_vector_t*>(out_buf) = fidl_vector_t{
      .count = count,
      .data = reinterpret_cast<void*>(FIDL_ALLOC_PRESENT),
  };
  memcpy(out_buf + sizeof(fidl_vector_t), data, sizeof(fidl_envelope_t) * N);
  callback(out_buf, sizeof out_buf);
  return true;
}

template <size_t N>
bool DecodeUint8TableStruct(uint8_t* bytes, size_t bytes_size, zx_handle_t* handles,
                            size_t handles_size, const char** error) {
  if (handles_size != 0) {
    *error = "no handles expected";
    return false;
  }

  fidl_vector_t* table_vec = reinterpret_cast<fidl_vector_t*>(bytes);
  if (unlikely(table_vec->data == nullptr && table_vec->count != 0)) {
    *error = "table with null data had non-zero element count";
    return false;
  }

  fidl_envelope_t* start_envelopes =
      reinterpret_cast<fidl_envelope_t*>(bytes + sizeof(fidl_vector_t));
  table_vec->data = start_envelopes;
  fidl_envelope_t* end_known_envelopes = start_envelopes + std::min(N, table_vec->count);
  fidl_envelope_t* end_all_envelopes = start_envelopes + table_vec->count;

  uint8_t* next_out_of_line = reinterpret_cast<uint8_t*>(end_all_envelopes);
  if (unlikely(next_out_of_line - bytes > static_cast<int64_t>(bytes_size))) {
    *error = "byte size exceeds available size";
    return false;
  }

  for (fidl_envelope_t* envelope = start_envelopes; envelope != end_known_envelopes; envelope++) {
    if (unlikely(envelope->num_handles != 0)) {
      *error = "incorrect num_handles in envelope";
      return false;
    }
    if (unlikely(envelope->flags != FIDL_ENVELOPE_FLAGS_INLINING_MASK)) {
      *error = "invalid envelope flags";
      return false;
    }
  }

  // Unknown envelopes
  uint32_t num_handles = 0;
  for (fidl_envelope_t* envelope = end_known_envelopes; envelope != end_all_envelopes; envelope++) {
    uint16_t flags = envelope->flags;
    if (flags == FIDL_ENVELOPE_FLAGS_INLINING_MASK) {
      continue;
    }
    if (unlikely(flags != 0)) {
      *error = "invalid envelope flags";
      return false;
    }
    size_t num_bytes = envelope->num_bytes;
    if (unlikely(num_bytes % 8 != 0)) {
      *error = "incorrect num_bytes in envelope";
      return false;
    }
    num_handles += envelope->num_handles;
    auto envelope_as_ptr = reinterpret_cast<uint8_t**>(envelope);
    *envelope_as_ptr = next_out_of_line;
    next_out_of_line += num_bytes;
    if (unlikely(next_out_of_line - bytes > static_cast<int64_t>(bytes_size))) {
      *error = "byte size exceeds available size";
      return false;
    }
  }
#ifdef __Fuchsia__
  zx_handle_close_many(handles, num_handles);
#endif
  return true;
}

bool BenchmarkEncodeTableAllSet1(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_AllSet_1,
                                                EncodeUint8TableStruct<1>);
}
bool BenchmarkEncodeTableAllSet16(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_AllSet_16,
                                                EncodeUint8TableStruct<16>);
}
bool BenchmarkEncodeTableAllSet63(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_AllSet_63,
                                                EncodeUint8TableStruct<63>);
}
bool BenchmarkEncodeTableUnset1(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_Unset_1,
                                                EncodeUint8TableStruct<1>);
}
bool BenchmarkEncodeTableUnset16(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_Unset_16,
                                                EncodeUint8TableStruct<16>);
}
bool BenchmarkEncodeTableUnset63(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(state, benchmark_suite::Build_Table_Unset_63,
                                                EncodeUint8TableStruct<63>);
}
bool BenchmarkEncodeTableSingleSet_1_of_1(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_1, EncodeUint8TableStruct<1>);
}
bool BenchmarkEncodeTableSingleSet_1_of_16(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_16, EncodeUint8TableStruct<16>);
}
bool BenchmarkEncodeTableSingleSet_16_of_16(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_16_of_16, EncodeUint8TableStruct<16>);
}
bool BenchmarkEncodeTableSingleSet_1_of_63(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_63, EncodeUint8TableStruct<63>);
}
bool BenchmarkEncodeTableSingleSet_16_of_63(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_16_of_63, EncodeUint8TableStruct<63>);
}
bool BenchmarkEncodeTableSingleSet_63_of_63(perftest::RepeatState* state) {
  return encode_benchmark_util::EncodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_63_of_63, EncodeUint8TableStruct<63>);
}
bool BenchmarkDecodeTableAllSet1(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_AllSet_1,
                                                DecodeUint8TableStruct<1>);
}
bool BenchmarkDecodeTableAllSet16(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_AllSet_16,
                                                DecodeUint8TableStruct<16>);
}
bool BenchmarkDecodeTableAllSet63(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_AllSet_63,
                                                DecodeUint8TableStruct<63>);
}
bool BenchmarkDecodeTableUnset1(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_Unset_1,
                                                DecodeUint8TableStruct<1>);
}
bool BenchmarkDecodeTableUnset16(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_Unset_16,
                                                DecodeUint8TableStruct<16>);
}
bool BenchmarkDecodeTableUnset63(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(state, benchmark_suite::Build_Table_Unset_63,
                                                DecodeUint8TableStruct<63>);
}
bool BenchmarkDecodeTableSingleSet_1_of_1(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_1, DecodeUint8TableStruct<1>);
}
bool BenchmarkDecodeTableSingleSet_1_of_16(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_16, DecodeUint8TableStruct<16>);
}
bool BenchmarkDecodeTableSingleSet_16_of_16(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_16_of_16, DecodeUint8TableStruct<16>);
}
bool BenchmarkDecodeTableSingleSet_1_of_63(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_1_of_63, DecodeUint8TableStruct<63>);
}
bool BenchmarkDecodeTableSingleSet_16_of_63(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_16_of_63, DecodeUint8TableStruct<63>);
}
bool BenchmarkDecodeTableSingleSet_63_of_63(perftest::RepeatState* state) {
  return decode_benchmark_util::DecodeBenchmark(
      state, benchmark_suite::Build_Table_SingleSet_63_of_63, DecodeUint8TableStruct<63>);
}

void RegisterTests() {
  perftest::RegisterTest("Reference/Encode/Table/AllSet/1/Steps", BenchmarkEncodeTableAllSet1);
  perftest::RegisterTest("Reference/Encode/Table/AllSet/16/Steps", BenchmarkEncodeTableAllSet16);
  perftest::RegisterTest("Reference/Encode/Table/AllSet/63/Steps", BenchmarkEncodeTableAllSet63);
  perftest::RegisterTest("Reference/Encode/Table/Unset/1/Steps", BenchmarkEncodeTableUnset1);
  perftest::RegisterTest("Reference/Encode/Table/Unset/16/Steps", BenchmarkEncodeTableUnset16);
  perftest::RegisterTest("Reference/Encode/Table/Unset/63/Steps", BenchmarkEncodeTableUnset63);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/1_of_1/Steps",
                         BenchmarkEncodeTableSingleSet_1_of_1);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/1_of_16/Steps",
                         BenchmarkEncodeTableSingleSet_1_of_16);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/16_of_16/Steps",
                         BenchmarkEncodeTableSingleSet_16_of_16);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/1_of_63/Steps",
                         BenchmarkEncodeTableSingleSet_1_of_63);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/16_of_63/Steps",
                         BenchmarkEncodeTableSingleSet_16_of_63);
  perftest::RegisterTest("Reference/Encode/Table/SingleSet/63_of_63/Steps",
                         BenchmarkEncodeTableSingleSet_63_of_63);
  perftest::RegisterTest("Reference/Decode/Table/AllSet/1/Steps", BenchmarkDecodeTableAllSet1);
  perftest::RegisterTest("Reference/Decode/Table/AllSet/16/Steps", BenchmarkDecodeTableAllSet16);
  perftest::RegisterTest("Reference/Decode/Table/AllSet/63/Steps", BenchmarkDecodeTableAllSet63);
  perftest::RegisterTest("Reference/Decode/Table/Unset/1/Steps", BenchmarkDecodeTableUnset1);
  perftest::RegisterTest("Reference/Decode/Table/Unset/16/Steps", BenchmarkDecodeTableUnset16);
  perftest::RegisterTest("Reference/Decode/Table/Unset/63/Steps", BenchmarkDecodeTableUnset63);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/1_of_1/Steps",
                         BenchmarkDecodeTableSingleSet_1_of_1);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/1_of_16/Steps",
                         BenchmarkDecodeTableSingleSet_1_of_16);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/16_of_16/Steps",
                         BenchmarkDecodeTableSingleSet_16_of_16);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/1_of_63/Steps",
                         BenchmarkDecodeTableSingleSet_1_of_63);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/16_of_63/Steps",
                         BenchmarkDecodeTableSingleSet_16_of_63);
  perftest::RegisterTest("Reference/Decode/Table/SingleSet/63_of_63/Steps",
                         BenchmarkDecodeTableSingleSet_63_of_63);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
