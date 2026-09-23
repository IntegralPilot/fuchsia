// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_FASTBOOT_TEST_FASTBOOT_TEST_H_
#define SRC_FIRMWARE_LIB_FASTBOOT_TEST_FASTBOOT_TEST_H_

#include <lib/fastboot/fastboot.h>
#include <lib/fastboot/test/test-transport.h>
#include <zircon/assert.h>

#include <cstring>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <safemath/safe_math.h>

#include "src/lib/fxl/strings/string_printf.h"
#include "third_party/android/platform/system/core/libsparse/sparse_format.h"
// The above header includes the
// third_party/android/platform/system/core/libsparse/sparse_defs.h which defines a `error()`
// macro. It confuses the compiler when we try to create a zx::result with zx::error(). Thus,
// we need to undefine the macro.
#ifdef error
#undef error
#endif

namespace fastboot {

class FastbootDownloadTest : public testing::Test {
 public:
  // Exercises the protocol to download the given data to the Fastboot instance.
  static void DownloadData(Fastboot& fastboot, const std::vector<uint8_t>& download_content) {
    std::string size_hex_str = fxl::StringPrintf("%08zx", download_content.size());

    std::string command = "download:" + size_hex_str;
    TestTransport transport;
    transport.AddInPacket(command);
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();
    std::vector<std::string> expected_packets = {
        "DATA" + size_hex_str,
    };
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
    ASSERT_EQ(fastboot.download_vmo_mapper_.size(), download_content.size());
    // start the download

    // Transmit the first half.
    std::span<const uint8_t> first_half{download_content.data(), download_content.size() / 2};
    transport.AddInPacket(first_half);
    ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();
    // There should be no new response packet.
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
    ASSERT_EQ(
        std::memcmp(fastboot.download_vmo_mapper_.start(), first_half.data(), first_half.size()),
        0);

    // Transmit the second half
    std::span<const uint8_t> second_half{download_content.data() + first_half.size(),
                                         download_content.size() - first_half.size()};
    transport.AddInPacket(second_half);
    ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();
    expected_packets.push_back("OKAY");
    ASSERT_THAT(transport.GetOutPackets(), testing::ContainerEq(expected_packets));
    ASSERT_EQ(std::memcmp(fastboot.download_vmo_mapper_.start(), download_content.data(),
                          download_content.size()),
              0);
  }

  // Overload for string data; does not include any trailing null.
  static void DownloadData(Fastboot& fastboot, std::string_view content) {
    std::vector<uint8_t> byte_content(content.size());
    memcpy(byte_content.data(), content.data(), content.size());
    DownloadData(fastboot, byte_content);
  }
};

class SparseImageBuilder {
 public:
  explicit SparseImageBuilder(const size_t block_size)
      : buffer_(sizeof(sparse_header_t)), block_size_(block_size) {}

  SparseImageBuilder& RawChunk(const std::span<const uint8_t> data) {
    ZX_ASSERT_MSG(data.size() % block_size_ == 0, "data must be block aligned");
    const size_t num_blocks = data.size() / block_size_;
    const chunk_header_t header = {
        .chunk_type = CHUNK_TYPE_RAW,
        .chunk_sz = safemath::checked_cast<uint32_t>(num_blocks),
        .total_sz = safemath::checked_cast<uint32_t>(sizeof(chunk_header_t) + data.size()),
    };
    AddChunk(header, data);
    return *this;
  }

  SparseImageBuilder& SkipChunk(const size_t num_blocks) {
    const chunk_header_t header = {
        .chunk_type = CHUNK_TYPE_DONT_CARE,
        .chunk_sz = safemath::checked_cast<uint32_t>(num_blocks),
        .total_sz = uint32_t{sizeof(chunk_header_t)},
    };
    AddChunk(header);
    return *this;
  }

  SparseImageBuilder& FillChunk(const size_t num_blocks, const uint8_t fill_pattern) {
    const std::array<uint8_t, 4> fill_value = {fill_pattern, fill_pattern, fill_pattern,
                                               fill_pattern};
    const chunk_header_t header = {
        .chunk_type = CHUNK_TYPE_FILL,
        .chunk_sz = safemath::checked_cast<uint32_t>(num_blocks),
        .total_sz = uint32_t{sizeof(chunk_header_t) + fill_value.size()},
    };
    AddChunk(header, fill_value);
    return *this;
  }

  std::vector<uint8_t> Build() {
    ZX_ASSERT(!buffer_.empty());
    const sparse_header_t header = {
        .magic = SPARSE_HEADER_MAGIC,
        .major_version = 1,
        .minor_version = 0,
        .file_hdr_sz = sizeof(sparse_header_t),
        .chunk_hdr_sz = sizeof(chunk_header_t),
        .blk_sz = safemath::checked_cast<uint32_t>(block_size_),
        .total_blks = safemath::checked_cast<uint32_t>(total_blocks_),
        .total_chunks = safemath::checked_cast<uint32_t>(total_chunks_),
    };
    std::copy(reinterpret_cast<const uint8_t*>(&header),
              reinterpret_cast<const uint8_t*>(&header) + sizeof(header), buffer_.begin());
    return std::move(buffer_);
  }

 private:
  void AddChunk(const chunk_header_t header, const std::span<const uint8_t> data = {}) {
    buffer_.insert(buffer_.end(), reinterpret_cast<const uint8_t*>(&header),
                   reinterpret_cast<const uint8_t*>(&header) + sizeof(header));
    if (!data.empty()) {
      buffer_.insert(buffer_.end(), data.begin(), data.end());
    }
    ++total_chunks_;
    total_blocks_ += header.chunk_sz;
  }

  std::vector<uint8_t> buffer_;
  size_t block_size_;
  size_t total_blocks_ = 0;
  size_t total_chunks_ = 0;
};

}  // namespace fastboot

#endif  // SRC_FIRMWARE_LIB_FASTBOOT_TEST_FASTBOOT_TEST_H_
