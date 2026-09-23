// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iommu.h>

#include <ranges>

#include <zxtest/zxtest.h>

#include "../needs-next.h"
#include "../vmo/helpers.h"
#include "test_thread.h"
#include "userpager.h"

NEEDS_NEXT_SYSCALL(zx_pager_query_dirty_ranges);

namespace pager_tests {
namespace {

class ThreadedPagePinner {
 public:
  static zx::result<std::unique_ptr<ThreadedPagePinner>> Create(
      const zx::resource& iommu_resource) {
    zx::iommu iommu;
    zx::bti bti;

    zx_iommu_desc_stub_t desc;
    if (const zx_status_t status =
            zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu);
        status != ZX_OK) {
      return zx::error(status);
    }

    if (const zx_status_t status = zx::bti::create(iommu, 0, 0xdeadbeef, &bti); status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok(std::unique_ptr<ThreadedPagePinner>{
        new ThreadedPagePinner(std::move(iommu), std::move(bti))});
  }

  bool StartPinOperation(zx::unowned_vmo vmo_to_pin, uint64_t amt,
                         uint32_t perms = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE,
                         uint64_t offset = 0) {
    if (thread_state_ != ThreadState::kInitial) {
      return false;
    }

    if (!*vmo_to_pin || !amt) {
      return false;
    }

    vmo_to_pin_ = vmo_to_pin;
    perms_ = perms;
    offset_ = offset;
    amt_ = amt;

    thread_state_ = thread_.Start() ? ThreadState::kStarted : ThreadState::kFailedToStart;
    return (thread_state_ == ThreadState::kStarted);
  }

  bool WaitForPin() {
    if (thread_state_ != ThreadState::kStarted) {
      return false;
    }

    if (thread_.Wait()) {
      thread_state_ = ThreadState::kCompleted;
      return true;
    }

    return false;
  }

  bool WaitForPinFailure() {
    if (thread_state_ != ThreadState::kStarted) {
      return false;
    }

    if (thread_.WaitForFailure()) {
      thread_state_ = ThreadState::kCompleted;
      return true;
    }

    return false;
  }

  bool ExplicitUnpin() {
    if (thread_state_ != ThreadState::kCompleted) {
      return false;
    }

    if (pmt_) {
      pmt_.unpin();
      pmt_.reset();
    }

    return true;
  }

 private:
  friend std::unique_ptr<ThreadedPagePinner>::deleter_type;

  enum class ThreadState {
    kInitial,
    kStarted,
    kCompleted,
    kFailedToStart,
  };

  ThreadedPagePinner(zx::iommu iommu, zx::bti bti)
      : thread_{[this]() { return ThreadMain(); }},
        iommu_{std::move(iommu)},
        bti_{std::move(bti)} {}

  ~ThreadedPagePinner() {
    // IF the thread has started, but has not been waited on, wait for it to finish now.
    if (thread_state_ == ThreadState::kStarted) {
      ZX_ASSERT(thread_.WaitForTerm());
    }

    // If we successfully pinned our memory, unpin it before we close the PMT handle.
    if (pmt_) {
      pmt_.unpin();
    }
  }

  bool ThreadMain() {
    const uint64_t page_size = zx_system_get_page_size();
    const uint64_t pages_to_pin = (amt_ + page_size - 1) / page_size;
    std::unique_ptr<zx_paddr_t[]> addrs = std::make_unique<zx_paddr_t[]>(pages_to_pin);
    return bti_.pin(perms_, *vmo_to_pin_, offset_, amt_, addrs.get(), pages_to_pin, &pmt_) == ZX_OK;
  }

  TestThread thread_;
  ThreadState thread_state_{ThreadState::kInitial};
  zx::iommu iommu_;
  zx::bti bti_;
  zx::pmt pmt_;
  zx::unowned_vmo vmo_to_pin_;
  uint32_t perms_{0};
  uint64_t offset_{0};
  uint64_t amt_{0};
};

}  // namespace

// This value corresponds to `VmObjectPaged::ReadWriteInternal::kMaxWriteWaitPages`
static constexpr uint64_t kMaxWriteWaitPages = 256;

// Convenience macro for tests that want to create VMOs both with and without the ZX_VMO_TRAP_DIRTY
// flag. |base_create_option| specifies the common create options to be used for both cases. The
// test body must use the local variable |create_option| to create VMOs.
#define TEST_WITH_AND_WITHOUT_TRAP_DIRTY(fn_name, base_create_option)                             \
  void fn_name(uint32_t create_option);                                                           \
  TEST(PagerWriteback, fn_name##TrapDirty) { fn_name((base_create_option) | ZX_VMO_TRAP_DIRTY); } \
  TEST(PagerWriteback, fn_name##NoTrapDirty) { fn_name(base_create_option); }                     \
  void fn_name(uint32_t create_option)

// Tests that a VMO created with TRAP_DIRTY can be supplied, and generates VMO_DIRTY requests when
// written to.
VMO_VMAR_TEST(PagerWriteback, SimpleTrapDirty) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));

  TestThread t1([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  // Supply the page first and then attempt to write to it.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  TestThread t2([vmo, &expected]() -> bool {
    uint8_t data = 0x77;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t2.Wait());

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), check_vmar));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Writes to a VMO created without TRAP_DIRTY go through without blocking.
  Vmo* vmo_no_trap;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo_no_trap));
  ASSERT_TRUE(pager.SupplyPages(vmo_no_trap, 0, 1));
  uint8_t data = 0xcc;
  ASSERT_OK(vmo_no_trap->vmo().write(&data, 0, sizeof(data)));

  vmo_no_trap->GenerateBufferContents(expected.data(), 1, 0);
  expected[0] = data;
  ASSERT_TRUE(check_buffer_data(vmo_no_trap, 0, 1, expected.data(), check_vmar));

  // Verify that a non pager-backed vmo cannot be created with TRAP_DIRTY.
  zx_handle_t handle;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_vmo_create(zx_system_get_page_size(), ZX_VMO_TRAP_DIRTY, &handle));

  // No requests seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo_no_trap, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo_no_trap, 0, &offset, &length));
}

// Tests that OP_DIRTY dirties pages even without a write to the VMO.
TEST(PagerWriteback, OpDirtyNoWrite) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a VMO and supply a page.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  // Dirty the page directly with the pager op.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  // The page should now be dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // VMO content is unchanged.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing to the VMO with zx_vmo_write generates DIRTY requests as expected.
TEST(PagerWriteback, DirtyRequestsOnVmoWrite) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 20;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  TestThread t([vmo, &expected]() -> bool {
    uint8_t data = 0x77;
    // write alternate pages {0, 2, 4, 6, 8}.
    for (uint64_t i = 0; i < kNumPages / 2; i += 2) {
      expected[i * zx_system_get_page_size()] = data;
      if (vmo->vmo().write(&data, i * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
        return false;
      }
    }
    // write consecutive runs of pages too.
    // pages written at this point are [0] [2,3,4] [6] [8].
    expected[3 * zx_system_get_page_size()] = data;
    if (vmo->vmo().write(&data, 3 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    const auto kZeroFill = vmo_test::TestFillPages<5>();
    memset(&expected[11 * zx_system_get_page_size()], 0, 5 * zx_system_get_page_size());
    // pages written are [11, 16).
    return vmo->vmo().write(kZeroFill.data(), 11 * zx_system_get_page_size(),
                            kZeroFill.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  for (uint64_t i = 0; i < kNumPages / 2; i += 2) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, i, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, i, 1));
  }

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 3, 1));

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 11, 5, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 11, 5));

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{0, 1, 0}, {2, 3, 0}, {6, 1, 0}, {8, 1, 0}, {11, 5, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing to the VMO through a VM mapping generates DIRTY requests as expected.
TEST(PagerWriteback, DirtyRequestsViaMapping) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 20;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  zx_vaddr_t ptr;
  TestThread t([vmo, &ptr, &expected]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   kNumPages * zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    uint8_t data = 0xcc;
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    // write alternate pages {0, 2, 4, 6, 8}.
    for (uint64_t i = 0; i < kNumPages / 2; i += 2) {
      expected[i * zx_system_get_page_size()] = data;
      buf[i * zx_system_get_page_size()] = data;
    }
    // write consecutive runs of pages too.
    // pages written at this point are [0] [2,3,4] [6] [8].
    expected[3 * zx_system_get_page_size()] = data;
    buf[3 * zx_system_get_page_size()] = data;
    // pages written are [11, 16).
    for (uint64_t i = 11; i < 16; i++) {
      expected[i * zx_system_get_page_size()] = data;
      buf[i * zx_system_get_page_size()] = data;
    }
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, kNumPages * zx_system_get_page_size());
  });

  ASSERT_TRUE(t.Start());

  for (uint64_t i = 0; i < kNumPages / 2; i += 2) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, i, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, i, 1));
  }

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 3, 1));

  // We're touching pages one by one via the mapping, so we'll see page requests for individual
  // pages. Wait for the first page request and dirty the whole range.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 11, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 11, 5));

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{0, 1, 0}, {2, 3, 0}, {6, 1, 0}, {8, 1, 0}, {11, 5, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that no DIRTY requests are generated on a read.
TEST(PagerWriteback, NoDirtyRequestsOnRead) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 3;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  zx_vaddr_t ptr;
  uint8_t tmp;
  TestThread t([vmo, &ptr, &tmp]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   kNumPages * zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    auto buf = reinterpret_cast<uint8_t*>(ptr);
    // Read pages.
    for (uint64_t i = 0; i < kNumPages; i++) {
      tmp = buf[i * zx_system_get_page_size()];
    }
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, kNumPages * zx_system_get_page_size());
  });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t.Wait());

  // No dirty requests should be seen as none of the pages were dirtied.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // Should be able to read from the VMO without faulting now.
  std::vector<uint8_t> buf(kNumPages * zx_system_get_page_size());
  ASSERT_TRUE(vmo->vmo().read(buf.data(), 0, buf.size()) == ZX_OK);

  // No dirty requests should be seen as none of the pages were dirtied.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // No remaining reads.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // No dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify contents.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));
}

// Tests that DIRTY requests are generated only on the first write.
TEST(PagerWriteback, DirtyRequestsRepeatedWrites) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  zx_vaddr_t ptr;
  TestThread t1([vmo, &ptr, &expected]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    uint8_t data = 0xcc;
    expected[0] = data;
    *reinterpret_cast<uint8_t*>(ptr) = data;
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t1.Wait());

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page again.
  TestThread t2([ptr, &expected]() -> bool {
    uint8_t data = 0xdd;
    expected[0] = data;
    *reinterpret_cast<uint8_t*>(ptr) = data;
    return true;
  });

  ASSERT_TRUE(t2.Start());

  // No more requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t2.Wait());

  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
}

// Tests that DIRTY requests are generated on a write to a page that was previously read from.
TEST(PagerWriteback, DirtyRequestsOnWriteAfterRead) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  zx_vaddr_t ptr;
  uint8_t tmp;
  TestThread t1([vmo, &ptr, &tmp]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    // Read from the page.
    tmp = *reinterpret_cast<uint8_t*>(ptr);
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  ASSERT_TRUE(t1.Start());

  // No read or dirty requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t1.Wait());

  // Now write to the page. This should trigger a dirty request.
  TestThread t2([ptr, &expected]() -> bool {
    uint8_t data = 0xdd;
    expected[0] = data;
    *reinterpret_cast<uint8_t*>(ptr) = data;
    return true;
  });

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t2.Wait());

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that no DIRTY requests are generated for clones of pager-backed VMOs.
TEST(PagerWriteback, NoDirtyRequestsForClones) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 3;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Write to the clone.
  TestThread t1([vmo_clone = clone.get()]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<kNumPages, 0xc>();
    return vmo_clone->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // Writing the pages in the clone should trigger faults in the parent.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // No dirty requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t1.Wait());

  // No dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  for (uint64_t i = 0; i < kNumPages; i++) {
    const auto kExpected = vmo_test::TestFillPages<1, 0xc>();
    std::vector<uint8_t> data(zx_system_get_page_size());
    ASSERT_OK(clone->vmo().read(data.data(), i * zx_system_get_page_size(), data.size()));
    ASSERT_EQ(0, memcmp(kExpected.data(), data.data(), kExpected.size_bytes()));
  }

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // Write to the parent now. This should trigger dirty requests.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<kNumPages, 0xd>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, kNumPages, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t2.Wait());

  // Should now see the pages dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = kNumPages};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  memset(expected.data(), 0xd, kNumPages * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No remaining requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writes for overlapping ranges generate the expected DIRTY requests.
TEST(PagerWriteback, DirtyRequestsOverlap) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  // This test cannot run as a component due to its usage of WaitForBlocked() which will return true
  // if the test thread is blocked on pagers outside of the test. WaitForBlocked() can only be
  // relied upon in a non-component environment. The pager-writeback tests cannot run as standalone
  // bootfs tests either because they need the next vDSO. Hence the only supported mode for this
  // test is unified mode, where the system resource will be available.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping\n");
  }

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 20;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  // We should not have any page requests yet.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  TestThread t1([vmo]() -> bool {
    // write pages [4,9).
    const auto kFillData = vmo_test::TestFillPages<5, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 4 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 5, ZX_TIME_INFINITE));

  // No other page requests seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  TestThread t2([vmo, &expected]() -> bool {
    // write pages [2,9).
    const auto kFillData = vmo_test::TestFillPages<7, 0xbb>();
    memset(expected.data() + 2 * zx_system_get_page_size(), 0xbb, kFillData.size_bytes());
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  // We should only see a request for the non-overlapping portion.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 2, ZX_TIME_INFINITE));

  // No other page requests seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Dirty the range [4,9).
  ASSERT_TRUE(pager.DirtyPages(vmo, 4, 5));
  ASSERT_TRUE(t1.Wait());

  // Dirty the range [2,4).
  ASSERT_TRUE(pager.DirtyPages(vmo, 2, 2));
  ASSERT_TRUE(t2.Wait());

  // Verify dirty ranges.
  std::vector<zx_vmo_dirty_range_t> ranges;
  ranges.push_back({.offset = 2, .length = 7});
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges.data(), ranges.size()));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  TestThread t3([vmo, &expected]() -> bool {
    // write pages [11,16).
    const auto kFillData = vmo_test::TestFillPages<5, 0xcc>();
    memset(expected.data() + 11 * zx_system_get_page_size(), 0xcc, kFillData.size_bytes());
    return vmo->vmo().write(kFillData.data(), 11 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 11, 5, ZX_TIME_INFINITE));

  TestThread t4([vmo, &expected]() -> bool {
    // write pages [15,19).
    const auto kFillData = vmo_test::TestFillPages<4, 0xdd>();
    memset(expected.data() + 15 * zx_system_get_page_size(), 0xdd, kFillData.size_bytes());
    return vmo->vmo().write(kFillData.data(), 15 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t4.Start());
  ASSERT_TRUE(t4.WaitForBlocked());

  // No remaining requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // Dirty the range [11,16).
  ASSERT_TRUE(pager.DirtyPages(vmo, 11, 5));

  // This should terminate t3, and wake up t4 until it blocks again for the remaining range.
  ASSERT_TRUE(t3.Wait());
  ASSERT_TRUE(t4.WaitForBlocked());

  // Verify dirty ranges.
  ranges.push_back({.offset = 11, .length = 5});
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges.data(), ranges.size()));

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 16, 3, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 16, 3));

  ASSERT_TRUE(t4.Wait());

  // Verify dirty ranges.
  ranges.back().length = 8;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges.data(), ranges.size()));

  // The contents of page 15 can vary depending on which of t3 or t4 wrote to it last, as both were
  // blocked on a dirty request for it at the same time, so there's a race.
  bool outcome1 = check_buffer_data(vmo, 0, kNumPages, expected.data(), true);
  memset(expected.data() + 15 * zx_system_get_page_size(), 0xcc, zx_system_get_page_size());
  bool outcome2 = check_buffer_data(vmo, 0, kNumPages, expected.data(), true);
  ASSERT_TRUE(outcome1 || outcome2);

  // No remaining requests.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Tests that DIRTY requests are generated as expected for a VMO that has random offsets in various
// page states: {Empty, Clean, Dirty}.
TEST(PagerWriteback, DirtyRequestsRandomOffsets) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 10;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  int page_state[kNumPages];  // 0 for empty, 1 for clean, and 2 for dirty
  for (uint64_t i = 0; i < kNumPages; i++) {
    page_state[i] = rand() % 3;
    if (page_state[i] == 0) {
      // Page not present. Skip ahead.
      continue;
    } else if (page_state[i] == 1) {
      // Page is present and clean.
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
    } else {
      // Page is present and dirty.
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
      ASSERT_TRUE(pager.DirtyPages(vmo, i, 1));
    }
  }

  // Now write to the entire range. We should see a combination of read and dirty requests.
  TestThread t([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<kNumPages>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  uint64_t clean_start = 0;
  uint64_t clean_len = 0;
  for (int64_t i = 0; i < static_cast<int64_t>(kNumPages); i++) {
    if (page_state[i] == 0) {
      // Page is not present.
      // This might break an in-progress clean run, resolve that first.
      if (clean_len > 0) {
        ASSERT_TRUE(pager.WaitForPageDirty(vmo, clean_start, clean_len, ZX_TIME_INFINITE));
        ASSERT_TRUE(pager.DirtyPages(vmo, clean_start, clean_len));
      }

      // Check how many pages after this are not present. The request will be batched.
      uint64_t not_present_run_size = 1;
      for (uint64_t j = i + 1; j < kNumPages; j++) {
        if (page_state[j] != 0) {
          break;
        }
        ++not_present_run_size;
      }

      // Should see a read request for this not-present page run.
      ASSERT_TRUE(pager.WaitForPageRead(vmo, i, not_present_run_size, ZX_TIME_INFINITE));
      ASSERT_TRUE(pager.SupplyPages(vmo, i, not_present_run_size));

      // After the supply, visit this page again, as it might get combined into a subsequent clean
      // run. Set all the pages in the run to clean and decrement i.
      for (uint64_t j = i; j < i + not_present_run_size; j++) {
        page_state[j] = 1;
      }
      i--;

      clean_start = i + 1;
      clean_len = 0;

    } else if (page_state[i] == 1) {
      // Page is present and clean. Accumulate into the clean run.
      clean_len++;

    } else {
      // Page is present and dirty.
      // This might break an in-progress clean run, resolve that first.
      if (clean_len > 0) {
        ASSERT_TRUE(pager.WaitForPageDirty(vmo, clean_start, clean_len, ZX_TIME_INFINITE));
        ASSERT_TRUE(pager.DirtyPages(vmo, clean_start, clean_len));
      }
      clean_start = i + 1;
      clean_len = 0;
    }
  }

  // Resolve the last clean run if any.
  if (clean_len > 0) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, clean_start, clean_len, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, clean_start, clean_len));
  }

  ASSERT_TRUE(t.Wait());

  // No remaining requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that ZX_PAGER_OP_FAIL can fail DIRTY page requests and propagate the failure up.
TEST(PagerWriteback, FailDirtyRequests) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 2;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  zx_vaddr_t ptr;
  TestThread t1([vmo, &ptr]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }
    // Write page 0.
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    buf[0] = 0xcc;
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));

  ASSERT_TRUE(t1.WaitForCrash(ptr, ZX_ERR_IO));

  // No pages should be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  TestThread t2([vmo]() -> bool {
    uint8_t data = 0xdd;
    // Write page 1.
    return vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1));

  ASSERT_TRUE(t2.WaitForFailure());

  // No pages should be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that DIRTY requests are generated when offsets with zero page markers are written to.
TEST(PagerWriteback, DirtyRequestsForZeroPages) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint64_t kNumPages = 2;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  // Supply with empty source vmo so that the destination gets zero page markers.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(kNumPages * zx_system_get_page_size(), 0, &vmo_src));
  ASSERT_OK(
      pager.pager().supply_pages(vmo->vmo(), 0, kNumPages * zx_system_get_page_size(), vmo_src, 0));

  // Verify that the pager vmo has no committed pages, i.e. it only has markers.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);

  // Write to the first page with zx_vmo_write.
  TestThread t1([vmo, &expected]() -> bool {
    uint8_t data = 0xaa;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Dirty the first page.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify that the pager vmo has one committed page now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  zx_vaddr_t ptr;
  // Map the second page of the vmo.
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(),
                                       zx_system_get_page_size(), zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Write to the second page via the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  uint8_t data = 0xbb;
  TestThread t2([buf, data, &expected]() -> bool {
    *buf = data;
    expected[zx_system_get_page_size()] = data;
    return true;
  });

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));

  // Dirty the second page.
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  ASSERT_TRUE(t2.Wait());

  // Verify that the pager vmo has both pages committed now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(kNumPages * zx_system_get_page_size(), info.committed_bytes);

  // Verify that both the pages are now dirty.
  range = {.offset = 0, .length = 2};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that ZX_PAGER_OP_DIRTY works for a mix of zero and non-zero pages.
TEST(PagerWriteback, DirtyZeroAndNonZeroPages) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint64_t kNumPages = 10;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);

  // Empty source vmo to supply with zero pages.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));

  // For each page offset, supply either a zero or a non-zero page.
  uint64_t non_zero_count = 0;
  for (uint64_t i = 0; i < kNumPages; i++) {
    if (rand() % 2) {
      non_zero_count++;
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
      vmo->GenerateBufferContents(expected.data() + i * zx_system_get_page_size(), 1, i);
    } else {
      ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), i * zx_system_get_page_size(),
                                           zx_system_get_page_size(), vmo_src, 0));
    }
  }

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // Only non-zero pages should be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(non_zero_count * zx_system_get_page_size(), info.committed_bytes);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify that we're able to dirty the entire range regardless of the type of page.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, kNumPages));

  // All the pages should be committed and dirty now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(kNumPages * zx_system_get_page_size(), info.committed_bytes);
  zx_vmo_dirty_range_t range = {.offset = 0, .length = kNumPages};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that ZX_PAGER_OP_FAIL can fail DIRTY page requests for zero pages.
TEST(PagerWriteback, FailDirtyRequestsForZeroPages) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));

  // Supply with empty source vmo so that the destination gets zero page markers.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, zx_system_get_page_size(), vmo_src, 0));

  // Verify that the pager vmo has no committed pages, i.e. it only has markers.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);

  // Attempt to write to the first page.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the dirty request.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));

  // The thread should exit with failure.
  ASSERT_TRUE(t.WaitForFailure());

  // No committed pages still.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No dirty pages too.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that DIRTY requests are generated for ranges including zero pages as expected.
TEST(PagerWriteback, DirtyRequestsForZeroRanges) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint64_t kNumPages = 10;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);

  // Empty source vmo to supply with zero pages.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));

  // Supply zero page markers for pages 0 and 1.
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, zx_system_get_page_size(), vmo_src, 0));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), zx_system_get_page_size(),
                                       zx_system_get_page_size(), vmo_src, 0));

  // Attempt to write to the range [0, 2).
  TestThread t1([vmo, &expected]() -> bool {
    std::vector<uint8_t> data(2 * zx_system_get_page_size(), 0xaa);
    memset(expected.data(), 0xaa, 2 * zx_system_get_page_size());
    return vmo->vmo().write(data.data(), 0, 2 * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());

  // We should see a dirty request for the range [0, 2). Verifies that the range is extended to
  // include another marker.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 2));
  ASSERT_TRUE(t1.Wait());

  // Verify dirty pages.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Supply a zero marker for page 2 and a non-zero page for page 3.
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 2 * zx_system_get_page_size(),
                                       zx_system_get_page_size(), vmo_src, 0));
  ASSERT_TRUE(pager.SupplyPages(vmo, 3, 1));

  // Attempt to write to the range [2, 4).
  TestThread t2([vmo, &expected]() -> bool {
    std::vector<uint8_t> data(2 * zx_system_get_page_size(), 0xbb);
    memset(expected.data() + 2 * zx_system_get_page_size(), 0xbb, 2 * zx_system_get_page_size());
    return vmo->vmo().write(data.data(), 2 * zx_system_get_page_size(),
                            2 * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());

  // We should see a dirty request for the range [2, 4). Verifies that the range is extended to
  // include a non-zero clean page.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 2, 2));
  ASSERT_TRUE(t2.Wait());

  // Verify dirty pages.
  range = {.offset = 0, .length = 4};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // For the rest of the pages, supply a mix of zero and non-zero pages, leaving a gap at the end.
  for (uint64_t i = 4; i < kNumPages - 1; i++) {
    if (rand() % 2) {
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
    } else {
      ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), i * zx_system_get_page_size(),
                                           zx_system_get_page_size(), vmo_src, 0));
    }
  }

  // Attempt to write to the range [4, 10).
  TestThread t3([vmo, &expected]() -> bool {
    size_t len = kNumPages - 4;
    std::vector<uint8_t> data(len * zx_system_get_page_size(), 0xcc);
    memset(expected.data() + 4 * zx_system_get_page_size(), 0xcc, len * zx_system_get_page_size());
    return vmo->vmo().write(data.data(), 4 * zx_system_get_page_size(),
                            len * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t3.Start());

  // We should see a dirty request for pages [4, 9). Verifies that zero and non-zero clean pages get
  // picked up in a single range, and that the range stops before a gap.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 5, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 4, 5));

  // We should now see a read request followed by a dirty request for the last gap.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 9, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 9, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 9, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 9, 1));
  ASSERT_TRUE(t3.Wait());

  // Verify dirty pages.
  range = {.offset = 0, .length = kNumPages};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that no DIRTY requests are generated on a commit.
TEST(PagerWriteback, NoDirtyRequestsOnCommit) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 5;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  // Supply some pages.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Commit the vmo.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_COMMIT, 0, kNumPages * zx_system_get_page_size(), nullptr,
                               0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // Should see a read request for the uncommitted portion.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 2, kNumPages - 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, kNumPages - 2));

  // The thread should be able to exit now.
  ASSERT_TRUE(t.Wait());

  // No dirty requests should be seen as none of the pages were dirtied.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // No remaining reads.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // No dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
}

// Tests that no DIRTY requests are generated when a mapping is created with MAP_RANGE.
TEST(PagerWriteback, NoDirtyRequestsOnMapRange) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 3;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);

  zx_vaddr_t ptr;
  TestThread t1([vmo, &ptr]() -> bool {
    // Map the vmo, and populate mappings for all committed pages. We know the pages are
    // pre-committed so we should not block on reads. And we should not be generating any dirty
    // requests to block on either.
    return zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_MAP_RANGE, 0,
                                      vmo->vmo(), 0, kNumPages * zx_system_get_page_size(),
                                      &ptr) == ZX_OK;
  });
  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, kNumPages * zx_system_get_page_size());
  });

  ASSERT_TRUE(t1.Start());

  // No dirty requests should be seen as none of the pages were dirtied.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  // No reads either.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t1.Wait());

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  uint8_t tmp;
  TestThread t2([&ptr, &tmp]() -> bool {
    // Read the mapped pages. This will not block.
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    for (uint64_t i = 0; i < kNumPages; i++) {
      tmp = buf[i * zx_system_get_page_size()];
    }
    return true;
  });

  ASSERT_TRUE(t2.Start());

  // No dirty or read requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t2.Wait());

  // No dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  TestThread t3([&ptr, &expected]() -> bool {
    // Now try to write to the vmo. This should result in write faults and dirty requests.
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    for (uint64_t i = 0; i < kNumPages; i++) {
      uint8_t data = 0xcc;
      buf[i * zx_system_get_page_size()] = data;
      expected[i * zx_system_get_page_size()] = data;
    }
    return true;
  });

  ASSERT_TRUE(t3.Start());

  // The thread will block on dirty requests for each page.
  for (uint64_t i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, i, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, i, 1));
  }

  // The thread should now exit.
  ASSERT_TRUE(t3.Wait());

  // All pages are dirty now.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = kNumPages};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more dirty or read requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that no DIRTY requests are generated when previously dirty pages are mapped and written to.
TEST(PagerWriteback, NoDirtyRequestsMapExistingDirty) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  // Dirty the page.
  TestThread t1([vmo, &expected]() -> bool {
    uint8_t data = 0xcc;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t1.Wait());

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Map the page and try writing to it.
  zx_vaddr_t ptr;
  TestThread t2([vmo, &ptr, &expected]() -> bool {
    // Map the vmo.
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    uint8_t data = 0xdd;
    *reinterpret_cast<uint8_t*>(ptr) = data;
    expected[0] = data;
    return true;
  });

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  ASSERT_TRUE(t2.Start());

  // No read or dirty requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t2.Wait());

  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
}

// Tests that dirty ranges cannot be queried on a clone.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NoQueryOnClone, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  uint8_t data = 0xaa;
  TestThread t([vmo, data]() -> bool { return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK; });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Create a clone.
  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Write to the clone.
  uint8_t data_clone = 0x77;
  ASSERT_OK(clone->vmo().write(&data_clone, 0, sizeof(data_clone)));

  // Can query dirty ranges on the parent.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify parent contents.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Cannot query dirty ranges on the clone.
  uint64_t num_ranges = 0;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_dirty_ranges(pager.pager().get(), clone->vmo().get(), 0,
                                        zx_system_get_page_size(), &range, sizeof(range),
                                        &num_ranges, nullptr));

  // Verify clone contents.
  memset(expected.data(), data_clone, sizeof(data));
  ASSERT_TRUE(check_buffer_data(clone.get(), 0, 1, expected.data(), true));

  // No requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that WRITEBACK_BEGIN/END clean pages as expected.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(SimpleWriteback, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  // Dirty the page by writing to it.
  uint8_t data = 0xaa;
  TestThread t1(
      [vmo, data]() -> bool { return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK; });
  ASSERT_TRUE(t1.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t1.Wait());

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Begin writeback on the page.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // The page is still dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // This should transition the page to clean, and a subsequent write should trigger
  // another dirty request.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // No dirty pages after writeback end.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Dirty the page again.
  TestThread t2([vmo, &expected]() -> bool {
    uint8_t data = 0x77;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // We should see a dirty request now.
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t2.Wait());

  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a write after WRITEBACK_BEGIN but before WRITEBACK_END is handled correctly.
TEST(PagerWriteback, DirtyDuringWriteback) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  // This test cannot run as a component due to its usage of WaitForBlocked() which will return true
  // if the test thread is blocked on pagers outside of the test. WaitForBlocked() can only be
  // relied upon in a non-component environment. The pager-writeback tests cannot run as standalone
  // bootfs tests either because they need the next vDSO. Hence the only supported mode for this
  // test is unified mode, where the system resource will be available.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping");
  }

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  // Dirty the page.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Begin writeback on the page.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // The page is still dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page before ending writeback. This should generate a dirty request.
  TestThread t1([vmo, &expected]() -> bool {
    uint8_t data = 0xcc;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());

  // Verify that we saw the dirty request but do not acknowledge it yet. The write will remain
  // blocked.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // End the writeback. This should transition the page to clean.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The writing thread is still blocked.
  ASSERT_TRUE(t1.WaitForBlocked());

  // Now dirty the page, unblocking the writing thread.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // The page is dirty again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Begin another writeback, and try writing again before ending it. This time acknowledge the
  // dirty request while the writeback is in progress.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to the page before ending writeback. This should generate a dirty request.
  TestThread t2([vmo, &expected]() -> bool {
    uint8_t data = 0xdd;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());

  // Verify that we saw the dirty request.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // This should reset the page state to dirty so that it is not moved to clean when the writeback
  // ends later.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t2.Wait());

  // Verify that the page is dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Now end the writeback. This should *not* clean the page, as a write was accepted after
  // beginning the writeback.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that mapping write permissions are cleared as expected on writeback.
TEST(PagerWriteback, WritebackWithMapping) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  zx_vaddr_t ptr;
  // Map the vmo.
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Write to the vmo. This will be trapped and generate a dirty request.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  uint8_t data = 0xaa;
  TestThread t1([buf, data, &expected]() -> bool {
    *buf = data;
    expected[0] = data;
    return true;
  });

  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Dirty the page.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page again. This should go through without any page faults / dirty requests.
  data = 0xbb;
  *buf = data;
  expected[0] = data;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Start a writeback.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page again. This should result in a fault / dirty request.
  TestThread t2([buf]() -> bool {
    *buf = 0xcc;
    return true;
  });

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the dirty request so the writeback can complete.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));
  ASSERT_TRUE(t2.WaitForCrash(ptr, ZX_ERR_IO));

  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Complete the writeback, making the page clean.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page again. This should again be trapped.
  data = 0xdd;
  TestThread t3([buf, data, &expected]() -> bool {
    *buf = data;
    expected[0] = data;
    return true;
  });

  ASSERT_TRUE(t3.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t3.Wait());

  // The page is dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that the zero page marker cannot be overwritten by another page, unless written to at which
// point it is forked.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(CannotOverwriteZeroPage, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));

  // Supply with empty source vmo so that the destination gets zero page markers.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, zx_system_get_page_size(), vmo_src, 0));

  // Verify that the pager vmo has no committed pages, i.e. it only has markers.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Commit a page in the source to attempt another supply.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo_src.write(&data, 0, sizeof(data)));

  // Supplying the same page again should not overwrite the zero page marker. The supply will
  // succeed as a no-op.
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, zx_system_get_page_size(), vmo_src, 0));

  // No committed pages still.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // The VMO is still all zeros.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Now write to the VMO. This should fork the zero page.
  TestThread t1([vmo, &expected]() -> bool {
    uint8_t data = 0xbb;
    expected[0] = data;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // Wait for and acknowledge the dirty request if configured to trap dirty transitions.
  if (create_option == ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    // Dirty the first page.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  ASSERT_TRUE(t1.Wait());

  // Verify that the pager vmo has one committed page now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify written data.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that VMOs created without the ZX_VMO_TRAP_DIRTY flag track dirty pages as expected.
TEST(PagerWriteback, SimpleDirtyNoTrap) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a VMO without the ZX_VMO_TRAP_DIRTY flag.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Write to the page now. This should go through without blocking.
  uint8_t data = 0x77;
  expected[0] = data;
  ASSERT_OK(vmo->vmo().write(&data, 0, sizeof(data)));

  // We should now have one dirty page.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify written data.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Begin writeback on the page.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // The page is still dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // This should transition the page to clean, and a subsequent write should trigger
  // another dirty request.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // No dirty pages after writeback end.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Map the vmo.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Write to the vmo again via the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  data = 0x55;
  *buf = data;
  expected[0] = data;

  // The page should get dirtied again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No dirty or read requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that VMOs created without the ZX_VMO_TRAP_DIRTY flag track dirty pages as expected for a
// random mix of zero and non-zero pages.
TEST(PagerWriteback, DirtyNoTrapRandomOffsets) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a VMO without the ZX_VMO_TRAP_DIRTY flag.
  Vmo* vmo;
  constexpr uint64_t kNumPages = 10;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);

  // Empty source vmo to supply with zero pages.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));

  // For each page offset, supply either a zero or a non-zero page.
  uint64_t non_zero_count = 0;
  for (uint64_t i = 0; i < kNumPages; i++) {
    if (rand() % 2) {
      non_zero_count++;
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
      vmo->GenerateBufferContents(expected.data() + i * zx_system_get_page_size(), 1, i);
    } else {
      ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), i * zx_system_get_page_size(),
                                           zx_system_get_page_size(), vmo_src, 0));
    }
  }

  // Only non-zero pages should be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(non_zero_count * zx_system_get_page_size(), info.committed_bytes);

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // Verify that we're able to write to the entire range regardless of the type of page. Alter the
  // expected contents to verify later.
  uint8_t data = 0x77;
  for (uint64_t i = 0; i < kNumPages; i++) {
    expected[i * zx_system_get_page_size()] = data++;
  }
  ASSERT_OK(vmo->vmo().write(expected.data(), 0, kNumPages * zx_system_get_page_size()));

  // All the pages should be committed and dirty now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(kNumPages * zx_system_get_page_size(), info.committed_bytes);
  zx_vmo_dirty_range_t range = {.offset = 0, .length = kNumPages};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, kNumPages, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that adding the WRITE permission with zx_vmar_protect does not override read-only mappings
// required in order to track dirty transitions.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(DirtyAfterMapProtect, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a temporary VMAR to work with.
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  ASSERT_OK(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE, 0,
                                            zx_system_get_page_size(), &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Buffer to verify VMO contents later.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);

  zx_vaddr_t ptr;
  // Map the vmo read-only first so that the protect step below is not a no-op.
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(), &ptr));

  // Read the VMO through the mapping so that the hardware mapping is created.
  uint8_t data = *reinterpret_cast<uint8_t*>(ptr);
  ASSERT_EQ(data, expected[0]);

  // Add the write permission now. This will allow us to write to the VMO below.
  ASSERT_OK(vmar.protect(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, ptr, zx_system_get_page_size()));

  // Write to the vmo. This should trigger a write fault. If the protect above added the write
  // permission on the hardware mapping, this write will go through without generating a write
  // fault for dirty tracking.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  data = 0xaa;
  TestThread t([buf, data, &expected]() -> bool {
    *buf = data;
    expected[0] = data;
    return true;
  });

  ASSERT_TRUE(t.Start());

  if (create_option == ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    // Dirty the page.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_EQ(data, *buf);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that zero pages are supplied by the kernel for the newly extended range after a resize, and
// are not overwritten by a pager supply.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeSupplyZero, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  static constexpr uint64_t kNumPages = 2;
  static constexpr uint64_t kNumPagesAfterResize = 4;
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, create_option, &vmo));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(kNumPagesAfterResize));

  // Now try to access all the pages. The first two should result in read requests, but the last
  // two should be supplied with zeros without any read requests.
  TestThread t([vmo]() -> bool {
    std::vector<uint8_t> data(kNumPagesAfterResize * zx_system_get_page_size());
    return vmo->vmo().read(data.data(), 0, data.size()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  // No more read requests seen for the newly extended range.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t.Wait());

  // Verify that the last two pages are zeros.
  std::vector<uint8_t> expected(kNumPagesAfterResize * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // Only two pages should be committed in the VMO.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(2 * zx_system_get_page_size(), info.committed_bytes);

  // Supply pages in the newly extended range. This should be a no-op. Since the range is already
  // implicitly "supplied", another supply will be ignored.
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 2));
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(2 * zx_system_get_page_size(), info.committed_bytes);

  // Verify that the last two pages are still zero.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // Writes for this case are tested separately in ResizeDirtyRequest. Skip the rest.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    return;
  }

  // Write to the last two pages now.
  const auto kFillData = vmo_test::TestFillPages<2, 0xaa>();
  ASSERT_OK(
      vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(), kFillData.size_bytes()));

  // All four pages should be committed now.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(4 * zx_system_get_page_size(), info.committed_bytes);

  // Verify the contents.
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, kFillData.size_bytes());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // The last two pages should be dirty.
  zx_vmo_dirty_range_t range = {.offset = 2, .length = 2};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing to the newly extended range after a resize can generate DIRTY requests as
// expected.
TEST(PagerWriteback, ResizeDirtyRequest) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  // This test cannot run as a component due to its usage of WaitForBlocked() which will return true
  // if the test thread is blocked on pagers outside of the test. WaitForBlocked() can only be
  // relied upon in a non-component environment. The pager-writeback tests cannot run as standalone
  // bootfs tests either because they need the next vDSO. Hence the only supported mode for this
  // test is unified mode, where the system resource will be available.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping");
  }

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, ZX_VMO_TRAP_DIRTY | ZX_VMO_RESIZABLE, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Now try to write pages 1 and 2. We should see dirty requests for both.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<2, 0xaa>();
    return vmo->vmo().write(kFillData.data(), zx_system_get_page_size(), kFillData.size_bytes()) ==
           ZX_OK;
  });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(t1.WaitForBlocked());

  // No read requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Dirty request seen for the entire write range.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 2));

  ASSERT_TRUE(t1.Wait());

  // Verify the VMO contents. (Allocate a buffer large enough to reuse across all resizes.)
  std::vector<uint8_t> expected(8 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + zx_system_get_page_size(), 0xaa, 2 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(3 * zx_system_get_page_size(), info.committed_bytes);

  // Verify that pages 1 and 2 are dirty.
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 2};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Resize the VMO up again, and try writing to a page after a gap.
  ASSERT_TRUE(vmo->Resize(6));

  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xbb>();
    // Write to page 4.
    return vmo->vmo().write(kFillData.data(), 4 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(t2.WaitForBlocked());

  // No read requests seen.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // We should only see a dirty request for page 4.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));

  ASSERT_TRUE(t2.Wait());

  // Verify the contents again.
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xbb, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {
      {1, 2, 0}, {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {4, 1, 0}, {5, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Resize up again, and try writing to the entire VMO at once.
  ASSERT_TRUE(vmo->Resize(8));

  TestThread t3([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<8, 0xcc>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(t3.WaitForBlocked());

  // No read requests seen.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // We should see a dirty request for page 0.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  // We should see a dirty request for page 3.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 3, 1));

  // We should see a dirty request for pages 5,6,7.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 5, 3, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 5, 3));

  ASSERT_TRUE(t3.Wait());

  // Verify the contents.
  memset(expected.data(), 0xcc, 8 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 8, expected.data(), true));

  // Verify that all the pages are dirty.
  range = {.offset = 0, .length = 8};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writeback on a resized VMO works as expected.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWriteback, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Write to the first and the last page, leaving a gap in between.
  TestThread t([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    if (vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) != ZX_OK) {
      return false;
    }
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0xaa);
  memset(expected.data() + zx_system_get_page_size(), 0, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Verify that all the pages are dirty.
  zx_vmo_dirty_range_t ranges_before[] = {{0, 1, 0}, {1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges_before,
                                      sizeof(ranges_before) / sizeof(zx_vmo_dirty_range_t)));

  // Attempt to writeback all the pages.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 3));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 3));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resize down unblocks outstanding DIRTY requests that are out-of-bounds.
TEST(PagerWriteback, ResizeWithOutstandingDirtyRequests) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(5, ZX_VMO_RESIZABLE | ZX_VMO_TRAP_DIRTY, &vmo));

  // Supply page 1 as a zero page marker.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), zx_system_get_page_size(),
                                       zx_system_get_page_size(), vmo_src, 0));

  // Supply page 3 as an actual page.
  ASSERT_TRUE(pager.SupplyPages(vmo, 3, 1));

  // Resize the VMO up so there's a non-zero range that will be supplied as zero.
  ASSERT_TRUE(vmo->Resize(6));

  // The new "page" at the end should be indicated dirty and zero.
  zx_vmo_dirty_range_t range = {.offset = 5, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Try to write to page 1 which is a zero marker.
  TestThread t1([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) == ZX_ERR_OUT_OF_RANGE;
  });

  // Try to write to page 3 which is an actual clean page.
  TestThread t2([vmo]() -> bool {
    uint8_t data = 0xbb;
    return vmo->vmo().write(&data, 3 * zx_system_get_page_size(), sizeof(data)) ==
           ZX_ERR_OUT_OF_RANGE;
  });

  // Try to write to page 5 which is a gap in the newly extended range.
  TestThread t3([vmo]() -> bool {
    uint8_t data = 0xcc;
    return vmo->vmo().write(&data, 5 * zx_system_get_page_size(), sizeof(data)) ==
           ZX_ERR_OUT_OF_RANGE;
  });

  // Try to read page 2 which is a non-resident page.
  TestThread t4([vmo]() -> bool {
    uint8_t data;
    return vmo->vmo().read(&data, 2 * zx_system_get_page_size(), sizeof(data)) ==
           ZX_ERR_OUT_OF_RANGE;
  });

  // All four threads should block.
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(t4.Start());

  // We should see dirty requests for pages 1, 3 and 5.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 5, 1, ZX_TIME_INFINITE));

  // We should see a read request for page 2.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 2, 1, ZX_TIME_INFINITE));

  // No more requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // Now resize down so that the pages all four threads are waiting for become out-of-bounds.
  ASSERT_TRUE(vmo->Resize(1));

  // All four threads should now see ZX_ERR_OUT_OF_RANGE returned for their reads/writes.
  ASSERT_TRUE(t1.Wait());
  ASSERT_TRUE(t2.Wait());
  ASSERT_TRUE(t3.Wait());
  ASSERT_TRUE(t4.Wait());

  // Trying to resolve the dirty and read requests we previously saw should fail.
  ASSERT_FALSE(pager.DirtyPages(vmo, 1, 1));
  ASSERT_FALSE(pager.DirtyPages(vmo, 3, 1));
  ASSERT_FALSE(pager.DirtyPages(vmo, 5, 1));
  ASSERT_FALSE(pager.SupplyPages(vmo, 2, 1));

  // The VMO has no dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resize down unblocks outstanding DIRTY requests that are out-of-bounds when the
// out-of-bounds range is in the process of being written back.
TEST(PagerWriteback, ResizeWritebackWithOutstandingDirtyRequests) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, ZX_VMO_RESIZABLE | ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(5));

  // Write to a page leaving a gap beyond the old size.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 4 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t ranges[] = {{2, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {4, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  std::vector<uint8_t> expected(5 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 5, expected.data(), true));

  // Begin writeback for all the dirty pages. This will result in DIRTY requests if they are written
  // again.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 2, 3));

  // Try to write to pages 1 and 2. This will trigger a DIRTY request.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<2, 0xbb>();
    return vmo->vmo().write(kFillData.data(), zx_system_get_page_size(), kFillData.size_bytes()) ==
           ZX_ERR_OUT_OF_RANGE;
  });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 2, ZX_TIME_INFINITE));

  // Try to write to pages 3 and 4. This will also trigger a DIRTY request.
  TestThread t3([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<2, 0xcc>();
    return vmo->vmo().write(kFillData.data(), 3 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_ERR_OUT_OF_RANGE;
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 2, ZX_TIME_INFINITE));

  // Complete writeback for the start of the dirty range so that the zero tail can be advanced. This
  // will give us a gap before the tail. Now we will be able to test all four cases - a non-dirty
  // page before the tail, a gap before the tail, a non-dirty page after the tail, and a gap after
  // the tail.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 2, 1));

  // Resize down so that both the DIRTY requests are now out of bounds.
  ASSERT_TRUE(vmo->Resize(1));

  // Wait for the threads to complete.
  ASSERT_TRUE(t2.Wait());
  ASSERT_TRUE(t3.Wait());

  // Verify dirty ranges and VMO contents.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // End the remaining range of the writeback we began previously. This will fail as it is out of
  // bounds.
  ASSERT_FALSE(pager.WritebackEndPages(vmo, 3, 2));

  // Verify dirty ranges and VMO contents again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing again to resized range that is being written back triggers new DIRTY requests.
TEST(PagerWriteback, ResizeWritebackNewDirtyRequestsInterleaved) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_RESIZABLE | ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Write to a page leaving a gap.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Beging writeback for all the dirty pages.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 2));

  // Try to write to page 1. This will trigger a DIRTY request.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xbb>();
    return vmo->vmo().write(kFillData.data(), zx_system_get_page_size(), kFillData.size_bytes()) ==
           ZX_OK;
  });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));

  // Try to write to page 2. This will trigger a DIRTY request.
  TestThread t3([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xcc>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));

  // Resolve the DIRTY requests and wait for the threads to complete.
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 2));
  ASSERT_TRUE(t2.Wait());
  ASSERT_TRUE(t3.Wait());

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t range = {1, 2, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  memset(expected.data() + zx_system_get_page_size(), 0xbb, zx_system_get_page_size());
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xcc, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // End the writeback we began previously. This will be a no-op as both pages were dirtied again.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 2));

  // Verify dirty ranges and VMO contents again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Should be able to write to the two dirty pages again without blocking.
  const auto kFillData = vmo_test::TestFillPages<2, 0xdd>();
  ASSERT_OK(vmo->vmo().write(kFillData.data(), zx_system_get_page_size(), kFillData.size_bytes()));

  // Verify dirty ranges and VMO contents again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));
  memset(expected.data() + zx_system_get_page_size(), 0xdd, 2 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing again to a written back resized range triggers new DIRTY requests.
TEST(PagerWriteback, ResizeWritebackNewDirtyRequests) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_RESIZABLE | ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Write to a page leaving a gap.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Writeback all the dirty pages.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 2));

  // No dirty ranges remaining.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Try to write to page 1. This will trigger a DIRTY request.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xbb>();
    return vmo->vmo().write(kFillData.data(), zx_system_get_page_size(), kFillData.size_bytes()) ==
           ZX_OK;
  });
  ASSERT_TRUE(t2.Start());
  // This was a gap that we've written back. So we'll first need to supply the page.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));

  // Try to write to page 2. This will trigger a DIRTY request.
  TestThread t3([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xcc>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));

  // Resolve the DIRTY requests and wait for the threads to complete.
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 2));
  ASSERT_TRUE(t2.Wait());
  ASSERT_TRUE(t3.Wait());

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t range = {1, 2, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  memset(expected.data() + zx_system_get_page_size(), 0xbb, zx_system_get_page_size());
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xcc, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a write interleaved with a writeback retains the dirtied page that falls in the zero
// range being written back.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackIntersectingWrite, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 3, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 3));

  // Write to a page in the range.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(4 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // Verify that the last three pages are dirty.
  zx_vmo_dirty_range_t ranges_before[] = {
      {1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}, {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges_before,
                                      sizeof(ranges_before) / sizeof(zx_vmo_dirty_range_t)));

  // End the writeback that we began previously.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 3));

  // We should not have been able to clean the page that was dirtied after beginning the writeback.
  range = {2, 1, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Now attempt a writeback again for the entire VMO.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 4));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 4));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a write outside of an awaiting clean zero range does not affect it.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackNonIntersectingWrite, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 3, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for a portion of the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 2));

  // Write to a page following the awaiting clean range.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 3 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 3, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Write to a page preceding the awaiting clean range.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xbb>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t2.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(4 * zx_system_get_page_size(), 0);
  memset(expected.data(), 0xbb, zx_system_get_page_size());
  memset(expected.data() + 3 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // Verify that all of the pages are dirty.
  zx_vmo_dirty_range_t ranges_before[] = {{0, 1, 0}, {1, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {3, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges_before,
                                      sizeof(ranges_before) / sizeof(zx_vmo_dirty_range_t)));

  // End the writeback that we began previously.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 2));

  // The range that was written back should be clean now. The pages that were written should be
  // dirty.
  zx_vmo_dirty_range_t ranges_after[] = {{0, 1, 0}, {3, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges_after,
                                      sizeof(ranges_after) / sizeof(zx_vmo_dirty_range_t)));

  // Attempt another writeback for the entire VMO.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 4));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 4));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests extending the VMO, starting a writeback on the extended range, and then writing to random
// pages in the extended range before ending the writeback.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackIntersectingRandomWrites, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  constexpr uint64_t kExtendedLen = 40;
  ASSERT_TRUE(vmo->Resize(1 + kExtendedLen));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, kExtendedLen, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, kExtendedLen));

  // Write to random pages in the range. Compute the ranges to write to.
  std::vector<uint64_t> offsets, lengths;
  uint64_t len = 0;
  for (size_t i = 1; i <= kExtendedLen; i++) {
    bool write = rand() % 2;
    if (write) {
      // Begin a new range if we're not accumulated any pages to write yet.
      if (len == 0) {
        offsets.push_back(i);
      }
      // Add to the running length.
      len++;
    } else if (len > 0) {
      // If there's a break in the run of pages to be written, store the length accumulated so far
      // and reset.
      lengths.push_back(len);
      len = 0;
    }
  }
  if (len > 0) {
    lengths.push_back(len);
  }
  ASSERT_EQ(offsets.size(), lengths.size());

  TestThread t1([&vmo, &offsets, &lengths]() -> bool {
    for (size_t i = 0; i < offsets.size(); i++) {
      const std::vector<uint8_t> kFillData(lengths[i] * zx_system_get_page_size(), 0xaa);
      zx_status_t status = vmo->vmo().write(
          kFillData.data(), offsets[i] * zx_system_get_page_size(), kFillData.size());
      if (status != ZX_OK) {
        return false;
      }
    }
    return true;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    for (size_t i = 0; i < offsets.size(); i++) {
      ASSERT_TRUE(pager.WaitForPageDirty(vmo, offsets[i], lengths[i], ZX_TIME_INFINITE));
      ASSERT_TRUE(pager.DirtyPages(vmo, offsets[i], lengths[i]));
    }
  }
  ASSERT_TRUE(t1.Wait());

  // End the writeback that we began previously.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, kExtendedLen));

  // We should not have been able to clean the dirties pages but we should be able to clean the zero
  // ranges remaining.
  std::vector<zx_vmo_dirty_range_t> ranges(
      std::from_range,
      std::views::transform(std::views::zip(offsets, lengths), [](const auto& elt) {
        const auto& [offset, length] = elt;
        return zx_vmo_dirty_range_t{
            .offset = offset,
            .length = length,
            // No dirty range will be a zero range.
            .options = 0,
        };
      }));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges.data(), ranges.size()));

  // Now attempt a writeback again for the entire VMO.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, kExtendedLen + 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, kExtendedLen + 1));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests extending the VMO, starting a writeback on only a portion of the extended range, and then
// writing to random pages in the extended range before ending the writeback.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizePartialWritebackIntersectingRandomWrites, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  constexpr uint64_t kExtendedLen = 40;
  ASSERT_TRUE(vmo->Resize(1 + kExtendedLen));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, kExtendedLen, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for only a portion of the dirty zero range.
  constexpr uint64_t kWritebackLen = kExtendedLen / 2;
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, kWritebackLen));

  // Write to random pages in the range. Compute the ranges to write to.
  std::vector<uint64_t> offsets, lengths;
  uint64_t len = 0;
  for (size_t i = 1; i <= kExtendedLen; i++) {
    bool write = rand() % 2;
    if (write) {
      // Begin a new range if we're not accumulated any pages to write yet.
      if (len == 0) {
        offsets.push_back(i);
      }
      // Add to the running length.
      len++;
    } else if (len > 0) {
      // If there's a break in the run of pages to be written, store the length accumulated so far
      // and reset.
      lengths.push_back(len);
      len = 0;
    }
  }
  if (len > 0) {
    lengths.push_back(len);
  }
  ASSERT_EQ(offsets.size(), lengths.size());

  TestThread t1([&vmo, &offsets, &lengths]() -> bool {
    for (size_t i = 0; i < offsets.size(); i++) {
      const std::vector<uint8_t> kFillData(lengths[i] * zx_system_get_page_size(), 0xaa);
      zx_status_t status = vmo->vmo().write(
          kFillData.data(), offsets[i] * zx_system_get_page_size(), kFillData.size());
      if (status != ZX_OK) {
        return false;
      }
    }
    return true;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    for (size_t i = 0; i < offsets.size(); i++) {
      ASSERT_TRUE(pager.WaitForPageDirty(vmo, offsets[i], lengths[i], ZX_TIME_INFINITE));
      ASSERT_TRUE(pager.DirtyPages(vmo, offsets[i], lengths[i]));
    }
  }
  ASSERT_TRUE(t1.Wait());

  // End the writeback that we began previously.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, kWritebackLen));

  // We should not have been able to clean the dirtied pages but we should be able to clean the zero
  // ranges that fell in the written back range.
  zx_vmo_dirty_range_t ranges[kExtendedLen];
  size_t index = 0;
  for (size_t i = 0; i < offsets.size(); i++) {
    if (offsets[i] > kWritebackLen + 1) {
      uint64_t prev_end = index == 0 ? 0 : ranges[index - 1].offset + ranges[index - 1].length;
      ranges[index].offset = std::max(prev_end, kWritebackLen + 1);
      ranges[index].length = offsets[i] - ranges[index].offset;
      ranges[index].options = ZX_VMO_DIRTY_RANGE_IS_ZERO;
      index++;
    }
    ranges[index].offset = offsets[i];
    ranges[index].length = lengths[i];
    ranges[index].options = 0;
    index++;
  }
  auto last_range = ranges[index - 1];
  if (last_range.offset + last_range.length < kExtendedLen + 1) {
    ranges[index].offset = last_range.offset + last_range.length;
    ranges[index].length = kExtendedLen + 1 - ranges[index].offset;
    ranges[index].options = ZX_VMO_DIRTY_RANGE_IS_ZERO;
    index++;
  }
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, index));

  // Now attempt a writeback again for the entire VMO.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, kExtendedLen + 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, kExtendedLen + 1));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resize interleaved with a writeback trims / resets an awaiting clean zero range if
// it intersects it.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackIntersectingResize, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify VMO contents.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Start writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 2));

  // Resize the VMO down, so that part of the dirty range is still valid.
  ASSERT_TRUE(vmo->Resize(2));

  // Verify that the second page is still dirty.
  range.length = 1;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Try to end the writeback that we began previously. This should fail as it is out of bounds.
  ASSERT_FALSE(pager.WritebackEndPages(vmo, 1, 2));

  // Verify that the second page is still dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // End the writeback with the correct length.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 1));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO up again.
  ASSERT_TRUE(vmo->Resize(3));

  // Newly extended range should be dirty and zero.
  range = {2, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Supply the second page as it has already been written back, and the user pager is expected to
  // supply it.
  // TODO(rashaeqbal): Supply with zeros once we have a quick OP_SUPPLY_ZERO. For now just supply
  // non-zero content; the content is irrelevant for this test.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  vmo->GenerateBufferContents(expected.data() + zx_system_get_page_size(), 1, 1);

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Start writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 2, 1));

  // Resize the VMO down, so that the entire dirty range is invalid.
  ASSERT_TRUE(vmo->Resize(2));

  // No pages should be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Ending the writeback we began should fail as it is out of bounds.
  ASSERT_FALSE(pager.WritebackEndPages(vmo, 2, 1));

  // All pages are clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Resize the VMO up again.
  ASSERT_TRUE(vmo->Resize(3));

  // Newly extended range should be dirty and zero.
  range = {2, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Start writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 2, 1));

  // Resize the VMO down even further to before the start of the dirty range.
  ASSERT_TRUE(vmo->Resize(1));

  // No pages should be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Ending the writeback we began should fail as it is out of bounds.
  ASSERT_FALSE(pager.WritebackEndPages(vmo, 2, 1));

  // All pages are clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Verify VMO contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resize beyond an awaiting clean zero range does not affect it.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackNonIntersectingResize, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for a portion of the range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 1));

  // Resize the VMO down, so that the new size falls beyond the awaiting clean range.
  ASSERT_TRUE(vmo->Resize(2));

  // Verify that the second page is still dirty.
  range.length = 1;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Try to end the writeback that we began previously. This should succeed as the resize did not
  // affect it.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 1));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writeback on a resized range that starts after a gap (zero range) is handled.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackAfterGap, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to page 2 leaving a gap.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Attempt writeback page 2, leaving a gap at 1.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 2, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 2, 1));

  // We should be able to clean this page, leaving only one dirty range.
  range.length = 1;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Since we cleaned the page, we should see a DIRTY request on another write (if applicable).
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xbb>();
    return vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t2.Wait());

  // Verify dirty ranges. Page 2 should be dirty again.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Verify VMO contents.
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xbb, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writeback on a resized range with multiple zero ranges (gaps) can clean all the gaps.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackMulipleGaps, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(6));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 5, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to pages 2 and 4, leaving gaps at 1, 3, and 5.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    if (vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(), kFillData.size_bytes()) !=
        ZX_OK) {
      return false;
    }
    return vmo->vmo().write(kFillData.data(), 4 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(6 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges_before[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                          {2, 1, 0},
                                          {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                          {4, 1, 0},
                                          {5, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges_before,
                                      sizeof(ranges_before) / sizeof(zx_vmo_dirty_range_t)));

  // Begin writeback for all the dirty pages.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 5));

  // Writing to the AwaitingClean pages should trigger DIRTY requests, and so should writing to
  // gaps.
  TestThread t2([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<2, 0xbb>();
    return vmo->vmo().write(kFillData.data(), 3 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 2, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 3, 2));
  }
  ASSERT_TRUE(t2.Wait());

  // Complete the writeback we started.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 5));

  // We should have been able to clean everything except the pages we just dirtied.
  range = {3, 2, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  memset(expected.data() + 3 * zx_system_get_page_size(), 0xbb, 2 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests starting multiple sequential writebacks on the resized range, both for gaps and pages.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackSequential, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(6));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 5, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to pages 2 and 4, leaving gaps at 1, 3, and 5.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    if (vmo->vmo().write(kFillData.data(), 2 * zx_system_get_page_size(), kFillData.size_bytes()) !=
        ZX_OK) {
      return false;
    }
    return vmo->vmo().write(kFillData.data(), 4 * zx_system_get_page_size(),
                            kFillData.size_bytes()) == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(6 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {2, 1, 0},
                                   {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {4, 1, 0},
                                   {5, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Begin writeback for all the dirty ranges.
  for (auto range : ranges) {
    if (range.options == ZX_VMO_DIRTY_RANGE_IS_ZERO) {
      ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, range.offset, range.length));
    } else {
      ASSERT_TRUE(pager.WritebackBeginPages(vmo, range.offset, range.length));
    }
  }

  // End writeback for all the dirty ranges.
  for (auto range : ranges) {
    ASSERT_TRUE(pager.WritebackEndPages(vmo, range.offset, range.length));
  }

  // All pages should be clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a WritebackBegin on a resized range followed by a partial WritebackEnd works as
// expected.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackPartialEnd, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(5));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Writeback only a portion of the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 1));

  // Verify that the written back portion has been cleaned.
  range = {2, 3, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Writeback another portion of the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 2, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 2, 1));

  // Verify that the written back portion has been cleaned.
  range = {3, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Writeback the remaining portion of the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 3, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 3, 2));

  // Verify that all pages are clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests repeated writebacks on a resized range.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackRepeated, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(5));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Start writeback for the entire zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 4));

  // Start another writeback but for a smaller sub-range. This should not override the previous
  // writeback.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 2));

  // Now try to end the first writeback we started.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 4));

  // We should have been able to clean all the pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO again so we have another dirty zero range.
  ASSERT_TRUE(vmo->Resize(10));

  // Newly extended range should be dirty and zero.
  range = {5, 5, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // End the second writeback we started. This should be a no-op.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 2));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Attempting to end the writeback without starting another one should have no effect.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 5, 2));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Begin another writeback.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 5, 2));
  // Starting a redundant writeback for the same range should be a no-op.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 5, 2));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Begin another writeback for the remaining range. This should have no effect.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 7, 3));

  // End the first writeback.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 5, 2));

  // End the second writeback.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 7, 3));

  // Only the first portion should have been cleaned.
  range.offset = 7;
  range.length = 3;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // End the redundant writeback we started. This should be a no-op.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 5, 2));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Writeback the remaining range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 7, 3));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 7, 3));

  // Verify that all pages are clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resized range that has mappings can be written back as expected.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackWithMapping, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(2));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Map the resized VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS, 0,
                                       vmo->vmo(), 0, 2 * zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, 2 * zx_system_get_page_size());
  });

  // Commit a page in the resized range.
  TestThread t1([ptr]() -> bool {
    uint8_t data = 0xaa;
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    buf[zx_system_get_page_size()] = data;
    return true;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify dirty ranges and VMO contents.
  range.options = 0;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + zx_system_get_page_size(), 0xaa, sizeof(uint8_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Writeback the VMO.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 2));

  // Verify that all pages are clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Trying to write to the committed page again should trap as write permissions will have been
  // cleared.
  TestThread t2([ptr]() -> bool {
    uint8_t data = 0xbb;
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    buf[zx_system_get_page_size()] = data;
    return true;
  });

  ASSERT_TRUE(t2.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  }
  ASSERT_TRUE(t2.Wait());

  // The page should now be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify VMO contents.
  memset(expected.data() + zx_system_get_page_size(), 0xbb, sizeof(uint8_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a resized range that has mappings and is in the process of being written back is
// dirtied again on a write.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackInterleavedWriteWithMapping, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(6));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 5, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Map the resized VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS, 0,
                                       vmo->vmo(), 0, 6 * zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, 6 * zx_system_get_page_size());
  });

  // Begin a writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 5));

  // Write to two pages in the resized range leaving gaps.
  TestThread t1([ptr]() -> bool {
    uint8_t data = 0xaa;
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    buf[2 * zx_system_get_page_size()] = data;
    buf[4 * zx_system_get_page_size()] = data;
    return true;
  });

  ASSERT_TRUE(t1.Start());
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges1[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                    {2, 1, 0},
                                    {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                    {4, 1, 0},
                                    {5, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(
      pager.VerifyDirtyRanges(vmo, ranges1, sizeof(ranges1) / sizeof(zx_vmo_dirty_range_t)));

  // Verify VMO contents.
  std::vector<uint8_t> expected(6 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xaa, sizeof(uint8_t));
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xaa, sizeof(uint8_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // We should be able to write to the two committed pages again without blocking as they were
  // dirtied after beginning the writeback.
  uint8_t data = 0xbb;
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  buf[2 * zx_system_get_page_size()] = data;
  buf[4 * zx_system_get_page_size()] = data;

  // Verify dirty ranges and VMO contents.
  ASSERT_TRUE(
      pager.VerifyDirtyRanges(vmo, ranges1, sizeof(ranges1) / sizeof(zx_vmo_dirty_range_t)));
  memset(expected.data() + 2 * zx_system_get_page_size(), 0xbb, sizeof(uint8_t));
  memset(expected.data() + 4 * zx_system_get_page_size(), 0xbb, sizeof(uint8_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // End the writeback we started previously. We should only have been able to clean the gaps.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 5));
  zx_vmo_dirty_range_t ranges2[] = {{2, 1, 0}, {4, 1, 0}};
  ASSERT_TRUE(
      pager.VerifyDirtyRanges(vmo, ranges2, sizeof(ranges2) / sizeof(zx_vmo_dirty_range_t)));

  // Try to write to a gap. This should block as well.
  TestThread t3([ptr]() -> bool {
    uint8_t data = 0xdd;
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    buf[3 * zx_system_get_page_size()] = data;
    return true;
  });
  ASSERT_TRUE(t3.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 3, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 3, 1));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 3, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 3, 1));
  }
  ASSERT_TRUE(t3.Wait());

  // Verify dirty ranges.
  range = {2, 3, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Verify VMO contents.
  vmo->GenerateBufferContents(expected.data() + 3 * zx_system_get_page_size(), 1, 3);
  memset(expected.data() + 3 * zx_system_get_page_size(), 0xdd, sizeof(uint8_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // Writeback the dirty ranges.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 2, 3));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 2, 3));

  // All pages should be clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 6, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing a page after a dirty zero range is queried but before it is written back is
// left dirty.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ResizeWritebackDirtyAfterQuery, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 3, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write a page in the dirty zero range so that a page is committed.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, 2 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Writeback the dirty zero range we previously queried, explicitly stating that we will be
  // writing back zeroes.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 3));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 3));

  // The writeback should have left the dirty (non-zero) page dirty.
  range = {2, 1, 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Verifies that multiple dirty ranges (zero and non-zero) can be written back together.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackCombined, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(10));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 9, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Dirty a few pages so that the zero range is fragmented. Pages dirtied are {1, 6, 9}.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    if (vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 6 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    return vmo->vmo().write(&data, 9 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 6, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 6, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 9, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 9, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, 0},
                                   {2, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {6, 1, 0},
                                   {7, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {9, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Writeback the entire range together.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 9));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 9));

  // All dirty ranges have been cleaned.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Verifies that writing back dirty ranges (both zero and non-zero) out of order cleans everything.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackOutOfOrder, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(10));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 9, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Dirty a few pages so that the zero range is fragmented. Pages dirtied are {1, 6, 9}.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    if (vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 6 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    return vmo->vmo().write(&data, 9 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 6, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 6, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 9, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 9, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, 0},
                                   {2, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {6, 1, 0},
                                   {7, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {9, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Writeback the dirty ranges out of order.
  size_t indices[] = {3, 0, 2, 1, 4};
  // Beging all the writebacks first.
  for (auto i : indices) {
    ASSERT_TRUE(pager.WritebackBeginPages(vmo, ranges[i].offset, ranges[i].length));
  }
  // End all the writebacks.
  for (auto i : indices) {
    ASSERT_TRUE(pager.WritebackEndPages(vmo, ranges[i].offset, ranges[i].length));
  }

  // All dirty ranges have been cleaned.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Verifies starting writeback on ranges out of order and ending all together.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackOutOfOrderCombinedEnd, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(10));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 9, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Dirty a few pages so that the zero range is fragmented. Pages dirtied are {1, 6, 9}.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    if (vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 6 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    return vmo->vmo().write(&data, 9 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 6, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 6, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 9, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 9, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, 0},
                                   {2, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {6, 1, 0},
                                   {7, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {9, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Writeback the dirty ranges out of order.
  size_t indices[] = {3, 0, 2, 1, 4};
  // Begin writeback for all the dirty ranges, and end writeback with a single call over the range.
  for (auto i : indices) {
    ASSERT_TRUE(pager.WritebackBeginPages(vmo, ranges[i].offset, ranges[i].length));
  }
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 9));

  // All dirty ranges have been cleaned.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Verifies starting writeback on ranges all together and ending out of order.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackOutOfOrderCombinedBegin, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(10));

  // Newly extended range should be dirty and zero.
  zx_vmo_dirty_range_t range = {1, 9, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Dirty a few pages so that the zero range is fragmented. Pages dirtied are {1, 6, 9}.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    if (vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 6 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    return vmo->vmo().write(&data, 9 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 6, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 6, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 9, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 9, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, 0},
                                   {2, 4, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {6, 1, 0},
                                   {7, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {9, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Writeback the dirty ranges out of order.
  size_t indices[] = {3, 0, 2, 1, 4};
  // Begin writeback with a single call and then end writeback for each range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 9));
  for (auto i : indices) {
    ASSERT_TRUE(pager.WritebackEndPages(vmo, ranges[i].offset, ranges[i].length));
  }

  // All dirty ranges have been cleaned.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Test that OP_ZERO writes zeros in a pager-backed VMO.
TEST(PagerWriteback, OpZero) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(3, ZX_VMO_RESIZABLE, &vmo));
  // Supply only one page and let the others be faulted in as required.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Verify VMO contents for the supplied page.
  std::vector<uint8_t> expected(4 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Verify that no pages are dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Zero the first two pages.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, 2 * zx_system_get_page_size(), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should be able to zero without a read request.
  ASSERT_TRUE(t.Wait());

  // Verify that the contents are zero.
  memset(expected.data(), 0, 2 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Verify that zero content is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Zero the tail end of the VMO that was newly extended. This should be a no-op as it is already
  // zero.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 3 * zx_system_get_page_size(),
                                zx_system_get_page_size(), nullptr, 0));

  // No pages should be committed as we should have been able to replace the supplied pages with
  // zero intervals.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0u, info.committed_bytes);

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t ranges[] = {{0, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {3, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(check_buffer_data(vmo, 3, 1, expected.data(), true));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Test OP_ZERO on a pager-backed VMO created with ZX_VMO_TRAP_DIRTY.
TEST(PagerWriteback, OpZeroTrapDirty) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, ZX_VMO_RESIZABLE | ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Verify VMO contents.
  std::vector<uint8_t> expected(4 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Verify that no pages are dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Zero the first page.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should be able to zero without a dirty request.
  ASSERT_TRUE(t.Wait());

  // Verify that the contents are zero.
  memset(expected.data(), 0, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // Verify that zero content is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Zero the tail end of the VMO that was newly extended. This should be a no-op as it is already
  // zero.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 2 * zx_system_get_page_size(),
                                2 * zx_system_get_page_size(), nullptr, 0));

  // Only the page that we supplied previously but did not zero should be committed in the VMO.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Verify dirty ranges and VMO contents.
  zx_vmo_dirty_range_t ranges[] = {{0, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO},
                                   {2, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test that OP_ZERO is a no-op over a newly extended (but not written back yet) uncommitted range.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroTail, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 2, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Only the single page we supplied previously should be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Zero the newly extended range.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(),
                                2 * zx_system_get_page_size(), nullptr, 0));

  // This should be a no-op and not alter the VMO's pages.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Only the single page we supplied previously should be committed.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test that OP_ZERO can decommit committed pages in a newly extended (but not written back yet)
// range.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroDecommit, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 2, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to a page in the newly extended range leaving a gap
  uint8_t data = 0xaa;
  TestThread t1([vmo, data]() -> bool {
    return vmo->vmo().write(&data, 2 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents and dirty pages.
  memset(expected.data() + 2 * zx_system_get_page_size(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Check that two pages are committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(2 * zx_system_get_page_size(), info.committed_bytes);

  // Now zero the entire VMO.
  TestThread t2([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, 3 * zx_system_get_page_size(), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  // We should be able to zero without generating any more DIRTY requests because the tail can
  // simply be advanced from 1 (set during the resize) to 0, indicating that everything from offset
  // 0 is dirty and filled with zeros.
  ASSERT_TRUE(t2.Wait());

  // Verify that the VMO is now all zeros.
  memset(expected.data(), 0, 3 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));

  // We should have been able to decommit all the pages.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0u, info.committed_bytes);

  // Verify dirty ranges.
  range = {0, 3, ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test OP_ZERO on a clone of a pager-backed VMO.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroClone, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  // Supply one page.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));

  // Create a clone and zero it entirely.
  auto clone1 = vmo->Clone();
  ASSERT_NOT_NULL(clone1);
  ASSERT_OK(clone1->vmo().op_range(ZX_VMO_OP_ZERO, 0, 2 * zx_system_get_page_size(), nullptr, 0));

  // No page requests were seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify that the clone reads zeros.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  ASSERT_TRUE(check_buffer_data(clone1.get(), 0, 2, expected.data(), true));

  // Verify that the parent is unaltered. Only one page should have been committed as we supplied
  // that previously. Zero'ing the other page in the clone should have proceeded without committing
  // the page in the parent.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);
  vmo->GenerateBufferContents(expected.data() + zx_system_get_page_size(), 1, 1);
  ASSERT_TRUE(check_buffer_data(vmo, 1, 1, expected.data() + zx_system_get_page_size(), true));

  // No pages should be dirty in the parent.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The clone does not support dirty pages.
  ASSERT_FALSE(pager.VerifyDirtyRanges(clone1.get(), nullptr, 0));

  // Create another clone and this time only zero a portion of it - an unsupplied page.
  auto clone2 = vmo->Clone();
  ASSERT_NOT_NULL(clone2);
  ASSERT_OK(clone2->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // No page requests were seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify that the clone reads the zeroed page as zero but is still able to see the other page
  // from the parent.
  ASSERT_TRUE(check_buffer_data(clone2.get(), 0, 2, expected.data(), true));

  // Verify that the parent is unaltered.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);
  ASSERT_TRUE(check_buffer_data(vmo, 1, 1, expected.data() + zx_system_get_page_size(), true));

  // No pages should be dirty in the parent.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The clone does not support dirty pages.
  ASSERT_FALSE(pager.VerifyDirtyRanges(clone2.get(), nullptr, 0));

  // Supply the remaining page in the parent.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Create another clone and zero only a portion of it - a supplied page this time.
  auto clone3 = vmo->Clone();
  ASSERT_NOT_NULL(clone3);
  ASSERT_OK(clone3->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // No page requests were seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify that the clone reads the zeroed page as zero but is still able to see the other page
  // from the parent.
  ASSERT_TRUE(check_buffer_data(clone3.get(), 0, 2, expected.data(), true));

  // Verify the parent's contents.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(2 * zx_system_get_page_size(), info.committed_bytes);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // No pages should be dirty in the parent.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The clone does not support dirty pages.
  ASSERT_FALSE(pager.VerifyDirtyRanges(clone3.get(), nullptr, 0));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test OP_ZERO that conflicts with a simultaneous resize.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroResize, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(4, create_option, &vmo));
  // Supply a page.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Verify VMO contents for the supplied page.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // Verify that no pages are dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Start the zeroing partway into page 1, so we can block on a page request for page 1.
  TestThread t1([vmo]() -> bool {
    zx_status_t status =
        vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() + sizeof(uint64_t),
                            2 * zx_system_get_page_size(), nullptr, 0);
    return status == ZX_ERR_OUT_OF_RANGE;
  });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));

  // End the zeroing partway into page 2, so we can block on a page request for page 2.
  TestThread t2([vmo]() -> bool {
    zx_status_t status =
        vmo->vmo().op_range(ZX_VMO_OP_ZERO, 2 * zx_system_get_page_size(),
                            zx_system_get_page_size() - sizeof(uint64_t), nullptr, 0);
    return status == ZX_ERR_OUT_OF_RANGE;
  });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 2, 1, ZX_TIME_INFINITE));

  // While the threads are blocked on the page requests, shrink the VMO so that the pages they're
  // requesting are no longer valid. This should unblock the waiting threads and their OP_ZERO
  // should fail with ZX_ERR_OUT_OF_RANGE.
  ASSERT_TRUE(vmo->Resize(1));
  ASSERT_TRUE(t1.Wait());
  ASSERT_TRUE(t2.Wait());

  // Verify VMO contents for the remaining page.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more page requests were seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test OP_ZERO on partial pages.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroPartialPage, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(2));

  // Verify VMO contents and dirty pages.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Zero a few bytes in the middle of first page.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, sizeof(uint64_t), sizeof(uint64_t), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Verify VMO contents.
  memset(expected.data() + sizeof(uint64_t), 0, sizeof(uint64_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // The first page should also be dirty now.
  zx_vmo_dirty_range_t ranges[] = {{0, 1, 0}, {1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Zero a range starting partway into the first page and ending before the end of the second page.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() - sizeof(uint64_t),
                                zx_system_get_page_size(), nullptr, 0));

  // Verify VMO contents.
  memset(expected.data() + zx_system_get_page_size() - sizeof(uint64_t), 0, sizeof(uint64_t));
  // Verify dirty ranges.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that OP_ZERO just before the tail can efficiently expand the tail and avoid page requests.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroExpandsTail, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));

  // No dirty pages yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(3));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  ASSERT_TRUE(check_buffer_data(vmo, 2, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 2, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Zero the second page. We should be able to perform this zeroing efficiently without having to
  // send any page requests.
  TestThread t1([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(), zx_system_get_page_size(),
                               nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t1.Wait());

  // Verify VMO contents and dirty pages.
  ASSERT_TRUE(check_buffer_data(vmo, 1, 2, expected.data(), true));
  range.offset = 1;
  range.length = 2;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Zero the first page partially. Since this is a partial page zero, this will generate page
  // requests.
  TestThread t2([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() - sizeof(uint64_t),
                               sizeof(uint64_t), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // No more page requests seen.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  ASSERT_TRUE(t2.Wait());

  // Verify VMO contents and dirty pages.
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data() + zx_system_get_page_size() - sizeof(uint64_t), 0, sizeof(uint64_t));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 3, expected.data(), true));
  zx_vmo_dirty_range_t ranges[] = {{0, 1, 0}, {1, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));
}

// Tests OP_ZERO with interleaved writeback.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroWriteback, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(2));

  // Verify VMO contents and dirty pages.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Begin writeback for the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 1));

  // Zero the first page while the writeback is in progress.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // Try to end the writeback we started. This will be a no-op.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 1));

  // Verify VMO contents and dirty pages.
  memset(expected.data(), 0, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  range = {.offset = 0, .length = 2, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Writeback the dirty zero range.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 0, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 2));

  // No dirty pages remain.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No page requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests OP_ZERO over zero page markers.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroWithMarkers, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(5, create_option, &vmo));

  // Supply with empty pages so we have zero markers. Insert zero markers at the tail as well as in
  // the middle with a gap.
  zx::vmo empty_src;
  ASSERT_OK(zx::vmo::create(2 * zx_system_get_page_size(), 0, &empty_src));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), zx_system_get_page_size(),
                                       zx_system_get_page_size(), empty_src, 0));
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 3 * zx_system_get_page_size(),
                                       2 * zx_system_get_page_size(), empty_src, 0));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(5 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  vmo->GenerateBufferContents(expected.data() + 2 * zx_system_get_page_size(), 1, 2);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 5, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Zero the marker in the middle. This should be a no-op.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(),
                                zx_system_get_page_size(), nullptr, 0));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify VMO contents and dirty pages.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 5, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Zero the markers at the end. This should also be a no-op.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 3 * zx_system_get_page_size(),
                                2 * zx_system_get_page_size(), nullptr, 0));

  // No page requests seen.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify VMO contents and dirty pages.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 5, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
}

// Tests that zeroing across a pinned page clips expansion of the tail.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroPinned, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(3, create_option, &vmo));

  // Supply the first two pages.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Pin a supplied page.
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo->vmo(), zx_system_get_page_size(),
                    zx_system_get_page_size(), &addr, 1, &pmt));

  auto unpin = fit::defer([&pmt]() {
    if (pmt) {
      pmt.unpin();
    }
  });

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(4));

  // Verify dirty pages.
  zx_vmo_dirty_range_t range = {.offset = 3, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Zero the VMO.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, 4 * zx_system_get_page_size(), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should only see a dirty request for the pinned page. The rest we should be able to zero.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  }
  ASSERT_TRUE(t.Wait());

  // No other page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify VMO contents and dirty pages.
  std::vector<uint8_t> expected(4 * zx_system_get_page_size(), 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 4, expected.data(), true));
  // Everything else except the pinned page is a zero range.
  zx_vmo_dirty_range_t ranges[] = {
      {0, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {1, 1, 0}, {2, 2, ZX_VMO_DIRTY_RANGE_IS_ZERO}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));
}

// Tests that zeroing the tail unblocks any previous read requests.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(OpZeroUnblocksReadRequest, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));

  // Supply the first page.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // No dirty ranges yet.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Read from the second (and last) page.
  std::vector<uint8_t> data(zx_system_get_page_size());
  TestThread t([vmo, &data]() -> bool {
    return vmo->vmo().read(data.data(), zx_system_get_page_size(), data.size()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));

  // Now zero the last page.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(),
                                zx_system_get_page_size(), nullptr, 0));

  // This should unblock the previous read request, as the kernel has been able to expand the tail
  // and will supply zeroes for this page from this point on.
  ASSERT_TRUE(t.Wait());

  // Verify VMO contents.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));

  // The last page should be dirty.
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The last page should have read as zeroes.
  ASSERT_EQ(0, memcmp(data.data(), expected.data() + zx_system_get_page_size(),
                      zx_system_get_page_size()));

  // No other page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that dirty pages can be written back after detach.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackDirtyPagesAfterDetach, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Write to a page.
  uint8_t data = 0xaa;
  TestThread t([vmo, data]() -> bool { return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK; });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page.
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t.Wait());

  // We should have committed the page.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // Verify that the page is still dirty.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Should be able to read the page and verify its contents.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  memset(expected.data(), data, sizeof(data));
  // We should be able to read the dirty range both through mappings and with a VMO read.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), false));

  // Writeback the page.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Verify that the page is clean now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a newly resized range can be written back after detach.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritebackResizedRangeAfterDetach, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));

  // Resize the VMO up and write a page leaving a gap.
  ASSERT_TRUE(vmo->Resize(3));

  uint8_t data = 0xbb;
  TestThread t([vmo, data]() -> bool {
    return vmo->vmo().write(&data, 2 * zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page.
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[] = {{1, 1, ZX_VMO_DIRTY_RANGE_IS_ZERO}, {2, 1, 0}};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Only the last page should be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // Everything beyond the original size is dirty so should remain intact.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, ranges, sizeof(ranges) / sizeof(zx_vmo_dirty_range_t)));

  // Verify VMO contents in the dirty range.
  std::vector<uint8_t> expected(3 * zx_system_get_page_size(), 0);
  memset(expected.data() + 2 * zx_system_get_page_size(), data, sizeof(data));
  // We should be able to read the dirty range both through mappings and with a VMO read.
  ASSERT_TRUE(check_buffer_data(vmo, 1, 2, expected.data() + zx_system_get_page_size(), true));
  ASSERT_TRUE(check_buffer_data(vmo, 1, 2, expected.data() + zx_system_get_page_size(), false));

  // Can writeback the dirty ranges.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 1, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 2));

  // No more dirty pages.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that clean pages are decommitted on detach.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(DecommitCleanOnDetach, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // We have one committed page.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // No dirty ranges.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // No dirty ranges.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No committed pages.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that DIRTY requests cannot be generated after detach.
VMO_VMAR_TEST(PagerWriteback, NoDirtyRequestsAfterDetach) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo1;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo1));
  ASSERT_TRUE(pager.SupplyPages(vmo1, 0, 1));

  // Verify that no pages are dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo1, nullptr, 0));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo1));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo1->key(), ZX_TIME_INFINITE));

  // Try to write to the VMO. As we are starting with a clean page, this would have generated a
  // DIRTY request pre-detach, but will now fail.
  if (check_vmar) {
    TestThread t1([vmo1]() -> bool {
      auto ptr = reinterpret_cast<uint8_t*>(vmo1->base_addr());
      *ptr = 0xaa;
      return true;
    });
    ASSERT_TRUE(t1.Start());
    ASSERT_TRUE(t1.WaitForCrash(vmo1->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    uint8_t data = 0xaa;
    ASSERT_EQ(ZX_ERR_BAD_STATE, vmo1->vmo().write(&data, 0, sizeof(data)));
  }

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo1, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo1, 0, &offset, &length));

  // No pages are dirty still.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo1, nullptr, 0));

  // Try again but this time with an AwaitingClean page, which would also have generated a DIRTY
  // request before the detach.
  Vmo* vmo2;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo2));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 0, 1));
  ASSERT_TRUE(pager.DirtyPages(vmo2, 0, 1));

  // Verify that the page is dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo2, &range, 1));

  // Being writeback, putting the page in AwaitingClean.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo2, 0, 1));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo2));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo2->key(), ZX_TIME_INFINITE));

  // Try to write to the VMO. This will fail.
  if (check_vmar) {
    TestThread t2([vmo2]() -> bool {
      auto ptr = reinterpret_cast<uint8_t*>(vmo2->base_addr());
      *ptr = 0xaa;
      return true;
    });
    ASSERT_TRUE(t2.Start());
    ASSERT_TRUE(t2.WaitForCrash(vmo2->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    uint8_t data = 0xaa;
    ASSERT_EQ(ZX_ERR_BAD_STATE, vmo2->vmo().write(&data, 0, sizeof(data)));
  }

  // The page is still dirty (AwaitingClean, but not clean yet).
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo2, &range, 1));

  // End the writeback. This should clean the page.
  ASSERT_TRUE(pager.WritebackEndPages(vmo2, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo2, nullptr, 0));

  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo2, 0, &offset, &length));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo2, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo2, 0, &offset, &length));
}

// Tests that detach with a pending DIRTY request fails the request.
VMO_VMAR_TEST(PagerWriteback, DetachWithPendingDirtyRequest) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // We have one committed page.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Try to write.
  TestThread t([vmo, check_vmar]() -> bool {
    uint8_t data = 0xaa;
    if (check_vmar) {
      auto ptr = reinterpret_cast<uint8_t*>(vmo->base_addr());
      *ptr = data;
      return true;
    }
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_ERR_BAD_STATE;
  });
  ASSERT_TRUE(t.Start());

  // Wait for the dirty request.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // The thread should terminate.
  if (check_vmar) {
    ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t.Wait());
  }

  // Verify that no pages are dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No pages are committed.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that resolving (or failing) a DIRTY request after the VMO is detached fails.
TEST(PagerWriteback, ResolveDirtyRequestAfterDetach) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_ERR_BAD_STATE;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Detach the VMO.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // The write should fail.
  ASSERT_TRUE(t.Wait());

  // No more requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // This should fail.
  ASSERT_FALSE(pager.FailPages(vmo, 0, 1));

  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // Dirtying the page should fail as well.
  ASSERT_FALSE(pager.DirtyPages(vmo, 0, 1));

  // The page was not dirtied.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is marked modified on a zx_vmo_write.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedOnVmoWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo->vmo().write(&data, 0, sizeof(data)));

  // The VMO should be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is marked modified when written through a mapping.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedOnMappingWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // Map the VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO via the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  uint8_t data = 0xbb;
  *buf = data;

  // The VMO should be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is marked modified on resize.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedOnResize, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been resized yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Resize the VMO down.
  ASSERT_TRUE(vmo->Resize(0));

  // The VMO should be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify dirty ranges.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the VMO up.
  ASSERT_TRUE(vmo->Resize(2));

  // The VMO should be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), 0, 2 * zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is marked modified on a ZX_VMO_OP_ZERO.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedOnOpZero, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Zero a page in the VMO.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // The VMO should be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), 0, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is not marked modified on a zx_vmo_read.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NotModifiedOnVmoRead, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Read from the VMO.
  uint8_t data;
  ASSERT_OK(vmo->vmo().read(&data, 0, sizeof(data)));

  // The VMO shouldn't be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is not marked modified when read through a mapping.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NotModifiedOnMappingRead, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Map the VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Read from the VMO via the mapping.
  auto buf = reinterpret_cast<uint8_t*>(ptr);
  uint8_t data = *buf;

  // The VMO shouldn't be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_EQ(*(uint8_t*)(expected.data()), data);

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is not marked modified when a write is failed by failing a DIRTY request.
TEST(PagerWriteback, NotModifiedOnFailedDirtyRequest) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Try to write to the VMO.
  TestThread t1([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the dirty request.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));
  ASSERT_TRUE(t1.WaitForFailure());

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Map the VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Try to write to the VMO via the mapping.
  TestThread t2([ptr]() -> bool {
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    *buf = 0xbb;
    return true;
  });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the dirty request.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));
  ASSERT_TRUE(t2.WaitForCrash(ptr, ZX_ERR_IO));

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is not marked modified on a failed zx_vmo_write.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NotModifiedOnFailedVmoWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO with the source buffer set up such that the copying fails. Make the source
  // buffer pager backed too, and fail reads from it.
  Vmo* src_vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &src_vmo));

  // Map the source VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, src_vmo->vmo(), 0,
                                       zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  // Attempt the VMO write.
  TestThread t([vmo, ptr]() -> bool {
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    return vmo->vmo().write(buf, 0, sizeof(uint8_t)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should see a read request when the VMO write attempts reading from the source VMO.
  ASSERT_TRUE(pager.WaitForPageRead(src_vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the read request so that the write fails.
  ASSERT_TRUE(pager.FailPages(src_vmo, 0, 1));
  ASSERT_TRUE(t.WaitForFailure());

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  // We mark pages dirty when they are looked up, i.e. *before* writing to them, so they will still
  // be reported as dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is not marked modified on a failed resize.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NotModifiedOnFailedResize, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been resized yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page.
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo->vmo(), zx_system_get_page_size(),
                    zx_system_get_page_size(), &addr, 1, &pmt));

  auto unpin = fit::defer([&pmt]() {
    if (pmt) {
      pmt.unpin();
    }
  });

  // Try to resize down across the pinned page. The resize should fail.
  ASSERT_EQ(vmo->vmo().set_size(zx_system_get_page_size()), ZX_ERR_BAD_STATE);

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a VMO is marked modified when a zx_vmo_write partially succeeds.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedOnPartialVmoWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the pages in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 2));
  }

  // Write to the VMO with the source buffer set up such that the copying partially fails. Make the
  // source buffer pager backed too, and fail reads from it.
  Vmo* src_vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &src_vmo));
  // Supply a single page in the source, so we can partially read from it.
  ASSERT_TRUE(pager.SupplyPages(src_vmo, 0, 1));

  // Map the source VMO.
  zx_vaddr_t ptr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, src_vmo->vmo(), 0,
                                       2 * zx_system_get_page_size(), &ptr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, 2 * zx_system_get_page_size());
  });

  // Attempt the VMO write.
  TestThread t([vmo, ptr]() -> bool {
    auto buf = reinterpret_cast<uint8_t*>(ptr);
    return vmo->vmo().write(buf, 0, 2 * zx_system_get_page_size()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should see a read request when the VMO write attempts reading from the source VMO.
  ASSERT_TRUE(pager.WaitForPageRead(src_vmo, 1, 1, ZX_TIME_INFINITE));

  // Fail the read request so that the write fails.
  ASSERT_TRUE(pager.FailPages(src_vmo, 1, 1));
  ASSERT_TRUE(t.WaitForFailure());

  // The write will have failed, so VMO should not be marked as modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify dirty pages and contents.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  // We mark pages dirty when they are looked up, i.e. *before* writing to them, so they will still
  // be reported as dirty.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // We will now try a partial write by failing dirty requests, which is only relevant for
  // TRAP_DIRTY.
  if (!(create_option & ZX_VMO_TRAP_DIRTY)) {
    return;
  }

  // Start with clean pages again.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 2));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 2));

  // Dirty a single page, so that writing to the other generates a dirty request.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  // Try to write to the VMO.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<2, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // Should see a dirty request for page 1.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));

  // Fail the dirty request.
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1));
  ASSERT_TRUE(t1.WaitForFailure());

  // The write succeeded partially, so the VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  range.length = 1;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a clone cannot be marked modified.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NotModifiedCloneWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been written to, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Create a clone.
  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Write to the clone.
  const auto kFillData = vmo_test::TestFillPages<1, 0xcc>();
  ASSERT_OK(clone->vmo().write(kFillData.data(), 0, kFillData.size_bytes()));

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The clone should not support the modified query.
  zx_pager_vmo_stats_t stats;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_vmo_stats(pager.pager().get(), clone->vmo().get(),
                                                          0, &stats, sizeof(stats)));
  ASSERT_FALSE(pager.VerifyModified(clone.get()));

  // Verify clone contents.
  memcpy(expected.data(), kFillData.data(), kFillData.size_bytes());
  ASSERT_TRUE(check_buffer_data(clone.get(), 0, 1, expected.data(), true));
  ASSERT_FALSE(pager.VerifyDirtyRanges(clone.get(), nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that querying the modified state without the reset option does not reset.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ModifiedNoReset, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    // Dirty the page in preparation for the write, avoiding the need to trap.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // The VMO hasn't been written to yet, so it shouldn't be marked modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo->vmo().write(&data, 0, sizeof(data)));

  // Verify modified state without resetting it.
  zx_pager_vmo_stats_t stats;
  ASSERT_OK(
      zx_pager_query_vmo_stats(pager.pager().get(), vmo->vmo().get(), 0, &stats, sizeof(stats)));
  ASSERT_EQ(ZX_PAGER_VMO_STATS_MODIFIED, stats.modified);

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The VMO should still be marked modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state now with the reset option should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that pinning a page for read does not dirty it and does not mark the VMO modified.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(PinForRead, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page for read.
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo->vmo(), 0, zx_system_get_page_size(), &addr, 1, &pmt));

  auto unpin = fit::defer([&pmt]() {
    if (pmt) {
      pmt.unpin();
    }
  });

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that pinning a page for write dirties it and marks the VMO modified.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(PinForWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // If we're trapping dirty transitions, the pin will generate a DIRTY request.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a page cannot be marked clean while it is pinned.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(PinnedWriteback, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // If we're trapping dirty transitions, the pin will generate a DIRTY request.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Try to writeback the VMO. Since it is still pinned, this will be a no-op.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Unpin the VMO and attempt writeback again.
  ASSERT_TRUE(page_pinner->ExplicitUnpin());

  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // The VMO should now be clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests pinned read with interleaved writeback.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(ReadPinAwaitingClean, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  TestThread t([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t.Wait());

  // Verify contents and dirty ranges.
  memset(expected.data(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Begin writeback on the dirty range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // Pin a page for read.
  zx::pmt pmt;
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo->vmo(), 0, zx_system_get_page_size(), &addr, 1, &pmt));

  auto unpin = fit::defer([&pmt]() {
    if (pmt) {
      pmt.unpin();
    }
  });

  // End the writeback we started earlier. This should succeed.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // The VMO should now be clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests pinned write with interleaved writeback.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(WritePinAwaitingClean, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // Verify contents and dirty ranges.
  memset(expected.data(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Begin writeback on the dirty range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // Pin a page for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // If we're trapping dirty transitions, the pin will generate a DIRTY request.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // End the writeback we started earlier. This should not clean anything.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Unpin the VMO and attempt writeback again.
  ASSERT_TRUE(page_pinner->ExplicitUnpin());

  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // The VMO should now be clean.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests delayed pinned write with interleaved writeback.
TEST(PagerWriteback, DelayedPinAwaitingClean) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify contents and dirty ranges.
  memset(expected.data(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Begin writeback on the dirty range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // Try to pin for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // We should see a DIRTY request from the pin.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // End the writeback we started earlier. This should succeed as we haven't successfully pinned
  // for write yet.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Now succeed the pin attempt.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Should not be able to writeback while pinned.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests failed pin with interleaved writeback.
TEST(PagerWriteback, FailedPinAwaitingClean) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  TestThread t1([vmo]() -> bool {
    const auto kFillData = vmo_test::TestFillPages<1, 0xaa>();
    return vmo->vmo().write(kFillData.data(), 0, kFillData.size_bytes()) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify contents and dirty ranges.
  memset(expected.data(), 0xaa, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Begin writeback on the dirty range.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));

  // Try to pin for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // We should see a DIRTY request from the pin.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // End the writeback we started earlier. This should succeed as we haven't successfully pinned
  // for write yet.
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Fail the pin attempt.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));
  ASSERT_TRUE(page_pinner->WaitForPinFailure());

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that writing to a page after pinning does not generate additional DIRTY requests.
TEST(PagerWriteback, DirtyAfterPin) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // The pin will generate a DIRTY request.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to the VMO. This should not generate further DIRTY requests.
  uint8_t data = 0xaa;
  ASSERT_OK(vmo->vmo().write(&data, 0, sizeof(data)));

  // The VMO should be modified as we wrote to it again.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that pinning an already dirty page does not generate additional DIRTY requests.
TEST(PagerWriteback, PinAfterDirty) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Write to the VMO.
  uint8_t data = 0xaa;
  TestThread t([vmo, data]() -> bool { return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK; });

  // We should see a DIRTY request.
  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t.Wait());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Pin a page for write. This should not generate further DIRTY requests.
  zx::pmt pmt;
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo->vmo(), 0, zx_system_get_page_size(),
                    &addr, 1, &pmt));

  auto unpin = fit::defer([&pmt]() {
    if (pmt) {
      pmt.unpin();
    }
  });

  // No DIRTY requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));

  // The VMO should be modified as we wrote to it again.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that both READ and DIRTY requests are generated as expected when pinning an unpopulated
// range for write.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(PinForWriteUnpopulated, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  // Supply only one page so we can fault on the other.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin both pages for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), 2 * zx_system_get_page_size()));

  // If we're trapping dirty transitions, the pin will generate a DIRTY request for the page already
  // present.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }

  // We should see a READ request for the unpopulated page.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));

  // If we're trapping dirty transitions, the pin will generate a DIRTY request for the second page.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  }

  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a failed pin write does not mark the VMO modified.
TEST(PagerWriteback, NotModifiedFailedPinWrite) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin a page for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), zx_system_get_page_size()));

  // We should see a DIRTY request.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Fail the DIRTY request, so that the overall pin fails.
  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));
  ASSERT_TRUE(page_pinner->WaitForPinFailure());

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a pin write that fails part of the way does not mark the VMO modified.
TEST(PagerWriteback, NotModifiedPartialFailedPinWrite) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Pin both pages for write.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(vmo->vmo().borrow(), 2 * zx_system_get_page_size()));

  // We should see a DIRTY request for both pages.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 2, ZX_TIME_INFINITE));

  // Dirty one page but fail the other and wait for the overall pin to fail.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1));
  ASSERT_TRUE(page_pinner->WaitForPinFailure());

  // The VMO should not be modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests pinning for write through a slice.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(SlicePinWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Create a slice.
  zx::vmo slice;
  ASSERT_OK(vmo->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, 2 * zx_system_get_page_size(), &slice));

  // Pin both pages for write through a slice.
  zx::result<std::unique_ptr<ThreadedPagePinner>> maybe_page_pinner =
      ThreadedPagePinner::Create(iommu_resource);
  ASSERT_OK(maybe_page_pinner.status_value());

  const std::unique_ptr<ThreadedPagePinner> page_pinner = std::move(*maybe_page_pinner);
  ASSERT_TRUE(page_pinner->StartPinOperation(slice.borrow(), 2 * zx_system_get_page_size()));

  // If we're trapping dirty transitions, we should see a DIRTY request for both pages.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 2, ZX_TIME_INFINITE));
    // Dirty the pages and wait for the pin to succeed.
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 2));
  }

  ASSERT_TRUE(page_pinner->WaitForPin());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 2, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The slice itself cannot be modified.
  zx_pager_vmo_stats_t stats;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.pager().get(), slice.get(), 0, &stats, sizeof(stats)));
  uint64_t num_ranges = 0;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager.pager().get(), slice.get(), 0,
                                                             zx_system_get_page_size(), &range,
                                                             sizeof(range), &num_ranges, nullptr));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests writing to a VMO through a slice.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(SliceWrite, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Create a slice.
  zx::vmo slice;
  ASSERT_OK(vmo->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, zx_system_get_page_size(), &slice));

  // Write the slice directly.
  uint8_t data = 0xaa;
  TestThread t1([&slice, data]() -> bool { return slice.write(&data, 0, sizeof(data)) == ZX_OK; });
  ASSERT_TRUE(t1.Start());

  // If we're trapping dirty transitions, we should see a DIRTY request.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t1.Wait());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The slice itself cannot be modified.
  zx_pager_vmo_stats_t stats;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.pager().get(), slice.get(), 0, &stats, sizeof(stats)));
  uint64_t num_ranges = 0;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager.pager().get(), slice.get(), 0,
                                                             zx_system_get_page_size(), &range,
                                                             sizeof(range), &num_ranges, nullptr));
  // Clean the page.
  ASSERT_TRUE(pager.WritebackBeginPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Map the slice and then write via the mapping.
  zx_vaddr_t ptr;
  data = 0xbb;
  TestThread t2([&slice, &ptr, data]() -> bool {
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, slice, 0,
                                   zx_system_get_page_size(), &ptr) != ZX_OK) {
      fprintf(stderr, "could not map vmo\n");
      return false;
    }

    auto buf = reinterpret_cast<uint8_t*>(ptr);
    *buf = data;
    return true;
  });

  auto unmap = fit::defer([&ptr]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(ptr, zx_system_get_page_size());
  });

  ASSERT_TRUE(t2.Start());

  // If we're trapping dirty transitions, we should see a DIRTY request.
  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  }
  ASSERT_TRUE(t2.Wait());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), data, sizeof(data));
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The slice itself cannot be modified.
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.pager().get(), slice.get(), 0, &stats, sizeof(stats)));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager.pager().get(), slice.get(), 0,
                                                             zx_system_get_page_size(), &range,
                                                             sizeof(range), &num_ranges, nullptr));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests OP_ZERO on a slice.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(SliceOpZero, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 2, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Create a slice.
  zx::vmo slice;
  ASSERT_OK(vmo->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, 2 * zx_system_get_page_size(), &slice));

  // Zero a page in the slice.
  TestThread t([&slice]() -> bool {
    return slice.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // Even if we are required to TRAP_DIRTY, we should be able to write zeros without a dirty request
  // since we are creating a sparse range.
  ASSERT_TRUE(t.Wait());

  // The VMO should be modified.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify contents and dirty ranges.
  memset(expected.data(), 0, zx_system_get_page_size());
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The slice itself cannot be modified.
  zx_pager_vmo_stats_t stats;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.pager().get(), slice.get(), 0, &stats, sizeof(stats)));
  uint64_t num_ranges = 0;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager.pager().get(), slice.get(), 0,
                                                             zx_system_get_page_size(), &range,
                                                             sizeof(range), &num_ranges, nullptr));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests a racing resize while a commit is blocked on a page request.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(CommitResizeRace, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create the VMO and supply only one page. Let the commit fault the other one in.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // The VMO hasn't been modified yet.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Commit all the pages.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_COMMIT, 0, 2 * zx_system_get_page_size(), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should see a READ request for the unpopulated page.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));

  // Resize down the VMO invalidating the unpopulated page, so that the commit has no work to do
  // when woken up from the page request wait.
  ASSERT_TRUE(vmo->Resize(1));

  // Since the remaining page was already supplied, the commit should succeed.
  ASSERT_TRUE(t.Wait());

  // Resize should have modified the VMO.
  ASSERT_TRUE(pager.VerifyModified(vmo));
  // Querying the modified state should have reset the modified flag.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // Verify VMO contents and dirty ranges.
  std::vector<uint8_t> expected(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 1, expected.data(), true));
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests dirtying a large range at once. The core-tests run with random delayed PMM allocation, so
// by requiring a large number of pages to be allocated at once, we increase the likelihood of
// falling back to single page allocations and gradually accumulating the required number of pages.
TEST(PagerWriteback, DirtyLargeRange) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(100, ZX_VMO_TRAP_DIRTY | ZX_VMO_RESIZABLE, &vmo));

  // Empty source VMO to supply with zero pages.
  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(100 * zx_system_get_page_size(), 0, &vmo_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, 100 * zx_system_get_page_size(), vmo_src, 0));

  // Resize the VMO up so that we also need to add zero pages at the tail.
  ASSERT_TRUE(vmo->Resize(200));

  // No pages in the VMO yet.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // Dirty the entire VMO at once. This will allocate all 200 pages.
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 200));

  // All pages have been allocated and dirtied.
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(200 * zx_system_get_page_size(), info.committed_bytes);
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 200, .options = 0};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that committing both actual pages and zero page markers does not dirty the pages.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(NoDirtyOnCommit, 0) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, create_option, &vmo));

  // Supply an actual page and a zero page marker.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  zx::vmo empty_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &empty_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), zx_system_get_page_size(),
                                       zx_system_get_page_size(), empty_src, 0));

  // Commit the entire VMO.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_COMMIT, 0, 2 * zx_system_get_page_size(), nullptr, 0));

  // Both pages should be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(2 * zx_system_get_page_size(), info.committed_bytes);

  // No pages should be dirty.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // The VMO wasn't modified.
  ASSERT_FALSE(pager.VerifyModified(vmo));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that committing pages in the newly extended tail does not lose dirtiness.
TEST_WITH_AND_WITHOUT_TRAP_DIRTY(CommitExtendedTail, ZX_VMO_RESIZABLE) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, create_option, &vmo));

  // Verify dirty ranges.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Resize the vmo up.
  vmo->Resize(2);

  // Verify VMO contents and dirty ranges.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  ASSERT_TRUE(check_buffer_data(vmo, 1, 1, expected.data(), true));
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Commit the VMO. The existing page will be faulted in and the page beyond the old size will
  // transition from dirty zero to dirty non-zero.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_COMMIT, 0, 2 * zx_system_get_page_size(), nullptr, 0) ==
           ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  if (create_option & ZX_VMO_TRAP_DIRTY) {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  }

  ASSERT_TRUE(t.Wait());

  // Verify VMO contents and dirty ranges.
  vmo->GenerateBufferContents(expected.data(), 1, 0);
  ASSERT_TRUE(check_buffer_data(vmo, 0, 2, expected.data(), true));
  range.options = 0;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests a race between resolving a DIRTY request for a zero range and marking the zero range clean.
TEST(PagerWriteback, DelayedDirtyNotFound) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY | ZX_VMO_RESIZABLE, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Resize the VMO up so that there's a zero range.
  ASSERT_TRUE(vmo->Resize(2));

  // Verify that the resized range is dirty and zero.
  zx_vmo_dirty_range_t range = {.offset = 1, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Write to the zero range. This will generate a DIRTY request.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, zx_system_get_page_size(), sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));

  // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
  // now responsible for supplying all pages.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 1, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 1, 1));

  // Verify that no pages are dirty now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No new requests generated.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Now try to resolve the DIRTY request we saw earlier. This will fail with ZX_ERR_NOT_FOUND as
  // there is no committed page for the kernel to dirty.
  ASSERT_EQ(ZX_ERR_NOT_FOUND,
            zx_pager_op_range(pager.pager().get(), ZX_PAGER_OP_DIRTY, vmo->vmo().get(),
                              zx_system_get_page_size(), zx_system_get_page_size(), 0));

  // Failing the DIRTY request will cause the kernel to try again, this time with a READ request
  // first as the zero page has been marked clean without being committed.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
  ASSERT_TRUE(t.Wait());

  // The last page should still be dirty but non-zero now.
  range.options = 0;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests zeroing a range and then marking it clean with an outstanding READ request.
TEST(PagerWriteback, DelayedReadZeroRange) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY | ZX_VMO_RESIZABLE, &vmo));

  // Read from the VMO. This will generate a READ request.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    if (vmo->vmo().read(&data, 0, sizeof(data)) != ZX_OK) {
      return false;
    }
    return data == 0;
  });
  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  // Zero the VMO.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // Verify that the range is dirty and zero.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // The zeroing should also have unblocked the read request to be fulfilled with zeros now.
  ASSERT_TRUE(t.Wait());

  // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
  // now responsible for supplying all pages.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Verify that no pages are dirty now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // No new requests generated.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // No pages populated in the VMO.
  zx_info_vmo_t info;
  ASSERT_OK(vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  ASSERT_EQ(0, info.committed_bytes);

  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests zeroing a range and then marking it clean with an outstanding DIRTY request.
TEST(PagerWriteback, DelayedDirtyZeroRange) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY | ZX_VMO_RESIZABLE, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Write to the VMO. This will generate a DIRTY request.
  TestThread t([vmo]() -> bool {
    uint8_t data = 0xaa;
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  // Zero the VMO.
  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));

  // Verify that the range is dirty and zero.
  zx_vmo_dirty_range_t range = {.offset = 0, .length = 1, .options = ZX_VMO_DIRTY_RANGE_IS_ZERO};
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // Meanwhile writeback the dirty zero range. This will reset the zero tail so that the pager is
  // now responsible for supplying all pages.
  ASSERT_TRUE(pager.WritebackBeginZeroPages(vmo, 0, 1));
  ASSERT_TRUE(pager.WritebackEndPages(vmo, 0, 1));

  // Verify that no pages are dirty now.
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, nullptr, 0));

  // Now try to resolve the DIRTY request we saw earlier. This will fail with ZX_ERR_NOT_FOUND as
  // there is no committed page for the kernel to dirty.
  ASSERT_EQ(ZX_ERR_NOT_FOUND, zx_pager_op_range(pager.pager().get(), ZX_PAGER_OP_DIRTY,
                                                vmo->vmo().get(), 0, zx_system_get_page_size(), 0));

  // Failing the DIRTY request will cause the kernel to try again, this time with a READ request
  // first as the zero page has been marked clean without being committed.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  zx::vmo empty_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &empty_src));
  ASSERT_OK(pager.pager().supply_pages(vmo->vmo(), 0, zx_system_get_page_size(), empty_src, 0));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
  ASSERT_TRUE(t.Wait());

  // The page is now non-zero and dirty.
  range.options = 0;
  ASSERT_TRUE(pager.VerifyDirtyRanges(vmo, &range, 1));

  // No new requests generated.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests attributed counts for a child created against a pager-backed parent.
TEST(PagerWriteback, ChildAttributedCounts) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* root_vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &root_vmo));

  zx::vmo child_vmo;
  ASSERT_OK(root_vmo->vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                                         zx_system_get_page_size(), &child_vmo));

  // Dirty a page in the parent.
  ASSERT_TRUE(pager.SupplyPages(root_vmo, 0, 1));
  ASSERT_TRUE(pager.DirtyPages(root_vmo, 0, 1));
  const uint32_t val = 42;
  ASSERT_OK(root_vmo->vmo().write(&val, 0, sizeof(val)));

  // Child can read the parent.
  uint32_t data;
  ASSERT_OK(child_vmo.read(&data, 0, sizeof(data)));
  EXPECT_EQ(val, data);

  // Committed pages are attributed to the parent.
  zx_info_vmo_t info;
  ASSERT_OK(root_vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Committed pages are not attributed to the child.
  ASSERT_OK(child_vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.committed_bytes);

  // Drop the parent.
  pager.ReleaseVmo(root_vmo);

  // Committed pages are still not attributed to the child.
  ASSERT_OK(child_vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.committed_bytes);

  // Child can read the parent's pages though.
  data = 0;
  ASSERT_OK(child_vmo.read(&data, 0, sizeof(data)));
  EXPECT_EQ(val, data);
}

// Tests API violations for zx_pager_op_range.
TEST(PagerWriteback, InvalidPagerOpRange) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  constexpr uint32_t kNumValidOpCodes = 4;
  const uint32_t opcodes[kNumValidOpCodes] = {
      ZX_PAGER_OP_FAIL, ZX_PAGER_OP_DIRTY, ZX_PAGER_OP_WRITEBACK_BEGIN, ZX_PAGER_OP_WRITEBACK_END};

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  // Test common failure conditions for all ops.
  for (uint32_t i = 0; i < kNumValidOpCodes; i++) {
    zx::vmo vmo;
    ASSERT_OK(zx_pager_create_vmo(pager.get(), ZX_VMO_TRAP_DIRTY, port.get(), 0,
                                  zx_system_get_page_size(), vmo.reset_and_get_address()));

    // bad handles
    ASSERT_EQ(ZX_ERR_BAD_HANDLE,
              zx_pager_op_range(ZX_HANDLE_INVALID, opcodes[i], vmo.get(), 0, 0, 0));
    ASSERT_EQ(ZX_ERR_BAD_HANDLE,
              zx_pager_op_range(pager.get(), opcodes[i], ZX_HANDLE_INVALID, 0, 0, 0));

    // wrong handle types
    ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_pager_op_range(vmo.get(), opcodes[i], vmo.get(), 0, 0, 0));
    ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_pager_op_range(pager.get(), opcodes[i], pager.get(), 0, 0, 0));

    // missing rights
    zx::vmo ro_vmo;
    ASSERT_OK(vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_WRITE, &ro_vmo));
    ASSERT_EQ(ZX_ERR_ACCESS_DENIED,
              zx_pager_op_range(pager.get(), opcodes[i], ro_vmo.get(), 0, 0, 0));

    // using a non-pager-backed vmo
    zx::vmo vmo2;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo2));
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_op_range(pager.get(), opcodes[i], vmo2.get(), 0, 0, 0));

    // using a pager vmo from another pager
    zx::pager pager2;
    ASSERT_EQ(zx::pager::create(0, &pager2), ZX_OK);
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_op_range(pager2.get(), opcodes[i], vmo.get(), 0, 0, 0));

    // misaligned offset or length
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_op_range(pager.get(), opcodes[i], vmo.get(), 1, 0, 0));
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_op_range(pager.get(), opcodes[i], vmo.get(), 0, 1, 0));

    // out of range
    ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
              zx_pager_op_range(pager.get(), opcodes[i], vmo.get(), zx_system_get_page_size(),
                                zx_system_get_page_size(),
                                opcodes[i] == ZX_PAGER_OP_FAIL ? ZX_ERR_BAD_STATE : 0));
  }

  // Test op-specific failure conditions
  zx::vmo vmo;
  ASSERT_OK(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                vmo.reset_and_get_address()));

  // invalid opcode
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, zx_pager_op_range(pager.get(), 0, vmo.get(), 0, 0, 0));

  // ZX_PAGER_OP_FAIL: invalid error codes
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_FAIL, vmo.get(), 0, 0, 0x11ffffffff));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_FAIL, vmo.get(), 0, 0, ZX_ERR_INTERNAL));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_FAIL, vmo.get(), 0, 0, 100));

  // ZX_PAGER_OP_DIRTY: VMO created without TRAP_DIRTY
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_DIRTY, vmo.get(), 0, 0, 0));

  ASSERT_OK(zx_pager_create_vmo(pager.get(), ZX_VMO_TRAP_DIRTY, port.get(), 0,
                                zx_system_get_page_size(), vmo.reset_and_get_address()));

  // ZX_PAGER_OP_DIRTY: dirty an unsupplied page
  ASSERT_EQ(ZX_ERR_NOT_FOUND, zx_pager_op_range(pager.get(), ZX_PAGER_OP_DIRTY, vmo.get(), 0,
                                                zx_system_get_page_size(), 0));

  // ZX_PAGER_OP_DIRTY: invalid data
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_DIRTY, vmo.get(), 0, 0, 100));

  // ZX_PAGER_OP_WRITEBACK_BEGIN: invalid data
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_WRITEBACK_BEGIN, vmo.get(), 0, 0, 100));

  // ZX_PAGER_OP_WRITEBACK_END: invalid data
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_op_range(pager.get(), ZX_PAGER_OP_WRITEBACK_END, vmo.get(), 0, 0, 100));

  // missing pager rights
  ASSERT_OK(pager.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_MANAGE_VMO, &pager));
  for (uint32_t i = 0; i < kNumValidOpCodes; i++) {
    ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx_pager_op_range(pager.get(), opcodes[i], vmo.get(), 0, 0, 0));
  }
}

// Tests API violations for zx_pager_query_vmo_stats and zx_pager_query_dirty_ranges.
TEST(PagerWriteback, InvalidQuery) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                vmo.reset_and_get_address()));

  // bad handles
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, zx_pager_query_dirty_ranges(ZX_HANDLE_INVALID, vmo.get(), 0, 0,
                                                           nullptr, 0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, zx_pager_query_dirty_ranges(pager.get(), ZX_HANDLE_INVALID, 0, 0,
                                                           nullptr, 0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_BAD_HANDLE,
            zx_pager_query_vmo_stats(ZX_HANDLE_INVALID, vmo.get(), 0, nullptr, 0));
  ASSERT_EQ(ZX_ERR_BAD_HANDLE,
            zx_pager_query_vmo_stats(pager.get(), ZX_HANDLE_INVALID, 0, nullptr, 0));

  // wrong handle types
  ASSERT_EQ(ZX_ERR_WRONG_TYPE,
            zx_pager_query_dirty_ranges(vmo.get(), vmo.get(), 0, 0, nullptr, 0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_pager_query_dirty_ranges(pager.get(), pager.get(), 0, 0, nullptr,
                                                           0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_pager_query_vmo_stats(vmo.get(), vmo.get(), 0, nullptr, 0));
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_pager_query_vmo_stats(pager.get(), pager.get(), 0, nullptr, 0));

  // missing rights
  zx::vmo dup;
  ASSERT_OK(vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_READ, &dup));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx_pager_query_dirty_ranges(pager.get(), dup.get(), 0, 0, nullptr,
                                                              0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx_pager_query_vmo_stats(pager.get(), dup.get(), 0, nullptr, 0));

  // using a non-pager-backed vmo
  zx::vmo vmo2;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo2));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager.get(), vmo2.get(), 0, 0, nullptr,
                                                             0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_vmo_stats(pager.get(), vmo2.get(), 0, nullptr, 0));

  // using a pager vmo from another pager
  zx::pager pager2;
  ASSERT_EQ(zx::pager::create(0, &pager2), ZX_OK);
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_dirty_ranges(pager2.get(), vmo.get(), 0, 0, nullptr,
                                                             0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx_pager_query_vmo_stats(pager2.get(), vmo.get(), 0, nullptr, 0));

  // out of range
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
            zx_pager_query_dirty_ranges(pager.get(), vmo.get(), zx_system_get_page_size(),
                                        zx_system_get_page_size(), nullptr, 0, nullptr, nullptr));

  // invalid option
  zx_pager_vmo_stats_t stats = {};
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.get(), vmo.get(), 100, &stats, sizeof(stats)));

  // buffer too small
  ASSERT_EQ(ZX_ERR_BUFFER_TOO_SMALL,
            zx_pager_query_vmo_stats(pager.get(), vmo.get(), 0, &stats, 0));

  // invalid buffer
  // Dirty a page first, so the query has something to enumerate.
  ASSERT_OK(
      zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(), vmo2.get(), 0));
  uint64_t data = 42;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                        nullptr, sizeof(zx_vmo_dirty_range_t), nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            zx_pager_query_vmo_stats(pager.get(), vmo.get(), 0, nullptr, sizeof(stats)));

  // missing pager rights
  ASSERT_OK(pager.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_MANAGE_VMO, &pager));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, 0, nullptr,
                                                              0, nullptr, nullptr));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx_pager_query_vmo_stats(pager.get(), vmo.get(), 0, nullptr, 0));
}

// Regression test for https://fxbug.dev/504723239.
TEST(PagerWriteback, QueryVmoStatsInvalidPointer) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, 4096, &vmo));

  // Pass an invalid non-canonical / kernel address as the buffer. This should not cause the kernel
  // to crash.
  zx_status_t status = zx_pager_query_vmo_stats(pager.get(), vmo.get(), 0,
                                                reinterpret_cast<void*>(0xffffffffffffffff),
                                                sizeof(zx_pager_vmo_stats_t));
  EXPECT_NE(ZX_OK, status);
}

// Tests that attempt to exercise the early termination of the query dirty ranges loop to avoid
// redundantly calculating an additional range prior to terminating if the user did not provide a
// large enough buffer.
TEST(PagerWriteback, QueryEarlyTermination) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size() * 3,
                                vmo.reset_and_get_address()));
  // Dirty some pages first, so the query has something to enumerate.
  zx::vmo vmo2;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo2));
  ASSERT_OK(
      zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(), vmo2.get(), 0));
  ASSERT_OK(zx_pager_supply_pages(pager.get(), vmo.get(), zx_system_get_page_size() * 2,
                                  zx_system_get_page_size(), vmo2.get(), 0));
  uint64_t data = 42;
  ASSERT_OK(vmo.write(&data, 0, sizeof(data)));
  ASSERT_OK(vmo.write(&data, zx_system_get_page_size() * 2, sizeof(data)));

  // Query with a buffer size too small to hold any of the results, but otherwise valid.
  size_t actual = 0;
  ASSERT_EQ(ZX_OK, zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                               &data, sizeof(data), &actual, nullptr));
  EXPECT_EQ(actual, 0);
  // Now query with a buffer large enough for the first dirty range, but not the second.
  zx_vmo_dirty_range_t dirty_range = {};
  ASSERT_EQ(ZX_OK,
            zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, zx_system_get_page_size() * 3,
                                        &dirty_range, sizeof(dirty_range), &actual, nullptr));
  EXPECT_EQ(dirty_range.offset, 0);
  EXPECT_EQ(dirty_range.length, zx_system_get_page_size());
  EXPECT_EQ(actual, 1);
  // Query again to get and check avail.
  size_t avail = 0;
  ASSERT_EQ(ZX_OK,
            zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, zx_system_get_page_size() * 3,
                                        &dirty_range, sizeof(dirty_range), &actual, &avail));
  EXPECT_EQ(avail, 2);
  // Now query both ranges;
  zx_vmo_dirty_range_t dirty_ranges[2] = {};
  ASSERT_EQ(ZX_OK, zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0,
                                               zx_system_get_page_size() * 3, dirty_ranges,
                                               sizeof(zx_vmo_dirty_range_t) * 2, &actual, nullptr));
  EXPECT_EQ(dirty_range.offset, 0);
  EXPECT_EQ(dirty_range.length, zx_system_get_page_size());
  EXPECT_EQ(actual, 2);
}

// Test that a large write absent pages generates correct requests.
TEST(PagerWriteback, LargeWriteBatched) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr size_t kNumPages = kMaxWriteWaitPages * 8;
  constexpr size_t kPageSteps = 8;
  static_assert(kNumPages % kPageSteps == 0);
  static_assert(kPageSteps < kMaxWriteWaitPages);
  static_assert(kMaxWriteWaitPages % kPageSteps == 0);
  // Start the read at an offset to ensure there is a difference between the page requests VMO
  // offset, and the offset from the start of the request.
  constexpr size_t kStartOffset = 2;
  constexpr size_t kVmoPages = kNumPages + kStartOffset;

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kVmoPages, ZX_VMO_TRAP_DIRTY, &vmo));

  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, kStartOffset);

  TestThread t([&vmo, &expected] {
    return vmo->vmo().write(expected.data(), kStartOffset * zx_system_get_page_size(),
                            kNumPages * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t.Start());

  for (size_t i = 0; i < kNumPages; i += kMaxWriteWaitPages) {
    // Expect to see a read request for the batch range.
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i + kStartOffset, kMaxWriteWaitPages, ZX_TIME_INFINITE));
    // Supply the read request in pieces.
    for (size_t j = 0; j < kMaxWriteWaitPages; j += kPageSteps) {
      ASSERT_TRUE(pager.SupplyPages(vmo, i + j + kStartOffset, kPageSteps));
    }
    // Now there should be a dirty request for the batch range.
    ASSERT_TRUE(
        pager.WaitForPageDirty(vmo, i + kStartOffset, kMaxWriteWaitPages, ZX_TIME_INFINITE));
    // Dirty the range in pieces.
    for (size_t j = 0; j < kMaxWriteWaitPages; j += kPageSteps) {
      ASSERT_TRUE(pager.DirtyPages(vmo, i + j + kStartOffset, kPageSteps));
    }
  }
  ASSERT_TRUE(t.Wait());
  ASSERT_TRUE(vmo->CheckVmo(kStartOffset, kNumPages));
}

TEST(PagerWriteback, Unbounded) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_FALSE(pager.CreateVmoWithOptions(1, ZX_VMO_UNBOUNDED, &vmo));
}

// Tests that unbounded VMOs and setting the stream size correctly create a dirty-untracked zero
// range beyond the stream size.
TEST(PagerWriteback, UntrackedBeyondStreamSize) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateUnboundedVmo(2 * zx_system_get_page_size(), ZX_VMO_TRAP_DIRTY, &vmo));

  // Nothing dirty yet.
  EXPECT_FALSE(pager.VerifyModified(vmo));
  size_t actual, avail;
  // TODO: Replace with UserPager methods once unbounded VMOs are properly supported in the test
  // helpers. We can't use pager.VerifyDirtyRanges() for now since it uses the size_ which will be 0
  // for unbounded VMOs.
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the stream size should read as zeros.
  std::vector<uint8_t> expected(10 * zx_system_get_page_size(), 0);
  EXPECT_TRUE(check_buffer_data(vmo, 2, 8, expected.data(), false));

  // Increase the stream size. This should reveal a dirty zero range.
  ASSERT_OK(vmo->vmo().set_stream_size(5 * zx_system_get_page_size()));
  EXPECT_TRUE(pager.VerifyModified(vmo));
  zx_vmo_dirty_range_t range;
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), &range, sizeof(range),
                                        &actual, &avail));
  EXPECT_EQ(1u, actual);
  EXPECT_EQ(1u, avail);
  EXPECT_EQ(2 * zx_system_get_page_size(), range.offset);
  EXPECT_EQ(3 * zx_system_get_page_size(), range.length);
  EXPECT_EQ(ZX_VMO_DIRTY_RANGE_IS_ZERO, range.options);

  // Pages beyond the old stream size should still read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 2, 8, expected.data(), false));

  // Decrease the stream size. The dirty zero range should get absorbed back.
  ASSERT_OK(vmo->vmo().set_stream_size(zx_system_get_page_size()));
  EXPECT_TRUE(pager.VerifyModified(vmo));
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the new stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 1, 9, expected.data(), false));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Tests that single page dirty zero interval slots are correctly absorbed into untracked zero
// intervals when setting the stream size.
TEST(PagerWriteback, UntrackedAbsorbsIntervalSlots) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(3, ZX_VMO_TRAP_DIRTY, &vmo));
  // Create a single page untracked zero interval at page 2.
  ASSERT_OK(vmo->vmo().set_stream_size(2 * zx_system_get_page_size()));

  // Nothing dirty yet.
  EXPECT_TRUE(pager.VerifyModified(vmo));
  size_t actual, avail;
  // TODO: Replace with UserPager methods once unbounded VMOs are properly supported in the test
  // helpers. We can't use pager.VerifyDirtyRanges() for now since it uses the size_ which will be 0
  // for unbounded VMOs.
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        3 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the stream size should read as zeros.
  std::vector<uint8_t> expected(2 * zx_system_get_page_size(), 0);
  EXPECT_TRUE(check_buffer_data(vmo, 2, 1, expected.data(), false));

  // Decrease the stream size by a single page. This should absorb the existing interval slot at
  // page 2 into the new untracked range [1, 3).
  ASSERT_OK(vmo->vmo().set_stream_size(zx_system_get_page_size()));

  // Nothing dirty still.
  EXPECT_TRUE(pager.VerifyModified(vmo));
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        3 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the new stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 1, 2, expected.data(), false));

  // Increase the stream size again and commit page 1.
  ASSERT_OK(vmo->vmo().set_stream_size(2 * zx_system_get_page_size()));
  ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));

  // Verify that the committed page is dirty.
  EXPECT_TRUE(pager.VerifyModified(vmo));
  zx_vmo_dirty_range_t range;
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        3 * zx_system_get_page_size(), &range, sizeof(range),
                                        &actual, &avail));
  EXPECT_EQ(1u, actual);
  EXPECT_EQ(1u, avail);
  EXPECT_EQ(zx_system_get_page_size(), range.offset);
  EXPECT_EQ(zx_system_get_page_size(), range.length);
  EXPECT_EQ(0, range.options);

  // Pages beyond the old stream size should still read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 2, 1, expected.data(), false));

  // Decrease the stream size again, overwriting the page. The single untracked zero slot should get
  // absorbed back.
  ASSERT_OK(vmo->vmo().set_stream_size(zx_system_get_page_size()));
  EXPECT_TRUE(pager.VerifyModified(vmo));
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        3 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the new stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 1, 2, expected.data(), false));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Tests that pages committed beyond the stream size are absorbed back into untracked (or dirty)
// zero intervals when setting the stream size.
TEST(PagerWriteback, UntrackedAbsorbsPages) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateUnboundedVmo(2 * zx_system_get_page_size(), ZX_VMO_TRAP_DIRTY, &vmo));

  // Nothing dirty yet.
  EXPECT_FALSE(pager.VerifyModified(vmo));
  size_t actual, avail;
  // TODO: Replace with UserPager methods once unbounded VMOs are properly supported in the test
  // helpers. We can't use pager.VerifyDirtyRanges() for now since it uses the size_ which will be 0
  // for unbounded VMOs.
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the stream size should read as zeros.
  std::vector<uint8_t> expected(10 * zx_system_get_page_size(), 0);
  EXPECT_TRUE(check_buffer_data(vmo, 2, 8, expected.data(), false));

  // Dirty a few pages beyond the stream size. Pages 4, 5, and 8 are written.
  TestThread t([&]() {
    uint64_t data = 42;
    if (vmo->vmo().write(&data, 4 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 5 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    if (vmo->vmo().write(&data, 8 * zx_system_get_page_size(), sizeof(data)) != ZX_OK) {
      return false;
    }
    return true;
  });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 4, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 5, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 5, 1));
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 8, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.DirtyPages(vmo, 8, 1));

  ASSERT_TRUE(t.Wait());

  // Verify dirty ranges.
  zx_vmo_dirty_range_t ranges[2];
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), &ranges[0], sizeof(ranges),
                                        &actual, &avail));
  EXPECT_EQ(2u, actual);
  EXPECT_EQ(2u, avail);
  EXPECT_EQ(4 * zx_system_get_page_size(), ranges[0].offset);
  EXPECT_EQ(2 * zx_system_get_page_size(), ranges[0].length);
  EXPECT_EQ(0, ranges[0].options);
  EXPECT_EQ(8 * zx_system_get_page_size(), ranges[1].offset);
  EXPECT_EQ(zx_system_get_page_size(), ranges[1].length);
  EXPECT_EQ(0, ranges[1].options);

  // Pages not dirtied beyond stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 2, 2, expected.data(), false));
  EXPECT_TRUE(check_buffer_data(vmo, 6, 2, expected.data(), false));
  EXPECT_TRUE(check_buffer_data(vmo, 9, 1, expected.data(), false));

  // Increase the stream size so that some pages become a part of a dirty zero interval and some of
  // an untracked zero interval.
  ASSERT_OK(vmo->vmo().set_stream_size(7 * zx_system_get_page_size()));
  EXPECT_TRUE(pager.VerifyModified(vmo));

  // Verify dirty ranges.
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), &ranges[0], sizeof(ranges),
                                        &actual, &avail));
  EXPECT_EQ(1u, actual);
  EXPECT_EQ(1u, avail);
  EXPECT_EQ(2 * zx_system_get_page_size(), ranges[0].offset);
  EXPECT_EQ(5 * zx_system_get_page_size(), ranges[0].length);
  EXPECT_EQ(ZX_VMO_DIRTY_RANGE_IS_ZERO, ranges[0].options);

  // Pages beyond the old stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 2, 8, expected.data(), false));

  // Decrease the stream size.
  ASSERT_OK(vmo->vmo().set_stream_size(zx_system_get_page_size()));
  EXPECT_TRUE(pager.VerifyModified(vmo));
  ASSERT_OK(zx_pager_query_dirty_ranges(pager.pager().get(), vmo->vmo().get(), 0,
                                        10 * zx_system_get_page_size(), nullptr, 0, &actual,
                                        &avail));
  EXPECT_EQ(0u, actual);
  EXPECT_EQ(0u, avail);

  // Pages beyond the new stream size should read as zeros.
  EXPECT_TRUE(check_buffer_data(vmo, 1, 9, expected.data(), false));

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Test that suspending and resuming a thread in the middle of a dirty request works.
VMO_VMAR_TEST(PagerWriteback, SuspendDirtyTest) {
  // This test cannot run as a component due to its usage of WaitForBlocked() which will return true
  // if the test thread is blocked on pagers outside of the test. WaitForBlocked() can only be
  // relied upon in a non-component environment. The pager-writeback tests cannot run as standalone
  // bootfs tests either because they need the next vDSO. Hence the only supported mode for this
  // test is unified mode, where the system resource will be available.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available, skipping\n");
  }

  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_TRAP_DIRTY, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Write to the VMO, which will trigger a dirty request.
  TestThread t([vmo, check_vmar]() -> bool {
    uint8_t data = 0xaa;
    if (check_vmar) {
      auto ptr = reinterpret_cast<uint8_t*>(vmo->base_addr());
      *ptr = data;
      return true;
    }
    return vmo->vmo().write(&data, 0, sizeof(data)) == ZX_OK;
  });

  ASSERT_TRUE(t.Start());

  // Make sure we're blocked on the dirty request. Then suspend the thread.
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(t.WaitForBlocked());

  t.SuspendSync();
  t.Resume();

  // The thread should block again on resume and we should see another dirty request.
  ASSERT_TRUE(t.WaitForBlocked());
  ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Regression test for https://fxbug.dev/502272571
TEST(PagerWriteback, QueryDirtyRangesInvalidBuffer) {
  NEEDS_NEXT_SKIP(zx_pager_query_dirty_ranges);

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  uint64_t vmo_size = zx_system_get_page_size() * 10ul;
  ASSERT_OK(zx_pager_create_vmo(pager.get(), ZX_VMO_TRAP_DIRTY, port.get(), 0, vmo_size,
                                vmo.reset_and_get_address()));

  // Supply a page and dirty it so that EnumerateDirtyRanges finds something.
  zx::vmo aux_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo));
  ASSERT_OK(zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                  aux_vmo.get(), 0));
  ASSERT_OK(zx_pager_op_range(pager.get(), ZX_PAGER_OP_DIRTY, vmo.get(), 0,
                              zx_system_get_page_size(), 0));

  size_t actual, avail;
  // Use an invalid buffer pointer and ensure that the syscall fails in some way, and does not cause
  // a kernel panic.
  zx_status_t status = zx_pager_query_dirty_ranges(pager.get(), vmo.get(), 0, vmo_size,
                                                   reinterpret_cast<void*>(0xffffffff00000000),
                                                   zx_system_get_page_size(), &actual, &avail);
  EXPECT_EQ(status, ZX_ERR_INVALID_ARGS);
}

}  // namespace pager_tests
