// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/dma-buffer/buffer.h>
#include <lib/driver/mmio/cpp/mmio.h>

#include <fake-dma-buffer/fake-dma-buffer.h>
#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

namespace ddk_fake_test {

const zx::bti kFakeBti(42);
constexpr auto kAlignmentLog2 = 12;

TEST(FakeDmaBuffer, ContiguousBufferMultiPage) {
  auto factory = ddk_fake::CreateBufferFactory();
  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer;
  ASSERT_EQ(factory->CreateContiguous(kFakeBti, zx_system_get_page_size() * 2, 0,
                                      dma_buffer::CacheOptions::kEnabled, &buffer),
            ZX_ERR_NOT_SUPPORTED);
}

TEST(FakeDmaBuffer, ContiguousBuffer) {
  auto factory = ddk_fake::CreateBufferFactory();
  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer;
  ASSERT_OK(factory->CreateContiguous(kFakeBti, zx_system_get_page_size(), kAlignmentLog2,
                                      dma_buffer::CacheOptions::kEnabled, &buffer));
  ASSERT_EQ(ddk_fake::PhysToVirt(buffer->phys()), buffer->virt());
  auto& page = ddk_fake::GetPage(buffer->phys());
  ASSERT_EQ(page.alignment_log2, kAlignmentLog2);
  ASSERT_EQ(page.bti, kFakeBti.get());
  ASSERT_TRUE(page.contiguous);
  ASSERT_TRUE(page.enable_cache);
  ASSERT_EQ(page.size, zx_system_get_page_size());
}

TEST(FakeDmaBuffer, UncachedContiguousBuffer) {
  auto factory = ddk_fake::CreateBufferFactory();
  std::unique_ptr<dma_buffer::ContiguousBuffer> buffer;
  ASSERT_OK(factory->CreateContiguous(kFakeBti, zx_system_get_page_size(), kAlignmentLog2,
                                      dma_buffer::CacheOptions::kDisabled, &buffer));
  ASSERT_EQ(ddk_fake::PhysToVirt(buffer->phys()), buffer->virt());
  auto& page = ddk_fake::GetPage(buffer->phys());
  ASSERT_EQ(page.alignment_log2, kAlignmentLog2);
  ASSERT_EQ(page.bti, kFakeBti.get());
  ASSERT_TRUE(page.contiguous);
  ASSERT_FALSE(page.enable_cache);
  ASSERT_EQ(page.size, zx_system_get_page_size());
}

TEST(FakeDmaBuffer, UncachedPagedBuffer) {
  auto factory = ddk_fake::CreateBufferFactory();
  std::unique_ptr<dma_buffer::PagedBuffer> buffer;
  constexpr auto kPageCount = 4;
  ASSERT_OK(factory->CreatePaged(kFakeBti, zx_system_get_page_size() * kPageCount,
                                 dma_buffer::CacheOptions::kDisabled, &buffer));
  for (size_t i = 0; i < kPageCount; i++) {
    ASSERT_EQ(ddk_fake::PhysToVirt(buffer->phys()[i]),
              static_cast<uint8_t*>(buffer->virt()) + (i * zx_system_get_page_size()));
    auto& page = ddk_fake::GetPage(buffer->phys()[i]);
    ASSERT_EQ(page.bti, kFakeBti.get());
    ASSERT_FALSE(page.contiguous);
    ASSERT_FALSE(page.enable_cache);
    ASSERT_EQ(page.size, zx_system_get_page_size() * kPageCount);
  }
}

TEST(FakeDmaBuffer, CachedPagedBuffer) {
  auto factory = ddk_fake::CreateBufferFactory();
  std::unique_ptr<dma_buffer::PagedBuffer> buffer;
  constexpr auto kPageCount = 4;
  ASSERT_OK(factory->CreatePaged(kFakeBti, zx_system_get_page_size() * kPageCount,
                                 dma_buffer::CacheOptions::kEnabled, &buffer));
  for (size_t i = 0; i < kPageCount; i++) {
    ASSERT_EQ(ddk_fake::PhysToVirt(buffer->phys()[i]),
              static_cast<uint8_t*>(buffer->virt()) + (i * zx_system_get_page_size()));
    auto& page = ddk_fake::GetPage(buffer->phys()[i]);
    ASSERT_EQ(page.bti, kFakeBti.get());
    ASSERT_FALSE(page.contiguous);
    ASSERT_TRUE(page.enable_cache);
    ASSERT_EQ(page.size, zx_system_get_page_size() * kPageCount);
  }
}

}  // namespace ddk_fake_test
