// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fake-bti/bti.h>
#include <lib/zx/event.h>
#include <lib/zx/vmo.h>
#include <zircon/rights.h>
#include <zircon/syscalls.h>

#include <vector>

#include <fbl/ref_ptr.h>
#include <zxtest/zxtest.h>

namespace {

constexpr size_t kVmoTestSize = 512 << 10;  // 512KB
const uint32_t kPageSize = zx_system_get_page_size();
const uint32_t kPageCount = kVmoTestSize / kPageSize;

TEST(FakeBti, CreateFakeBti) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);
  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, PinVmo) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  // Create an address array with one extra entry and mark it with a sentinel value.
  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);
  addrs[kPageCount] = 42;

  // Now actually pin the region
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Check that the addresses returned are correct, including that the sentinel value wasn't
  // touched.
  for (size_t i = 0; i != kPageCount; ++i) {
    EXPECT_EQ(addrs[i], FAKE_BTI_PHYS_ADDR);
  }
  EXPECT_EQ(addrs[kPageCount], 42);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, GetPinnedVmos) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  std::vector<zx_paddr_t> addrs(kPageCount, 0);

  // Now actually pin the region
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Get VMO handle
  uint64_t num_pinned_vmos = 0u;
  std::vector<fake_bti_pinned_vmo_info_t> pinned_vmo_info;

  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 1u);
  pinned_vmo_info.resize(num_pinned_vmos);

  EXPECT_OK(
      fake_bti_get_pinned_vmos(bti, pinned_vmo_info.data(), num_pinned_vmos, &num_pinned_vmos));

  uint64_t size1 = 0u, size2 = 0u;
  zx_vmo_get_size(pinned_vmo_info[0].vmo, &size1);
  zx_vmo_get_size(vmo_handle, &size2);
  EXPECT_NE(size1, 0u);
  EXPECT_NE(size2, 0u);
  EXPECT_EQ(size1, size2);

  EXPECT_EQ(pinned_vmo_info[0].size, size1);
  EXPECT_EQ(pinned_vmo_info[0].offset, 0u);

  // Close the returned VMO handle.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 0u);

  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, GetPinnedVmosWithOffset) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  std::vector<zx_paddr_t> addrs(kPageCount, 0);

  // Now actually pin the region (with offset)
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs.data(), kPageCount - 1u, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Get VMO handle
  uint64_t num_pinned_vmos = 0u;
  std::vector<fake_bti_pinned_vmo_info_t> pinned_vmo_info;

  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 1u);
  pinned_vmo_info.resize(num_pinned_vmos);

  EXPECT_OK(fake_bti_get_pinned_vmos(bti, pinned_vmo_info.data(), num_pinned_vmos, nullptr));

  uint64_t size = 0u;
  zx_vmo_get_size(pinned_vmo_info[0].vmo, &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].size, kVmoTestSize - kPageSize);
  EXPECT_EQ(pinned_vmo_info[0].offset, kPageSize);

  // Try writing to the duplicated VMO and read it back from pinned VMO.
  zx_vmo_op_range(pinned_vmo_info[0].vmo, ZX_VMO_OP_COMMIT, /*offset=*/kPageSize,
                  /*size=*/kPageSize, nullptr, 0);

  uint8_t val = 42;
  uint8_t read_val = 0u;

  EXPECT_EQ(zx_vmo_write(pinned_vmo_info[0].vmo, &val, /*offset=*/kPageSize, sizeof(val)), ZX_OK);
  EXPECT_EQ(zx_vmo_read(vmo_handle, &read_val, /*offset=*/kPageSize, sizeof(read_val)), ZX_OK);
  EXPECT_EQ(val, read_val);

  // Close the returned VMO handle.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 0u);

  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, GetMultiplePinnedVmos) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  zx_handle_t vmo2_handle, pmt2_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo2_handle));

  std::vector<zx_paddr_t> addrs(kPageCount, 0);

  // Pin the first VMO.
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Pin the second VMO region with a non-zero offset.
  EXPECT_OK(zx_bti_pin(bti, 0, vmo2_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs.data(), kPageCount - 1u, &pmt2_handle));
  EXPECT_NE(pmt2_handle, ZX_HANDLE_INVALID);

  // Get VMO handles
  uint64_t num_pinned_vmos = 0u;
  std::vector<fake_bti_pinned_vmo_info_t> pinned_vmo_info;

  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 2u);
  pinned_vmo_info.resize(num_pinned_vmos);
  EXPECT_OK(fake_bti_get_pinned_vmos(bti, pinned_vmo_info.data(), num_pinned_vmos, nullptr));

  uint64_t size;
  zx_vmo_get_size(pinned_vmo_info[0].vmo, &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[0].offset, 0);

  zx_vmo_get_size(pinned_vmo_info[1].vmo, &size);
  EXPECT_EQ(size, kVmoTestSize);
  EXPECT_EQ(pinned_vmo_info[1].size, kVmoTestSize - kPageSize);
  EXPECT_EQ(pinned_vmo_info[1].offset, kPageSize);

  // Close the returned VMO handles.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[1].vmo), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 1u);

  ASSERT_NO_DEATH(([pmt2_handle]() { EXPECT_OK(zx_pmt_unpin(pmt2_handle)); }));
  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 0u);

  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, PinVmoWithPaddrGenerator) {
  std::vector<zx_paddr_t> expected_addrs(kPageCount + 1, 0);
  for (size_t i = 0; i < expected_addrs.size(); i++) {
    expected_addrs[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
  }

  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create_with_paddrs(expected_addrs.data(), expected_addrs.size(), &bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));

  // Create an address array with one extra entry and mark it with a sentinel value.
  std::vector<zx_paddr_t> addrs(kPageCount + 1, 0);
  addrs[kPageCount] = 42;

  // Now actually pin the region
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Check that the addresses returned are correct, including that the sentinel value wasn't
  // touched.
  for (size_t i = 0; i != kPageCount; ++i) {
    EXPECT_EQ(addrs[i], FAKE_BTI_PHYS_ADDR * (i + 1));
  }
  EXPECT_EQ(addrs[kPageCount], 42);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, GetPhysFromPinnedVmo) {
  std::vector<zx_paddr_t> expected_addrs(2 * kPageCount + 1, 0);
  for (size_t i = 0; i < expected_addrs.size(); i++) {
    expected_addrs[i] = FAKE_BTI_PHYS_ADDR * (i + 1);
  }

  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create_with_paddrs(expected_addrs.data(), expected_addrs.size(), &bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  zx_handle_t vmo2_handle, pmt2_handle;
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo_handle));
  EXPECT_OK(zx_vmo_create(kVmoTestSize, 0, &vmo2_handle));

  std::vector<zx_paddr_t> addrs(kPageCount, 0);
  std::vector<zx_paddr_t> addrs2(kPageCount - 1u, 0);

  // Pin the first VMO.
  EXPECT_OK(zx_bti_pin(bti, 0, vmo_handle, 0, kVmoTestSize, addrs.data(), kPageCount, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);

  // Pin the second VMO region with a non-zero offset.
  EXPECT_OK(zx_bti_pin(bti, 0, vmo2_handle, /*offset=*/kPageSize, kVmoTestSize - kPageSize,
                       addrs2.data(), kPageCount - 1u, &pmt2_handle));
  EXPECT_NE(pmt2_handle, ZX_HANDLE_INVALID);

  // Get VMO handles
  uint64_t num_pinned_vmos = 0u;
  std::vector<fake_bti_pinned_vmo_info_t> pinned_vmo_info;

  EXPECT_OK(fake_bti_get_pinned_vmos(bti, nullptr, 0u, &num_pinned_vmos));
  EXPECT_EQ(num_pinned_vmos, 2u);
  pinned_vmo_info.resize(num_pinned_vmos);
  EXPECT_OK(
      fake_bti_get_pinned_vmos(bti, pinned_vmo_info.data(), num_pinned_vmos, &num_pinned_vmos));

  uint64_t num_paddrs;
  std::vector<zx_paddr_t> paddrs;
  EXPECT_OK(fake_bti_get_phys_from_pinned_vmo(bti, pinned_vmo_info[0], nullptr, 0, &num_paddrs));
  EXPECT_EQ(num_paddrs, kPageCount);
  paddrs.resize(num_paddrs);
  EXPECT_OK(fake_bti_get_phys_from_pinned_vmo(bti, pinned_vmo_info[0], paddrs.data(), num_paddrs,
                                              &num_paddrs));
  EXPECT_EQ(std::memcmp(addrs.data(), paddrs.data(), sizeof(addrs)), 0);

  EXPECT_OK(fake_bti_get_phys_from_pinned_vmo(bti, pinned_vmo_info[1], nullptr, 0, &num_paddrs));
  EXPECT_EQ(num_paddrs, kPageCount - 1u);
  paddrs.resize(num_paddrs);
  EXPECT_OK(fake_bti_get_phys_from_pinned_vmo(bti, pinned_vmo_info[1], paddrs.data(), num_paddrs,
                                              &num_paddrs));
  EXPECT_EQ(std::memcmp(addrs2.data(), paddrs.data(), sizeof(addrs2)), 0);

  // Close the returned VMO handles.
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[0].vmo), ZX_OK);
  EXPECT_EQ(zx_handle_close(pinned_vmo_info[1].vmo), ZX_OK);

  // Unpin all the PMT handles.
  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([pmt2_handle]() { EXPECT_OK(zx_pmt_unpin(pmt2_handle)); }));
  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, CreateContiguousVmo) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create_contiguous(bti, kVmoTestSize, 0, &vmo_handle));
  EXPECT_NE(vmo_handle, ZX_HANDLE_INVALID);

  zx_paddr_t addr;
  EXPECT_OK(zx_bti_pin(bti, ZX_BTI_CONTIGUOUS, vmo_handle, 0, kVmoTestSize, &addr, 1, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(addr, FAKE_BTI_PHYS_ADDR);

  ASSERT_NO_DEATH(([pmt_handle]() { EXPECT_OK(zx_pmt_unpin(pmt_handle)); }));
  ASSERT_NO_DEATH(([bti]() { zx_handle_close(bti); }));
}

TEST(FakeBti, PmoCount) {
  zx_handle_t bti = ZX_HANDLE_INVALID;
  EXPECT_OK(fake_bti_create(&bti));
  EXPECT_NE(bti, ZX_HANDLE_INVALID);

  zx_handle_t vmo_handle, pmt_handle;
  EXPECT_OK(zx_vmo_create_contiguous(bti, kVmoTestSize, 0, &vmo_handle));
  EXPECT_NE(vmo_handle, ZX_HANDLE_INVALID);

  zx_paddr_t addr;
  EXPECT_OK(zx_bti_pin(bti, ZX_BTI_CONTIGUOUS, vmo_handle, 0, kVmoTestSize, &addr, 1, &pmt_handle));
  EXPECT_NE(pmt_handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(addr, FAKE_BTI_PHYS_ADDR);

  size_t actual = 0, avail = 0;
  zx_info_bti_t bti_info;
  EXPECT_OK(zx_object_get_info(bti, ZX_INFO_BTI, &bti_info, sizeof(bti_info), &actual, &avail));

  // After pinning, pmo_count should be 1.
  EXPECT_EQ(1, bti_info.pmo_count);

  EXPECT_OK(zx_pmt_unpin(pmt_handle));

  // After unpinning, pmo_count should be zero.
  EXPECT_OK(zx_object_get_info(bti, ZX_INFO_BTI, &bti_info, sizeof(bti_info), &actual, &avail));
  EXPECT_EQ(0, bti_info.pmo_count);
}

// TODO(https://fxbug.dev/42108106): when functionality is available, check that pinning a
// vmo with the ZX_BTI_CONTIGUOUS flag fails if the vmo was not created with
// zx_vmo_create_contiguous.

}  // namespace
