// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/iob/blob-id-allocator.h>
#include <lib/stdcompat/functional.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <memory>
#include <thread>

#include <gtest/gtest.h>

namespace {

struct Header {
  uint32_t next_id;
  uint32_t blob_head;
};

TEST(IobBlobIdAllocatorTests, InvalidHeader) {
  constexpr std::array<std::byte, 10> kBlob{std::byte{'a'}};

  alignas(8) std::array<std::byte, 100> buffer;
  iob::BlobIdAllocator allocator(buffer);
  allocator.Init();

  Header* header = reinterpret_cast<Header*>(buffer.data());

  // The blob head now extends past the buffer, so the header is invalid.
  header->blob_head = 101;
  {
    auto allocated = allocator.Allocate(kBlob);
    ASSERT_TRUE(allocated.is_error());
    EXPECT_EQ(iob::BlobIdAllocator::AllocateError::kInvalidHeader, allocated.error_value());
  }

  // The entry end now extends past the blob head, so the header is invalid.
  header->blob_head = 90;
  header->next_id = 11;  // Entry end at 96.
  {
    auto allocated = allocator.Allocate(kBlob);
    ASSERT_TRUE(allocated.is_error());
    EXPECT_EQ(iob::BlobIdAllocator::AllocateError::kInvalidHeader, allocated.error_value());
  }

  // Now reset to a valid state and perform the allocation so that we can
  // corrupt things in the 'get' path.
  header->blob_head = 100;
  header->next_id = 0;
  {
    auto allocated = allocator.Allocate(kBlob);
    ASSERT_TRUE(allocated.is_ok());
    EXPECT_EQ(0u, allocated.value());

    iob::BlobIdAllocator::IterableView view = allocator.iterable();
    EXPECT_NE(view.end(), view.begin());
    EXPECT_TRUE(view.take_error().is_ok());
  }

  // The blob head now extends past the buffer, so the header is invalid.
  header->blob_head = 101;
  {
    auto blob = allocator.GetBlob(0u);
    ASSERT_TRUE(blob.is_error());
    EXPECT_EQ(iob::BlobIdAllocator::BlobError::kInvalidHeader, blob.error_value());

    iob::BlobIdAllocator::IterableView view = allocator.iterable();
    EXPECT_EQ(view.end(), view.begin());
    auto error = view.take_error();
    EXPECT_EQ(iob::BlobIdAllocator::BlobError::kInvalidHeader, error.error_value());
  }

  // The entry end now extends past the blob head, so the header is invalid.
  header->blob_head = 90;
  header->next_id = 11;  // Entry end at 96.
  {
    auto blob = allocator.GetBlob(0u);
    ASSERT_TRUE(blob.is_error());
    EXPECT_EQ(iob::BlobIdAllocator::BlobError::kInvalidHeader, blob.error_value());

    iob::BlobIdAllocator::IterableView view = allocator.iterable();
    EXPECT_EQ(view.end(), view.begin());
    auto error = view.take_error();
    EXPECT_EQ(iob::BlobIdAllocator::BlobError::kInvalidHeader, error.error_value());
  }
}

TEST(IobBlobIdAllocatorTests, SingleThreaded) {
  constexpr std::array<std::byte, 51> kBlobA{std::byte{'a'}};
  constexpr std::array<std::byte, 17> kBlobB{std::byte{'b'}};
  constexpr std::array<std::byte, 1> kBlobC{std::byte{'c'}};

  alignas(8) std::array<std::byte, 100> buffer;
  iob::BlobIdAllocator allocator(buffer);
  allocator.Init();

  {
    auto remaining = allocator.RemainingBytes();
    ASSERT_TRUE(remaining.is_ok());
    EXPECT_EQ(92u, remaining.value());  // 100 - sizeof(header)
  }

  // Allocate an ID for kBlobA.
  {
    auto allocated = allocator.Allocate(kBlobA);
    ASSERT_TRUE(allocated.is_ok());
    EXPECT_EQ(0u, allocated.value());
  }
  {
    auto remaining = allocator.RemainingBytes();
    ASSERT_TRUE(remaining.is_ok());
    EXPECT_EQ(33u, remaining.value());  // prev remaining - sizeof(entry) - sizeof(blob)
  }
  {
    auto result = allocator.GetBlob(0u);
    ASSERT_TRUE(result.is_ok());
    cpp20::span<const std::byte> blob = result.value();
    ASSERT_EQ(kBlobA.size(), blob.size());
    EXPECT_EQ(0, memcmp(blob.data(), kBlobA.data(), kBlobA.size()));
  }

  // Allocate an ID for kBlobB.
  {
    auto allocated = allocator.Allocate(kBlobB);
    ASSERT_TRUE(allocated.is_ok());
    EXPECT_EQ(1u, allocated.value());
  }
  {
    auto remaining = allocator.RemainingBytes();
    ASSERT_TRUE(remaining.is_ok());
    EXPECT_EQ(8u, remaining.value());  // prev remaining - sizeof(entry) - sizeof(blob)
  }
  {
    auto result = allocator.GetBlob(1u);
    ASSERT_TRUE(result.is_ok());
    cpp20::span<const std::byte> blob = result.value();
    ASSERT_EQ(kBlobB.size(), blob.size());
    EXPECT_EQ(0, memcmp(blob.data(), kBlobB.data(), kBlobB.size()));
  }

  // Try (and fail) to allocate an ID for kBlobC.
  {
    auto allocated = allocator.Allocate(kBlobC);
    ASSERT_TRUE(allocated.is_error());
    EXPECT_EQ(iob::BlobIdAllocator::AllocateError::kOutOfMemory, allocated.error_value());
  }

  iob::BlobIdAllocator::IterableView view = allocator.iterable();
  unsigned count = 0;
  for (auto [id, blob] : view) {
    EXPECT_EQ(count, id);
    switch (count++) {
      case 0u:
        ASSERT_EQ(kBlobA.size(), blob.size());
        EXPECT_EQ(0, memcmp(blob.data(), kBlobA.data(), kBlobA.size()));
        break;
      case 1u:
        ASSERT_EQ(kBlobB.size(), blob.size());
        EXPECT_EQ(0, memcmp(blob.data(), kBlobB.data(), kBlobB.size()));
        break;
      default:
        EXPECT_TRUE(false);
        break;
    }
  }
  EXPECT_EQ(2u, count);
  EXPECT_TRUE(view.take_error().is_ok());
}

TEST(IobBlobIdAllocatorTests, MultiThreaded) {
  alignas(8) std::array<std::byte, 8 + 100 * 8 + 100 * 1> allocator_storage;
  iob::BlobIdAllocator allocator(allocator_storage);
  allocator.Init();

  std::array<uint32_t, 100> ids = {0xaabbccdd};
  std::array<std::byte, 100> blob_storage;

  // A simple routine that allocates an ID from the size-1 blob comprised of
  // `blob_storage[i] = i`.
  auto allocate_byte = [&allocator, &blob_storage, &ids](uint8_t i) {
    blob_storage[i] = std::byte{i};
    auto result = allocator.Allocate({&blob_storage[i], 1});
    ASSERT_TRUE(result.is_ok());
    ids[i] = result.value();
  };

  std::array<std::unique_ptr<std::thread>, 100> threads;
  for (uint8_t i = 0; i < 100; ++i) {
    threads[i] = std::make_unique<std::thread>(cpp20::bind_front(allocate_byte, i));
  }
  for (uint8_t i = 0; i < 100; ++i) {
    threads[i]->join();
  }

  // All IDs should now be populated from 0 to 99 in some nondetermistic order.
  std::sort(ids.begin(), ids.end());
  for (uint32_t i = 0; i < ids.size(); ++i) {
    EXPECT_EQ(i, ids[i]);
  }

  // Similarly, all recorded blobs should be of size 1 and have values ranging
  // 0 to 99 in some nondeterministic order.
  iob::BlobIdAllocator::IterableView view = allocator.iterable();
  std::array<std::byte, 100> blob_values;
  size_t count = 0;
  for (auto [id, blob] : view) {
    ASSERT_EQ(1u, blob.size());
    blob_values[count++] = blob[0];
  }
  EXPECT_EQ(100u, count);
  EXPECT_TRUE(view.take_error().is_ok());

  std::sort(blob_values.begin(), blob_values.end());
  for (uint8_t i = 0; i < 100; ++i) {
    EXPECT_EQ(std::byte{i}, blob_values[i]);
  }
}

TEST(IobBlobIdAllocatorTests, GetBlob) {
  static constexpr std::array<std::byte, 10> kBlobA{std::byte{'a'}};
  static constexpr std::array<std::byte, 20> kBlobB{std::byte{'b'}};
  static constexpr std::array<std::byte, 30> kBlobC{std::byte{'c'}};
  static constexpr std::array<cpp20::span<const std::byte>, 3> kBlobs{kBlobA, kBlobB, kBlobC};

  alignas(8) std::array<std::byte, 100> buffer;
  iob::BlobIdAllocator allocator(buffer);
  allocator.Init();

  // Allocate IDs for the blobs.
  for (auto blob : kBlobs) {
    auto allocated = allocator.Allocate(blob);
    ASSERT_TRUE(allocated.is_ok());
  }

  iob::BlobIdAllocator::IterableView view = allocator.iterable();
  unsigned count = 0;
  for (auto [id, expected] : view) {
    EXPECT_EQ(count, id);
    auto result = allocator.GetBlob(count++);
    ASSERT_TRUE(result.is_ok());
    auto actual = result.value();
    ASSERT_EQ(expected.size(), actual.size());
    EXPECT_EQ(0, memcmp(expected.data(), actual.data(), actual.size()));
  }
  EXPECT_EQ(3u, count);
  EXPECT_TRUE(view.take_error().is_ok());
}

TEST(IobBlobIdAllocatorTests, Find) {
  static constexpr std::array<std::byte, 10> kBlobA{std::byte{'a'}};
  static constexpr std::array<std::byte, 20> kBlobB{std::byte{'b'}};
  static constexpr std::array<std::byte, 30> kBlobC{std::byte{'c'}};
  static constexpr std::array<cpp20::span<const std::byte>, 3> kBlobs{kBlobA, kBlobB, kBlobC};

  alignas(8) std::array<std::byte, 100> buffer;
  iob::BlobIdAllocator allocator(buffer);
  allocator.Init();

  // Allocate IDs for the blobs.
  for (auto blob : kBlobs) {
    auto allocated = allocator.Allocate(blob);
    ASSERT_TRUE(allocated.is_ok());
  }

  auto view = allocator.iterable();
  for (uint32_t id = 0; id < 3; ++id) {
    auto it = view.find(id);
    ASSERT_NE(it, view.end());
    EXPECT_EQ(id, it->id);
  }
  EXPECT_EQ(view.end(), view.find(3));
  EXPECT_TRUE(view.take_error().is_ok());
}

}  // namespace

// Regression test: Verifies that a header with next_id that causes integer
// overflow in index_end() is correctly rejected, preventing OOB memory access.
TEST(BlobIdAllocatorTest, OverflowingNextIdIsInvalid) {
  constexpr size_t kRegionSize = 4096;
  alignas(8) std::byte bytes[kRegionSize] = {};

  iob::BlobIdAllocator allocator({bytes, kRegionSize});
  allocator.Init();

  // Corrupt next_id to the overflow-triggering value.
  // next_id = 0x20000000 causes: (uint32_t)(8 + 0x20000000*8) = 8 (wraps!)
  auto* raw = reinterpret_cast<uint32_t*>(bytes);
  cpp20::atomic_ref<uint32_t> ref{*raw};
  ref.store(0x20000000u, std::memory_order_relaxed);

  // All public API calls must safely fail with kInvalidHeader.
  EXPECT_TRUE(allocator.RemainingBytes().is_error());

  std::byte blob_data[8] = {};
  auto result = allocator.Allocate({blob_data, sizeof(blob_data)});
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(result.error_value(), iob::BlobIdAllocator::AllocateError::kInvalidHeader);

  auto blob_result = allocator.GetBlob(0);
  ASSERT_TRUE(blob_result.is_error());
  EXPECT_EQ(blob_result.error_value(), iob::BlobIdAllocator::BlobError::kInvalidHeader);
}
