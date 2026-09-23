// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/zx/event.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <zxtest/zxtest.h>

namespace {

using ElementType = uint64_t;
constexpr size_t kElementSize = sizeof(ElementType);

zx_signals_t GetSignals(const zx::fifo& fifo) {
  zx_signals_t pending;
  zx_status_t status = fifo.wait_one(0xFFFFFFFF, zx::time(), &pending);
  if ((status != ZX_OK) && (status != ZX_ERR_TIMED_OUT)) {
    return 0xFFFFFFFF;
  }
  return pending;
}

#define EXPECT_SIGNALS(h, s) EXPECT_EQ(GetSignals(h), s)

// Helper class to map a VMO somewhere in the address space with a page of padding to either side.
// Cleans up on destruction.
class VmoMapWithPadding {
 public:
  VmoMapWithPadding() = default;
  ~VmoMapWithPadding() {
    if (vmar_.is_valid()) {
      vmar_.unmap(vmar_addr_, vmar_size_);
      vmar_.destroy();
      vmar_.reset();
    }
  }

  zx_status_t Map(const zx::vmo& vmo, size_t vmo_size, zx_vaddr_t* out_addr) {
    if (vmar_.is_valid()) {
      return ZX_ERR_BAD_STATE;
    }

    // Create a vmar with a page on either side as padding.
    vmar_size_ = vmo_size + 2 * zx_system_get_page_size();
    zx_status_t err = zx::vmar::root_self()->allocate(
        ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0, vmar_size_, &vmar_,
        &vmar_addr_);
    if (err != ZX_OK) {
      return err;
    }

    // Map the passed in vmo.
    zx_vaddr_t addr;
    err = vmar_.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, zx_system_get_page_size(),
                    vmo, 0, vmo_size, &addr);
    if (err != ZX_OK) {
      return err;
    }

    *out_addr = addr;
    return ZX_OK;
  }

 private:
  zx::vmar vmar_;
  zx_vaddr_t vmar_addr_ = 0;
  size_t vmar_size_ = 0;
};

TEST(FifoTest, InvalidParametersReturnOutOfRange) {
  zx::fifo fifo_a, fifo_b;

  // ensure parameter validation works
  EXPECT_EQ(zx::fifo::create(0, 0, 0, &fifo_a, &fifo_b),
            ZX_ERR_OUT_OF_RANGE);  // too small
  EXPECT_EQ(zx::fifo::create(128, 33, 0, &fifo_a, &fifo_b),
            ZX_ERR_OUT_OF_RANGE);  // too large
  EXPECT_EQ(zx::fifo::create(0, 0, 1, &fifo_a, &fifo_b),
            ZX_ERR_INVALID_ARGS);  // invalid options
}

TEST(FifoTest, EndpointsAreRelated) {
  zx::fifo fifo_a, fifo_b;

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  // Check that koids line up.
  zx_info_handle_basic_t info_a = {}, info_b = {};
  ASSERT_OK(fifo_a.get_info(ZX_INFO_HANDLE_BASIC, &info_a, sizeof(info_a), nullptr, nullptr));

  ASSERT_OK(fifo_b.get_info(ZX_INFO_HANDLE_BASIC, &info_b, sizeof(info_b), nullptr, nullptr));
  ASSERT_NE(info_a.koid, 0u, "zero koid!");
  ASSERT_NE(info_a.related_koid, 0u, "zero peer koid!");
  ASSERT_NE(info_b.koid, 0u, "zero koid!");
  ASSERT_NE(info_b.related_koid, 0u, "zero peer koid!");
  ASSERT_EQ(info_a.koid, info_b.related_koid, "mismatched koids!");
  ASSERT_EQ(info_b.koid, info_a.related_koid, "mismatched koids!");
}

TEST(FifoTest, EmptyQueueReturnsErrShouldWait) {
  zx::fifo fifo_a, fifo_b;
  ElementType actual_elements[8] = {};

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  // should not be able to read any entries from an empty fifo
  size_t actual_count;
  EXPECT_EQ(fifo_a.read(kElementSize, actual_elements, 8, &actual_count), ZX_ERR_SHOULD_WAIT);
}

TEST(FifoTest, ReadAndWriteValidatesSizeAndElementCount) {
  zx::fifo fifo_a, fifo_b;
  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8};
  ElementType actual_elements[8] = {};
  size_t actual_count;

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  // not allowed to read or write zero elements
  EXPECT_EQ(fifo_a.read(kElementSize, actual_elements, 0, &actual_count), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(fifo_a.write(kElementSize, expected_elements, 0, &actual_count), ZX_ERR_OUT_OF_RANGE);

  // element size must match
  EXPECT_EQ(fifo_a.read(kElementSize + 1, actual_elements, 8, &actual_count), ZX_ERR_OUT_OF_RANGE);
  EXPECT_EQ(fifo_a.write(kElementSize + 1, expected_elements, 8, &actual_count),
            ZX_ERR_OUT_OF_RANGE);
}

TEST(FifoTest, DequeueSignalsWriteable) {
  zx::fifo fifo_a, fifo_b;
  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8};
  ElementType actual_elements[8] = {};
  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  size_t actual_count;
  // should be able to write all entries into empty fifo
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 8, &actual_count));
  ASSERT_EQ(actual_count, 8u);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_READABLE | ZX_FIFO_WRITABLE);

  // should be able to write no entries into a full fifo
  ASSERT_EQ(fifo_a.write(kElementSize, expected_elements, 8, &actual_count), ZX_ERR_SHOULD_WAIT);
  EXPECT_SIGNALS(fifo_a, 0u);

  // read half the entries, make sure they're what we expect
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, 4, &actual_count));
  ASSERT_EQ(actual_count, 4u);
  ASSERT_EQ(actual_elements[0], 1u);
  ASSERT_EQ(actual_elements[1], 2u);
  ASSERT_EQ(actual_elements[2], 3u);
  ASSERT_EQ(actual_elements[3], 4u);
  ASSERT_EQ(actual_elements[4], 0u);

  // should be writable again now
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);

  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, 4, &actual_count));
  ASSERT_EQ(actual_elements[0], 5u);
  ASSERT_EQ(actual_elements[1], 6u);
  ASSERT_EQ(actual_elements[2], 7u);
  ASSERT_EQ(actual_elements[3], 8u);
  ASSERT_EQ(actual_elements[4], 0u);

  // should no longer be readable
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);
}

TEST(FifoTest, FifoOrderIsPreserved) {
  zx::fifo fifo_a, fifo_b;
  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8};
  ElementType actual_elements[8] = {};

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;
  // should be able to write all entries into empty fifo
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 8, &actual_count));

  // read half the entries, make sure they're what we expect
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, 4, &actual_count));

  // write some more, wrapping to the front again
  expected_elements[0] = 9u;
  expected_elements[1] = 10u;
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 2, &actual_count));
  ASSERT_EQ(actual_count, 2u);

  // read across the wrap, test partial read
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, 8, &actual_count));
  ASSERT_EQ(actual_count, 6u);
  ASSERT_EQ(actual_elements[0], 5u);
  ASSERT_EQ(actual_elements[1], 6u);
  ASSERT_EQ(actual_elements[2], 7u);
  ASSERT_EQ(actual_elements[3], 8u);
  ASSERT_EQ(actual_elements[4], 9u);
  ASSERT_EQ(actual_elements[5], 10u);

  // write across the wrap
  expected_elements[0] = 11u;
  expected_elements[1] = 12u;
  expected_elements[2] = 13u;
  expected_elements[3] = 14u;
  expected_elements[4] = 15u;
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 5, &actual_count));
  ASSERT_EQ(actual_count, 5u);
}

TEST(FifoTest, PartialWriteQueuesElementsThatFit) {
  zx::fifo fifo_a, fifo_b;
  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8};

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;
  // Fill it up for 5 elements.
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 5, &actual_count));

  // partial write test
  expected_elements[0] = 16u;
  expected_elements[1] = 17u;
  expected_elements[2] = 18u;
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 5, &actual_count));
  ASSERT_EQ(actual_count, 3u);
}

TEST(FifoTest, IndividualReadsPreserveOrder) {
  zx::fifo fifo_a, fifo_b;
  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8};

  // simple 8 x 8 fifo
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;
  // Fill it up
  ASSERT_OK(fifo_a.write(kElementSize, expected_elements, 8, &actual_count));

  ElementType actual_element;
  // small reads
  for (unsigned i = 0; i < 8; i++) {
    ASSERT_OK(fifo_b.read(kElementSize, &actual_element, 1, &actual_count));
    ASSERT_EQ(actual_count, 1u);
    ASSERT_EQ(actual_element, expected_elements[i]);
  }
}

TEST(FifoTest, EndpointCloseSignalsPeerClosed) {
  zx::fifo fifo_b;
  ElementType expected_element = 19u;
  ElementType actual_elements[8] = {};

  size_t actual_count;

  {
    zx::fifo fifo_a;
    ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

    // write and then close, verify we can read written entries before
    // receiving ZX_ERR_PEER_CLOSED.
    ASSERT_OK(fifo_a.write(kElementSize, &expected_element, 1, &actual_count));
    ASSERT_EQ(actual_count, 1u);
    // end of scope for fifo_b so it's closed.
  }

  EXPECT_SIGNALS(fifo_b, ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED);
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, 8, &actual_count));
  ASSERT_EQ(actual_count, 1u);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_PEER_CLOSED);
  ASSERT_EQ(fifo_b.read(kElementSize, actual_elements, 8, &actual_count), ZX_ERR_PEER_CLOSED);
  ASSERT_EQ(fifo_b.signal_peer(0u, ZX_USER_SIGNAL_0), ZX_ERR_PEER_CLOSED);
}

TEST(FifoTest, NonPowerOfTwoCountSupported) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(10, kElementSize, 0, &fifo_a, &fifo_b));

  ElementType expected_elements[] = {1, 2, 3, 4, 5, 6, 7, 8, 9};
  ElementType actual_elements[9] = {};
  size_t actual_count;

  // Write to, then drain, the FIFO.
  // Intentionally write one element less than the FIFO can hold, so the next write will wrap.
  ASSERT_OK(
      fifo_a.write(kElementSize, &expected_elements, std::size(expected_elements), &actual_count));
  ASSERT_EQ(actual_count, 9u);
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, std::size(actual_elements), &actual_count));
  ASSERT_EQ(actual_count, 9u);

  // Repeat the process. This write spans the buffer wrap.
  ASSERT_OK(
      fifo_a.write(kElementSize, &expected_elements, std::size(expected_elements), &actual_count));
  ASSERT_EQ(actual_count, 9u);
  ASSERT_OK(fifo_b.read(kElementSize, actual_elements, std::size(actual_elements), &actual_count));
  ASSERT_EQ(actual_count, 9u);
}

TEST(FifoTest, ReadNullBuffer) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1};
  EXPECT_OK(fifo_a.write(kElementSize, &element, std::size(element), &actual_count));

  EXPECT_STATUS(fifo_b.read(kElementSize, nullptr, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, WriteNullBuffer) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1};
  EXPECT_STATUS(fifo_a.write(kElementSize, nullptr, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, ReadBadBuffer) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  zx_vaddr_t addr;

  // Note, no options means the buffer is not readable.
  ASSERT_OK(zx::vmar::root_self()->map(0, 0, vmo, 0, kVmoSize, &addr));
  auto unmap = fit::defer([&]() { zx::vmar::root_self()->unmap(addr, kVmoSize); });

  void* buffer = reinterpret_cast<void*>(addr);

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1};
  EXPECT_OK(fifo_a.write(kElementSize, &element, std::size(element), &actual_count));

  EXPECT_STATUS(fifo_b.read(kElementSize, buffer, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, WriteBadBuffer) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  zx_vaddr_t addr;

  // Note, no options means the buffer is not readable.
  ASSERT_OK(zx::vmar::root_self()->map(0, 0, vmo, 0, kVmoSize, &addr));
  auto unmap = fit::defer([&]() { zx::vmar::root_self()->unmap(addr, kVmoSize); });

  void* buffer = reinterpret_cast<void*>(addr);

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1};
  EXPECT_STATUS(fifo_a.write(kElementSize, buffer, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, ReadPartialBadBuffer) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  VmoMapWithPadding map;
  zx_vaddr_t addr;
  ASSERT_OK(map.Map(vmo, kVmoSize, &addr));

  // Calculate buffer such that 1 element will fit, and the next will be out of bounds.
  void* buffer = reinterpret_cast<void*>(addr + kVmoSize - sizeof(ElementType));

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1, 2};
  EXPECT_OK(fifo_a.write(kElementSize, &element, std::size(element), &actual_count));

  EXPECT_STATUS(fifo_b.read(kElementSize, buffer, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, WritePartialBadBuffer) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  VmoMapWithPadding map;
  zx_vaddr_t addr;
  ASSERT_OK(map.Map(vmo, kVmoSize, &addr));

  // Calculate buffer such that 1 element will fit, and the next will be out of bounds.
  void* buffer = reinterpret_cast<void*>(addr + kVmoSize - sizeof(ElementType));

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  size_t actual_count;

  ElementType element[] = {1, 2};
  EXPECT_STATUS(fifo_a.write(kElementSize, buffer, std::size(element), &actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, ReadNoReadRights) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(fifo_b.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  zx::fifo reduced_fifo;
  ASSERT_OK(fifo_b.replace(info.rights & ~ZX_RIGHT_READ, &reduced_fifo));

  ElementType actual_element;
  EXPECT_STATUS(reduced_fifo.read(kElementSize, &actual_element, 1, nullptr), ZX_ERR_ACCESS_DENIED);
}

TEST(FifoTest, WriteNoWriteRights) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(fifo_a.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  zx::fifo reduced_fifo;
  ASSERT_OK(fifo_a.replace(info.rights & ~ZX_RIGHT_WRITE, &reduced_fifo));

  ElementType expected_element = 42;
  EXPECT_STATUS(reduced_fifo.write(kElementSize, &expected_element, 1, nullptr),
                ZX_ERR_ACCESS_DENIED);
}

TEST(FifoTest, CreateZeroSize) {
  zx::fifo fifo_a, fifo_b;
  EXPECT_STATUS(zx::fifo::create(0, kElementSize, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx::fifo::create(8, 0, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
}

TEST(FifoTest, CreateMaxBufferBoundary) {
  zx::fifo fifo_a, fifo_b;
  // Exact maximum size (4096 bytes)
  EXPECT_OK(zx::fifo::create(512, 8, 0, &fifo_a, &fifo_b));

  // Over maximum size (4104 bytes)
  EXPECT_STATUS(zx::fifo::create(513, 8, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);

  // Over maximum size (4608 bytes)
  EXPECT_STATUS(zx::fifo::create(512, 9, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
}

TEST(FifoTest, ReadWriteNullActualCount) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  ElementType expected_element = 1234;
  ASSERT_OK(fifo_a.write(kElementSize, &expected_element, 1, nullptr));

  ElementType actual_element = 0;
  ASSERT_OK(fifo_b.read(kElementSize, &actual_element, 1, nullptr));
  EXPECT_EQ(actual_element, 1234u);
}

TEST(FifoTest, WritePeerClosedReturnsPeerClosed) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  // Close the peer FIFO endpoint.
  fifo_a.reset();

  ElementType element = 42;
  size_t actual_count = 0;
  EXPECT_STATUS(fifo_b.write(kElementSize, &element, 1, &actual_count), ZX_ERR_PEER_CLOSED);
}

TEST(FifoTest, ReadWriteBadActualCountReturnsInvalidArgs) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  size_t* bad_actual_count = reinterpret_cast<size_t*>(1);

  ElementType element = 1234;
  EXPECT_STATUS(fifo_a.write(kElementSize, &element, 1, bad_actual_count), ZX_ERR_INVALID_ARGS);

  // Write a valid element so that the FIFO has content for the read attempt.
  ASSERT_OK(fifo_a.write(kElementSize, &element, 1, nullptr));

  ElementType actual_element = 0;
  EXPECT_STATUS(fifo_b.read(kElementSize, &actual_element, 1, bad_actual_count),
                ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, ReadWriteKernelAddressBufferReturnsInvalidArgs) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  void* kernel_buffer = reinterpret_cast<void*>(~0UL);
  size_t actual_count = 0;

  EXPECT_STATUS(fifo_a.write(kElementSize, kernel_buffer, 1, &actual_count), ZX_ERR_INVALID_ARGS);

  ElementType element = 1;
  ASSERT_OK(fifo_a.write(kElementSize, &element, 1, &actual_count));

  EXPECT_STATUS(fifo_b.read(kElementSize, kernel_buffer, 1, &actual_count), ZX_ERR_INVALID_ARGS);
}

TEST(FifoTest, CreateArithmeticOverflow) {
  zx_handle_t out0 = ZX_HANDLE_INVALID;
  zx_handle_t out1 = ZX_HANDLE_INVALID;
  EXPECT_STATUS(zx_fifo_create(SIZE_MAX, 2, 0, &out0, &out1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx_fifo_create(2, SIZE_MAX, 0, &out0, &out1), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx_fifo_create(SIZE_MAX / 2, 4, 0, &out0, &out1), ZX_ERR_OUT_OF_RANGE);

  zx::fifo fifo_a, fifo_b;
  EXPECT_STATUS(zx::fifo::create(UINT32_MAX, 2, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx::fifo::create(2, UINT32_MAX, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx::fifo::create(1 << 20, 1 << 20, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
}

TEST(FifoTest, CreateMinAndMaxSizes) {
  zx::fifo fifo_a, fifo_b;
  // Minimum valid sizes
  EXPECT_OK(zx::fifo::create(1, 1, 0, &fifo_a, &fifo_b));

  // Maximum buffer boundary: count * elem_size == 4096
  EXPECT_OK(zx::fifo::create(4096, 1, 0, &fifo_a, &fifo_b));
  EXPECT_OK(zx::fifo::create(1, 4096, 0, &fifo_a, &fifo_b));

  // Exceeding maximum buffer size (4097 bytes)
  EXPECT_STATUS(zx::fifo::create(4097, 1, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
  EXPECT_STATUS(zx::fifo::create(1, 4097, 0, &fifo_a, &fifo_b), ZX_ERR_OUT_OF_RANGE);
}

TEST(FifoTest, DefaultRightsAndType) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  zx_info_handle_basic_t info_a = {};
  ASSERT_OK(fifo_a.get_info(ZX_INFO_HANDLE_BASIC, &info_a, sizeof(info_a), nullptr, nullptr));
  EXPECT_EQ(info_a.type, ZX_OBJ_TYPE_FIFO);
  EXPECT_EQ(info_a.rights, ZX_DEFAULT_FIFO_RIGHTS);

  zx_info_handle_basic_t info_b = {};
  ASSERT_OK(fifo_b.get_info(ZX_INFO_HANDLE_BASIC, &info_b, sizeof(info_b), nullptr, nullptr));
  EXPECT_EQ(info_b.type, ZX_OBJ_TYPE_FIFO);
  EXPECT_EQ(info_b.rights, ZX_DEFAULT_FIFO_RIGHTS);
}

TEST(FifoTest, UserSignalsAndSignalPeer) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  // Signal self with user signal
  ASSERT_OK(fifo_a.signal(0, ZX_USER_SIGNAL_0));
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE | ZX_USER_SIGNAL_0);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  // Signal peer with user signal
  ASSERT_OK(fifo_a.signal_peer(0, ZX_USER_SIGNAL_1));
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE | ZX_USER_SIGNAL_0);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE | ZX_USER_SIGNAL_1);

  // Clear user signals
  ASSERT_OK(fifo_a.signal(ZX_USER_SIGNAL_0, 0));
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);
  ASSERT_OK(fifo_a.signal_peer(ZX_USER_SIGNAL_1, 0));
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  // Non-user signals cannot be modified via signal / signal_peer
  EXPECT_STATUS(fifo_a.signal(0, ZX_FIFO_READABLE), ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(fifo_a.signal_peer(0, ZX_FIFO_READABLE), ZX_ERR_INVALID_ARGS);

  // Close fifo_a and verify signal on surviving endpoint fifo_b still works
  fifo_a.reset();
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_PEER_CLOSED);
  ASSERT_OK(fifo_b.signal(0, ZX_USER_SIGNAL_2));
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_PEER_CLOSED | ZX_USER_SIGNAL_2);
}

TEST(FifoTest, WriteToClosedPeerReturnsPeerClosed) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  fifo_b.reset();
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_PEER_CLOSED);

  ElementType element = 42;
  size_t actual_count = 0;
  EXPECT_STATUS(fifo_a.write(kElementSize, &element, 1, &actual_count), ZX_ERR_PEER_CLOSED);
}

TEST(FifoTest, SingleElementCapacity) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(1, kElementSize, 0, &fifo_a, &fifo_b));

  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  ElementType element = 100;
  size_t actual_count = 0;

  // Write 1 element -> FIFO becomes full, fifo_a loses WRITABLE, fifo_b becomes READABLE | WRITABLE
  ASSERT_OK(fifo_a.write(kElementSize, &element, 1, &actual_count));
  EXPECT_EQ(actual_count, 1u);
  EXPECT_SIGNALS(fifo_a, 0u);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_READABLE | ZX_FIFO_WRITABLE);

  // Further write should fail with SHOULD_WAIT
  EXPECT_STATUS(fifo_a.write(kElementSize, &element, 1, &actual_count), ZX_ERR_SHOULD_WAIT);

  // Read 1 element -> FIFO becomes empty, fifo_b loses READABLE, fifo_a becomes WRITABLE
  ElementType read_element = 0;
  ASSERT_OK(fifo_b.read(kElementSize, &read_element, 1, &actual_count));
  EXPECT_EQ(actual_count, 1u);
  EXPECT_EQ(read_element, 100u);
  EXPECT_SIGNALS(fifo_a, ZX_FIFO_WRITABLE);
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);

  // Further read should fail with SHOULD_WAIT
  EXPECT_STATUS(fifo_b.read(kElementSize, &read_element, 1, &actual_count), ZX_ERR_SHOULD_WAIT);
}

TEST(FifoTest, WriteRollbackOnWrapAroundFault) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  VmoMapWithPadding map;
  zx_vaddr_t addr;
  ASSERT_OK(map.Map(vmo, kVmoSize, &addr));

  // Buffer placed such that 1 element is within mapped memory, and the next is in unmapped padding.
  void* buffer = reinterpret_cast<void*>(addr + kVmoSize - sizeof(ElementType));
  *reinterpret_cast<ElementType*>(buffer) = 0xAA;

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  // Advance head and tail to offset 3 by writing and reading 3 elements.
  ElementType elements[3] = {1, 2, 3};
  size_t actual_count = 0;
  ASSERT_OK(fifo_a.write(kElementSize, elements, 3, &actual_count));
  ASSERT_EQ(actual_count, 3u);
  ASSERT_OK(fifo_b.read(kElementSize, elements, 3, &actual_count));
  ASSERT_EQ(actual_count, 3u);

  // FIFO is empty, head=3, tail=3.
  // Writing 2 elements will write 1 element at index 3 (success) and wrap around to index 0,
  // where copying the 2nd element from buffer will fault.
  EXPECT_STATUS(fifo_a.write(kElementSize, buffer, 2, &actual_count), ZX_ERR_INVALID_ARGS);

  // Verify rollback: FIFO must still be empty, not readable.
  EXPECT_SIGNALS(fifo_b, ZX_FIFO_WRITABLE);
  ElementType read_elements[4] = {};
  EXPECT_STATUS(fifo_b.read(kElementSize, read_elements, 4, &actual_count), ZX_ERR_SHOULD_WAIT);

  // Verify that subsequent valid writes and reads work properly across the wrap.
  ElementType valid_elements[2] = {10, 20};
  ASSERT_OK(fifo_a.write(kElementSize, valid_elements, 2, &actual_count));
  ASSERT_EQ(actual_count, 2u);

  ASSERT_OK(fifo_b.read(kElementSize, read_elements, 2, &actual_count));
  ASSERT_EQ(actual_count, 2u);
  EXPECT_EQ(read_elements[0], 10u);
  EXPECT_EQ(read_elements[1], 20u);
}

TEST(FifoTest, ReadRollbackOnWrapAroundFault) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  VmoMapWithPadding map;
  zx_vaddr_t addr;
  ASSERT_OK(map.Map(vmo, kVmoSize, &addr));

  // Buffer placed such that 1 element is within mapped memory, and the next is in unmapped padding.
  void* buffer = reinterpret_cast<void*>(addr + kVmoSize - sizeof(ElementType));

  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(4, kElementSize, 0, &fifo_a, &fifo_b));

  // Advance head and tail to offset 3.
  ElementType elements[3] = {1, 2, 3};
  size_t actual_count = 0;
  ASSERT_OK(fifo_a.write(kElementSize, elements, 3, &actual_count));
  ASSERT_EQ(actual_count, 3u);
  ASSERT_OK(fifo_b.read(kElementSize, elements, 3, &actual_count));
  ASSERT_EQ(actual_count, 3u);

  // Write 2 elements spanning the wrap (element at index 3, and element at index 0).
  ElementType write_elements[2] = {0x11, 0x22};
  ASSERT_OK(fifo_a.write(kElementSize, write_elements, 2, &actual_count));
  ASSERT_EQ(actual_count, 2u);

  // Reading 2 elements across the wrap into partial bad buffer will copy 1st element (index 3)
  // and fault copying 2nd element (index 0).
  EXPECT_STATUS(fifo_b.read(kElementSize, buffer, 2, &actual_count), ZX_ERR_INVALID_ARGS);

  // Verify rollback: FIFO must still contain both elements.
  ElementType read_elements[2] = {};
  ASSERT_OK(fifo_b.read(kElementSize, read_elements, 2, &actual_count));
  ASSERT_EQ(actual_count, 2u);
  EXPECT_EQ(read_elements[0], 0x11u);
  EXPECT_EQ(read_elements[1], 0x22u);
}

TEST(FifoTest, InvalidHandleAndWrongType) {
  ElementType element = 1;
  size_t actual_count = 0;

  // Invalid handle
  EXPECT_STATUS(zx_fifo_write(ZX_HANDLE_INVALID, kElementSize, &element, 1, &actual_count),
                ZX_ERR_BAD_HANDLE);
  EXPECT_STATUS(zx_fifo_read(ZX_HANDLE_INVALID, kElementSize, &element, 1, &actual_count),
                ZX_ERR_BAD_HANDLE);

  // Wrong object type (event instead of fifo)
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_STATUS(zx_fifo_write(event.get(), kElementSize, &element, 1, &actual_count),
                ZX_ERR_WRONG_TYPE);
  EXPECT_STATUS(zx_fifo_read(event.get(), kElementSize, &element, 1, &actual_count),
                ZX_ERR_WRONG_TYPE);
}

TEST(FifoTest, ReadWriteBadActualCount) {
  zx::fifo fifo_a, fifo_b;
  ASSERT_OK(zx::fifo::create(8, kElementSize, 0, &fifo_a, &fifo_b));

  ElementType element = 1234;
  auto bad_actual_ptr = reinterpret_cast<size_t*>(1);

  EXPECT_STATUS(zx_fifo_write(fifo_a.get(), kElementSize, &element, 1, bad_actual_ptr),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(zx_fifo_read(fifo_b.get(), kElementSize, &element, 1, bad_actual_ptr),
                ZX_ERR_INVALID_ARGS);
}

}  // namespace
