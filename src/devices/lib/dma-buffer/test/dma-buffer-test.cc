// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/dma-buffer/buffer.h>
#include <lib/dma-buffer/phys-iter.h>
#include <lib/fake-object/object.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <map>
#include <memory>
#include <mutex>

#include <gtest/gtest.h>

namespace {

const zx::bti kFakeBti(42);

struct VmoMetadata {
  size_t size = 0;
  uint32_t alignment_log2 = 0;
  zx_handle_t bti_handle = ZX_HANDLE_INVALID;
  uint32_t cache_policy = 0;
  zx_paddr_t start_phys = 0;
  void* virt = nullptr;
  bool contiguous = false;
};

bool unpinned = false;
uint32_t last_cache_flush_options = 0;
size_t last_cache_flush_len = 0;
int cache_flush_call_count = 0;

class VmoWrapper : public fake_object::Object {
 public:
  explicit VmoWrapper(zx::vmo vmo) : fake_object::Object(ZX_OBJ_TYPE_VMO), vmo_(std::move(vmo)) {}
  zx::unowned_vmo vmo() { return vmo_.borrow(); }
  VmoMetadata& metadata() { return metadata_; }

 private:
  zx::vmo vmo_;
  VmoMetadata metadata_ = {};
};

extern "C" {
zx_status_t zx_vmo_create_contiguous(zx_handle_t bti_handle, size_t size, uint32_t alignment_log2,
                                     zx_handle_t* out) {
  zx::vmo vmo = {};
  zx_status_t status = _zx_vmo_create(size, 0, vmo.reset_and_get_address());
  if (status != ZX_OK) {
    return status;
  }

  auto vmo_wrapper = std::make_shared<VmoWrapper>(std::move(vmo));
  vmo_wrapper->metadata().alignment_log2 = alignment_log2;
  vmo_wrapper->metadata().bti_handle = bti_handle;
  vmo_wrapper->metadata().size = size;
  zx::result add_res = fake_object::FakeHandleTable().Add(std::move(vmo_wrapper));
  if (add_res.is_ok()) {
    *out = add_res.value();
  }
  return add_res.status_value();
}

zx_status_t zx_vmo_create(uint64_t size, uint32_t options, zx_handle_t* out) {
  zx::vmo vmo = {};
  zx_status_t status = _zx_vmo_create(size, options, vmo.reset_and_get_address());
  if (status != ZX_OK) {
    return status;
  }

  auto vmo_wrapper = std::make_shared<VmoWrapper>(std::move(vmo));
  vmo_wrapper->metadata().size = size;
  zx::result add_res = fake_object::FakeHandleTable().Add(std::move(vmo_wrapper));
  if (add_res.is_ok()) {
    *out = add_res.value();
  }
  return add_res.status_value();
}

zx_status_t zx_vmar_map(zx_handle_t vmar_handle, zx_vm_option_t options, uint64_t vmar_offset,
                        zx_handle_t vmo_handle, uint64_t vmo_offset, uint64_t len,
                        zx_vaddr_t* mapped_addr) {
  zx::result get_res = fake_object::FakeHandleTable().Get(vmo_handle);
  if (!get_res.is_ok()) {
    return get_res.status_value();
  }
  std::shared_ptr<VmoWrapper> vmo = std::static_pointer_cast<VmoWrapper>(get_res.value());

  zx_status_t status = _zx_vmar_map(vmar_handle, options, vmar_offset, vmo->vmo()->get(),
                                    vmo_offset, len, mapped_addr);
  if (status == ZX_OK) {
    vmo->metadata().virt = reinterpret_cast<void*>(*mapped_addr);
  }
  return status;
}

zx_status_t zx_vmo_set_cache_policy(zx_handle_t handle, uint32_t cache_policy) {
  zx::result get_res = fake_object::FakeHandleTable().Get(handle);
  if (!get_res.is_ok()) {
    return get_res.status_value();
  }
  std::shared_ptr<VmoWrapper> vmo = std::static_pointer_cast<VmoWrapper>(get_res.value());
  vmo->metadata().cache_policy = cache_policy;
  return ZX_OK;
}

zx_status_t zx_bti_pin(zx_handle_t bti_handle, uint32_t options, zx_handle_t vmo_handle,
                       uint64_t offset, uint64_t size, zx_paddr_t* addrs, size_t addrs_count,
                       zx_handle_t* out) {
  static uint64_t current_phys = 0;
  static std::mutex phys_lock;

  if (bti_handle != kFakeBti.get()) {
    return ZX_ERR_BAD_HANDLE;
  }

  if (options & ZX_BTI_CONTIGUOUS) {
    if (addrs_count != 1) {
      return ZX_ERR_INVALID_ARGS;
    }
  } else {
    const auto num_pages =
        fbl::round_up(size, zx_system_get_page_size()) / zx_system_get_page_size();
    if (addrs_count != num_pages) {
      return ZX_ERR_INVALID_ARGS;
    }
  }

  zx::result get_res = fake_object::FakeHandleTable().Get(vmo_handle);
  if (!get_res.is_ok()) {
    return get_res.status_value();
  }
  std::shared_ptr<VmoWrapper> vmo = std::static_pointer_cast<VmoWrapper>(get_res.value());

  std::lock_guard lock(phys_lock);
  vmo->metadata().start_phys = current_phys;
  *addrs = current_phys;
  current_phys += vmo->metadata().size;
  *out = ZX_HANDLE_INVALID;
  return ZX_OK;
}

zx_status_t zx_pmt_unpin(zx_handle_t handle) {
  if (handle == ZX_HANDLE_INVALID) {
    unpinned = true;
  }
  return ZX_OK;
}

zx_status_t zx_cache_flush(const void* addr, size_t len, uint32_t options) {
  last_cache_flush_options = options;
  last_cache_flush_len = len;
  cache_flush_call_count++;
  return ZX_OK;
}

}  // extern "C"

}  // namespace

namespace dma_buffer {
TEST(DmaBufferTests, InitWithCacheEnabled) {
  unpinned = false;
  {
    std::unique_ptr<ContiguousBuffer> buffer;
    const size_t size = zx_system_get_page_size() * 4;
    const size_t alignment = 2;
    auto factory = CreateBufferFactory();
    ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, alignment, CacheOptions::kEnabled,
                                               &buffer));
    auto test_f = [&buffer, size](fake_object::Object* obj) -> bool {
      auto vmo = static_cast<VmoWrapper*>(obj);
      ZX_ASSERT(vmo->metadata().alignment_log2 == alignment);
      ZX_ASSERT(vmo->metadata().bti_handle == kFakeBti.get());
      ZX_ASSERT(vmo->metadata().cache_policy == 0);
      ZX_ASSERT(vmo->metadata().size == size);
      ZX_ASSERT(buffer->virt() == vmo->metadata().virt);
      ZX_ASSERT(buffer->size() == vmo->metadata().size);
      ZX_ASSERT(buffer->phys() == vmo->metadata().start_phys);
      return false;
    };
    fake_object::FakeHandleTable().ForEach(ZX_OBJ_TYPE_VMO, test_f);
  }
  ASSERT_TRUE(unpinned);
}

TEST(DmaBufferTests, InitContiguousWithCacheDisabled) {
  unpinned = false;
  {
    std::unique_ptr<ContiguousBuffer> buffer;
    const size_t size = zx_system_get_page_size() * 4;
    const size_t alignment = 2;
    auto factory = CreateBufferFactory();
    ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, alignment, CacheOptions::kDisabled,
                                               &buffer));
    auto test_f = [&buffer, size](fake_object::Object* obj) -> bool {
      auto vmo = static_cast<VmoWrapper*>(obj);
      ZX_ASSERT(vmo->metadata().alignment_log2 == alignment);
      ZX_ASSERT(vmo->metadata().bti_handle == kFakeBti.get());
      ZX_ASSERT(vmo->metadata().cache_policy == ZX_CACHE_POLICY_UNCACHED_DEVICE);
      ZX_ASSERT(vmo->metadata().size == size);
      ZX_ASSERT(buffer->virt() == vmo->metadata().virt);
      ZX_ASSERT(buffer->size() == vmo->metadata().size);
      ZX_ASSERT(buffer->phys() == vmo->metadata().start_phys);
      return false;
    };
    fake_object::FakeHandleTable().ForEach(ZX_OBJ_TYPE_VMO, test_f);
  }
  ASSERT_TRUE(unpinned);
}

TEST(DmaBufferTests, InitWithCacheDisabled) {
  unpinned = false;
  {
    std::unique_ptr<PagedBuffer> buffer;
    auto factory = CreateBufferFactory();
    ASSERT_EQ(ZX_OK, factory->CreatePaged(kFakeBti, zx_system_get_page_size(),
                                          CacheOptions::kDisabled, &buffer));
    auto test_f = [&buffer](fake_object::Object* object) -> bool {
      auto vmo = static_cast<VmoWrapper*>(object);
      ZX_ASSERT(vmo->metadata().alignment_log2 == 0);
      ZX_ASSERT(vmo->metadata().cache_policy == ZX_CACHE_POLICY_UNCACHED_DEVICE);
      ZX_ASSERT(vmo->metadata().size == zx_system_get_page_size());
      ZX_ASSERT(buffer->virt() == vmo->metadata().virt);
      ZX_ASSERT(buffer->size() == vmo->metadata().size);
      ZX_ASSERT(buffer->phys()[0] == vmo->metadata().start_phys);
      return false;
    };
    fake_object::FakeHandleTable().ForEach(ZX_OBJ_TYPE_VMO, test_f);
  }
  ASSERT_TRUE(unpinned);
}

TEST(DmaBufferTests, InitCachedMultiPageBuffer) {
  unpinned = false;
  {
    std::unique_ptr<ContiguousBuffer> buffer;
    auto factory = CreateBufferFactory();
    ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, zx_system_get_page_size() * 4, 0,
                                               CacheOptions::kEnabled, &buffer));
    auto test_f = [&buffer](fake_object::Object* object) -> bool {
      auto vmo = static_cast<VmoWrapper*>(object);
      ZX_ASSERT(vmo->metadata().alignment_log2 == 0);
      ZX_ASSERT(vmo->metadata().cache_policy == 0);
      ZX_ASSERT(vmo->metadata().bti_handle == kFakeBti.get());
      ZX_ASSERT(vmo->metadata().size == zx_system_get_page_size() * 4);
      ZX_ASSERT(buffer->virt() == vmo->metadata().virt);
      ZX_ASSERT(buffer->size() == vmo->metadata().size);
      ZX_ASSERT(buffer->phys() == vmo->metadata().start_phys);
      return false;
    };
    fake_object::FakeHandleTable().ForEach(ZX_OBJ_TYPE_VMO, test_f);
  }
  ASSERT_TRUE(unpinned);
}

TEST(DmaBufferTests, InitUncachedMultiPageBuffer) {
  unpinned = false;
  {
    std::unique_ptr<ContiguousBuffer> buffer;
    auto factory = CreateBufferFactory();
    ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, zx_system_get_page_size() * 4, 0,
                                               CacheOptions::kDisabled, &buffer));
    auto test_f = [&buffer](fake_object::Object* object) -> bool {
      auto vmo = static_cast<VmoWrapper*>(object);
      ZX_ASSERT(vmo->metadata().alignment_log2 == 0);
      ZX_ASSERT(vmo->metadata().cache_policy == ZX_CACHE_POLICY_UNCACHED_DEVICE);
      ZX_ASSERT(vmo->metadata().bti_handle == kFakeBti.get());
      ZX_ASSERT(vmo->metadata().size == zx_system_get_page_size() * 4);
      ZX_ASSERT(buffer->virt() == vmo->metadata().virt);
      ZX_ASSERT(buffer->size() == vmo->metadata().size);
      ZX_ASSERT(buffer->phys() == vmo->metadata().start_phys);
      return false;
    };
    fake_object::FakeHandleTable().ForEach(ZX_OBJ_TYPE_VMO, test_f);
  }
  ASSERT_TRUE(unpinned);
}

using Param = struct {
  // The description here will get rendered in the test name, along with a stringified variant of
  // the testing input. In total, it can be used to identify each individual test case in the
  // suite.
  const char* test_desc;

  // PhysIter ctor inputs.
  zx_paddr_t* chunk_list;
  uint64_t chunk_count;
  size_t chunk_size;
  zx_off_t vmo_offset;
  size_t buf_length;
  size_t max_length;

  // Expected outputs.
  uint loop_ct;  // Number of times to increment iterator for full buffer.
  zx_paddr_t* want_addr;
  size_t* want_size;
};

const size_t kPageSize{4096};  // Page size, for maximum brevity.
const size_t kHalfPage{2048};  // Half a page.

// clang-format off
const auto kCases = testing::Values(
    Param(/* test_desc   */ "SimplePageBoundary",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 2 * kPageSize},
          /* chunk_count */ 2,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 2 * kPageSize,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 2,
          /* want_addr   */ (zx_paddr_t[]){kPageSize, 2 * kPageSize},
          /* want_size   */ (size_t[]){kPageSize, kPageSize}),

    Param(/* test_desc   */ "NonContiguousPages",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 3 * kPageSize, 5 * kPageSize},
          /* chunk_count */ 3,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 3 * kPageSize,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){kPageSize, 3 * kPageSize, 5 * kPageSize},
          /* want_size   */ (size_t[]){kPageSize, kPageSize, kPageSize}),

    Param(/* test_desc   */ "PartialFirstPageContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 2 * kPageSize, 3 * kPageSize},
          /* chunk_count */ 3,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ kPageSize - 5,
          /* buf_length  */ 2 * kPageSize + 5,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){2 * kPageSize - 5, 2 * kPageSize, 3 * kPageSize},
          /* want_size   */ (size_t[]){5UL, kPageSize, kPageSize}),

    Param(/* test_desc   */ "PartialFirstPageNonContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 3 * kPageSize, 5 * kPageSize},
          /* chunk_count */ 3,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ kPageSize - 5,
          /* buf_length  */ 2 * kPageSize + 5,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){2 * kPageSize - 5, 3 * kPageSize, 5 * kPageSize},
          /* want_size   */ (size_t[]){5UL, kPageSize, kPageSize}),

    Param(/* test_desc   */ "PartialLastPageContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 2 * kPageSize, 3 * kPageSize},
          /* chunk_count */ 3,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 2 * kPageSize + 5,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){kPageSize, 2 * kPageSize, 3 * kPageSize},
          /* want_size   */ (size_t[]){kPageSize, kPageSize, 5UL}),

    Param(/* test_desc   */ "PartialLastPageNonContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 3 * kPageSize, 5 * kPageSize},
          /* chunk_count */ 3,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 2 * kPageSize + 5,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){kPageSize, 3 * kPageSize, 5 * kPageSize},
          /* want_size   */ (size_t[]){kPageSize, kPageSize, 5UL}),

    Param(/* test_desc   */ "SubChunkMaxLength",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize, 2 * kPageSize},
          /* chunk_count */ 2,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 2 * kPageSize,
          /* max_length  */ kHalfPage,
          /* loop_ct     */ 4,
          /* want_addr   */ (zx_paddr_t[]){2 * kHalfPage, 3 * kHalfPage, 4 * kHalfPage, 5 * kHalfPage},
          /* want_size   */ (size_t[]){kHalfPage, kHalfPage, kHalfPage, kHalfPage}),

    Param(/* test_desc   */ "ZxBtiContiguousLargeBuffer",
          /* chunk_list  */ (zx_paddr_t[]){kPageSize},
          /* chunk_count */ 1,
          /* chunk_size  */ kPageSize,
          /* vmo_offset  */ 0,
          /* buf_length  */ 1UL << 20, // 1MiB.
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 1,
          /* want_addr   */ (zx_paddr_t[]){kPageSize},
          /* want_size   */ (size_t[]){1UL << 20}),

    Param(/* test_desc   */ "ZxBtiCompressContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kHalfPage, 2 * kHalfPage, 3 * kHalfPage},
          /* chunk_count */ 3,
          /* chunk_size  */ kHalfPage,
          /* vmo_offset  */ 0,
          /* buf_length  */ 3 * kHalfPage,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){kHalfPage, 2 * kHalfPage, 3 * kHalfPage},
          /* want_size   */ (size_t[]){kHalfPage, kHalfPage, kHalfPage}),

    Param(/* test_desc   */ "ZxBtiCompressNonContiguous",
          /* chunk_list  */ (zx_paddr_t[]){kHalfPage, 3 * kHalfPage, 5 * kHalfPage},
          /* chunk_count */ 3,
          /* chunk_size  */ kHalfPage,
          /* vmo_offset  */ 0,
          /* buf_length  */ 3 * kHalfPage,
          /* max_length  */ UINT64_MAX,
          /* loop_ct     */ 3,
          /* want_addr   */ (zx_paddr_t[]){kHalfPage, 3 * kHalfPage, 5 * kHalfPage},
          /* want_size   */ (size_t[]){kHalfPage, kHalfPage, kHalfPage}));
// clang-format on

class Parameterized : public testing::TestWithParam<Param> {};

TEST_P(Parameterized, TestRun) {
  auto p = GetParam();

  PhysIter phys_iter{p.chunk_list, p.chunk_count, p.chunk_size,
                     p.vmo_offset, p.buf_length,  p.max_length};
  auto itr = phys_iter.begin();

  uint i;
  for (i = 0; i < p.loop_ct; i++) {
    EXPECT_NE(itr, phys_iter.end()) << "i=" << i << std::endl;
    auto [addr, size] = *(itr++);
    EXPECT_EQ(p.want_addr[i], addr) << "i=" << i << std::endl;
    EXPECT_EQ(p.want_size[i], size) << "i=" << i << std::endl;
  }

  EXPECT_EQ(itr, phys_iter.end()) << "i=" << i << std::endl;
}

INSTANTIATE_TEST_SUITE_P(PhysIterTest, Parameterized, kCases,
                         [](const testing::TestParamInfo<Parameterized::ParamType>& info) {
                           std::stringstream test_name;
                           test_name << info.index << "_" << info.param.test_desc;
                           return test_name.str();
                         });

TEST(DmaBufferTests, ReadWriteAndCacheOperations) {
  std::unique_ptr<ContiguousBuffer> buffer;
  const size_t size = zx_system_get_page_size();
  auto factory = CreateBufferFactory();
  ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, 0, CacheOptions::kEnabled, &buffer));

  // Reset cache flush counter.
  cache_flush_call_count = 0;

  // Test Write and Read raw memory.
  const uint8_t write_data[] = {0xDE, 0xAD, 0xBE, 0xEF};
  uint8_t read_data[4] = {0};
  EXPECT_TRUE(buffer->Write(write_data, 16, sizeof(write_data)).is_ok());
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(ZX_CACHE_FLUSH_DATA, last_cache_flush_options);

  cache_flush_call_count = 0;
  EXPECT_TRUE(buffer->Read(16, sizeof(read_data), read_data).is_ok());
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(static_cast<uint32_t>(ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE),
            last_cache_flush_options);
  EXPECT_EQ(0, std::memcmp(write_data, read_data, sizeof(write_data)));

  // Test WriteStruct and ReadStruct.
  struct TestPacket {
    uint32_t magic;
    uint16_t length;
    uint8_t flags;
  };
  TestPacket out_pkt{.magic = 0x12345678, .length = 64, .flags = 0xFF};
  EXPECT_TRUE(buffer->WriteStruct(out_pkt, 64).is_ok());

  zx::result<TestPacket> in_pkt_res = buffer->ReadStruct<TestPacket>(64);
  ASSERT_TRUE(in_pkt_res.is_ok());
  EXPECT_EQ(out_pkt.magic, in_pkt_res->magic);
  EXPECT_EQ(out_pkt.length, in_pkt_res->length);
  EXPECT_EQ(out_pkt.flags, in_pkt_res->flags);

  // Test Out-Of-Bounds handling.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            buffer->Write(write_data, size - 2, sizeof(write_data)).error_value());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE,
            buffer->Read(size - 2, sizeof(read_data), read_data).error_value());
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, buffer->Write(nullptr, 0, 4).error_value());

  // Test Cache operations directly.
  EXPECT_TRUE(buffer->enable_cache());
  cache_flush_call_count = 0;
  EXPECT_TRUE(buffer->CacheFlush(0, 0).is_ok());
  EXPECT_EQ(0, cache_flush_call_count);
  EXPECT_TRUE(buffer->CacheFlush(0, 32).is_ok());
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(ZX_CACHE_FLUSH_DATA, last_cache_flush_options);

  cache_flush_call_count = 0;
  EXPECT_TRUE(buffer->CacheFlushInvalidate(0, 0).is_ok());
  EXPECT_EQ(0, cache_flush_call_count);
  EXPECT_TRUE(buffer->CacheFlushInvalidate(0, 32).is_ok());
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(static_cast<uint32_t>(ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE),
            last_cache_flush_options);

  // Test that cache flush operations are NOT called when enable_cache is false.
  std::unique_ptr<ContiguousBuffer> uncached_buffer;
  ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, 0, CacheOptions::kDisabled,
                                             &uncached_buffer));
  EXPECT_FALSE(uncached_buffer->enable_cache());

  cache_flush_call_count = 0;
  EXPECT_TRUE(uncached_buffer->Write(write_data, 16, sizeof(write_data)).is_ok());
  EXPECT_TRUE(uncached_buffer->Read(16, sizeof(read_data), read_data).is_ok());
  EXPECT_TRUE(uncached_buffer->CacheFlush(0, 32).is_ok());
  EXPECT_TRUE(uncached_buffer->CacheFlushInvalidate(0, 32).is_ok());
  EXPECT_EQ(0, cache_flush_call_count);
}

TEST(DmaBufferTests, UncachedMemoryHelperAllCases) {
  std::unique_ptr<ContiguousBuffer> buffer;
  const size_t size = zx_system_get_page_size();
  auto factory = CreateBufferFactory();
  ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, 0, CacheOptions::kDisabled, &buffer));
  EXPECT_FALSE(buffer->enable_cache());

  // 1. Zero length transfer.
  uint8_t test_byte = 0x55;
  EXPECT_TRUE(buffer->Write(&test_byte, 0, 0).is_ok());
  EXPECT_TRUE(buffer->Read(0, 0, &test_byte).is_ok());
  EXPECT_EQ(0x55, test_byte);

  // 2. Setup pattern integrity verification.
  std::vector<uint8_t> bg_pattern(size, 0xAA);
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  auto verify_slice = [&](size_t offset, size_t length, uint8_t val) {
    std::vector<uint8_t> read_back(size, 0);
    EXPECT_TRUE(buffer->Read(0, size, read_back.data()).is_ok());
    for (size_t i = 0; i < size; i++) {
      if (i >= offset && i < offset + length) {
        EXPECT_EQ(val, read_back[i]) << "Mismatch at modified index " << i;
      } else {
        EXPECT_EQ(0xAA, read_back[i]) << "Corruption at background index " << i;
      }
    }
  };

  // 3. Small unaligned transfer (< WordType size, e.g., 3 bytes at offset 1).
  std::vector<uint8_t> small_data(3, 0x11);
  EXPECT_TRUE(buffer->Write(small_data.data(), 1, 3).is_ok());
  verify_slice(1, 3, 0x11);
  // Restore background
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  // 4. Unaligned head alignment (offset 3, length 24 - crossing word boundaries).
  std::vector<uint8_t> head_data(24, 0x22);
  EXPECT_TRUE(buffer->Write(head_data.data(), 3, 24).is_ok());
  verify_slice(3, 24, 0x22);
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  // 5. Unaligned tail alignment (offset 8, length 13 - starting aligned, ending unaligned).
  std::vector<uint8_t> tail_data(13, 0x33);
  EXPECT_TRUE(buffer->Write(tail_data.data(), 8, 13).is_ok());
  verify_slice(8, 13, 0x33);
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  // 6. Both unaligned head and unaligned tail (offset 5, length 19).
  std::vector<uint8_t> mid_data(19, 0x44);
  EXPECT_TRUE(buffer->Write(mid_data.data(), 5, 19).is_ok());
  verify_slice(5, 19, 0x44);
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  // 7. Exact word boundary transfer (offset 16, length 32).
  std::vector<uint8_t> exact_data(32, 0x66);
  EXPECT_TRUE(buffer->Write(exact_data.data(), 16, 32).is_ok());
  verify_slice(16, 32, 0x66);
  EXPECT_TRUE(buffer->Write(bg_pattern.data(), 0, size).is_ok());

  // 8. Large transfer spanning multiple words across the entire buffer.
  std::vector<uint8_t> large_data(size);
  for (size_t i = 0; i < size; i++) {
    large_data[i] = static_cast<uint8_t>((i * 7) & 0xFF);
  }
  EXPECT_TRUE(buffer->Write(large_data.data(), 0, size).is_ok());
  std::vector<uint8_t> large_read(size, 0);
  EXPECT_TRUE(buffer->Read(0, size, large_read.data()).is_ok());
  EXPECT_EQ(0, std::memcmp(large_data.data(), large_read.data(), size));
}

TEST(DmaBufferTests, ExecuteOpsLambdaTests) {
  std::unique_ptr<ContiguousBuffer> buffer;
  const size_t size = zx_system_get_page_size();
  auto factory = CreateBufferFactory();
  ASSERT_EQ(ZX_OK, factory->CreateContiguous(kFakeBti, size, 0, CacheOptions::kEnabled, &buffer));

  struct TestStruct {
    uint32_t a;
    uint32_t b;
  };

  // Test ExecuteWriteOps & ExecuteReadOps
  cache_flush_call_count = 0;
  zx::result<> status = buffer->ExecuteWriteOps(16, 64, [](void* ptr) {
    std::memset(ptr, 0xAB, 64);
  });
  EXPECT_TRUE(status.is_ok());
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(static_cast<uint32_t>(ZX_CACHE_FLUSH_DATA), last_cache_flush_options);

  cache_flush_call_count = 0;
  bool read_verified = false;
  status = buffer->ExecuteReadOps(16, 64, [&read_verified](const void* ptr) {
    const uint8_t* bytes = static_cast<const uint8_t*>(ptr);
    read_verified = (bytes[0] == 0xAB && bytes[63] == 0xAB);
  });
  EXPECT_TRUE(status.is_ok());
  EXPECT_TRUE(read_verified);
  EXPECT_EQ(1, cache_flush_call_count);
  EXPECT_EQ(static_cast<uint32_t>(ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE), last_cache_flush_options);

  // Error case: Out of bounds
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, buffer->ExecuteWriteOps(size - 10, 20, [](void*) {}).status_value());
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, buffer->ExecuteReadOps(size - 10, 20, [](const void*) {}).status_value());
}

}  // namespace dma_buffer
