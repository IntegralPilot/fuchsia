// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <trace-reader/reader.h>

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  // We don't care about what records we get, just that we don't crash or infinite loop.
  auto record_consumer = [](trace::Record record) {};
  auto error_handler = [](std::string_view error) {};
  trace::TraceReader reader(record_consumer, error_handler);

  // The fuzzer provides a buffer of bytes. We need to interpret this as a
  // sequence of 64-bit words for the Chunk.
  const uint64_t* words = reinterpret_cast<const uint64_t*>(data);
  size_t num_words = size / sizeof(uint64_t);

  if (num_words > 0) {
    trace::Chunk chunk(words, num_words);
    reader.ReadRecords(chunk);
  }

  return 0;
}
