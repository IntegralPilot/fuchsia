// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains unit tests specifically for the virtual memory and software
// bounds slicing logic (e.g. CopyTo, CopyFrom, cursor math, scatter-gather) of
// FidlRequest. Tests verifying hardware DMA pinning logic, VMAR mappings, and
// FidlRequestPool are located in request-fidl-test.cc.

#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <map>
#include <numeric>
#include <optional>
#include <vector>

#include <gtest/gtest.h>

#include "usb/request-fidl.h"

namespace {

using BufferTag = fuchsia_hardware_usb_request::Buffer::Tag;

class FidlRequestMemoryTest : public testing::TestWithParam<BufferTag> {
 protected:
  zx::result<std::optional<usb::internal::MappedVmo>> MapBuffer(
      const fuchsia_hardware_usb_request::Buffer& buffer) const {
    if (buffer.Which() == BufferTag::kVmoId) {
      auto vmo_id = buffer.vmo_id().value();
      auto it = mocked_vmos_.find(vmo_id);
      if (it != mocked_vmos_.end()) {
        return zx::ok(usb::internal::MappedVmo{
            .addr = reinterpret_cast<zx_vaddr_t>(it->second.data()),
            .size = it->second.size(),
        });
      }
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    return zx::ok(std::nullopt);
  }

  usb::FidlRequest::get_mapped_func_t GetMappedCallback() {
    return [this](const auto& b) { return MapBuffer(b); };
  }

  void AddBuffer(usb::FidlRequest& req, size_t alloc_size, std::optional<size_t> region_size,
                 size_t region_offset) {
    if (GetParam() == BufferTag::kVmoId) {
      uint64_t vmo_id = next_vmo_id_++;
      mocked_vmos_[vmo_id] = std::vector<uint8_t>(alloc_size, 0);
      req.add_vmo_id(vmo_id, region_size.value_or(alloc_size), region_offset);
    } else {
      req.add_data(std::vector<uint8_t>(alloc_size, 0), region_size.value_or(alloc_size),
                   region_offset);
    }
  }

  const std::vector<uint8_t>& GetBufferData(const usb::FidlRequest& req, size_t index) const {
    ZX_ASSERT_MSG(req->data().has_value(), "Request data is empty");
    ZX_ASSERT_MSG(index < req->data()->size(), "Index out of bounds");
    const auto& d = req->data()->at(index);
    ZX_ASSERT_MSG(d.buffer().has_value(), "Buffer is missing");
    if (d.buffer()->Which() == BufferTag::kVmoId) {
      auto vmo_id = d.buffer()->vmo_id().value();
      auto it = mocked_vmos_.find(vmo_id);
      ZX_ASSERT_MSG(it != mocked_vmos_.end(), "VMO ID %" PRIu64 " not mapped", vmo_id);
      return it->second;
    }
    return d.buffer()->data().value();
  }

  void AssertContent(const usb::FidlRequest& req, size_t index,
                     const std::vector<uint8_t>& expected) const {
    const auto& data = GetBufferData(req, index);
    EXPECT_EQ(expected, data);
  }

  std::map<uint64_t, std::vector<uint8_t>> mocked_vmos_;
  uint64_t next_vmo_id_ = 1;
};

// Verifies standard single-buffer CopyTo and CopyFrom payload transfers.
TEST_P(FidlRequestMemoryTest, StandardTransfer) {
  usb::FidlRequest req;
  AddBuffer(req, 16, 16, 0);

  std::vector<uint8_t> src = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16};
  auto copied = req.CopyTo(0, src.data(), 16, GetMappedCallback());

  ASSERT_EQ(copied.size(), 1u);
  ASSERT_EQ(copied[0], 16u);
  ASSERT_NO_FATAL_FAILURE(AssertContent(req, 0, src));

  std::vector<uint8_t> dst(16, 0);
  auto read = req.CopyFrom(0, dst.data(), 16, GetMappedCallback());
  ASSERT_EQ(read.size(), 1u);
  ASSERT_EQ(read[0], 16u);
  EXPECT_EQ(src, dst);
}

// Verifies offset slicing and byte boundary constraints on fixed-size VMO allocations.
TEST_P(FidlRequestMemoryTest, BoundariesAndSlicing) {
  if (GetParam() != BufferTag::kVmoId) {
    GTEST_SKIP() << "BoundariesAndSlicing is only applicable to kVmoId allocations";
  }
  usb::FidlRequest req;
  // Buffer allocation: 32 bytes. Region size: 24 bytes. Offset within buffer: 4
  AddBuffer(req, 32, 24, 4);

  // Verify that exactly 24 bytes can be written.
  std::vector<uint8_t> src(64, 0xAB);
  // Copying 25 bytes should only copy 24
  auto copied = req.CopyTo(0, src.data(), 25, GetMappedCallback());

  ASSERT_EQ(copied.size(), 1u);
  ASSERT_EQ(copied[0], 24u);

  auto& buf = GetBufferData(req, 0);
  // Verify offsets: 0-3 empty, 4-27 0xAB, 28-31 empty
  // kVmoId backing VMO was pre-allocated to 32 bytes in AddBuffer, and must retain its 32-byte
  // capacity.
  ASSERT_EQ(buf.size(), 32u);
  EXPECT_EQ(buf[3], 0);
  EXPECT_EQ(buf[4], 0xAB);
  EXPECT_EQ(buf[27], 0xAB);
  EXPECT_EQ(buf[28], 0);

  // Offset slicing on the request side: start writing at offset 8 within the region
  std::vector<uint8_t> src2(8, 0xCD);
  copied = req.CopyTo(8, src2.data(), 8, GetMappedCallback());
  ASSERT_EQ(copied.size(), 1u);
  ASSERT_EQ(copied[0], 8u);

  // Start+8 in region maps to Start+4+8 = 12 in the underlying buffer
  EXPECT_EQ(buf[11], 0xAB);
  EXPECT_EQ(buf[12], 0xCD);
  EXPECT_EQ(buf[19], 0xCD);
  EXPECT_EQ(buf[20], 0xAB);
}

// Verifies multi-region scatter-gather copies across adjacent request buffers while preserving
// pre-allocated buffer capacity.
// Disabled: On origin/main, CopyTo() resizes partial-write kData vectors to written size (5u),
// truncating the vector from 10 to 5 bytes while leaving the logical size at 10. Activated in CL 2.
TEST_P(FidlRequestMemoryTest, DISABLED_ScatterGather) {
  usb::FidlRequest req;
  AddBuffer(req, 10, 10, 0);
  AddBuffer(req, 10, 10, 0);
  AddBuffer(req, 10, 10, 0);

  std::vector<uint8_t> src(30);
  std::iota(src.begin(), src.end(), 1);

  // Copy 25 bytes. Should fill first two completely, and half of the third.
  auto copied = req.CopyTo(0, src.data(), 25, GetMappedCallback());
  ASSERT_EQ(copied.size(), 3u);
  EXPECT_EQ(copied[0], 10u);
  EXPECT_EQ(copied[1], 10u);
  EXPECT_EQ(copied[2], 5u);

  auto& buf0 = GetBufferData(req, 0);
  EXPECT_EQ(buf0[9], 10);

  auto& buf1 = GetBufferData(req, 1);
  EXPECT_EQ(buf1[0], 11);
  EXPECT_EQ(buf1[9], 20);

  auto& buf2 = GetBufferData(req, 2);
  EXPECT_EQ(buf2[0], 21);
  EXPECT_EQ(buf2[4], 25);
  EXPECT_EQ(buf2[5], 0);
  EXPECT_EQ(buf2.size(), 10u);
  EXPECT_EQ(req.request().data()->at(2).size().value(), 10u);

  std::vector<uint8_t> dst(25, 0);
  auto read = req.CopyFrom(0, dst.data(), 25, GetMappedCallback());
  ASSERT_EQ(read.size(), 3u);
  EXPECT_EQ(read[0], 10u);
  EXPECT_EQ(read[1], 10u);
  EXPECT_EQ(read[2], 5u);

  std::vector<uint8_t> expected_read(25);
  std::iota(expected_read.begin(), expected_read.end(), 1);
  EXPECT_EQ(expected_read, dst);
}

// Verifies copy behavior on zero-length regions and out-of-bounds offsets.
// Disabled: CopyTo and CopyFrom require bounds validation updates for zero-length regions.
TEST_P(FidlRequestMemoryTest, DISABLED_ZeroLengthAndOutOfBounds) {
  usb::FidlRequest req;
  AddBuffer(req, 5, 0, 5);  // Zero size region

  std::vector<uint8_t> src(10, 0xFF);
  auto copied = req.CopyTo(0, src.data(), 10, GetMappedCallback());

  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);

  // Test Out of Bounds cur_offset
  usb::FidlRequest req_oob;
  AddBuffer(req_oob, 10, 10, 0);
  auto copied_oob = req_oob.CopyTo(15, src.data(), 5, GetMappedCallback());
  ASSERT_EQ(copied_oob.size(), 1u);
  EXPECT_EQ(copied_oob[0], 0u);

  std::vector<uint8_t> dst(10, 0);
  // Test Out of Bounds cur_offset for CopyFrom
  auto read_oob = req_oob.CopyFrom(15, dst.data(), 5, GetMappedCallback());
  ASSERT_EQ(read_oob.size(), 1u);
  EXPECT_EQ(read_oob[0], 0u);

  // Test Zero size region for CopyFrom
  auto read_zero = req.CopyFrom(0, dst.data(), 10, GetMappedCallback());
  ASSERT_EQ(read_zero.size(), 1u);
  EXPECT_EQ(read_zero[0], 0u);
}

// Verifies that request copy operations skip past intermediate preceding buffers when cur_offset
// spans past them.
TEST_P(FidlRequestMemoryTest, IntermediateBufferSkipping) {
  usb::FidlRequest req;
  AddBuffer(req, 10, 10, 0);
  AddBuffer(req, 10, 10, 0);
  AddBuffer(req, 10, 10, 0);

  std::vector<uint8_t> src(5, 0xAA);
  // cur_offset=25. Skips buf 0 (10 bytes) and buf 1 (10 bytes), starts at offset 5 in buf 2, writes
  // 5 bytes.
  auto copied = req.CopyTo(25, src.data(), 5, GetMappedCallback());
  ASSERT_EQ(copied.size(), 3u);
  EXPECT_EQ(copied[0], 0u);
  EXPECT_EQ(copied[1], 0u);
  EXPECT_EQ(copied[2], 5u);

  auto& buf0 = GetBufferData(req, 0);
  EXPECT_EQ(buf0[9], 0);

  auto& buf1 = GetBufferData(req, 1);
  EXPECT_EQ(buf1[9], 0);

  auto& buf2 = GetBufferData(req, 2);
  EXPECT_EQ(buf2[4], 0);
  EXPECT_EQ(buf2[5], 0xAA);
  EXPECT_EQ(buf2[9], 0xAA);
}

// Verifies that CopyTo dynamically resizes kData vector buffers to account for region offsets.
// Disabled: CopyTo requires dynamic vector capacity expansion for region offset + payload size.
TEST_P(FidlRequestMemoryTest, DISABLED_DataResizeBehavior) {
  if (GetParam() == BufferTag::kVmoId) {
    GTEST_SKIP() << "DataResizeBehavior is only applicable to kData dynamic vectors";
  }
  usb::FidlRequest req;
  // Explicitly provision memory boundary logic to test reallocation
  AddBuffer(req, 5, 50, 5);  // Provide explicit 50 byte boundary

  std::vector<uint8_t> src(50, 0xEE);
  auto copied = req.CopyTo(0, src.data(), 50, GetMappedCallback());

  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 50u);

  auto& buf = req->data()->at(0).buffer()->data().value();
  ASSERT_EQ(buf.size(), 55u);
  EXPECT_EQ(buf[4], 0);
  EXPECT_EQ(buf[5], 0xEE);
  EXPECT_EQ(buf[54], 0xEE);
}

// Verifies out-of-bounds error handling (ZX_ERR_OUT_OF_RANGE) for CacheFlush and
// CacheFlushInvalidate across request-level capacity bounds.
TEST_P(FidlRequestMemoryTest, CacheHelperValidation) {
  usb::FidlRequest req;
  AddBuffer(req, 10, 15, 0);

  // Request-level capacity bounds check (offset 20 + size 5 exceeds capacity 15).
  zx_status_t res = req.CacheFlush(GetMappedCallback(), 20, 5);
  EXPECT_EQ(res, ZX_ERR_OUT_OF_RANGE);

  usb::FidlRequest req2;
  // Cache check for OOB offset
  AddBuffer(req2, 10, 5, 15);
  res = req2.CacheFlush(GetMappedCallback(), 20, 5);
  EXPECT_EQ(res, ZX_ERR_OUT_OF_RANGE);

  // Cache check for invalidation
  res = req2.CacheFlushInvalidate(GetMappedCallback(), 20, 5);
  EXPECT_EQ(res, ZX_ERR_OUT_OF_RANGE);
}

using FidlRequestMemorySimpleTest = FidlRequestMemoryTest;

// Verifies that mapping errors from GetMappedCallback gracefully return 0 copied bytes.
TEST_F(FidlRequestMemorySimpleTest, GetMappedErrorPropagation) {
  usb::FidlRequest req;
  mocked_vmos_[1] = std::vector<uint8_t>(10, 0);
  req.add_vmo_id(1, 10, 0);

  auto error_callback = [](const fuchsia_hardware_usb_request::Buffer& /*buffer*/)
      -> zx::result<std::optional<usb::internal::MappedVmo>> {
    return zx::error(ZX_ERR_BAD_STATE);
  };

  std::vector<uint8_t> src(10, 0xBB);
  auto copied = req.CopyTo(0, src.data(), 10, error_callback);
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);

  std::vector<uint8_t> dst(10, 0);
  auto read = req.CopyFrom(0, dst.data(), 10, error_callback);
  ASSERT_EQ(read.size(), 1u);
  EXPECT_EQ(read[0], 0u);
}

// Verifies that empty requests report length 0 and return empty copy results without crashing on
// clear_buffers() or reset_buffers().
// Disabled: length(), clear_buffers(), and reset_buffers() require optional check for empty request
// data vectors.
TEST_F(FidlRequestMemorySimpleTest, DISABLED_EmptyRequestHandling) {
  usb::FidlRequest req;
  EXPECT_EQ(req.length(), 0u);

  req.clear_buffers();
  req.reset_buffers(GetMappedCallback());
  EXPECT_EQ(req.length(), 0u);

  std::vector<uint8_t> src(16, 0xAB);
  auto copied = req.CopyTo(0, src.data(), 16, GetMappedCallback());
  EXPECT_TRUE(copied.empty());

  std::vector<uint8_t> dst(16, 0);
  auto read = req.CopyFrom(0, dst.data(), 16, GetMappedCallback());
  EXPECT_TRUE(read.empty());
}

// Verifies that clear_buffers() clears size while preserving vector capacity, and reset_buffers()
// restores payload limits.
// Disabled: reset_buffers() requires payload limit retention instead of default size restoration.
TEST_F(FidlRequestMemorySimpleTest, DISABLED_DataCapacityRetention) {
  usb::FidlRequest req;
  req.add_data(std::vector<uint8_t>(10, 0), 10, 0);

  // Clearing the request destroys the elements and sets size to 0, but retains the underlying
  // vector capacity
  req.clear_buffers();
  ASSERT_EQ(req->data()->at(0).buffer()->data()->size(), 0u);
  ASSERT_GE(req->data()->at(0).buffer()->data()->capacity(), 10u);

  // Simulating an enqueue/recycle operation for an IN transfer
  // This MUST reallocate the vector limits up to hardware limits
  req.reset_buffers(GetMappedCallback());

  // The vector capacity should be restored to standard limits to natively accept IN payloads
  EXPECT_EQ(req->data()->at(0).buffer()->data()->size(), 0u);
  EXPECT_EQ(req->data()->at(0).size().value(), 10u);
  EXPECT_GE(req->data()->at(0).buffer()->data()->capacity(), 10u);
}

// Verifies CopyTo after clear_buffers() when an explicit region offset is assigned.
// Disabled: CopyTo requires dynamic vector resizing for offset + written payload size after buffer
// clearing.
TEST_F(FidlRequestMemorySimpleTest, DISABLED_ClearedBufferWithOffsetCopyTo) {
  usb::FidlRequest req;
  req.add_data(std::vector<uint8_t>(20, 0), 0, 0);

  req.clear_buffers();
  ASSERT_TRUE(req->data()->at(0).buffer()->data()->empty());
  req->data()->at(0).offset(5);
  req->data()->at(0).size(15);

  std::vector<uint8_t> src(10, 0xAA);
  auto copied = req.CopyTo(0, src.data(), 10, GetMappedCallback());
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 10u);

  // CopyTo dynamically expands kData vectors to fit region_offset + bytes written (5 + 10 = 15)
  // rather than eagerly allocating the full region capacity (20), avoiding over-allocation for
  // partial transfers.
  auto& buf = req->data()->at(0).buffer()->data().value();
  ASSERT_EQ(buf.size(), 15u);
  EXPECT_EQ(buf[4], 0);
  EXPECT_EQ(buf[5], 0xAA);
  EXPECT_EQ(buf[14], 0xAA);
}

// Verifies that length() correctly computes total payload length for kVmoId buffers larger than
// 1024 bytes (e.g. MTUs 1514B, 2048B, and jumbo frames).
TEST_F(FidlRequestMemorySimpleTest, LengthWithLargeVmoTest) {
  usb::FidlRequest req;
  mocked_vmos_[0] = std::vector<uint8_t>(4096, 0);
  req.add_vmo_id(0, 1514, 0);
  EXPECT_EQ(req.length(), 1514u);

  usb::FidlRequest jumbo_req;
  mocked_vmos_[1] = std::vector<uint8_t>(16384, 0);
  jumbo_req.add_vmo_id(1, 9000, 0);
  EXPECT_EQ(jumbo_req.length(), 9000u);
}

// Verifies that CachedCopyFrom succeeds on kVmoId payloads larger than 1024 bytes without returning
// ZX_ERR_OUT_OF_RANGE.
TEST_F(FidlRequestMemorySimpleTest, CachedCopyFromWithLargeVmoTest) {
  usb::FidlRequest req;
  mocked_vmos_[0] = std::vector<uint8_t>(2048, 0x5A);
  req.add_vmo_id(0, 1514, 0);

  std::vector<uint8_t> dst(1514, 0);
  auto res = req.CachedCopyFrom(0, dst.data(), 1514, GetMappedCallback());
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ((*res)[0], 1514u);
  EXPECT_EQ(dst[0], 0x5A);
  EXPECT_EQ(dst[1513], 0x5A);
}

// Verifies that add_data handles near-overflow region offsets without causing out-of-memory dynamic
// vector allocations.
// Disabled: add_data() requires arithmetic overflow validation before vector allocation.
TEST_F(FidlRequestMemorySimpleTest, DISABLED_AddDataNearOverflowOffsetNoSize) {
  usb::FidlRequest req;
  req.add_data(std::vector<uint8_t>(10, 0), 0, std::numeric_limits<size_t>::max() - 100);

  std::vector<uint8_t> src(10, 0xAA);
  auto copied = req.CopyTo(0, src.data(), 10, GetMappedCallback());
  ASSERT_EQ(copied.size(), 1u);
  EXPECT_EQ(copied[0], 0u);
}

// Verifies length calculation when a kVmoId buffer has an omitted size field.
// Disabled: length() requires optional region size validation for kVmoId buffers.
TEST_F(FidlRequestMemorySimpleTest, DISABLED_LengthWithUnresolvedVmoSizeTest) {
  fuchsia_hardware_usb_request::Request request;
  request.data()
      .emplace()
      .emplace_back()
      .buffer(fuchsia_hardware_usb_request::Buffer::WithVmoId(0))
      .offset(0);  // size is intentionally omitted
  usb::FidlRequest fidl_request(std::move(request));
  EXPECT_EQ(fidl_request.length(), 0u);
}

INSTANTIATE_TEST_SUITE_P(FidlRequestTests, FidlRequestMemoryTest,
                         testing::Values(BufferTag::kData, BufferTag::kVmoId));

}  // namespace
