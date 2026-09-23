// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/zx/event.h>
#include <lib/zx/vmo.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>

#include <vector>

#include <fbl/ref_ptr.h>
#include <zxtest/zxtest.h>

namespace {

constexpr size_t kVmoTestSize = 512 << 10;  // 512KB

const size_t kPageSize = zx_system_get_page_size();
const uint32_t kPageCount = kVmoTestSize / kPageSize;

TEST(FakeBti, CreateFakeBti) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());

  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, PinVmo) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  // Create an address array with one extra entry and mark it with a sentinel value.
  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);
  addrs[kPageCount] = 42;

  // Now actually pin the region
  EXPECT_OK(
      zx_bti_pin(bti.get(), 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Check that the addresses returned are correct, including that the sentinel value wasn't
  // touched.
  for (size_t i = 0; i != kPageCount; ++i) {
    EXPECT_EQ(addrs[i], FAKE_BTI_PHYS_ADDR);
  }
  EXPECT_EQ(addrs[kPageCount], 42);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, GetPinnedVmos) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);

  // Now actually pin the region
  EXPECT_OK(
      zx_bti_pin(bti.get(), 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Get VMO handle
  auto vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_EQ(1u, vmo_result->size());

  std::vector<fake_bti::FakeBtiPinnedVmoInfo> pinned_vmo_info = std::move(vmo_result.value());

  uint64_t size1 = 0u, size2 = 0u;
  zx_vmo_get_size(pinned_vmo_info[0].vmo.get(), &size1);
  zx_vmo_get_size(vmo_handle, &size2);
  EXPECT_NE(size1, 0u);
  EXPECT_NE(size2, 0u);
  EXPECT_EQ(size1, size2);

  EXPECT_EQ(pinned_vmo_info[0].size, size1);
  EXPECT_EQ(pinned_vmo_info[0].offset, 0u);

  // Close the returned VMO handle.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo.get()), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));

  vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_TRUE(vmo_result->empty());

  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, GetPinnedVmosWithOffset) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);

  // Now actually pin the region (with offset)
  EXPECT_OK(zx_bti_pin(bti.get(), 0, vmo_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs.data(), kPageCount - 1u, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Get VMO handle
  auto vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_EQ(1u, vmo_result->size());
  std::vector<fake_bti::FakeBtiPinnedVmoInfo> pinned_vmo_info = std::move(vmo_result.value());

  uint64_t size = 0u;
  zx_vmo_get_size(pinned_vmo_info[0].vmo.get(), &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].size, kVmoTestSize - kPageSize);
  EXPECT_EQ(pinned_vmo_info[0].offset, kPageSize);

  // Try writing to the duplicated VMO and read it back from pinned VMO.
  zx_vmo_op_range(pinned_vmo_info[0].vmo.get(), ZX_VMO_OP_COMMIT, /*offset=*/kPageSize,
                  /*size=*/kPageSize, nullptr, 0);

  uint8_t val = 42;
  uint8_t read_val = 0u;

  EXPECT_EQ(zx_vmo_write(pinned_vmo_info[0].vmo.get(), &val, /*offset=*/kPageSize, sizeof(val)),
            ZX_OK);
  EXPECT_EQ(zx_vmo_read(vmo_handle, &read_val, /*offset=*/kPageSize, sizeof(read_val)), ZX_OK);
  EXPECT_EQ(val, read_val);

  // Close the returned VMO handle.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo.get()), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_TRUE(vmo_result->empty());
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, GetMultiplePinnedVmos) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  zx_handle_t vmo2_handle, pmt2_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo2_handle));

  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);

  // Pin the first VMO.
  EXPECT_OK(
      zx_bti_pin(bti.get(), 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Pin the second VMO region with a non-zero offset.
  EXPECT_OK(zx_bti_pin(bti.get(), 0, vmo2_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs.data(), kPageCount - 1u, &pmt2_handle));
  EXPECT_NE(pmt2_handle, ZX_HANDLE_INVALID);

  // Get VMO handles
  auto vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_EQ(2u, vmo_result->size());
  std::vector<fake_bti::FakeBtiPinnedVmoInfo> pinned_vmo_info = std::move(vmo_result.value());

  uint64_t size;
  zx_vmo_get_size(pinned_vmo_info[0].vmo.get(), &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].offset, 0);

  zx_vmo_get_size(pinned_vmo_info[1].vmo.get(), &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[1].size, kVmoTestSize - kPageSize);
  EXPECT_EQ(pinned_vmo_info[1].offset, kPageSize);

  // Close the returned VMO handles.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo.get()), ZX_OK);
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[1].vmo.get()), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_EQ(1u, vmo_result->size());

  ASSERT_NO_DEATH(([pmt2_handle]() { EXPECT_OK(zx_pmt_unpin(pmt2_handle)); }));
  vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_TRUE(vmo_result->empty());

  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, PinVmoWithPaddrGenerator) {
  std::vector<zx_paddr_t> expected_addrs(kPageCount + 1, 0);
  for (size_t i = 0; i < expected_addrs.size(); i++) {
    expected_addrs[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
  }

  zx::result result = fake_bti::CreateFakeBtiWithPaddrs(expected_addrs);
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  // Create an address array with one extra entry and mark it with a sentinel value.
  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);
  addrs[kPageCount] = 42;

  // Now actually pin the region
  EXPECT_OK(
      zx_bti_pin(bti.get(), 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Check that the addresses returned are correct, including that the sentinel value wasn't
  // touched.
  for (size_t i = 0; i != kPageCount; ++i) {
    EXPECT_EQ(addrs[i], FAKE_BTI_PHYS_ADDR * (i + 1));
  }
  EXPECT_EQ(addrs[kPageCount], 42);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, GetPhysFromPinnedVmo) {
  std::vector<zx_paddr_t> expected_addrs(2 * kPageCount + 1, 0);
  for (size_t i = 0; i < expected_addrs.size(); i++) {
    expected_addrs[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
  }

  zx::result result = fake_bti::CreateFakeBtiWithPaddrs(expected_addrs);
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  zx_handle_t vmo2_handle, pmt2_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo2_handle));

  std::vector<zx_paddr_t> addrs(kPageCount, 0);
  std::vector<zx_paddr_t> addrs2(kPageCount - 1u, 0);

  // Pin the first VMO.
  EXPECT_OK(
      zx_bti_pin(bti.get(), 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Pin the second VMO region with a non-zero offset.
  EXPECT_OK(zx_bti_pin(bti.get(), 0, vmo2_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs2.data(), kPageCount - 1u, &pmt2_handle));
  EXPECT_NE(pmt2_handle, ZX_HANDLE_INVALID);

  // Get VMO handles
  auto vmo_result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  EXPECT_TRUE(vmo_result.is_ok());
  EXPECT_EQ(2u, vmo_result->size());
  std::vector<fake_bti::FakeBtiPinnedVmoInfo> pinned_vmo_info = std::move(vmo_result.value());

  zx::result phys_result = GetVmoPhysAddress(zx::unowned_bti(bti), pinned_vmo_info[0]);
  EXPECT_TRUE(phys_result.is_ok());
  std::vector<zx_paddr_t> paddrs = std::move(phys_result.value());
  EXPECT_EQ(std::memcmp(addrs.data(), paddrs.data(), addrs.size()), 0);

  phys_result = GetVmoPhysAddress(zx::unowned_bti(bti), pinned_vmo_info[1]);
  EXPECT_TRUE(phys_result.is_ok());
  paddrs = std::move(phys_result.value());
  EXPECT_EQ(std::memcmp(addrs2.data(), paddrs.data(), addrs2.size()), 0);

  // Close the returned VMO handles.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo.get()), ZX_OK);
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[1].vmo.get()), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([pmt2_handle]() { EXPECT_OK(zx_pmt_unpin(pmt2_handle)); }));
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, CreateContiguousVmo) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create_contiguous(bti.get(), kVmoTestSize, 0, &vmo_handle));
  EXPECT_NE(vmo_handle, ZX_HANDLE_INVALID);

  zx_paddr_t addr;
  EXPECT_OK(
      zx_bti_pin(bti.get(), ZX_BTI_CONTIGUOUS, vmo_handle, 0, kVmoTestSize, &addr, 1, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(addr, FAKE_BTI_PHYS_ADDR);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([&bti]() { zx_handle_close(bti.get()); }));
}

TEST(FakeBti, PmoCount) {
  zx::result result = fake_bti::CreateFakeBti();
  ASSERT_TRUE(result.is_ok());
  zx::bti bti = std::move(result.value());
  EXPECT_NE(bti.get(), ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create_contiguous(bti.get(), kVmoTestSize, 0, &vmo_handle));
  EXPECT_NE(vmo_handle, ZX_HANDLE_INVALID);

  zx_paddr_t addr;
  EXPECT_OK(
      zx_bti_pin(bti.get(), ZX_BTI_CONTIGUOUS, vmo_handle, 0, kVmoTestSize, &addr, 1, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(addr, FAKE_BTI_PHYS_ADDR);

  size_t actual = 0, avail = 0;
  zx_info_bti_t bti_info;
  EXPECT_OK(
      zx_object_get_info(bti.get(), ZX_INFO_BTI, &bti_info, sizeof(bti_info), &actual, &avail));

  // After pinning, pmo_count should be 1.
  EXPECT_EQ(1, bti_info.pmo_count);

  EXPECT_OK(zx_pmt_unpin(pmt_handle));

  // After unpinning, pmo_count should be zero.
  EXPECT_OK(
      zx_object_get_info(bti.get(), ZX_INFO_BTI, &bti_info, sizeof(bti_info), &actual, &avail));
  EXPECT_EQ(0, bti_info.pmo_count);
}

// TODO(https://fxbug.dev/42108106): when functionality is available, check that pinning a
// vmo with the ZX_BTI_CONTIGUOUS flag fails if the vmo was not created with
// zx_vmo_create_contiguous.

}  // namespace
