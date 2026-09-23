// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/span.h>

#include <algorithm>
#include <cstring>
#include <iostream>
#include <random>
#include <vector>

#include <fbl/string_printf.h>
#include <perftest/perftest.h>

namespace {

constexpr size_t kCacheSizeMB = 16;  // Larger than last-level cache on common CPUs.
constexpr size_t kBufferSizeMB = 128;
constexpr size_t kMaxAccessSequenceLen = 100000;

// Measure the time taken to copy a randomly chosen block of |block_size_bytes|
// to a random destination, both within a buffer of size |region_size_bytes|,
// using std::copy on a cpp20::span.
bool RandomSpanCopy(perftest::RepeatState* state, size_t block_size_bytes, size_t buffer_size_mb) {
  const size_t buffer_size_bytes = buffer_size_mb * 1024 * 1024;
  if (block_size_bytes >= buffer_size_bytes) {
    std::cerr << "Invalid configuration: block_size_bytes >= buffer_size_bytes ("
              << block_size_bytes << " >= " << buffer_size_bytes << ")" << std::endl;
    return false;
  }

  // Prepare the buffer.
  auto buf = std::make_unique<uint8_t[]>(buffer_size_bytes);
  memset(buf.get(), 0xAA, buffer_size_bytes);

  // Prepare the random source and destination addresses.
  const size_t cache_size_bytes = kCacheSizeMB * 1024 * 1024;
  const size_t num_blocks_to_overflow_cache = cache_size_bytes / block_size_bytes + 1;
  const size_t access_sequence_len = std::min(num_blocks_to_overflow_cache, kMaxAccessSequenceLen);
  std::random_device rand_dev;
  std::uniform_int_distribution rand_offset_gen(
      size_t(0), buffer_size_bytes - block_size_bytes  // Ensure end of block is within buffer.
  );
  std::vector<uint8_t*> src_addrs(access_sequence_len);
  std::vector<uint8_t*> dst_addrs(access_sequence_len);
  std::generate(src_addrs.begin(), src_addrs.end(),
                [&] { return buf.get() + rand_offset_gen(rand_dev); });
  std::generate(dst_addrs.begin(), dst_addrs.end(),
                [&] { return buf.get() + rand_offset_gen(rand_dev); });

  // Run the benchmark task.
  for (size_t i = 0; state->KeepRunning(); ++i) {
    cpp20::span<uint8_t> src(src_addrs[i % access_sequence_len], block_size_bytes);
    cpp20::span<uint8_t> dst(dst_addrs[i % access_sequence_len], block_size_bytes);
    if (dst.data() >= src.data()) {
      std::copy_backward(src.begin(), src.end(), dst.end());
    } else {
      std::copy(src.begin(), src.end(), dst.begin());
    }
  }

  return true;
}

void RegisterTest(size_t block_size_bytes, size_t buffer_size_mb) {
  fbl::String test_name;
  if (block_size_bytes < 1024) {
    test_name = fbl::StringPrintf("Stdcompat/CopySpan/%zubytes/%zuMbytes", block_size_bytes,
                                  buffer_size_mb);
  } else if (block_size_bytes < 1024 * 1024) {
    test_name = fbl::StringPrintf("Stdcompat/CopySpan/%zuKbytes/%zuMbytes", block_size_bytes / 1024,
                                  buffer_size_mb);
  } else {
    test_name = fbl::StringPrintf("Stdcompat/CopySpan/%zuMbytes/%zuMbytes",
                                  block_size_bytes / 1024 / 1024, buffer_size_mb);
  }
  perftest::RegisterTest(test_name.c_str(), RandomSpanCopy, block_size_bytes, buffer_size_mb);
}

void RegisterTests() {
  for (auto block_size_bytes : {1, 4, 16, 64, 256}) {
    RegisterTest(block_size_bytes, kBufferSizeMB);
  }

  for (auto block_size_kb : {1, 4, 16, 64, 256}) {
    RegisterTest(block_size_kb * 1024, kBufferSizeMB);
  }

  for (auto block_size_mb : {1, 4, 16}) {
    RegisterTest(block_size_mb * 1024 * 1024, kBufferSizeMB);
  }
}

PERFTEST_CTOR(RegisterTests)

}  // namespace
