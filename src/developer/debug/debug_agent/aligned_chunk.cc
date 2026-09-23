// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/aligned_chunk.h"

#include <algorithm>

namespace debug_agent {

namespace {

// Provides an way to access the bytes of a 64-bit word in order the word is written to memory
// (taking into account the current endianness).
union WordBytes {
  uint64_t word;
  uint8_t bytes[sizeof(uint64_t)];
};

}  // namespace

bool WriteAligned64Chunks(const void* buf, size_t len,
                          fit::function<bool(uint64_t addr, uint64_t* value)> read,
                          fit::function<bool(uint64_t addr, uint64_t value)> write,
                          uint64_t dest_addr) {
  constexpr size_t kWordSize = sizeof(uint64_t);

  if (len == 0)
    return true;

  // The last byte written.
  uint64_t last_addr = dest_addr + len - 1;

  // The 64-bit word address of the first and last byte written.
  uint64_t begin_word_addr = dest_addr / kWordSize * kWordSize;
  uint64_t last_word_addr = last_addr / kWordSize * kWordSize;

  // The byte offsets within the word of the first and last byte written. This are offsets in
  // memory (independent of word endianness).
  uint64_t begin_offset = dest_addr % kWordSize;
  uint64_t last_offset = last_addr % kWordSize;

  // Source as bytes for easier math.
  const uint8_t* buf_bytes = reinterpret_cast<const uint8_t*>(buf);

  if (begin_word_addr == last_word_addr) {
    // Span beings and ends in the same word.
    WordBytes data;
    if (!read(begin_word_addr, &data.word))
      return false;

    std::copy(buf_bytes, buf_bytes + len, data.bytes + begin_offset);

    return write(begin_word_addr, data.word);
  }

  // First word.
  uint64_t begin_full_addr;  // Address of first full word to write.
  const uint8_t* cur_source = buf_bytes;
  if (begin_offset == 0) {
    // First word is full.
    begin_full_addr = begin_word_addr;
  } else {
    // Partial first word.
    WordBytes data;
    if (!read(begin_word_addr, &data.word))
      return false;

    std::copy(cur_source, cur_source + kWordSize - begin_offset, data.bytes + begin_offset);

    if (!write(begin_word_addr, data.word))
      return false;

    // Set up for the full-word copying loop.
    cur_source += kWordSize - begin_offset;
    begin_full_addr = begin_word_addr + kWordSize;
  }

  uint64_t last_full_addr;  // Address of last full word to write.
  if (last_offset == kWordSize - 1) {
    // Last word is full.
    last_full_addr = last_word_addr;
  } else {
    // Do partial last word.
    WordBytes data;
    if (!read(last_word_addr, &data.word))
      return false;

    std::copy(buf_bytes + len - last_offset - 1, buf_bytes + len, data.bytes);

    if (!write(last_word_addr, data.word))
      return false;

    last_full_addr = last_word_addr - kWordSize;
  }

  // Do all the full words in between.
  for (uint64_t cur_dest = begin_full_addr; cur_dest <= last_full_addr;
       cur_dest += kWordSize, cur_source += kWordSize) {
    // Read the current source value into a single word (it's not necessarily aligned).
    uint64_t src_word;
    memcpy(&src_word, cur_source, kWordSize);
    if (!write(cur_dest, src_word))
      return false;
  }

  return true;
}

}  // namespace debug_agent
