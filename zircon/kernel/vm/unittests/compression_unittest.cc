// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/page/size.h>

#include <cstddef>

#include <ktl/variant.h>
#include <vm/lz4_compressor.h>
#include <vm/slot_page_storage.h>

#include "test_helper.h"

#include <ktl/enforce.h>

namespace vm_unittest {

namespace {

bool lz4_compress_smoke_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);

  fbl::AllocChecker ac;
  fbl::Array<char> src = fbl::MakeArray<char>(&ac, kPageSize);
  ASSERT_TRUE(ac.check());
  fbl::Array<char> compressed = fbl::MakeArray<char>(&ac, kPageSize);
  ASSERT_TRUE(ac.check());
  fbl::Array<char> uncompressed = fbl::MakeArray<char>(&ac, kPageSize);
  ASSERT_TRUE(ac.check());

  // Cannot generate random data, as it might not compress, so generate something that would
  // definitely be RLE compressible.
  for (size_t i = 0; i < kPageSize; i++) {
    src[i] = static_cast<char>((i / 128) & 0xff);
  }

  VmCompressionStrategy::CompressResult result =
      lz4->Compress(src.get(), compressed.get(), kPageSize);
  EXPECT_TRUE(ktl::holds_alternative<size_t>(result));

  lz4->Decompress(compressed.get(), ktl::get<size_t>(result), uncompressed.get());
  EXPECT_EQ(0, memcmp(src.get(), uncompressed.get(), kPageSize));

  END_TEST;
}

bool lz4_zero_dedupe_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);

  fbl::AllocChecker ac;
  fbl::Array<char> zero = fbl::MakeArray<char>(&ac, kPageSize);
  ASSERT_TRUE(ac.check());
  memset(zero.get(), 0, kPageSize);
  fbl::Array<char> dst = fbl::MakeArray<char>(&ac, kPageSize);
  ASSERT_TRUE(ac.check());

  // Check the zero page is determined to be zero.
  EXPECT_TRUE(ktl::holds_alternative<VmCompressionStrategy::ZeroTag>(
      lz4->Compress(zero.get(), dst.get(), kPageSize)));

  // Restricting the output size somewhat significantly should not prevent zero detection.
  constexpr size_t max_size = (kPageSize == 0x4000) ? 128 : 64;
  static_assert(kPageSize == 0x1000 || kPageSize == 0x4000, "update test to handle 64k pages");
  EXPECT_TRUE(ktl::holds_alternative<VmCompressionStrategy::ZeroTag>(
      lz4->Compress(zero.get(), dst.get(), max_size)));

  // Setting a byte should prevent zero detection.
  zero[4] = 1;
  EXPECT_TRUE(ktl::holds_alternative<size_t>(lz4->Compress(zero.get(), dst.get(), kPageSize)));

  END_TEST;
}

void write_zeros(vm_page_t* page, size_t len) {
  DEBUG_ASSERT(page);
  DEBUG_ASSERT(len <= kPageSize);
  uint8_t* data = static_cast<uint8_t*>(paddr_to_physmap(page->paddr()));
  for (size_t i = 0; i < len; i++) {
    data[i] = 0;
  }
}

void write_pattern(vm_page_t* page, size_t len, uint64_t offset) {
  DEBUG_ASSERT(page);
  DEBUG_ASSERT(len <= kPageSize);
  uint8_t* data = static_cast<uint8_t*>(paddr_to_physmap(page->paddr()));
  for (size_t i = 0; i < len; i++) {
    data[i] = static_cast<uint8_t>((i + offset) % 256);
  }
}

bool validate_pattern(const void* data, size_t len, uint64_t offset) {
  for (size_t i = 0; i < len; i++) {
    if (static_cast<const uint8_t*>(data)[i] != static_cast<uint8_t>((i + offset) % 256)) {
      return false;
    }
  }
  return true;
}

bool validate_pattern_ref(VmCompressedStorage& storage, VmCompressedStorage::CompressedRef ref,
                          size_t expected_len, uint64_t offset) {
  auto [data, metadata, size] = storage.CompressedData(ref);
  if (size != expected_len) {
    return false;
  }
  return validate_pattern(data, size, offset);
}

VmCompressedStorage::CompressedRef store(VmCompressedStorage& storage, vm_page_t* page,
                                         size_t len) {
  auto [maybe_ref, return_page] = storage.Store(page, len);
  if (return_page) {
    pmm_free_page(return_page);
  }
  ASSERT(maybe_ref.has_value());
  return *maybe_ref;
}

VmCompressedStorage::CompressedRef store_pattern(VmCompressedStorage& storage, size_t len,
                                                 uint64_t offset) {
  vm_page_t* page = nullptr;

  zx_status_t status = pmm_alloc_page(0, &page);
  ASSERT(status == ZX_OK);

  write_pattern(page, len, offset);

  return store(storage, page, len);
}

bool slot_page_storage_size_rounding() {
  BEGIN_TEST;

  VmSlotPageStorage storage;

  constexpr size_t kNumItems = 5;
  constexpr size_t items[kNumItems] = {64, 128, 63, 1, 65};

  // Store items that are a perfectly multiple of the slot size, as well as +/- 1 byte, to ensure
  // that the correct number of slots is used and the remainder is calculated correctly.
  VmCompressedStorage::CompressedRef refs[kNumItems] = {
      store_pattern(storage, items[0], 0), store_pattern(storage, items[1], 1),
      store_pattern(storage, items[2], 2), store_pattern(storage, items[3], 3),
      store_pattern(storage, items[4], 4)};

  // So far should have used 7 slots to store our 5 items. Using the remaining 57 slots should not
  // cause us to need a second storage page.
  VmCompressedStorage::CompressedRef padding = store_pattern(storage, 57ul * 64, 5);
  EXPECT_EQ(storage.GetInternalMemoryUsage().data_bytes, static_cast<size_t>(kPageSize));

  for (size_t i = 0; i < kNumItems; i++) {
    EXPECT_TRUE(validate_pattern_ref(storage, refs[i], items[i], i));
  }

  ktl::ranges::for_each(refs, [&storage](auto& r) { storage.Free(r); });
  storage.Free(padding);

  END_TEST;
}

// Test that the high-level compression-deconmpression flow preserves page data and metadata.
bool compression_smoke_test() {
  BEGIN_TEST;

  constexpr uint32_t kCompressionThreshhold = static_cast<uint32_t>(kPageSize) * 70u / 100u;
  fbl::AllocChecker ac;
  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);
  fbl::RefPtr<VmSlotPageStorage> storage = fbl::MakeRefCountedChecked<VmSlotPageStorage>(&ac);
  ASSERT_TRUE(ac.check());
  fbl::RefPtr<VmCompression> compression = fbl::MakeRefCountedChecked<VmCompression>(
      &ac, ktl::move(storage), ktl::move(lz4), kCompressionThreshhold);
  ASSERT_TRUE(ac.check());

  constexpr uint32_t kMetadataStart = 0xdeadbeef;
  constexpr uint32_t kMetadataUpdate = 0xc0ffee;

  // Compress a page full of content and get its ref.
  VmCompressor::CompressedRef compressed_ref{0};
  {
    vm_page_t* page;
    zx_status_t status = pmm_alloc_page(0, &page);
    ASSERT_EQ(ZX_OK, status);
    write_pattern(page, kPageSize, 0);

    auto compressor_guard = compression->AcquireCompressor();
    VmCompressor& compressor = compressor_guard.get();

    compressor.Arm();
    auto temp_ref =
        compressor.Start(VmCompressor::PageAndMetadata{.page = page, .metadata = kMetadataStart});
    EXPECT_EQ(kMetadataStart, compression->GetMetadata(temp_ref));

    compressor.Compress();

    // Modifying metadata while before acquiring the compression result is legal - the final result
    // should reflect the modifications.
    compression->SetMetadata(temp_ref, kMetadataUpdate);
    EXPECT_EQ(kMetadataUpdate, compression->GetMetadata(temp_ref));

    auto compress_result = compressor.TakeCompressionResult();
    ASSERT_TRUE(ktl::holds_alternative<VmCompressor::CompressedRef>(compress_result));
    compressed_ref = ktl::get<VmCompressor::CompressedRef>(compress_result);
    EXPECT_EQ(kMetadataUpdate, compression->GetMetadata(temp_ref));
    EXPECT_EQ(kMetadataUpdate, compression->GetMetadata(compressed_ref));

    compressor.ReturnTempReference(temp_ref);
    compressor.Finalize();

    pmm_free_page(page);
  }

  // Decompress the previous page and check that the data and metadata were preserved.
  {
    vm_page_t* page;
    zx_status_t status = pmm_alloc_page(0, &page);
    ASSERT_EQ(ZX_OK, status);

    uint32_t page_metadata;
    void* page_data = paddr_to_physmap(page->paddr());
    compression->Decompress(compressed_ref, page_data, &page_metadata);
    EXPECT_TRUE(validate_pattern(page_data, kPageSize, 0));
    EXPECT_EQ(kMetadataUpdate, page_metadata);

    pmm_free_page(page);
  }

  END_TEST;
}

// Test that compression can detect zero pages as input and flag them appropriately.
bool compression_zero_test() {
  BEGIN_TEST;

  constexpr uint32_t kCompressionThreshhold = static_cast<uint32_t>(kPageSize) * 70u / 100u;
  fbl::AllocChecker ac;
  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);
  fbl::RefPtr<VmSlotPageStorage> storage = fbl::MakeRefCountedChecked<VmSlotPageStorage>(&ac);
  ASSERT_TRUE(ac.check());
  fbl::RefPtr<VmCompression> compression = fbl::MakeRefCountedChecked<VmCompression>(
      &ac, ktl::move(storage), ktl::move(lz4), kCompressionThreshhold);
  ASSERT_TRUE(ac.check());

  // Compress a page full of zeros and ensure the compression system detects this.
  {
    vm_page_t* page;
    zx_status_t status = pmm_alloc_page(0, &page);
    ASSERT_EQ(ZX_OK, status);
    write_zeros(page, kPageSize);

    auto compressor_guard = compression->AcquireCompressor();
    VmCompressor& compressor = compressor_guard.get();

    compressor.Arm();
    auto temp_ref = compressor.Start(VmCompressor::PageAndMetadata{.page = page, .metadata = 0});

    compressor.Compress();
    auto compress_result = compressor.TakeCompressionResult();
    EXPECT_TRUE(ktl::holds_alternative<VmCompressor::ZeroTag>(compress_result));

    compressor.ReturnTempReference(temp_ref);
    compressor.Finalize();

    pmm_free_page(page);
  }

  END_TEST;
}

// Test that compression correctly handles failure to compress a page.
bool compression_fail_test() {
  BEGIN_TEST;

  constexpr uint32_t kCompressionThreshhold = 0u;  // Guarantee failure to compress.
  fbl::AllocChecker ac;
  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);
  fbl::RefPtr<VmSlotPageStorage> storage = fbl::MakeRefCountedChecked<VmSlotPageStorage>(&ac);
  ASSERT_TRUE(ac.check());
  fbl::RefPtr<VmCompression> compression = fbl::MakeRefCountedChecked<VmCompression>(
      &ac, ktl::move(storage), ktl::move(lz4), kCompressionThreshhold);
  ASSERT_TRUE(ac.check());

  constexpr uint32_t kMetadataStart = 0xdeadbeef;
  constexpr uint32_t kMetadataUpdate = 0xc0ffee;

  // Compress a page full of content, which fails, and ensure the compression system handles this.
  {
    vm_page_t* page;
    zx_status_t status = pmm_alloc_page(0, &page);
    ASSERT_EQ(ZX_OK, status);
    write_pattern(page, kPageSize, 0);

    auto compressor_guard = compression->AcquireCompressor();
    VmCompressor& compressor = compressor_guard.get();

    compressor.Arm();
    auto temp_ref =
        compressor.Start(VmCompressor::PageAndMetadata{.page = page, .metadata = kMetadataStart});
    EXPECT_EQ(kMetadataStart, compression->GetMetadata(temp_ref));

    compressor.Compress();

    // Modifying metadata while before acquiring the compression result is legal - the final result
    // should reflect the modifications.
    compression->SetMetadata(temp_ref, kMetadataUpdate);
    EXPECT_EQ(kMetadataUpdate, compression->GetMetadata(temp_ref));

    auto compress_result = compressor.TakeCompressionResult();
    ASSERT_TRUE(ktl::holds_alternative<VmCompressor::FailTag>(compress_result));
    auto fail = ktl::get<VmCompressor::FailTag>(compress_result);
    EXPECT_EQ(page, fail.src_page.page);
    EXPECT_EQ(kMetadataUpdate, fail.src_page.metadata);

    compressor.ReturnTempReference(temp_ref);
    compressor.Finalize();

    pmm_free_page(page);
  }

  END_TEST;
}

// Test that compression correctly handles moving of temp references.
bool compression_move_reference_test() {
  BEGIN_TEST;

  constexpr uint32_t kCompressionThreshhold = static_cast<uint32_t>(kPageSize) * 70u / 100u;
  fbl::AllocChecker ac;
  fbl::RefPtr<VmLz4Compressor> lz4 = VmLz4Compressor::Create();
  ASSERT_TRUE(lz4);
  fbl::RefPtr<VmSlotPageStorage> storage = fbl::MakeRefCountedChecked<VmSlotPageStorage>(&ac);
  ASSERT_TRUE(ac.check());
  fbl::RefPtr<VmCompression> compression = fbl::MakeRefCountedChecked<VmCompression>(
      &ac, ktl::move(storage), ktl::move(lz4), kCompressionThreshhold);
  ASSERT_TRUE(ac.check());

  constexpr uint32_t kMetadataStart = 0xdeadbeef;
  constexpr uint32_t kMetadataUpdate = 0xc0ffee;

  // Compress a page full of content but move the ref while compression is in progress.
  {
    vm_page_t* page;
    zx_status_t status = pmm_alloc_page(0, &page);
    ASSERT_EQ(ZX_OK, status);
    write_pattern(page, kPageSize, 0);

    auto compressor_guard = compression->AcquireCompressor();
    VmCompressor& compressor = compressor_guard.get();

    compressor.Arm();
    auto temp_ref =
        compressor.Start(VmCompressor::PageAndMetadata{.page = page, .metadata = kMetadataStart});
    EXPECT_EQ(kMetadataStart, compression->GetMetadata(temp_ref));

    compressor.Compress();

    // Modifying metadata while before acquiring the compression result is legal - the final result
    // should reflect the modifications.
    compression->SetMetadata(temp_ref, kMetadataUpdate);
    EXPECT_EQ(kMetadataUpdate, compression->GetMetadata(temp_ref));

    // Move the temp reference before acquiring the compression result, which will invalidate it.
    auto maybe_page = compression->MoveReference(temp_ref);
    ASSERT_TRUE(maybe_page.has_value());
    EXPECT_EQ(kMetadataUpdate, maybe_page->metadata);
    EXPECT_TRUE(!compressor.IsTempReferenceInUse());

    // The compression result can still be obtained, but the metadata will be empty.
    auto compress_result = compressor.TakeCompressionResult();
    ASSERT_TRUE(ktl::holds_alternative<VmCompressor::CompressedRef>(compress_result));
    auto compressed_ref = ktl::get<VmCompressor::CompressedRef>(compress_result);
    EXPECT_EQ(0u, compression->GetMetadata(compressed_ref));

    compressor.Free(compressed_ref);
    compressor.Finalize();

    pmm_free_page(maybe_page->page);
    pmm_free_page(page);
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(compression_tests)
VM_UNITTEST(lz4_compress_smoke_test)
VM_UNITTEST(lz4_zero_dedupe_test)
VM_UNITTEST(slot_page_storage_size_rounding)
VM_UNITTEST(compression_smoke_test)
VM_UNITTEST(compression_zero_test)
VM_UNITTEST(compression_fail_test)
VM_UNITTEST(compression_move_reference_test)
UNITTEST_END_TESTCASE(compression_tests, "compression", "Compression tests")

}  // namespace vm_unittest
