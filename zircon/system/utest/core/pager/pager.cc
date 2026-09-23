// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/intrin.h>
#include <lib/fit/defer.h>
#include <lib/fzl/memory-probe.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/port.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/object.h>
#include <zircon/time.h>

#include <iterator>
#include <memory>
#include <thread>
#include <vector>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

#include "test_thread.h"
#include "userpager.h"
#include "zircon/system/utest/core/vmo/helpers.h"

namespace pager_tests {

using TestLimiter = vmo_test::TestLimiter;

// Simple test that checks that a single thread can access a single page.
VMO_VMAR_TEST(Pager, SinglePageTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
}

// Test that calling zx_port_cancel_key on a pager's port with a VMO's key does not
// wedge the pager completion protocol (i.e. packet_busy_ is properly reset by Free()).
VMO_VMAR_TEST(Pager, CancelKeyDoesNotWedgeThePager) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  // 1. Provoke a page request on page 0 so a pager packet is bound to the port queue.
  TestThread t1([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(t1.WaitForBlocked());

  // 2. Call zx_port_cancel_key on the pager's port with that VMO's key while the packet is queued.
  // This unlinks the bound PagerProxy packet and calls Free(), returning ZX_OK.
  ASSERT_OK(pager.port().cancel_key(0, vmo->key()));

  // 3. Service page 0 directly so that thread t1 completes cleanly.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // 4. Assert the pager can still service a subsequent request - i.e., provoke a fault
  // on page 1 and verify that the reader finishes within a bounded wait without hanging.
  TestThread t2([vmo, check_vmar]() -> bool { return check_buffer(vmo, 1, 1, check_vmar); });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  ASSERT_TRUE(t2.Wait());
}

// Test that a fault can be fulfilled with an uncommitted page.
VMO_VMAR_TEST(Pager, UncommittedSinglePageTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  std::vector<uint8_t> data(zx_system_get_page_size(), 0);

  TestThread t([vmo, check_vmar, &data]() -> bool {
    return check_buffer_data(vmo, 0, 1, data.data(), check_vmar);
  });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  zx::vmo empty;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &empty));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1, std::move(empty)));

  ASSERT_TRUE(t.Wait());
}

// Tests that pre-supplied pages don't result in requests.
VMO_VMAR_TEST(Pager, PresupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(t.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that supplies between the request and reading the port
// causes the request to be aborted.
VMO_VMAR_TEST(Pager, EarlySupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  TestThread t1([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  // Use a second thread to make sure the queue of requests is flushed.
  TestThread t2([vmo, check_vmar]() -> bool { return check_buffer(vmo, 1, 1, check_vmar); });

  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(t1.WaitForBlocked());
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(t2.WaitForBlocked());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  ASSERT_TRUE(t2.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Checks that a single thread can sequentially access multiple pages.
VMO_VMAR_TEST(Pager, SequentialMultipageTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint32_t kNumPages = 32;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, kNumPages, check_vmar); });

  ASSERT_TRUE(t.Start());

  if (check_vmar) {
    // When mapped, the hardware faults will fault in pages one by one.
    for (uint64_t i = 0; i < kNumPages; i++) {
      ASSERT_TRUE(pager.WaitForPageRead(vmo, i, 1, ZX_TIME_INFINITE));
      ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
    }
  } else {
    ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));
  }

  ASSERT_TRUE(t.Wait());
}

// Tests that multiple threads can concurrently access different pages.
VMO_VMAR_TEST(Pager, ConcurrentMultipageAccessTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  TestThread t2([vmo, check_vmar]() -> bool { return check_buffer(vmo, 1, 1, check_vmar); });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  ASSERT_TRUE(t.Wait());
  ASSERT_TRUE(t2.Wait());
}

// Tests that multiple threads can concurrently access a single page.
VMO_VMAR_TEST(Pager, ConcurrentOverlappingAccessTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  constexpr uint64_t kNumThreads = 32;
  std::unique_ptr<TestThread> threads[kNumThreads];
  for (unsigned i = 0; i < kNumThreads; i++) {
    threads[i] = std::make_unique<TestThread>(
        [vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

    threads[i]->Start();
    ASSERT_TRUE(threads[i]->WaitForBlocked());
  }

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  for (unsigned i = 0; i < kNumThreads; i++) {
    ASSERT_TRUE(threads[i]->Wait());
  }

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that multiple threads can concurrently access multiple pages and
// be satisfied by a single supply operation.
VMO_VMAR_TEST(Pager, BulkSingleSupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint32_t kNumPages = 8;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  std::unique_ptr<TestThread> ts[kNumPages];
  for (unsigned i = 0; i < kNumPages; i++) {
    ts[i] = std::make_unique<TestThread>(
        [vmo, i, check_vmar]() -> bool { return check_buffer(vmo, i, 1, check_vmar); });
    ASSERT_TRUE(ts[i]->Start());
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i, 1, ZX_TIME_INFINITE));
  }

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  for (unsigned i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(ts[i]->Wait());
  }
}

// Test body for odd supply tests.
void BulkOddSupplyTestInner(bool check_vmar, bool use_src_offset) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  // Interesting supply lengths that will exercise splice logic.
  constexpr uint64_t kSupplyLengths[] = {2, 3, 5, 7, 37, 5, 13, 23};
  uint64_t sum = 0;
  for (unsigned i = 0; i < std::size(kSupplyLengths); i++) {
    sum += kSupplyLengths[i];
  }

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(sum, &vmo));

  uint64_t page_idx = 0;
  for (unsigned supply_idx = 0; supply_idx < std::size(kSupplyLengths); supply_idx++) {
    uint64_t supply_len = kSupplyLengths[supply_idx];
    uint64_t offset = page_idx;

    std::unique_ptr<TestThread> ts[kSupplyLengths[supply_idx]];
    for (uint64_t j = 0; j < kSupplyLengths[supply_idx]; j++) {
      uint64_t thread_offset = offset + j;
      ts[j] = std::make_unique<TestThread>([vmo, thread_offset, check_vmar]() -> bool {
        return check_buffer(vmo, thread_offset, 1, check_vmar);
      });
      ASSERT_TRUE(ts[j]->Start());
      ASSERT_TRUE(pager.WaitForPageRead(vmo, thread_offset, 1, ZX_TIME_INFINITE));
    }

    uint64_t src_offset = use_src_offset ? offset : 0;
    ASSERT_TRUE(pager.SupplyPages(vmo, offset, supply_len, src_offset));

    for (unsigned i = 0; i < kSupplyLengths[supply_idx]; i++) {
      ASSERT_TRUE(ts[i]->Wait());
    }

    page_idx += kSupplyLengths[supply_idx];
  }
}

// Test that exercises supply logic by supplying data in chunks of unusual length.
VMO_VMAR_TEST(Pager, BulkOddLengthSupplyTest) { return BulkOddSupplyTestInner(check_vmar, false); }

// Test that exercises supply logic by supplying data in chunks of
// unusual lengths and offsets.
VMO_VMAR_TEST(Pager, BulkOddOffsetSupplyTest) { return BulkOddSupplyTestInner(check_vmar, true); }

// Tests that supply doesn't overwrite existing content.
VMO_VMAR_TEST(Pager, OverlapSupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  zx::vmo alt_data_vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &alt_data_vmo), ZX_OK);
  std::vector<uint8_t> alt_data(zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(alt_data.data(), 1, 2);
  ASSERT_EQ(alt_data_vmo.write(alt_data.data(), 0, zx_system_get_page_size()), ZX_OK);

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1, std::move(alt_data_vmo)));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));

  TestThread t([vmo, &alt_data, check_vmar]() -> bool {
    return check_buffer_data(vmo, 0, 1, alt_data.data(), check_vmar) &&
           check_buffer(vmo, 1, 1, check_vmar);
  });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(t.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that a pager can handle lots of pending page requests.
VMO_VMAR_TEST(Pager, ManyRequestTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  constexpr uint32_t kNumPages = 257;  // Arbitrary large number
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  std::unique_ptr<TestThread> ts[kNumPages];
  for (unsigned i = 0; i < kNumPages; i++) {
    ts[i] = std::make_unique<TestThread>(
        [vmo, i, check_vmar]() -> bool { return check_buffer(vmo, i, 1, check_vmar); });
    ASSERT_TRUE(ts[i]->Start());
    ASSERT_TRUE(ts[i]->WaitForBlocked());
  }

  for (unsigned i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
    ASSERT_TRUE(ts[i]->Wait());
  }
}

// Tests that a pager can support creating and destroying successive vmos.
TEST(Pager, SuccessiveVmoTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint32_t kNumVmos = 64;
  for (unsigned i = 0; i < kNumVmos; i++) {
    Vmo* vmo;
    ASSERT_TRUE(pager.CreateVmo(1, &vmo));

    TestThread t([vmo]() -> bool { return check_buffer(vmo, 0, 1, true); });

    ASSERT_TRUE(t.Start());

    ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

    ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

    ASSERT_TRUE(t.Wait());

    pager.ReleaseVmo(vmo);
  }
}

// Tests that a pager can support multiple concurrent vmos.
TEST(Pager, MultipleConcurrentVmoTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint32_t kNumVmos = 8;
  Vmo* vmos[kNumVmos];
  std::unique_ptr<TestThread> ts[kNumVmos];
  for (unsigned i = 0; i < kNumVmos; i++) {
    ASSERT_TRUE(pager.CreateVmo(1, vmos + i));

    ts[i] = std::make_unique<TestThread>(
        [vmo = vmos[i]]() -> bool { return check_buffer(vmo, 0, 1, true); });

    ASSERT_TRUE(ts[i]->Start());

    ASSERT_TRUE(pager.WaitForPageRead(vmos[i], 0, 1, ZX_TIME_INFINITE));
  }

  for (unsigned i = 0; i < kNumVmos; i++) {
    ASSERT_TRUE(pager.SupplyPages(vmos[i], 0, 1));

    ASSERT_TRUE(ts[i]->Wait());
  }
}

// Tests that unmapping a vmo while threads are blocked on a pager read
// eventually results in pagefaults.
TEST(Pager, VmarUnmapTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(),
                                       &addr));

  TestThread t([addr]() -> bool {
    auto buf = reinterpret_cast<uint8_t*>(addr);
    uint8_t data = *buf;
    // This comparison is not relevant. Just use the data so that the dereference is not optimized
    // out. The thread is expected to crash before this point anyway.
    return data > 0;
  });
  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t.WaitForBlocked());

  ASSERT_OK(zx::vmar::root_self()->unmap(addr, zx_system_get_page_size()));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.WaitForCrash(addr, ZX_ERR_NOT_FOUND));
}

// Tests that replacing a vmar mapping while threads are blocked on a
// pager read results in reads to the new mapping.
TEST(Pager, VmarRemapTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  // Create two pager VMOs. We will switch the VM mapping from one VMO to the other mid-access.
  Vmo *vmo1, *vmo2;
  constexpr uint32_t kNumPages = 8;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo1));
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo2));

  // Create a mapping for vmo1.
  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo1->vmo(), 0,
                                       kNumPages * zx_system_get_page_size(), &addr));
  auto unmap = fit::defer(
      [&]() { zx::vmar::root_self()->unmap(addr, kNumPages * zx_system_get_page_size()); });

  std::unique_ptr<TestThread> ts[kNumPages];
  for (unsigned t = 0; t < kNumPages; t++) {
    ts[t] = std::make_unique<TestThread>([addr, t, vmo2]() -> bool {
      uint8_t data[zx_system_get_page_size()];
      vmo2->GenerateBufferContents(data, 1, t);
      // Thread |t| accesses page |t|. Each thread will block here and generate a page request. We
      // will resolve the page request after switching out the mapping to vmo2, so the contents will
      // match vmo2, not vmo1.
      return memcmp(data, reinterpret_cast<uint8_t*>(addr + t * zx_system_get_page_size()),
                    zx_system_get_page_size()) == 0;
    });
    ASSERT_TRUE(ts[t]->Start());
  }

  for (unsigned t = 0; t < kNumPages; t++) {
    ASSERT_TRUE(ts[t]->WaitForBlocked());
  }

  // Now that all the threads are blocked, swap out the mapping to point to vmo2.
  zx_info_vmar_t info;
  uint64_t a1, a2;
  ASSERT_OK(zx::vmar::root_self()->get_info(ZX_INFO_VMAR, &info, sizeof(info), &a1, &a2));
  zx_vaddr_t new_addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_SPECIFIC_OVERWRITE, addr - info.base,
                                       vmo2->vmo(), 0, kNumPages * zx_system_get_page_size(),
                                       &new_addr));
  ASSERT_EQ(addr, new_addr);

  // Supply pages in vmo1. The threads will still remain blocked.
  ASSERT_TRUE(pager.SupplyPages(vmo1, 0, kNumPages));
  for (unsigned t = 0; t < kNumPages; t++) {
    ASSERT_TRUE(ts[t]->WaitForBlocked());
  }

  // We should see page requests for vmo2.
  for (unsigned i = 0; i < kNumPages; i++) {
    uint64_t offset, length;
    ASSERT_TRUE(pager.GetPageReadRequest(vmo2, ZX_TIME_INFINITE, &offset, &length));
    ASSERT_EQ(length, 1);
    ASSERT_TRUE(pager.SupplyPages(vmo2, offset, 1));
    ASSERT_TRUE(ts[offset]->Wait());
  }
}

// Tests that ZX_VM_MAP_RANGE works with pager vmos (i.e. maps in backed regions
// but doesn't try to pull in new regions).
TEST(Pager, VmarMapRangeTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  // Create a vmo with 2 pages. Supply the first page but not the second.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Map the vmo. This shouldn't block or generate any new page requests.
  uint64_t ptr;
  TestThread t([vmo, &ptr]() -> bool {
    ZX_ASSERT(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_MAP_RANGE, 0, vmo->vmo(), 0,
                                         2 * zx_system_get_page_size(), &ptr) == ZX_OK);
    return true;
  });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t.Wait());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Verify the buffer contents. This should generate a new request for
  // the second page, which we want to fulfill.
  TestThread t2([vmo, &ptr]() -> bool {
    uint8_t data[2 * zx_system_get_page_size()];
    vmo->GenerateBufferContents(data, 2, 0);

    return memcmp(data, reinterpret_cast<uint8_t*>(ptr), 2 * zx_system_get_page_size()) == 0;
  });

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));

  ASSERT_TRUE(t2.Wait());

  // After the verification is done, make sure there are no unexpected
  // page requests.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Cleanup the mapping we created.
  zx::vmar::root_self()->unmap(ptr, 2 * zx_system_get_page_size());
}

// Tests that reads don't block forever if a vmo is resized out from under a read.
VMO_VMAR_TEST(Pager, ReadResizeTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(1, ZX_VMO_RESIZABLE, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(vmo->Resize(0));

  if (check_vmar) {
    ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_OUT_OF_RANGE));
  } else {
    ASSERT_TRUE(t.WaitForFailure());
  }
}

// Test that suspending and resuming a thread in the middle of a read works.
VMO_VMAR_TEST(Pager, SuspendReadTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  // Make sure the thread is blocked before suspending.
  ASSERT_TRUE(t.WaitForBlocked());

  t.SuspendSync();
  t.Resume();

  // Make sure the thread is blocked on resume.
  ASSERT_TRUE(t.WaitForBlocked());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
}

// Tests the ZX_INFO_VMO_PAGER_BACKED flag
TEST(Pager, VmoInfoPagerTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(zx_system_get_page_size(), &vmo));

  // Check that the flag is set on a pager created vmo.
  zx_info_vmo_t info;
  ASSERT_EQ(ZX_OK, vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr), "");
  ASSERT_EQ(ZX_INFO_VMO_PAGER_BACKED, info.flags & ZX_INFO_VMO_PAGER_BACKED, "");

  // Check that the flag isn't set on a regular vmo.
  zx::vmo plain_vmo;
  ASSERT_EQ(ZX_OK, zx::vmo::create(zx_system_get_page_size(), 0, &plain_vmo), "");
  ASSERT_EQ(ZX_OK, plain_vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr), "");
  ASSERT_EQ(0, info.flags & ZX_INFO_VMO_PAGER_BACKED, "");
}

// Tests that detaching results in a complete request.
TEST(Pager, DetachPageCompleteTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  ASSERT_TRUE(pager.DetachVmo(vmo));

  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));
}

// Tests that pages are decommitted on a detach, and accessing pages (via the parent VMO or the
// clone) after the detach results in failures.
VMO_VMAR_TEST(Pager, DecommitOnDetachTest) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager backed VMO and a clone.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));
  auto clone = vmo->Clone();

  // Reading the first page via the clone should create a read request packet.
  TestThread t1([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  // Supply the page and wait for the thread to successfully exit.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(t1.Wait());

  // Verify that a page is committed in the parent VMO.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  ASSERT_EQ(zx_system_get_page_size(), info.committed_bytes);

  // Detach the VMO.
  pager.DetachVmo(vmo);
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // Verify that no committed pages remain in the parent VMO.
  ASSERT_EQ(ZX_OK, vmo->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  ASSERT_EQ(0ul, info.committed_bytes);

  // Try to access the first page in the parent vmo, which was previously paged in but is now
  // decommitted. This should fail.
  TestThread t2([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  ASSERT_TRUE(t2.Start());
  if (check_vmar) {
    ASSERT_TRUE(t2.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t2.WaitForFailure());
  }

  // Try to access the first page from the clone. This should also fail.
  TestThread t3([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });
  ASSERT_TRUE(t3.Start());
  if (check_vmar) {
    ASSERT_TRUE(t3.WaitForCrash(clone->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t3.WaitForFailure());
  }

  // Try to access the second page in the parent vmo, which was previously not paged in.
  // This should fail.
  TestThread t4([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  ASSERT_TRUE(t4.Start());
  if (check_vmar) {
    ASSERT_TRUE(t4.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t4.WaitForFailure());
  }

  // Try to access the second page from the clone. This should also fail.
  TestThread t5([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });
  ASSERT_TRUE(t5.Start());
  if (check_vmar) {
    ASSERT_TRUE(t5.WaitForCrash(clone->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t5.WaitForFailure());
  }

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that closing results in a complete request.
TEST(Pager, ClosePageCompleteTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  uint64_t key = vmo->key();
  pager.ReleaseVmo(vmo);

  ASSERT_TRUE(pager.WaitForPageComplete(key, ZX_TIME_INFINITE));
}

// Tests that accessing a VMO in non-mapping ways returns appropriate errors if detached.
TEST(Pager, DetachNonMappingAccess) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  zx_status_t vmo_result = ZX_OK;
  TestThread t([vmo, &vmo_result]() -> bool {
    uint64_t val;
    // Do a read that strides two pages so we can succeed one and fail one.
    vmo_result =
        vmo->vmo().read(&val, zx_system_get_page_size() - sizeof(uint64_t) / 2, sizeof(uint64_t));
    return true;
  });

  ASSERT_TRUE(t.Start());

  // Expect the whole range to be batched into one request.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  pager.DetachVmo(vmo);
  ASSERT_TRUE(t.WaitForTerm());

  EXPECT_EQ(ZX_ERR_BAD_STATE, vmo_result);

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that interrupting a read after receiving the request doesn't result in hanging threads.
void ReadInterruptLateTest(bool check_vmar, bool detach) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  if (detach) {
    ASSERT_TRUE(pager.DetachVmo(vmo));
  } else {
    pager.ClosePagerHandle();
  }

  if (check_vmar) {
    ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t.WaitForFailure());
  }

  if (detach) {
    ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));
  }
}

VMO_VMAR_TEST(Pager, ReadCloseInterruptLateTest) { ReadInterruptLateTest(check_vmar, false); }

VMO_VMAR_TEST(Pager, ReadDetachInterruptLateTest) { ReadInterruptLateTest(check_vmar, true); }

// Tests that interrupt a read before receiving requests doesn't result in hanging threads.
void ReadInterruptEarlyTest(bool check_vmar, bool detach) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t.WaitForBlocked());

  if (detach) {
    ASSERT_TRUE(pager.DetachVmo(vmo));
  } else {
    pager.ClosePagerHandle();
  }

  if (check_vmar) {
    ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  } else {
    ASSERT_TRUE(t.WaitForFailure());
  }

  if (detach) {
    ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));
  }
}

VMO_VMAR_TEST(Pager, ReadCloseInterruptEarlyTest) { ReadInterruptEarlyTest(check_vmar, false); }

VMO_VMAR_TEST(Pager, ReadDetachInterruptEarlyTest) { ReadInterruptEarlyTest(check_vmar, true); }

// Tests that closing a pager while a thread is accessing it doesn't cause
// problems (other than a page fault in the accessing thread).
TEST(Pager, ClosePagerTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  TestThread t([vmo]() -> bool { return check_buffer(vmo, 0, 1, true); });
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t.WaitForBlocked());

  pager.ClosePagerHandle();

  ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
  ASSERT_TRUE(check_buffer(vmo, 1, 1, true));
}

// Tests that closing a pager while a vmo is being detached doesn't cause problems.
TEST(Pager, DetachClosePagerTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  ASSERT_TRUE(pager.DetachVmo(vmo));

  pager.ClosePagerHandle();
}

// Tests that closing an in use port doesn't cause issues (beyond no
// longer being able to receive requests).
TEST(Pager, ClosePortTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));

  TestThread t([vmo]() -> bool { return check_buffer(vmo, 0, 1, true); });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t.WaitForBlocked());

  pager.ClosePortHandle();

  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  ASSERT_TRUE(check_buffer(vmo, 1, 1, true));

  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_BAD_STATE));
}

// Tests that reading from a clone populates the vmo.
VMO_VMAR_TEST(Pager, CloneReadFromCloneTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
}

// Tests that reading from the parent populates the clone.
VMO_VMAR_TEST(Pager, CloneReadFromParentTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());

  TestThread t2([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(t2.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that overlapping reads on clone and parent work.
VMO_VMAR_TEST(Pager, CloneSimultaneousReadTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  TestThread t2([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(t.WaitForBlocked());
  ASSERT_TRUE(t2.WaitForBlocked());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
  ASSERT_TRUE(t2.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that overlapping reads from two clones work.
VMO_VMAR_TEST(Pager, CloneSimultaneousChildReadTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);
  auto clone2 = vmo->Clone();
  ASSERT_NOT_NULL(clone2);

  TestThread t([clone = clone.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });
  TestThread t2([clone = clone2.get(), check_vmar]() -> bool {
    return check_buffer(clone, 0, 1, check_vmar);
  });

  ASSERT_TRUE(t.Start());
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(t.WaitForBlocked());
  ASSERT_TRUE(t2.WaitForBlocked());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
  ASSERT_TRUE(t2.Wait());

  ASSERT_FALSE(pager.WaitForPageRead(vmo, 0, 1, 0));
}

// Tests that writes don't propagate to the parent.
VMO_VMAR_TEST(Pager, CloneWriteToCloneTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool {
    *reinterpret_cast<uint64_t*>(clone->base_addr()) = 0xdeadbeef;
    return true;
  });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());

  ASSERT_TRUE(vmo->CheckVmar(0, 1));
  ASSERT_EQ(*reinterpret_cast<uint64_t*>(clone->base_addr()), 0xdeadbeef);
  *reinterpret_cast<uint64_t*>(clone->base_addr()) = clone->key();
  ASSERT_TRUE(clone->CheckVmar(0, 1));
}

// Tests that detaching the parent crashes the clone only for pages owned by the parent, not for
// pages that have been forked by the clone.
TEST(Pager, CloneDetachTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  // Create a pager backed VMO and a clone.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(3, &vmo));
  auto clone = vmo->Clone();

  // Read the second page.
  TestThread t1([clone = clone.get()]() -> bool { return check_buffer(clone, 1, 1, true); });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));

  // Write to the first page, forking it.
  TestThread t2([clone = clone.get()]() -> bool {
    // Fork a page in the clone.
    *reinterpret_cast<uint64_t*>(clone->base_addr()) = 0xdeadbeef;
    return true;
  });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  // Threads t1 and t2 should have generated page requests. Fulfill them and wait for the threads to
  // exit successfully.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));
  ASSERT_TRUE(t1.Wait());
  ASSERT_TRUE(t2.Wait());

  // Detach the parent vmo.
  ASSERT_TRUE(pager.DetachVmo(vmo));
  ASSERT_TRUE(pager.WaitForPageComplete(vmo->key(), ZX_TIME_INFINITE));

  // Declare read buffer outside threads so we can free them as the threads themselves will fault.
  std::vector<uint8_t> data(zx_system_get_page_size(), 0);

  // Read the third page. This page was not previously paged in (and not forked either) and should
  // result in a fatal page fault.
  TestThread t3([clone = clone.get(), &data]() -> bool {
    return check_buffer_data(clone, 2, 1, data.data(), true);
  });
  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(
      t3.WaitForCrash(clone->base_addr() + 2 * zx_system_get_page_size(), ZX_ERR_BAD_STATE));

  // Read the second page. This page was previously paged in but not forked, and should now have
  // been decomitted. Should result in a fatal page fault.
  TestThread t4([clone = clone.get(), &data]() -> bool {
    return check_buffer_data(clone, 1, 1, data.data(), true);
  });
  ASSERT_TRUE(t4.Start());
  ASSERT_TRUE(t4.WaitForCrash(clone->base_addr() + zx_system_get_page_size(), ZX_ERR_BAD_STATE));

  // Read the first page and verify its contents. This page was forked in the clone and should still
  // be valid.
  TestThread t5([clone = clone.get()]() -> bool {
    return (*reinterpret_cast<uint64_t*>(clone->base_addr()) == 0xdeadbeef);
  });
  ASSERT_TRUE(t5.Start());
  ASSERT_TRUE(t5.Wait());
}

// Tests that commit on the clone populates things properly.
TEST(Pager, CloneCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 32;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t.Wait());

  // Verify that the pages have been copied into the clone. (A commit simulates write faults.)
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Tests that commit on the clone of a clone populates things properly.
TEST(Pager, CloneChainCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 32;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto intermediate = vmo->Clone();
  ASSERT_NOT_NULL(intermediate);

  auto clone = intermediate->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t.Wait());

  // Verify that the pages have been copied into the clone. (A commit simulates write faults.)
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);

  // Verify that the intermediate has no pages committed.
  ASSERT_EQ(ZX_OK, intermediate->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(0ul, info.populated_bytes);
}

// Tests that commit on the clone populates things properly if things have already been touched.
TEST(Pager, CloneSplitCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 4;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  // Populate pages 1 and 2 of the parent vmo, and page 1 of the clone.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 2));
  ASSERT_TRUE(clone->CheckVmar(1, 1));

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(pager.WaitForPageRead(vmo, kNumPages - 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, kNumPages - 1, 1));

  ASSERT_TRUE(t.Wait());

  // Verify that the pages have been copied into the clone. (A commit simulates write faults.)
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Resizing a cloned VMO causes a fault.
TEST(Pager, CloneResizeCloneHazard) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  const uint64_t kSize = 2 * zx_system_get_page_size();
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(2, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  zx::vmo clone_vmo;
  EXPECT_EQ(ZX_OK, vmo->vmo().create_child(
                       ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE | ZX_VMO_CHILD_RESIZABLE, 0, kSize,
                       &clone_vmo));

  uintptr_t ptr_rw;
  EXPECT_EQ(ZX_OK, zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone_vmo, 0,
                                              kSize, &ptr_rw));

  auto int_arr = reinterpret_cast<int*>(ptr_rw);
  EXPECT_EQ(int_arr[1], 0);

  EXPECT_EQ(ZX_OK, clone_vmo.set_size(0u));

  EXPECT_STATUS(probe_for_read(&int_arr[1]), ZX_ERR_OUT_OF_RANGE, "read probe");
  EXPECT_STATUS(probe_for_write(&int_arr[1]), ZX_ERR_OUT_OF_RANGE, "write probe");

  EXPECT_EQ(ZX_OK, zx::vmar::root_self()->unmap(ptr_rw, kSize), "unmap");
}

// Resizing the parent VMO and accessing via a mapped VMO is ok.
TEST(Pager, CloneResizeParentOK) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  const uint64_t kSize = 2 * zx_system_get_page_size();
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(2, ZX_VMO_RESIZABLE, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  zx::vmo clone_vmo;
  ASSERT_EQ(ZX_OK,
            vmo->vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, kSize, &clone_vmo));

  uintptr_t ptr_rw;
  EXPECT_EQ(ZX_OK, zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone_vmo, 0,
                                              kSize, &ptr_rw));

  auto int_arr = reinterpret_cast<int*>(ptr_rw);
  EXPECT_EQ(int_arr[1], 0);

  EXPECT_TRUE(vmo->Resize(0u));

  EXPECT_OK(probe_for_read(&int_arr[1]), "read probe");
  EXPECT_OK(probe_for_write(&int_arr[1]), "write probe");

  EXPECT_EQ(ZX_OK, zx::vmar::root_self()->unmap(ptr_rw, kSize), "unmap");
}

// Pages exposed by growing the parent after shrinking it aren't visible to the child.
TEST(Pager, CloneShrinkGrowParent) {
  struct {
    uint64_t vmo_size;
    uint64_t clone_offset;
    uint64_t clone_size;
    uint64_t clone_test_offset;
    uint64_t resize_size;
  } configs[3] = {
      // Aligned, truncate to parent offset.
      {zx_system_get_page_size(), 0, zx_system_get_page_size(), 0, 0},
      // Offset, truncate to before parent offset.
      {2 * zx_system_get_page_size(), zx_system_get_page_size(), zx_system_get_page_size(), 0, 0},
      // Offset, truncate to partway through clone.
      {3 * zx_system_get_page_size(), zx_system_get_page_size(), 2 * zx_system_get_page_size(),
       zx_system_get_page_size(), 2 * zx_system_get_page_size()},
  };

  for (auto& config : configs) {
    UserPager pager;

    ASSERT_TRUE(pager.Init());

    Vmo* vmo;
    ASSERT_TRUE(pager.CreateVmoWithOptions(config.vmo_size / zx_system_get_page_size(),
                                           ZX_VMO_RESIZABLE, &vmo));

    zx::vmo aux;
    ASSERT_EQ(ZX_OK, zx::vmo::create(config.vmo_size, 0, &aux));
    ASSERT_EQ(ZX_OK, aux.op_range(ZX_VMO_OP_COMMIT, 0, config.vmo_size, nullptr, 0));
    ASSERT_TRUE(
        pager.SupplyPages(vmo, 0, config.vmo_size / zx_system_get_page_size(), std::move(aux)));

    zx::vmo clone_vmo;
    ASSERT_EQ(ZX_OK, vmo->vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE,
                                             config.clone_offset, config.vmo_size, &clone_vmo));

    uintptr_t ptr_ro;
    EXPECT_EQ(ZX_OK, zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, clone_vmo, 0, config.clone_size,
                                                &ptr_ro));

    auto ptr = reinterpret_cast<int*>(ptr_ro + config.clone_test_offset);
    EXPECT_EQ(0, *ptr);

    uint32_t data = 1;
    const uint64_t vmo_offset = config.clone_offset + config.clone_test_offset;
    EXPECT_EQ(ZX_OK, vmo->vmo().write(&data, vmo_offset, sizeof(data)));

    EXPECT_EQ(1, *ptr);

    EXPECT_TRUE(vmo->Resize(0u));

    EXPECT_EQ(0, *ptr);

    EXPECT_TRUE(vmo->Resize(config.vmo_size / zx_system_get_page_size()));

    ASSERT_EQ(ZX_OK, zx::vmo::create(config.vmo_size, 0, &aux));
    ASSERT_EQ(ZX_OK, aux.op_range(ZX_VMO_OP_COMMIT, 0, config.vmo_size, nullptr, 0));
    ASSERT_TRUE(
        pager.SupplyPages(vmo, 0, config.vmo_size / zx_system_get_page_size(), std::move(aux)));

    data = 2;
    EXPECT_EQ(ZX_OK, vmo->vmo().write(&data, vmo_offset, sizeof(data)));

    EXPECT_EQ(0, *ptr);

    EXPECT_EQ(ZX_OK, zx::vmar::root_self()->unmap(ptr_ro, config.clone_size));
  }
}

// Tests that a commit properly populates the whole range.
TEST(Pager, SimpleCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 555;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t.Wait());
}

// Tests that a commit over a partially populated range is properly split.
TEST(Pager, SplitCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 33;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  ASSERT_TRUE(pager.SupplyPages(vmo, (kNumPages / 2), 1));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, (kNumPages / 2), ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, (kNumPages / 2)));

  ASSERT_TRUE(pager.WaitForPageRead(vmo, (kNumPages / 2) + 1, kNumPages / 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, ((kNumPages / 2) + 1), (kNumPages / 2)));

  ASSERT_TRUE(t.Wait());
}

// Tests that overlapping commits don't result in redundant requests.
TEST(Pager, OverlapCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 32;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t1([vmo]() -> bool { return vmo->Commit((kNumPages / 4), (kNumPages / 2)); });
  TestThread t2([vmo]() -> bool { return vmo->Commit(0, kNumPages); });

  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, (kNumPages / 4), (kNumPages / 2), ZX_TIME_INFINITE));

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, (kNumPages / 4), ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, (3 * kNumPages / 4)));

  ASSERT_TRUE(pager.WaitForPageRead(vmo, (3 * kNumPages / 4), (kNumPages / 4), ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, (3 * kNumPages / 4), (kNumPages / 4)));

  ASSERT_TRUE(t1.Wait());
  ASSERT_TRUE(t2.Wait());
}

// Tests that overlapping commits are properly supplied.
TEST(Pager, OverlapCommitSupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kSupplyLen = 3;
  constexpr uint64_t kCommitLenA = 7;
  constexpr uint64_t kCommitLenB = 5;
  constexpr uint64_t kNumPages = kCommitLenA * kCommitLenB * kSupplyLen;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  std::unique_ptr<TestThread> tsA[kNumPages / kCommitLenA];
  for (unsigned i = 0; i < std::size(tsA); i++) {
    tsA[i] = std::make_unique<TestThread>(
        [vmo, i]() -> bool { return vmo->Commit(i * kCommitLenA, kCommitLenA); });

    ASSERT_TRUE(tsA[i]->Start());
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i * kCommitLenA, kCommitLenA, ZX_TIME_INFINITE));
  }

  std::unique_ptr<TestThread> tsB[kNumPages / kCommitLenB];
  for (unsigned i = 0; i < std::size(tsB); i++) {
    tsB[i] = std::make_unique<TestThread>(
        [vmo, i]() -> bool { return vmo->Commit(i * kCommitLenB, kCommitLenB); });

    ASSERT_TRUE(tsB[i]->Start());
    ASSERT_TRUE(tsB[i]->WaitForBlocked());
  }

  for (unsigned i = 0; i < kNumPages / kSupplyLen; i++) {
    ASSERT_TRUE(pager.SupplyPages(vmo, i * kSupplyLen, kSupplyLen));
  }

  for (unsigned i = 0; i < std::size(tsA); i++) {
    ASSERT_TRUE(tsA[i]->Wait());
  }
  for (unsigned i = 0; i < std::size(tsB); i++) {
    ASSERT_TRUE(tsB[i]->Wait());
  }
}

// Tests that a single commit can be fulfilled by multiple supplies.
TEST(Pager, MultisupplyCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 32;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  for (unsigned i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
  }

  ASSERT_TRUE(t.Wait());
}

// Tests that a single supply can fulfil multiple commits.
TEST(Pager, MulticommitSupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumCommits = 5;
  constexpr uint64_t kNumSupplies = 7;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumCommits * kNumSupplies, &vmo));

  std::unique_ptr<TestThread> ts[kNumCommits];
  for (unsigned i = 0; i < kNumCommits; i++) {
    ts[i] = std::make_unique<TestThread>(
        [vmo, i]() -> bool { return vmo->Commit(i * kNumSupplies, kNumSupplies); });
    ASSERT_TRUE(ts[i]->Start());
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i * kNumSupplies, kNumSupplies, ZX_TIME_INFINITE));
  }

  for (unsigned i = 0; i < kNumSupplies; i++) {
    ASSERT_TRUE(pager.SupplyPages(vmo, kNumCommits * i, kNumCommits));
  }

  for (unsigned i = 0; i < kNumCommits; i++) {
    ASSERT_TRUE(ts[i]->Wait());
  }
}

// Tests that redundant supplies for a single commit don't cause errors.
TEST(Pager, CommitRedundantSupplyTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 8;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  for (unsigned i = 1; i <= kNumPages; i++) {
    ASSERT_TRUE(pager.SupplyPages(vmo, 0, i));
  }

  ASSERT_TRUE(t.Wait());
}

// Test that resizing out from under a commit is handled.
TEST(Pager, ResizeCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(3, ZX_VMO_RESIZABLE, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, 3); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 3, ZX_TIME_INFINITE));

  // Supply one of the pages that will be removed.
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));

  // Truncate the VMO.
  ASSERT_TRUE(vmo->Resize(1));

  // Make sure the thread is still blocked (i.e. check the accounting
  // w.r.t. the page that was removed).
  ASSERT_TRUE(t.WaitForBlocked());

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());

  // Make sure there are no extra requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Test that suspending and resuming a thread in the middle of commit works.
TEST(Pager, SuspendCommitTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, 1); });

  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  t.SuspendSync();
  t.Resume();

  ASSERT_TRUE(t.WaitForBlocked());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  ASSERT_TRUE(t.Wait());
}

// Tests API violations for pager_create.
TEST(Pager, InvalidPagerCreate) {
  zx_handle_t handle;

  // bad options
  ASSERT_EQ(zx_pager_create(1, &handle), ZX_ERR_INVALID_ARGS);
}

// Tests API violations for pager_create_vmo.
TEST(Pager, InvalidPagerCreateVmo) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx_handle_t vmo;

  // bad options
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), ~0u, port.get(), 0, zx_system_get_page_size(), &vmo),
            ZX_ERR_INVALID_ARGS);

  // bad handles for pager and port
  ASSERT_EQ(
      zx_pager_create_vmo(ZX_HANDLE_INVALID, 0, port.get(), 0, zx_system_get_page_size(), &vmo),
      ZX_ERR_BAD_HANDLE);
  ASSERT_EQ(
      zx_pager_create_vmo(pager.get(), 0, ZX_HANDLE_INVALID, 0, zx_system_get_page_size(), &vmo),
      ZX_ERR_BAD_HANDLE);

  // missing write right on port
  zx::port ro_port;
  ASSERT_EQ(port.duplicate(ZX_DEFAULT_PORT_RIGHTS & ~ZX_RIGHT_WRITE, &ro_port), ZX_OK);
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, ro_port.get(), 0, zx_system_get_page_size(), &vmo),
            ZX_ERR_ACCESS_DENIED);

  // bad handle types for pager and port
  ASSERT_EQ(zx_pager_create_vmo(port.get(), 0, port.get(), 0, zx_system_get_page_size(), &vmo),
            ZX_ERR_WRONG_TYPE);
  zx::vmo tmp_vmo;  // writability handle 2 is checked before the type, so use a new vmo
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &tmp_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, tmp_vmo.get(), 0, zx_system_get_page_size(), &vmo),
            ZX_ERR_WRONG_TYPE);

  // invalid size
  const uint64_t kBadSize = fbl::round_down(UINT64_MAX, zx_system_get_page_size()) + 1;
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, kBadSize, &vmo),
            ZX_ERR_OUT_OF_RANGE);

  // missing pager rights
  ASSERT_OK(pager.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_ATTACH_VMO, &pager));
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(), &vmo),
            ZX_ERR_ACCESS_DENIED);
}

// Tests API violations for pager_detach_vmo.
TEST(Pager, InvalidPagerDetachVmo) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                vmo.reset_and_get_address()),
            ZX_OK);

  // bad handles
  ASSERT_EQ(zx_pager_detach_vmo(ZX_HANDLE_INVALID, vmo.get()), ZX_ERR_BAD_HANDLE);
  ASSERT_EQ(zx_pager_detach_vmo(pager.get(), ZX_HANDLE_INVALID), ZX_ERR_BAD_HANDLE);

  // wrong handle types
  ASSERT_EQ(zx_pager_detach_vmo(vmo.get(), vmo.get()), ZX_ERR_WRONG_TYPE);
  ASSERT_EQ(zx_pager_detach_vmo(pager.get(), pager.get()), ZX_ERR_WRONG_TYPE);

  // missing rights
  zx::vmo ro_vmo;
  ASSERT_EQ(vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_WRITE, &ro_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_detach_vmo(pager.get(), ro_vmo.get()), ZX_ERR_ACCESS_DENIED);

  // detaching a non-paged vmo
  zx::vmo tmp_vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &tmp_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_detach_vmo(pager.get(), tmp_vmo.get()), ZX_ERR_INVALID_ARGS);

  // detaching with the wrong pager
  zx::pager pager2;
  ASSERT_EQ(zx::pager::create(0, &pager2), ZX_OK);
  ASSERT_EQ(zx_pager_detach_vmo(pager2.get(), vmo.get()), ZX_ERR_INVALID_ARGS);

  // missing pager rights
  ASSERT_OK(pager.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_ATTACH_VMO, &pager));
  ASSERT_EQ(zx_pager_detach_vmo(pager.get(), vmo.get()), ZX_ERR_ACCESS_DENIED);

  ASSERT_OK(zx_pager_create_vmo(pager2.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                vmo.reset_and_get_address()));
  ASSERT_OK(pager2.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_MANAGE_VMO, &pager2));
  ASSERT_EQ(zx_pager_detach_vmo(pager2.get(), vmo.get()), ZX_ERR_ACCESS_DENIED);
}

// Tests API violations for supply_pages.
TEST(Pager, InvalidPagerSupplyPages) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                vmo.reset_and_get_address()),
            ZX_OK);

  zx::vmo aux_vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo), ZX_OK);

  // bad handles
  ASSERT_EQ(zx_pager_supply_pages(ZX_HANDLE_INVALID, vmo.get(), 0, 0, aux_vmo.get(), 0),
            ZX_ERR_BAD_HANDLE);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), ZX_HANDLE_INVALID, 0, 0, aux_vmo.get(), 0),
            ZX_ERR_BAD_HANDLE);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 0, ZX_HANDLE_INVALID, 0),
            ZX_ERR_BAD_HANDLE);

  // wrong handle types
  ASSERT_EQ(zx_pager_supply_pages(vmo.get(), vmo.get(), 0, 0, aux_vmo.get(), 0), ZX_ERR_WRONG_TYPE);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), pager.get(), 0, 0, aux_vmo.get(), 0),
            ZX_ERR_WRONG_TYPE);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 0, port.get(), 0), ZX_ERR_WRONG_TYPE);

  // using a non-paged vmo
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), aux_vmo.get(), 0, 0, aux_vmo.get(), 0),
            ZX_ERR_INVALID_ARGS);

  // using a pager vmo from another pager
  zx::pager pager2;
  ASSERT_EQ(zx::pager::create(0, &pager2), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager2.get(), vmo.get(), 0, 0, ZX_HANDLE_INVALID, 0),
            ZX_ERR_INVALID_ARGS);

  // missing permissions on the aux vmo
  zx::vmo ro_vmo;
  ASSERT_EQ(aux_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_WRITE, &ro_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 0, ro_vmo.get(), 0),
            ZX_ERR_ACCESS_DENIED);
  zx::vmo wo_vmo;
  ASSERT_EQ(aux_vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_READ, &wo_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 0, wo_vmo.get(), 0),
            ZX_ERR_ACCESS_DENIED);

  // missing permissions on the pager vmo
  zx::vmo ro_pager_vmo;
  ASSERT_EQ(vmo.duplicate(ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_WRITE, &ro_pager_vmo), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), ro_pager_vmo.get(), 0, 0, aux_vmo.get(), 0),
            ZX_ERR_ACCESS_DENIED);

  // misaligned offset, size, or aux alignment
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 1, 0, aux_vmo.get(), 0),
            ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 1, aux_vmo.get(), 0),
            ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, 0, aux_vmo.get(), 1),
            ZX_ERR_INVALID_ARGS);

  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (system_resource->is_valid()) {
    zx::result<vmo_test::PhysVmo> result = vmo_test::GetTestPhysVmo(zx_system_get_page_size());
    ASSERT_TRUE(result.is_ok());
    zx_handle_t physical_vmo_handle = result.value().vmo.get();
    ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                    physical_vmo_handle, 0),
              ZX_ERR_NOT_SUPPORTED);
  }

  // violations of conditions for taking pages from a vmo
  enum PagerViolation {
    kFromPager = 0,
    kHasPinned,
    kViolationCount,
  };
  for (uint32_t i = 0; i < kViolationCount; i++) {
    if (i == kHasPinned && !system_resource->is_valid()) {
      continue;
    }

    zx::vmo aux_vmo;  // aux vmo given to supply pages
    zx::vmo alt_vmo;  // alt vmo if clones are involved

    // The expected status of this test.
    zx_status_t expected_status = ZX_ERR_BAD_STATE;

    if (i == kFromPager) {
      ASSERT_EQ(zx_pager_create_vmo(pager.get(), 0, port.get(), 0, zx_system_get_page_size(),
                                    aux_vmo.reset_and_get_address()),
                ZX_OK);
      expected_status = ZX_ERR_NOT_SUPPORTED;
    } else {
      ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo), ZX_OK);
    }

    if (i == kFromPager) {
      ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &alt_vmo), ZX_OK);
      ASSERT_EQ(alt_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0),
                ZX_OK);
      ASSERT_EQ(zx_pager_supply_pages(pager.get(), aux_vmo.get(), 0, zx_system_get_page_size(),
                                      alt_vmo.get(), 0),
                ZX_OK);
    } else {
      ASSERT_EQ(aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0),
                ZX_OK);
    }

    zx::iommu iommu;
    zx::bti bti;
    zx::pmt pmt;

    if (i == kHasPinned) {
      zx::result<zx::resource> result =
          maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
      ASSERT_OK(result.status_value());
      zx::resource iommu_resource = std::move(result.value());
      zx_iommu_desc_stub_t desc;
      ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                                iommu.reset_and_get_address()),
                ZX_OK);
      ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);
      zx_paddr_t addr;
      ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, aux_vmo, 0, zx_system_get_page_size(), &addr, 1, &pmt),
                ZX_OK);
    }

    ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                    aux_vmo.get(), 0),
              expected_status);

    if (pmt) {
      pmt.unpin();
    }
  }

  // out of range pager_vmo region
  ASSERT_EQ(aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), zx_system_get_page_size(),
                                  zx_system_get_page_size(), aux_vmo.get(), 0),
            ZX_ERR_OUT_OF_RANGE);

  // out of range aux_vmo region
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo), ZX_OK);
  ASSERT_EQ(aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0), ZX_OK);
  ASSERT_EQ(zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(),
                                  aux_vmo.get(), zx_system_get_page_size()),
            ZX_ERR_OUT_OF_RANGE);

  // missing pager rights
  ASSERT_OK(pager.replace(ZX_DEFAULT_PAGER_RIGHTS & ~ZX_RIGHT_MANAGE_VMO, &pager));
  ASSERT_EQ(
      zx_pager_supply_pages(pager.get(), vmo.get(), 0, zx_system_get_page_size(), aux_vmo.get(), 0),
      ZX_ERR_ACCESS_DENIED);
}

// Tests that supply_pages works when the source is mapped.
TEST(Pager, MappedSupplyPages) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  zx::vmo aux_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &aux_vmo));

  // Map the aux vmo.
  auto root_vmar = zx::vmar::root_self();
  zx_vaddr_t addr;
  ASSERT_OK(root_vmar->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, aux_vmo, 0,
                           zx_system_get_page_size(), &addr));
  auto unmap = fit::defer([&]() { root_vmar->unmap(addr, zx_system_get_page_size()); });

  // Write something to the aux vmo that can be verified later.
  constexpr uint8_t kData = 0xcc;
  *reinterpret_cast<volatile uint8_t*>(addr) = kData;

  ASSERT_OK(pager.supply_pages(vmo, 0, zx_system_get_page_size(), aux_vmo, 0));

  // Verify that the right page was moved over.
  uint8_t buf;
  ASSERT_OK(vmo.read(&buf, 0, sizeof(buf)));
  EXPECT_EQ(buf, kData);

  // The mapped address should now read zero.
  EXPECT_EQ(*reinterpret_cast<volatile uint8_t*>(addr), 0u);
}

// Tests that supply_pages works when the destination has some pinned pages.
TEST(Pager, PinnedSupplyPages) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  const size_t kNumPages = 3;
  ASSERT_OK(pager.create_vmo(0, port, 0, kNumPages * zx_system_get_page_size(), &vmo));

  zx::vmo aux_vmo;
  ASSERT_OK(zx::vmo::create(kNumPages * zx_system_get_page_size(), 0, &aux_vmo));
  ASSERT_OK(
      aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, kNumPages * zx_system_get_page_size(), nullptr, 0));

  // Supply only the middle page, this will be pinned below.
  ASSERT_OK(zx_pager_supply_pages(pager.get(), vmo.get(), zx_system_get_page_size(),
                                  zx_system_get_page_size(), aux_vmo.get(), 0));

  // Verify contents of the supplied page.
  uint8_t buf[zx_system_get_page_size()];
  ASSERT_OK(vmo.read(&buf[0], zx_system_get_page_size(), sizeof(buf)));
  uint8_t expected[zx_system_get_page_size()];
  memset(&expected[0], 0, zx_system_get_page_size());
  ASSERT_EQ(0, memcmp(buf, expected, zx_system_get_page_size()));

  // Overwrite the supplied page that we can check for later.
  memset(&buf[0], 0xa, zx_system_get_page_size());
  memset(&expected[0], 0xa, zx_system_get_page_size());
  ASSERT_OK(vmo.write(&buf[0], zx_system_get_page_size(), sizeof(buf)));

  // Now pin the middle page.
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));
  zx_paddr_t addr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo, zx_system_get_page_size(), zx_system_get_page_size(),
                    &addr, 1, &pmt));
  auto unpin = fit::defer([&]() { pmt.unpin(); });

  // Try to supply the entire VMO now. This should skip the middle page which was pinned.
  ASSERT_OK(
      aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, kNumPages * zx_system_get_page_size(), nullptr, 0));
  ASSERT_OK(zx_pager_supply_pages(pager.get(), vmo.get(), 0, kNumPages * zx_system_get_page_size(),
                                  aux_vmo.get(), 0));

  // Verify that the middle page hasn't been overwritten.
  ASSERT_OK(vmo.read(&buf[0], zx_system_get_page_size(), sizeof(buf)));
  ASSERT_EQ(0, memcmp(buf, expected, zx_system_get_page_size()));

  // Verify that the remaining pages have been supplied with zeros from the aux_vmo.
  memset(&expected[0], 0, zx_system_get_page_size());
  ASSERT_OK(vmo.read(&buf[0], 0, sizeof(buf)));
  ASSERT_EQ(0, memcmp(buf, expected, zx_system_get_page_size()));
  ASSERT_OK(vmo.read(&buf[0], 2 * zx_system_get_page_size(), sizeof(buf)));
  ASSERT_EQ(0, memcmp(buf, expected, zx_system_get_page_size()));
}

// Tests that resizing a non-resizable pager vmo fails.
TEST(Pager, ResizeNonresizableVmo) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;

  ASSERT_EQ(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo), ZX_OK);

  ASSERT_EQ(vmo.set_size(2 * zx_system_get_page_size()), ZX_ERR_UNAVAILABLE);
}

// Tests that decommiting a clone fails
TEST(Pager, DecommitTest) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;

  ASSERT_EQ(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo), ZX_OK);

  ASSERT_EQ(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0),
            ZX_ERR_NOT_SUPPORTED);

  zx::vmo child;
  ASSERT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, zx_system_get_page_size(),
                             &child),
            ZX_OK);

  ASSERT_EQ(child.op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0),
            ZX_ERR_NOT_SUPPORTED);
}

// Test that supplying uncommitted pages prevents faults.
TEST(Pager, UncommittedSupply) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;

  ASSERT_EQ(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo), ZX_OK);

  zx::vmo empty;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &empty));

  ASSERT_OK(pager.supply_pages(vmo, 0, zx_system_get_page_size(), empty, 0));

  // A read should not fault and give zeros.
  uint32_t val = 42;
  ASSERT_OK(vmo.read(&val, 0, sizeof(val)));
  ASSERT_EQ(val, 0);
}

// Simple test for a ZX_PAGER_OP_FAIL on a single page, accessed from a single thread.
// Tests both cases, where the client accesses the vmo directly, and where the client has the vmo
// mapped in a vmar.
VMO_VMAR_TEST(Pager, FailSinglePage) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  TestThread t([vmo, check_vmar]() -> bool { return check_buffer(vmo, 0, 1, check_vmar); });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.FailPages(vmo, 0, 1));

  if (check_vmar) {
    // Verify that the thread crashes if the page was accessed via a vmar.
    ASSERT_TRUE(t.WaitForCrash(vmo->base_addr(), ZX_ERR_IO));
  } else {
    // Verify that the vmo read fails if the thread directly accessed the vmo.
    ASSERT_TRUE(t.WaitForFailure());
  }
  // Make sure there are no extra requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests failing the exact range requested.
TEST(Pager, FailExactRange) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.FailPages(vmo, 0, kNumPages));

  // Failing the pages will cause the COMMIT to fail.
  ASSERT_TRUE(t.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that multiple page requests can be failed at once.
TEST(Pager, FailMultipleCommits) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // Multiple threads requesting disjoint ranges.
  std::unique_ptr<TestThread> threads[kNumPages];
  for (uint64_t i = 0; i < kNumPages; i++) {
    threads[i] = std::make_unique<TestThread>([vmo, i]() -> bool { return vmo->Commit(i, 1); });
    ASSERT_TRUE(threads[i]->Start());
  }

  for (uint64_t i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(threads[i]->WaitForBlocked());
    ASSERT_TRUE(pager.WaitForPageRead(vmo, i, 1, ZX_TIME_INFINITE));
  }

  // Fail the entire range.
  ASSERT_TRUE(pager.FailPages(vmo, 0, kNumPages));

  for (uint64_t i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(threads[i]->WaitForFailure());
  }

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Multiple threads requesting the same range.
  for (uint64_t i = 0; i < kNumPages; i++) {
    threads[i] =
        std::make_unique<TestThread>([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
    ASSERT_TRUE(threads[i]->Start());
  }

  for (uint64_t i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(threads[i]->WaitForBlocked());
  }

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));
  // No more requests seen as after the first all the others should overlap.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Fail the entire range.
  ASSERT_TRUE(pager.FailPages(vmo, 0, kNumPages));

  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  for (uint64_t i = 0; i < kNumPages; i++) {
    ASSERT_TRUE(threads[i]->WaitForFailure());
  }

  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests failing multiple vmos.
TEST(Pager, FailMultipleVmos) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo1;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo1));
  TestThread t1([vmo1]() -> bool { return vmo1->Commit(0, 1); });

  Vmo* vmo2;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo2));
  TestThread t2([vmo2]() -> bool { return vmo2->Commit(0, 1); });

  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo1, 0, 1, ZX_TIME_INFINITE));

  uint64_t offset, length;
  // No page requests for vmo2 yet.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo2, 0, &offset, &length));

  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo2, 0, 1, ZX_TIME_INFINITE));

  // Fail vmo1.
  ASSERT_TRUE(pager.FailPages(vmo1, 0, 1));
  ASSERT_TRUE(t1.WaitForFailure());

  // No more requests for vmo1.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo1, 0, &offset, &length));

  // Fail vmo2.
  ASSERT_TRUE(pager.FailPages(vmo2, 0, 1));
  ASSERT_TRUE(t2.WaitForFailure());

  // No more requests for either vmo1 or vmo2.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo1, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageReadRequest(vmo2, 0, &offset, &length));
}

// Tests failing a range overlapping with a page request when using commit.
TEST(Pager, FailOverlappingRangeCommit) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // End of the request range overlaps with the failed range.
  TestThread t1([vmo]() -> bool { return vmo->Commit(0, 2); });
  // The start of the request range overlaps with the failed range.
  TestThread t2([vmo]() -> bool { return vmo->Commit(9, 2); });
  // The entire request range overlaps with the failed range.
  TestThread t3([vmo]() -> bool { return vmo->Commit(5, 2); });

  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 9, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 5, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 9));
  ASSERT_TRUE(pager.SupplyPages(vmo, 10, 1));

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 9));

  ASSERT_TRUE(t1.WaitForFailure());
  ASSERT_TRUE(t2.WaitForFailure());
  ASSERT_TRUE(t3.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests failing a range overlapping with a page request when using vmo_read.
TEST(Pager, FailOverlappingRangeRead) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto read_helper = [vmo](uint64_t page_offset, uint64_t page_count) -> bool {
    uint8_t data[page_count * zx_system_get_page_size()];
    return vmo->vmo().read(data, page_offset * zx_system_get_page_size(),
                           page_count * zx_system_get_page_size()) == ZX_OK;
  };

  // End of the request range overlaps with the failed range.
  TestThread t1([read_helper]() -> bool { return read_helper(0, 2); });
  // The start of the request range overlaps with the failed range.
  TestThread t2([read_helper]() -> bool { return read_helper(9, 2); });
  // The entire request range overlaps with the failed range.
  TestThread t3([read_helper]() -> bool { return read_helper(5, 2); });

  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 9, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(t3.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 5, 2, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.FailPages(vmo, 1, 9));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));
  ASSERT_TRUE(pager.SupplyPages(vmo, 10, 1));

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.FailPages(vmo, 1, 9));

  ASSERT_TRUE(t1.WaitForFailure());
  ASSERT_TRUE(t2.WaitForFailure());
  ASSERT_TRUE(t3.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests failing the requested range via multiple pager_op_range calls - after the first one, the
// rest are redundant.
TEST(Pager, FailRedundant) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  for (uint64_t i = 0; i < kNumPages; i++) {
    // The first call with i = 0 should cause the thread to cause.
    // The following calls are no-ops.
    ASSERT_TRUE(pager.FailPages(vmo, i, 1));
  }

  ASSERT_TRUE(t.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that failing a range after the vmo is detached fails.
TEST(Pager, FailAfterDetach) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.DetachVmo(vmo));
  // Detaching the vmo should cause the COMMIT to fail.
  ASSERT_TRUE(t.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_FALSE(pager.FailPages(vmo, 0, kNumPages));

  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

TEST(Pager, FailEarlyWake) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(3, &vmo));

  // To account for pages potentially getting evicted start a page fault handler to re-supply
  // anything.
  vmo->SetPageFaultSupplyLimit(0);
  ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

  TestThread t([vmo]() -> bool { return vmo->CheckVmo(0, 3); });
  ASSERT_TRUE(t.Start());

  ASSERT_TRUE(t.WaitForBlocked());

  // Supply the first page, this should wake up the internal kernel request and process that page.
  vmo->SetPageFaultSupplyLimit(1);
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1, 0));

  // Wait for the thread to have completed processing that page and be blocked again. This ensures
  // that we do not race by failing the pages before it blocks, causing it to keep waiting. This
  // race, and the resulting waiting, is not a kernel bug as we are intentionally *not* providing a
  // status of the third page in order to test the early wake optimization. In practice if this race
  // were to occur, the user would be failing all the remaining pages, which would cause the request
  // to unblock in all scenarios.
  ASSERT_TRUE(t.WaitForBlocked());

  // Fail the second page, although this isn't the first page in the request, due to the early wake
  // from the first request it is the next page to be processed and so should complete and fail the
  // request, even though we have not supplied or failed the third page.
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1, ZX_ERR_IO));

  ASSERT_TRUE(t.WaitForFailure());
}

// Fail overlap request
TEST(Pager, FailOverlapRequest) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(4, &vmo));

  TestThread t1([vmo]() -> bool { return vmo->CheckVmo(0, 4); });
  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 4, ZX_TIME_INFINITE));

  // Start another read, this one should be considered overlapping and not generate a request.
  TestThread t2([vmo]() -> bool { return vmo->CheckVmo(1, 1); });
  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(t2.WaitForBlocked());
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Fail the second page, this will set the failure status for the overlap, but it will not wake up
  // till the primary request wakes.
  ASSERT_TRUE(pager.FailPages(vmo, 1, 1, ZX_ERR_IO));

  // Now succeed the first page, this will early wake the first the request and cause it to process,
  // but then continue waiting for the rest of the request.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1, 0));
  ASSERT_TRUE(t1.WaitForBlocked());
  // The overlap is still blocked
  ASSERT_TRUE(t2.WaitForBlocked());

  // Succeed the last two pages.
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 2, 2));

  // Both requests should wake up, the overlap gets its failure and the primary, having forgotten
  // where the error was, generates another request.
  ASSERT_TRUE(t2.WaitForFailure());
  // The exact amount the will be waited for is slightly arbitrary, it will be at least one page but
  // the two end pages previously supplied could also have been evicted.
  ASSERT_TRUE(pager.GetPageReadRequest(vmo, ZX_TIME_INFINITE, &offset, &length));
  EXPECT_EQ(1u, offset);
  EXPECT_GE(length, 1u);

  // Succeed it this time.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 3, 1));

  // Should be complete now.
  ASSERT_TRUE(t1.Wait());
}

// Test that supply_pages fails after detach.
TEST(Pager, SupplyAfterDetach) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  ASSERT_TRUE(pager.DetachVmo(vmo));

  // No page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  ASSERT_FALSE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that a supply_pages succeeds after failing i.e. a fail is not fatal.
TEST(Pager, SupplyAfterFail) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 11;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  TestThread t1([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t1.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.FailPages(vmo, 0, kNumPages));
  ASSERT_TRUE(t1.WaitForFailure());

  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));

  // Try to COMMIT the failed range again.
  TestThread t2([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t2.Start());

  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  // This should supply the pages as expected.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));
  ASSERT_TRUE(t2.Wait());

  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that the error code passed in when failing is correctly propagated.
TEST(Pager, FailErrorCode) {
  constexpr zx_status_t valid_errors[] = {
      ZX_ERR_IO,       ZX_ERR_IO_DATA_INTEGRITY, ZX_ERR_BAD_STATE,
      ZX_ERR_NO_SPACE, ZX_ERR_BUFFER_TOO_SMALL,
  };
  for (const zx_status_t valid_error : valid_errors) {
    UserPager pager;
    ASSERT_TRUE(pager.Init());

    constexpr uint64_t kNumPages = 11;
    Vmo* vmo;
    ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

    zx_status_t status_commit;
    TestThread t_commit([vmo, &status_commit]() -> bool {
      // |status_commit| should get set to the error code passed in via FailPages.
      status_commit = vmo->vmo().op_range(ZX_VMO_OP_COMMIT, 0,
                                          kNumPages * zx_system_get_page_size(), nullptr, 0);
      return (status_commit == ZX_OK);
    });

    zx_status_t status_read;
    TestThread t_read([vmo, &status_read]() -> bool {
      zx::vmo tmp_vmo;
      zx_vaddr_t buf = 0;
      const uint64_t len = kNumPages * zx_system_get_page_size();

      if (zx::vmo::create(len, ZX_VMO_RESIZABLE, &tmp_vmo) != ZX_OK) {
        return false;
      }

      if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, tmp_vmo, 0, len,
                                     &buf) != ZX_OK) {
        return false;
      }

      auto unmap = fit::defer([&]() { zx_vmar_unmap(zx_vmar_root_self(), buf, len); });

      // |status_read| should get set to the error code passed in via FailPages.
      status_read = vmo->vmo().read(reinterpret_cast<void*>(buf), 0, len);
      return (status_read == ZX_OK);
    });

    ASSERT_TRUE(t_commit.Start());
    ASSERT_TRUE(t_commit.WaitForBlocked());
    ASSERT_TRUE(t_read.Start());
    ASSERT_TRUE(t_read.WaitForBlocked());
    ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

    // Fail with a specific valid error code.
    ASSERT_TRUE(pager.FailPages(vmo, 0, kNumPages, valid_error));

    ASSERT_TRUE(t_commit.WaitForFailure());
    // Verify that op_range(ZX_VMO_OP_COMMIT) returned the provided error code.
    ASSERT_EQ(status_commit, valid_error);

    ASSERT_TRUE(t_read.WaitForFailure());
    // Verify that vmo_read() returned the provided error code.
    ASSERT_EQ(status_read, valid_error);

    uint64_t offset, length;
    ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  }
}

// Test that if we resize a vmo while it is waiting on a page to fullfill the commit for a pin
// request that neither the resize nor the pin cause a crash and fail gracefully.
TEST(Pager, ResizeBlockedPin) {
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

  constexpr uint64_t kNumPages = 2;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_RESIZABLE, &vmo));

  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;
  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  // Spin up a thread to do the pin, this will block as it has to wait for pages from the user pager
  TestThread pin_thread([&bti, &pmt, &vmo]() -> bool {
    zx_paddr_t addr;
    // Pin the second page so we can resize such that there is absolutely no overlap in the ranges.
    // The pin itself is expected to ultimately fail as the resize will complete first.
    return bti.pin(ZX_BTI_PERM_READ, vmo->vmo(), zx_system_get_page_size(),
                   zx_system_get_page_size(), &addr, 1, &pmt) == ZX_ERR_OUT_OF_RANGE;
  });

  // Wait till the userpager gets the request.
  ASSERT_TRUE(pin_thread.Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 1, 1, ZX_TIME_INFINITE));

  // Resize the VMO down such that the pin request is completely out of bounds. This should succeed
  // as nothing has been pinned yet.
  ASSERT_TRUE(vmo->Resize(0));

  // The pin request should have been implicitly unblocked from the resize, and should have
  // ultimately failed. pin_thread returns true if it got the correct failure result from pin.
  ASSERT_TRUE(pin_thread.Wait());
}

TEST(Pager, DeepHierarchy) {
  zx::pager pager;
  ASSERT_EQ(zx::pager::create(0, &pager), ZX_OK);

  zx::port port;
  ASSERT_EQ(zx::port::create(0, &port), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo), ZX_OK);

  for (TestLimiter limiter(1000, zx::sec(60)); !limiter.Finished(); limiter.next()) {
    zx::vmo temp;
    EXPECT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                               zx_system_get_page_size(), &temp));
    vmo = std::move(temp);
  }
  vmo.reset();
}

// This tests that if there are intermediate parents that children see at least the state when
// they were created, and might (or might not) see writes that occur after creation.
TEST(Pager, CloneMightSeeIntermediateForks) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* root_vmo;
  ASSERT_TRUE(pager.CreateVmo(16, &root_vmo));

  // We are not testing page fault specifics, so just spin up a thread to handle all page faults.
  ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

  // Create a child that sees the full range, and put in an initial page fork
  // Create first child slightly inset.
  std::unique_ptr<Vmo> child = root_vmo->Clone(0, zx_system_get_page_size() * 16);
  ASSERT_NOT_NULL(child);
  uint64_t val = 1;
  EXPECT_OK(child->vmo().write(&val, zx_system_get_page_size() * 8, sizeof(uint64_t)));

  // Create two children of this, one in the fully empty half and one with the forked page.
  std::unique_ptr<Vmo> empty_child = child->Clone(0, zx_system_get_page_size() * 8);
  ASSERT_NOT_NULL(empty_child);
  std::unique_ptr<Vmo> forked_child =
      child->Clone(zx_system_get_page_size() * 8, zx_system_get_page_size() * 8);
  ASSERT_NOT_NULL(forked_child);

  EXPECT_TRUE(empty_child->CheckVmo(0, 8));
  EXPECT_TRUE(forked_child->CheckVmo(1, 7));
  EXPECT_OK(forked_child->vmo().read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(val, 1u);

  // Preemptively fork a distinct page in both children
  val = 2;
  EXPECT_OK(empty_child->vmo().write(&val, 0, sizeof(uint64_t)));
  val = 3;
  EXPECT_OK(forked_child->vmo().write(&val, zx_system_get_page_size(), sizeof(uint64_t)));

  // Fork these and other pages in the original child
  val = 4;
  EXPECT_OK(child->vmo().write(&val, 0, sizeof(uint64_t)));
  val = 5;
  EXPECT_OK(child->vmo().write(&val, zx_system_get_page_size() * 9, sizeof(uint64_t)));
  val = 6;
  EXPECT_OK(child->vmo().write(&val, zx_system_get_page_size(), sizeof(uint64_t)));
  val = 7;
  EXPECT_OK(child->vmo().write(&val, zx_system_get_page_size() * 10, sizeof(uint64_t)));

  // For the pages we had already forked in the child, we expect to see precisely what we wrote
  // originally, as we should have forked.
  EXPECT_OK(empty_child->vmo().read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(val, 2u);
  EXPECT_OK(forked_child->vmo().read(&val, zx_system_get_page_size(), sizeof(uint64_t)));
  EXPECT_EQ(val, 3u);

  // For the other forked pages we should either see what child wrote, or the original contents.
  // With the current implementation we know deterministically that empty_child should see the
  // original contents, and forked_child should see the forked. The commented out checks represent
  // the equally correct, but not current implementation, behavior.
  EXPECT_OK(empty_child->vmo().read(&val, zx_system_get_page_size(), sizeof(uint64_t)));
  // EXPECT_EQ(val, 6u);
  EXPECT_TRUE(empty_child->CheckVmo(1, 1));
  EXPECT_OK(forked_child->vmo().read(&val, zx_system_get_page_size() * 2, sizeof(uint64_t)));
  EXPECT_EQ(val, 7u);
  // EXPECT_TRUE(forked_child->CheckVmo(2, 1));
}

// Test that clones always see committed parent pages. This is validating that if a clone is hung
// off a higher parent internally than it was created on, that we never hang too high (i.e. any
// forked pages in intermediaries are always seen), and it has the correct limits and does cannot
// see more of the parent it hangs of than any of its intermediaries would have allowed it.
TEST(Pager, CloneSeesCorrectParentPages) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* root_vmo;
  ASSERT_TRUE(pager.CreateVmo(16, &root_vmo));

  // We are not testing page fault specifics, so just spin up a thread to handle all page faults.
  ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

  // Create first child slightly inset.
  std::unique_ptr<Vmo> child1 =
      root_vmo->Clone(zx_system_get_page_size(), zx_system_get_page_size() * 14);
  ASSERT_NOT_NULL(child1);

  // Fork some pages in the child.
  uint64_t val = 1;
  EXPECT_OK(child1->vmo().write(&val, 0, sizeof(uint64_t)));
  val = 2;
  EXPECT_OK(child1->vmo().write(&val, zx_system_get_page_size() * 4, sizeof(uint64_t)));
  val = 3;
  EXPECT_OK(child1->vmo().write(&val, zx_system_get_page_size() * 8, sizeof(uint64_t)));

  // Create a child that covers the full range.
  std::unique_ptr<Vmo> child2 = child1->Clone();

  // Create children that should always have at least 1 forked page (in child1), and validate they
  // see it.
  std::unique_ptr<Vmo> child3 = child2->Clone(0, zx_system_get_page_size() * 4);
  ASSERT_NOT_NULL(child2);

  EXPECT_OK(child3->vmo().read(&val, 0, sizeof(uint64_t)));
  EXPECT_EQ(val, 1u);
  // Rest of the vmo should be unchanged.
  EXPECT_TRUE(child3->CheckVmo(1, 3));
  // Hanging a large child in the non-forked portion of child3/2 should not see more of child2.
  std::unique_ptr<Vmo> child4 =
      child3->Clone(zx_system_get_page_size(), zx_system_get_page_size() * 4);
  ASSERT_NOT_NULL(child4);
  // First 3 pages should be original content, full view back to the root and no forked pages.
  EXPECT_TRUE(child4->CheckVmo(0, 3));
  // In the fourth page we should *not* see the forked page in child1 as we should have been clipped
  // by the limits of child3, and thus see zeros instead.
  EXPECT_OK(child4->vmo().read(&val, zx_system_get_page_size() * 3, sizeof(uint64_t)));
  EXPECT_NE(val, 2u);
  EXPECT_EQ(val, 0u);
  child4.reset();
  child3.reset();

  child3 = child2->Clone(zx_system_get_page_size(), zx_system_get_page_size() * 7);
  ASSERT_NOT_NULL(child3);
  // Here our page 3 should be the forked second page from child1, the rest should be original.
  EXPECT_TRUE(child3->CheckVmo(0, 2));
  EXPECT_TRUE(child3->CheckVmo(4, 3));
  EXPECT_OK(child3->vmo().read(&val, zx_system_get_page_size() * 3, sizeof(uint64_t)));
  EXPECT_EQ(val, 2u);
  // Create a child smaller than child3.
  child4 = child3->Clone(0, zx_system_get_page_size() * 6);
  ASSERT_NOT_NULL(child4);
  // Fork a new low page in child4
  val = 4;
  EXPECT_OK(child4->vmo().write(&val, 0, sizeof(uint64_t)));
  // Now create a child larger than child4
  std::unique_ptr<Vmo> child5 = child4->Clone(0, zx_system_get_page_size() * 10);
  ASSERT_NOT_NULL(child5);
  // Now create a child that skips the forked page in child 4, but sees the forked page in child1.
  std::unique_ptr<Vmo> child6 =
      child5->Clone(zx_system_get_page_size(), zx_system_get_page_size() * 7);
  ASSERT_NOT_NULL(child6);
  // Although we see the forked page in child1, due to our intermediate parent (child4) having a
  // limit of 5 pages relative to child6, that is the point at which our view back should terminate
  // and we should start seeing zeroes.
  EXPECT_TRUE(child6->CheckVmo(0, 2));
  EXPECT_OK(child6->vmo().read(&val, zx_system_get_page_size() * 2, sizeof(uint64_t)));
  EXPECT_EQ(val, 2u);
  EXPECT_TRUE(child6->CheckVmo(3, 2));
  EXPECT_OK(child6->vmo().read(&val, zx_system_get_page_size() * 5, sizeof(uint64_t)));
  EXPECT_EQ(val, 0u);
  EXPECT_OK(child6->vmo().read(&val, zx_system_get_page_size() * 6, sizeof(uint64_t)));
  EXPECT_EQ(val, 0u);
}

// Tests that a commit on a clone generates a single batch page request when the parent has no
// populated pages. Also verifies that pages are populated (copied into) the clone as expected.
TEST(Pager, CloneCommitSingleBatch) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 4;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  ASSERT_TRUE(t.Start());

  // Committing the clone should generate a batch request for pages [0, kNumPages).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kNumPages));

  ASSERT_TRUE(t.Wait());

  // Verify that the clone has all pages committed.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Tests that a commit on a clone generates two batch page requests when the parent has a page
// populated in the middle. Also verifies that pages are populated (copied into) the clone as
// expected.
TEST(Pager, CloneCommitTwoBatches) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 5;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  // Populate pages 2 in the parent, so it's already present before committing the clone.
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));

  ASSERT_TRUE(t.Start());

  // Batch request for pages [0, 2).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Batch request for pages [3, kNumPages).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 3, kNumPages - 3, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 3, kNumPages - 3));

  ASSERT_TRUE(t.Wait());

  // Verify that the clone has all pages committed.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Tests that a commit on a clone generates three batch page requests when the parent has two
// disjoint populated pages in the middle. Also verifies that pages are populated (copied into) the
// clone as expected.
TEST(Pager, CloneCommitMultipleBatches) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 8;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  // Populate pages 2 and 5 in the parent, so that the commit gets split up into 3 batch requests.
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));
  ASSERT_TRUE(pager.SupplyPages(vmo, 5, 1));

  ASSERT_TRUE(t.Start());

  // Batch request for pages [0, 2).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 2));

  // Batch request for pages [3, 5).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 3, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 3, 2));

  // Batch request for pages [6, kNumPages).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 6, kNumPages - 6, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 6, kNumPages - 6));

  ASSERT_TRUE(t.Wait());

  // Verify that the clone has all pages committed.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Tests that a commit on a clone populates pages as expected when the parent has some populated
// pages at random offsets. Also verifies that pages are populated (copied into) the clone as
// expected.
TEST(Pager, CloneCommitRandomBatches) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 100;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  TestThread t([clone = clone.get()]() -> bool { return clone->Commit(0, kNumPages); });

  // Populate around 25% of the parent's pages.
  std::vector<uint64_t> populated_offsets;
  for (uint64_t i = 0; i < kNumPages; i++) {
    if (rand() % 4) {
      continue;
    }
    ASSERT_TRUE(pager.SupplyPages(vmo, i, 1));
    populated_offsets.push_back(i);
  }

  ASSERT_TRUE(t.Start());

  uint64_t prev_offset = 0;
  for (uint64_t offset : populated_offsets) {
    // Supply pages in the range [prev_offset, offset).
    if (prev_offset < offset) {
      pager.SupplyPages(vmo, prev_offset, offset - prev_offset);
    }
    prev_offset = offset + 1;
  }
  // Supply pages in the last range [prev_offset, kNumPages).
  if (prev_offset < kNumPages) {
    pager.SupplyPages(vmo, prev_offset, kNumPages - prev_offset);
  }

  ASSERT_TRUE(t.Wait());

  // Verify that the clone has all pages committed.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_EQ(ZX_OK, clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Tests that the ZX_VMO_OP_ALWAYS_NEED hint works as expected.
TEST(Pager, EvictionHintAlwaysNeed) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 30;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // Hint ALWAYS_NEED on 5 pages starting at page 10. This will commit those pages and we should
  // see pager requests.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 10 * zx_system_get_page_size(),
                               5 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // Verify read requests for pages [10,15).
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 10, 5, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 10, 5));

  // The thread should now successfully terminate.
  ASSERT_TRUE(t.Wait());
}

// Tests that the zx_vmo_op_range() API succeeds and fails as expected for hints.
TEST(Pager, EvictionHintsOpRange) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 20;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 10));

  // Trivial success cases.
  ASSERT_OK(
      vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 0, 10 * zx_system_get_page_size(), nullptr, 0));
  ASSERT_OK(
      vmo->vmo().op_range(ZX_VMO_OP_DONT_NEED, 0, 20 * zx_system_get_page_size(), nullptr, 0));

  // Verify that offsets get aligned to page boundaries.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 15 * zx_system_get_page_size() - 8u, 16u,
                               nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // We should see read requests for pages 14 and 15.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 14, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 14, 2));

  ASSERT_TRUE(t.Wait());

  ASSERT_OK(vmo->vmo().op_range(ZX_VMO_OP_DONT_NEED, 32u, 20 * zx_system_get_page_size() - 64u,
                                nullptr, 0));

  // Hinting an invalid range should fail.
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 15 * zx_system_get_page_size(),
                                10 * zx_system_get_page_size(), nullptr, 0));
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo->vmo().op_range(ZX_VMO_OP_DONT_NEED, kNumPages * zx_system_get_page_size(),
                                20 * zx_system_get_page_size(), nullptr, 0));
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE,
            vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, zx_system_get_page_size(), UINT64_MAX,
                                nullptr, 0));
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, vmo->vmo().op_range(ZX_VMO_OP_DONT_NEED, zx_system_get_page_size(),
                                                     UINT64_MAX, nullptr, 0));

  // Hinting trivially succeeds for non-pager VMOs too. It will have no effect internally.
  zx::vmo vmo2;
  ASSERT_OK(zx::vmo::create(kNumPages, 0, &vmo2));
  ASSERT_OK(
      vmo2.op_range(ZX_VMO_OP_ALWAYS_NEED, 0, kNumPages * zx_system_get_page_size(), nullptr, 0));
  ASSERT_OK(
      vmo2.op_range(ZX_VMO_OP_DONT_NEED, 0, kNumPages * zx_system_get_page_size(), nullptr, 0));
}

// Tests that hints work when indicated via VMO clones too (where applicable).
TEST(Pager, EvictionHintsWithClones) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 40;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // Create a clone.
  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Supply a page in the parent, and fork it in the clone.
  pager.SupplyPages(vmo, 25, 1);
  uint8_t data = 0xc;
  clone->vmo().write(&data, 25 * zx_system_get_page_size(), sizeof(data));

  // Hint ALWAYS_NEED on a range including the forked page.
  TestThread t1([clone = clone.get()]() -> bool {
    return clone->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 23 * zx_system_get_page_size(),
                                 4 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // Verify read requests for all pages in the range [23,27) except the forked page 25.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 23, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 23, 2));
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 26, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 26, 1));

  // The thread should now successfully terminate.
  ASSERT_TRUE(t1.Wait());

  // Create a second level clone.
  auto clone2 = clone->Clone();
  ASSERT_NOT_NULL(clone2);

  // Fork another page in the intermediate clone.
  pager.SupplyPages(vmo, 30, 1);
  clone->vmo().write(&data, 30 * zx_system_get_page_size(), sizeof(data));

  // Hinting should work through the second level clone too.
  TestThread t2([clone = clone2.get()]() -> bool {
    return clone->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 29 * zx_system_get_page_size(),
                                 3 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t2.Start());

  // We should see read requests only for pages 29 and 31. Page 30 has been forked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 29, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 29, 1));
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 31, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 31, 1));

  // The thread should now successfully terminate.
  ASSERT_TRUE(t2.Wait());

  // Verify that we can hint DONT_NEED through both the clones without failing or generating new
  // page requests. Whether DONT_NEED pages are evicted is tested separately.
  ASSERT_OK(clone2->vmo().op_range(ZX_VMO_OP_DONT_NEED, 20 * zx_system_get_page_size(),
                                   8 * zx_system_get_page_size(), nullptr, 0));
  ASSERT_OK(clone->vmo().op_range(ZX_VMO_OP_DONT_NEED, 28 * zx_system_get_page_size(),
                                  10 * zx_system_get_page_size(), nullptr, 0));

  // No page requests are seen for the uncommitted pages.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that the ALWAYS_NEED hint works as expected with a racing VMO resize.
TEST(Pager, EvictionHintsWithResize) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 20;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_RESIZABLE, &vmo));

  // Hint ALWAYS_NEED on 10 pages starting at page 10. This will try to commit those pages and we
  // should see pager requests.
  TestThread t([vmo]() -> bool {
    return vmo->vmo().op_range(ZX_VMO_OP_ALWAYS_NEED, 10 * zx_system_get_page_size(),
                               10 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t.Start());

  // Supply a couple of pages, and then resize down across the hinted range, cutting it short.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 10, 10, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 10, 2));
  ASSERT_OK(vmo->vmo().set_size(12 * zx_system_get_page_size()));

  // The hinting range should terminate now.
  ASSERT_TRUE(t.Wait());

  // No more page requests are seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Tests that hints work as expected via zx_vmar_op_range().
TEST(Pager, EvictionHintsVmar) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 15 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create two pager VMOs.
  constexpr uint64_t kNumPages = 3;
  Vmo* vmo1;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo1));
  Vmo* vmo2;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo2));

  // Map the two VMOs with no gap in between.
  const uint64_t kVmoSize = kNumPages * zx_system_get_page_size();
  zx_vaddr_t addr1, addr2;
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, 0, vmo1->vmo(), 0, kVmoSize, &addr1));
  ASSERT_EQ(addr1, base_addr);
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, kVmoSize, vmo2->vmo(), 0, kVmoSize, &addr2));
  ASSERT_EQ(addr2, base_addr + kVmoSize);

  // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted pages.
  ASSERT_TRUE(pager.SupplyPages(vmo1, 1, 1));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 1, 1));

  // Also map in a non pager-backed VMO to work with.
  zx::vmo anon_vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &anon_vmo));
  zx_vaddr_t addr3;
  ASSERT_OK(
      vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, 2 * kVmoSize, anon_vmo, 0, kVmoSize, &addr3));
  ASSERT_EQ(addr3, base_addr + 2 * kVmoSize);

  ASSERT_OK(vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, 3 * kVmoSize, nullptr, 0));

  TestThread t1([&vmar, base_addr, kVmoSize]() -> bool {
    return vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, base_addr, 3 * kVmoSize, nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // We should see page requests for both VMOs, however we do not know if the pages we marked
  // DONT_NEED got evicted or not. As such there are three page request outcomes we might see
  //  1. Single request for all three pages (page was evicted before our ALWAYS_NEED op)
  //  2. Request for page 1 then request for pages 2 and 3 (page was evicted while servicing request
  //     for page 1)
  //  3. Request for page 1 then request for page 3 (page was not evicted at all)
  auto wait_for_pages = [&pager](Vmo* vmo) {
    uint64_t req_offset;
    uint64_t req_count;
    ASSERT_TRUE(pager.GetPageReadRequest(vmo, ZX_TIME_INFINITE, &req_offset, &req_count));
    // First page is always absent, so our request should be for that.
    ASSERT_EQ(req_offset, 0);
    // 1 or 3 pages
    ASSERT_TRUE(req_count == 3 || req_count == 1);
    pager.SupplyPages(vmo, 0, req_count);
    if (req_count == 3) {
      return;
    }
    // The next request is either just the last page, or the last two pages.
    ASSERT_TRUE(pager.GetPageReadRequest(vmo, ZX_TIME_INFINITE, &req_offset, &req_count));
    if (req_offset == 1) {
      ASSERT_EQ(req_count, 2);
      pager.SupplyPages(vmo, 1, 2);
    } else {
      ASSERT_EQ(req_offset, 2);
      ASSERT_EQ(req_count, 1);
      pager.SupplyPages(vmo, 2, 1);
    }
  };
  ASSERT_NO_FATAL_FAILURE(wait_for_pages(vmo1));
  ASSERT_NO_FATAL_FAILURE(wait_for_pages(vmo2));

  ASSERT_TRUE(t1.Wait());

  // This is redundant, but hinting again is harmless and should succeed.
  ASSERT_OK(vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, base_addr, 3 * kVmoSize, nullptr, 0));

  ASSERT_OK(vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, 3 * kVmoSize, nullptr, 0));

  // Can't hint on gaps in the VMAR.
  ASSERT_EQ(ZX_ERR_BAD_STATE,
            vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, kVmarSize, nullptr, 0));
}

// Tests that hints work as expected via zx_vmar_op_range(), when working with a nested VMAR tree.
TEST(Pager, EvictionHintsNestedVmar) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 10 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create two pager VMOs.
  constexpr uint64_t kNumPages = 3;
  const uint64_t kVmoSize = kNumPages * zx_system_get_page_size();
  Vmo* vmo1;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo1));
  Vmo* vmo2;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo2));

  // Create two sub-VMARs to hold the mappings, with no gap between them.
  zx::vmar sub_vmar1, sub_vmar2;
  zx_vaddr_t base_addr1, base_addr2;
  ASSERT_OK(vmar.allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC, 0,
      kVmoSize, &sub_vmar1, &base_addr1));
  ASSERT_EQ(base_addr1, base_addr);
  ASSERT_OK(vmar.allocate(
      ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_SPECIFIC, kVmoSize,
      kVmoSize, &sub_vmar2, &base_addr2));
  ASSERT_EQ(base_addr2, base_addr + kVmoSize);

  // Map the two VMOs in the two sub-VMARs.
  zx_vaddr_t addr1, addr2;
  ASSERT_OK(sub_vmar1.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, 0, vmo1->vmo(), 0, kVmoSize, &addr1));
  ASSERT_EQ(base_addr1, addr1);
  ASSERT_OK(sub_vmar2.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ, 0, vmo2->vmo(), 0, kVmoSize, &addr2));
  ASSERT_EQ(base_addr2, addr2);

  // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted pages.
  ASSERT_TRUE(pager.SupplyPages(vmo1, 1, 1));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 1, 1));

  ASSERT_OK(vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, 2 * kVmoSize, nullptr, 0));

  TestThread t1([&vmar, base_addr, kVmoSize]() -> bool {
    return vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, base_addr, 2 * kVmoSize, nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // We should see page requests for both VMOs.
  ASSERT_TRUE(pager.WaitForPageRead(vmo1, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo1, 0, 1));
  // The next page was committed and then marked DONT_NEED and could have been evicted already, so
  // get the next request manually and see where we're at.
  uint64_t req_offset;
  uint64_t req_count;
  ASSERT_TRUE(pager.GetPageReadRequest(vmo1, ZX_TIME_INFINITE, &req_offset, &req_count));
  if (req_offset == 1) {
    pager.SupplyPages(vmo1, 1, 1);
    ASSERT_TRUE(pager.WaitForPageRead(vmo1, 2, 1, ZX_TIME_INFINITE));
  } else {
    ASSERT_EQ(req_offset, 2);
  }
  ASSERT_TRUE(pager.SupplyPages(vmo1, 2, 1));
  ASSERT_TRUE(pager.WaitForPageRead(vmo2, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 0, 1));
  // Similar as before, page might have been evicted.
  ASSERT_TRUE(pager.GetPageReadRequest(vmo2, ZX_TIME_INFINITE, &req_offset, &req_count));
  if (req_offset == 1) {
    pager.SupplyPages(vmo2, 1, 1);
    ASSERT_TRUE(pager.WaitForPageRead(vmo2, 2, 1, ZX_TIME_INFINITE));
  } else {
    ASSERT_EQ(req_offset, 2);
  }
  ASSERT_TRUE(pager.SupplyPages(vmo2, 2, 1));

  ASSERT_TRUE(t1.Wait());

  ASSERT_OK(vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, 2 * kVmoSize, nullptr, 0));

  // Can't hint on gaps in the VMAR.
  ASSERT_EQ(ZX_ERR_BAD_STATE,
            vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, kVmarSize, nullptr, 0));
}

// Tests that hints work as expected via zx_vmar_op_range() with mapped clones.
TEST(Pager, EvictionHintsCloneVmar) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 5 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a VMO and a clone.
  constexpr uint64_t kNumPages = 4;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));
  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Map the clone.
  zx_vaddr_t addr;
  const uint64_t kVmoSize = kNumPages * zx_system_get_page_size();
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone->vmo(), 0,
                     kVmoSize, &addr));
  ASSERT_EQ(addr, base_addr);

  // Fork a page in the clone.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  uint8_t data = 0xcc;
  ASSERT_OK(clone->vmo().write(&data, zx_system_get_page_size(), sizeof(data)));

  TestThread t1([&vmar, base_addr]() -> bool {
    // Hint only a few pages, not all.
    return vmar.op_range(ZX_VMAR_OP_ALWAYS_NEED, base_addr + zx_system_get_page_size(),
                         2 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // We should see page requests for the root VMO only for pages that were not forked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));

  ASSERT_TRUE(t1.Wait());

  // The clone should only have one committed page, the one it forked previously.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_OK(clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  ASSERT_EQ(zx_system_get_page_size(), info.populated_bytes);
  uint8_t new_data;
  ASSERT_OK(clone->vmo().read(&new_data, zx_system_get_page_size(), sizeof(data)));
  ASSERT_EQ(data, new_data);

  // Can't hint on gaps in the VMAR.
  ASSERT_EQ(ZX_ERR_BAD_STATE,
            vmar.op_range(ZX_VMAR_OP_DONT_NEED, base_addr, kVmarSize, nullptr, 0));

  // No more page requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

TEST(Pager, ZeroHandlesRace) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  std::atomic<bool> running = true;
  // Keep the most recent pager handle stashed in an atomic. This lets the test synchronize the
  // handle value without causing undefined behavior with racy memory accesses.
  std::atomic<zx_handle_t> pager_handle = pager.get();

  std::thread thread([&pager_handle, &running] {
    zx::port port;
    ASSERT_OK(zx::port::create(0, &port));
    while (running) {
      zx_handle_t vmo;
      // Load the most recent pager handle value and attempt to create a vmo. This handle might have
      // already been closed, so this call could fail, so just ignore any errors.
      zx_status_t result = zx_pager_create_vmo(pager_handle.load(std::memory_order_relaxed), 0,
                                               port.get(), 0, zx_system_get_page_size(), &vmo);
      if (result == ZX_OK) {
        // If the call succeeded make sure to close the vmo to not leak the handle.
        zx_handle_close(vmo);
      }
    }
  });

  // Create and close pager handles in a loop. This is intended to trigger any race conditions that
  // might exist between on_zero_handles getting called, and an in-progress pager_create_vmo call.
  for (TestLimiter limiter(10000, zx::sec(60)); !limiter.Finished(); limiter.next()) {
    pager.reset();
    ASSERT_OK(zx::pager::create(0, &pager));
    pager_handle.store(pager.get(), std::memory_order_relaxed);
  }

  running = false;
  thread.join();
}

// Tests that OP_COMMIT works as expected via zx_vmar_op_range().
TEST(Pager, OpCommitVmar) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 15 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create two pager VMOs.
  constexpr uint64_t kNumPages = 3;
  Vmo* vmo1;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo1));
  Vmo* vmo2;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo2));

  // Map the two VMOs with no gap in between.
  const uint64_t kVmoSize = kNumPages * zx_system_get_page_size();
  zx_vaddr_t addr1, addr2;
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo1->vmo(), 0,
                     kVmoSize, &addr1));
  ASSERT_EQ(addr1, base_addr);
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, kVmoSize, vmo2->vmo(), 0,
                     kVmoSize, &addr2));
  ASSERT_EQ(addr2, base_addr + kVmoSize);

  // Supply a page in each VMO, so that we're working with a mix of committed and uncommitted pages.
  ASSERT_TRUE(pager.SupplyPages(vmo1, 1, 1));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 1, 1));

  // Also map in a non pager-backed VMO to work with.
  zx::vmo anon_vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &anon_vmo));
  zx_vaddr_t addr3;
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 2 * kVmoSize, anon_vmo, 0,
                     kVmoSize, &addr3));
  ASSERT_EQ(addr3, base_addr + 2 * kVmoSize);

  TestThread t1([&vmar, base_addr, kVmoSize]() -> bool {
    return vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr, 3 * kVmoSize, nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // We should see page requests for both VMOs.
  ASSERT_TRUE(pager.WaitForPageRead(vmo1, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo1, 0, 1));
  ASSERT_TRUE(pager.WaitForPageRead(vmo1, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo1, 2, 1));
  ASSERT_TRUE(pager.WaitForPageRead(vmo2, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 0, 1));
  ASSERT_TRUE(pager.WaitForPageRead(vmo2, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo2, 2, 1));

  ASSERT_TRUE(t1.Wait());

  // Can't commit with gaps in the VMAR.
  ASSERT_EQ(ZX_ERR_BAD_STATE, vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr, kVmarSize, nullptr, 0));
}

// Tests that OP_COMMIT works as expected via zx_vmar_op_range() with mapped clones.
TEST(Pager, OpCommitCloneVmar) {
  // Create a temporary VMAR to work with.
  auto root_vmar = zx::vmar::root_self();
  zx::vmar vmar;
  zx_vaddr_t base_addr;
  const uint64_t kVmarSize = 5 * zx_system_get_page_size();
  ASSERT_OK(root_vmar->allocate(ZX_VM_CAN_MAP_SPECIFIC | ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE,
                                0, kVmarSize, &vmar, &base_addr));
  auto destroy = fit::defer([&vmar]() { vmar.destroy(); });

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a VMO and a clone.
  constexpr uint64_t kNumPages = 4;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));
  auto clone = vmo->Clone();
  ASSERT_NOT_NULL(clone);

  // Map the clone.
  zx_vaddr_t addr;
  const uint64_t kVmoSize = kNumPages * zx_system_get_page_size();
  ASSERT_OK(vmar.map(ZX_VM_SPECIFIC | ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, clone->vmo(), 0,
                     kVmoSize, &addr));
  ASSERT_EQ(addr, base_addr);

  // Fork a page in the clone.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, 1));
  uint8_t data = 0xcc;
  ASSERT_OK(clone->vmo().write(&data, zx_system_get_page_size(), sizeof(data)));

  TestThread t1([&vmar, base_addr]() -> bool {
    // Commit only a few pages, not all.
    return vmar.op_range(ZX_VMAR_OP_COMMIT, base_addr + zx_system_get_page_size(),
                         2 * zx_system_get_page_size(), nullptr, 0) == ZX_OK;
  });
  ASSERT_TRUE(t1.Start());

  // We should see page requests for the root VMO only for pages that were not forked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 2, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 2, 1));

  ASSERT_TRUE(t1.Wait());

  // The clone should have two pages committed now.
  zx_info_vmo_t info;
  uint64_t a1, a2;
  ASSERT_OK(clone->vmo().get_info(ZX_INFO_VMO, &info, sizeof(info), &a1, &a2));
  ASSERT_EQ(2 * zx_system_get_page_size(), info.populated_bytes);

  // The previously forked page should not have been overwritten.
  uint8_t new_data;
  ASSERT_OK(clone->vmo().read(&new_data, zx_system_get_page_size(), sizeof(data)));
  ASSERT_EQ(data, new_data);

  // No more page requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
}

// Regression test for https://fxbug.dev/42173905. Tests that a port dequeue racing with pager
// destruction on a detached VMO does not result in use-after-frees.
TEST(Pager, RacyPortDequeue) {
  // Repeat multiple times so we can hit the race. 1000 is a good balance between trying to
  // reproduce the race without drastically increasing the test runtime.
  for (TestLimiter limiter(1000, zx::sec(60)); !limiter.Finished(); limiter.next()) {
    zx_handle_t pager;
    ASSERT_OK(zx_pager_create(0, &pager));

    zx_handle_t port;
    ASSERT_OK(zx_port_create(0, &port));

    zx_handle_t vmo;
    ASSERT_OK(zx_pager_create_vmo(pager, 0, port, 0, zx_system_get_page_size(), &vmo));

    std::atomic<bool> ready = false;
    TestThread t1([pager, &ready]() -> bool {
      while (!ready) {
        arch::Yield();
      }
      // Destroy the pager.
      return zx_handle_close(pager) == ZX_OK;
    });

    TestThread t2([port, &ready]() -> bool {
      while (!ready) {
        arch::Yield();
      }
      // Dequeue the complete packet from the port.
      zx_port_packet_t packet;
      zx_status_t status = zx_port_wait(port, 0u, &packet);
      // We can time out if the queued packet was successfully cancelled and taken back from the
      // port during pager destruction.
      return status == ZX_OK || status == ZX_ERR_TIMED_OUT;
    });

    // Destroy the vmo so that the complete packet is queued, and the page source is closed.
    ASSERT_OK(zx_handle_close(vmo));

    // Start both the threads.
    ASSERT_TRUE(t1.Start());
    ASSERT_TRUE(t2.Start());

    // Try to race the pager destruction with the port dequeue.
    ready = true;

    // Wait for both threads to exit.
    ASSERT_TRUE(t1.Wait());
    ASSERT_TRUE(t2.Wait());

    // Destroy the port.
    ASSERT_OK(zx_handle_close(port));
  }
}

TEST(Pager, PresentPageSplitsRange) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 5;
  constexpr uint64_t kSuppliedPageIndex = 2;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  // Supply page in the middle of the VMO.
  ASSERT_TRUE(pager.SupplyPages(vmo, kSuppliedPageIndex, 1));

  TestThread t([vmo]() -> bool { return vmo->Commit(0, kNumPages); });
  ASSERT_TRUE(t.Start());

  // Expect the first request to be the lower half of the VMO.
  ASSERT_TRUE(t.WaitForBlocked());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kSuppliedPageIndex, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kSuppliedPageIndex));

  // Expect the second request to be the upper half of the VMO.
  ASSERT_TRUE(t.WaitForBlocked());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, kSuppliedPageIndex + 1, kNumPages - kSuppliedPageIndex - 1,
                                    ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(vmo, kSuppliedPageIndex + 1, kNumPages - kSuppliedPageIndex - 1));

  ASSERT_TRUE(t.Wait());
}

// Tests that page request batching checks for pages present in ancestor VMOs before including a
// page in a batched request.
TEST(Pager, PageRequestBatchingChecksAncestorPages) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr uint64_t kNumPages = 8;

  Vmo* root_vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &root_vmo));

  zx::vmo child_vmo;
  ASSERT_OK(root_vmo->vmo().create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                                         kNumPages * zx_system_get_page_size(), &child_vmo));

  // Write to pages 1-2 in `child_vmo`.
  TestThread t1([&child_vmo] {
    const std::vector<char> buffer(2 * zx_system_get_page_size(), 'b');
    zx_status_t status =
        child_vmo.write(buffer.data(), 1 * zx_system_get_page_size(), buffer.size());
    return status == ZX_OK;
  });

  ASSERT_TRUE(t1.Start());
  ASSERT_TRUE(pager.SupplyPages(root_vmo, 1, 2));
  ASSERT_TRUE(t1.Wait());

  zx::vmo grandchild_vmo;
  ASSERT_OK(child_vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                                   kNumPages * zx_system_get_page_size(), &grandchild_vmo));

  // Write to pages 5-7 in `grandchild_vmo`.
  TestThread t2([&grandchild_vmo] {
    const std::vector<char> buffer(3 * zx_system_get_page_size(), 'c');
    zx_status_t status =
        grandchild_vmo.write(buffer.data(), 5 * zx_system_get_page_size(), buffer.size());
    return status == ZX_OK;
  });

  ASSERT_TRUE(t2.Start());
  ASSERT_TRUE(pager.SupplyPages(root_vmo, 5, 3));
  ASSERT_TRUE(t2.Wait());

  zx::vmo great_grandchild_vmo;
  ASSERT_OK(grandchild_vmo.create_child(
      ZX_VMO_CHILD_SLICE, 0, kNumPages * zx_system_get_page_size(), &great_grandchild_vmo));

  TestThread t3([&great_grandchild_vmo] {
    std::unique_ptr<char[]> data = std::make_unique<char[]>(kNumPages * zx_system_get_page_size());

    zx_status_t status =
        great_grandchild_vmo.read(data.get(), 0, kNumPages * zx_system_get_page_size());
    if (status != ZX_OK) {
      printf("Read failed with %d\n", status);
      return false;
    }

    // Ensure the contents of pages [1, 2] were from `child_vmo`.
    const std::vector<char> expected_from_child(2 * zx_system_get_page_size(), 'b');
    if (memcmp(&data[zx_system_get_page_size()], expected_from_child.data(),
               expected_from_child.size()) != 0) {
      return false;
    }

    // Ensure the contents of pages [5, 7] were from `grandchild_vmo`.
    const std::vector<char> expected_from_grandchild(3 * zx_system_get_page_size(), 'c');
    if (memcmp(&data[5 * zx_system_get_page_size()], expected_from_grandchild.data(),
               expected_from_grandchild.size()) != 0) {
      return false;
    }

    return true;
  });

  ASSERT_TRUE(t3.Start());

  // Expect and supply page requests for page ranges [0, 1) and [3, 5).
  ASSERT_TRUE(pager.WaitForPageRead(root_vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(root_vmo, 0, 1));
  ASSERT_TRUE(pager.WaitForPageRead(root_vmo, 3, 2, ZX_TIME_INFINITE));
  ASSERT_TRUE(pager.SupplyPages(root_vmo, 3, 2));

  ASSERT_TRUE(t3.Wait());
}

// After supply the pages the target thread remains logically blocked on a page request until it
// actually gets scheduled and makes progress out of the pager code. As such we cannot wait for
// it to block again, since we do not know whether it is in the previous or next block event.
// Therefore we just spin waiting to see the data.
void WaitForData(const void* expected, const volatile void* data, size_t size) {
  // data is volatile since we are intentionally not synchronizing the kernel, who could be
  // performing modifications to it as we are reading it. To avoid undefined behavior we must
  // therefore treat this entirely as volatile data, which means we cannot use existing methods like
  // memcmp, as they only work on non-volatile pointers.
  auto cmp = [expected, data, size]() {
    for (size_t i = 0; i < size; i++) {
      if (static_cast<const unsigned char*>(expected)[i] !=
          static_cast<const volatile unsigned char*>(data)[i]) {
        return false;
      }
    }
    return true;
  };
  // Just keep spinning until we either see the data or the test times out. Due to the potential of
  // being in a nested virtualization environment we have no other reliable way to know how long we
  // might have to wait for the data to appear.
  while (!cmp()) {
    zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));
  }
  // Although the above check was racy, once it succeeds we know we were able to see all the
  // expected data, however we still have not performed any synchronization. As such it would not
  // be correct to now perform `ASSERT(cmp())` since the loads could get re-ordered and/or
  // speculated prior to the cmp() in the `while` block completing. This could re-ordering /
  // speculation could result in an `ASSERT` here failing, even after the loop exited.
  //
  // As we are not using any atomic objects there is no way to create defined behavior in the C++
  // memory model that would allow us to put an `ASSERT(cmp())` here. We cannot even assume that the
  // kernel will perform an atomic operation to release a lock since we could observe the data being
  // present exactly as the kernel finishes its memcmp, but before it performs any other (atomic or
  // otherwise) operations.
  // Pragmatically we could perform a `std::atomic_thread_fence(std::memory_order_seq_cst)` and this
  // would force the compiler to emit a sufficient barrier, however it would not be doing so because
  // we are actually following its requirements, but rather because the compiler is unable to
  // observe enough of the system to notice that we are actually performing undefined behavior.
  //
  // If we really wanted to insert this ASSERT here we could treat the kernel like a device and act
  // like we are a driver and perform a read memory barrier (hw_rmb() in our ddk). This adds a
  // dependency on the ddk and is, arguably, making things more confusing when we can simply leave
  // it to the calling code to synchronize further with the kernel and perform a memcmp on a non
  // volatile, but synchronized, reference to the data.
}

TEST(Pager, EarlyWakeOnSupply) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr size_t kNumPages = 128;
  constexpr size_t kPageSteps = 32;
  static_assert(kNumPages % kPageSteps == 0);
  // Start the read at an offset to ensure there is a difference between the page requests VMO
  // offset, and the offset from the start of the request.
  constexpr size_t kStartOffset = 2;
  constexpr size_t kVmoPages = kNumPages + kStartOffset;

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kVmoPages, &vmo));

  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, kStartOffset);
  std::vector<uint8_t> data(kNumPages * zx_system_get_page_size(), 0);

  TestThread t([&vmo, &data] {
    return vmo->vmo().read(data.data(), kStartOffset * zx_system_get_page_size(),
                           kNumPages * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t.Start());

  // Expect a request for all the pages.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, kStartOffset, kNumPages, ZX_TIME_INFINITE));

  // Gradually supply all the pages, validating that data gets populated.
  for (size_t i = 0; i < kNumPages; i += kPageSteps) {
    // Thread should be blocked waiting.
    ASSERT_TRUE(t.WaitForBlocked());
    ASSERT_TRUE(pager.SupplyPages(vmo, i + kStartOffset, kPageSteps));

    WaitForData(&expected.data()[i * zx_system_get_page_size()],
                &data.data()[i * zx_system_get_page_size()],
                kPageSteps * zx_system_get_page_size());
  }
  ASSERT_TRUE(t.Wait());
  ASSERT_BYTES_EQ(expected.data(), data.data(), kNumPages * zx_system_get_page_size());
}

TEST(Pager, EarlyWakeNonLinearSupply) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr size_t kNumPages = 128;

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);
  std::vector<uint8_t> data(kNumPages * zx_system_get_page_size(), 0);

  TestThread t([&vmo, &data] {
    return vmo->vmo().read(data.data(), 0, kNumPages * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t.Start());

  // Expect a request for all the pages.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  // To account for pages potentially getting evicted start a page fault handler to re-supply
  // anything.
  vmo->SetPageFaultSupplyLimit(0);
  ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

  // Supply some blocks of pages not at the start of the request.
  ASSERT_TRUE(pager.SupplyPages(vmo, 16, 16));
  ASSERT_TRUE(pager.SupplyPages(vmo, 48, 16));

  // Now supply the first pages.
  vmo->SetPageFaultSupplyLimit(32);
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 16));

  // Expect everything to get copied all the way till the gap.
  WaitForData(expected.data(), data.data(), 32 * zx_system_get_page_size());

  // Supply the gap.
  vmo->SetPageFaultSupplyLimit(64);
  ASSERT_TRUE(pager.SupplyPages(vmo, 32, 16));

  // Expect everything till the next gap to be supplied.
  WaitForData(expected.data(), data.data(), 64 * zx_system_get_page_size());

  // Finally supply the rest of the data.
  vmo->SetPageFaultSupplyLimit(kNumPages);
  ASSERT_TRUE(pager.SupplyPages(vmo, 64, kNumPages - 64));
  ASSERT_TRUE(t.Wait());
  // No more page requests.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_BYTES_EQ(expected.data(), data.data(), kNumPages * zx_system_get_page_size());
}

TEST(Pager, EarlyWakeOverlappingRequests) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr size_t kVmoSize = 128;
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kVmoSize, &vmo));

  // Setup a collection of requests that overlap in different ways. The ranges listed here cannot
  // actually be changed arbitrarily, as the test code below hard codes various interesting values
  // and so while the test would not 'break', it would lose a lot of the desired coverage.
  struct {
    const uint64_t start;
    const uint64_t end;
    std::vector<uint8_t> data;
    std::optional<TestThread> thread;
  } requests[] = {{0, 92}, {16, 42}, {0, 32}, {48, 64}, {82, 128}};
  std::vector<uint8_t> expected(kVmoSize * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kVmoSize, 0);
  for (auto& req : requests) {
    req.data.resize((req.end - req.start) * zx_system_get_page_size(), 0);
    fit::function<bool()> start_method = [request = &req, &vmo] {
      return vmo->vmo().read(request->data.data(), request->start * zx_system_get_page_size(),
                             (request->end - request->start) * zx_system_get_page_size()) == ZX_OK;
    };
    req.thread.emplace(std::move(start_method));
  }
  // Start the first request and wait for it to generate the page request.
  ASSERT_TRUE(requests[0].thread->Start());
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, requests[0].end - requests[0].start, ZX_TIME_INFINITE));

  // Start all the other requests, should not see any additional page requests.
  for (size_t i = 1; i < sizeof(requests) / sizeof(requests[0]); i++) {
    ASSERT_TRUE(requests[i].thread->Start());
  }
  {
    // No more requests.
    uint64_t offset, length;
    ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  }

  auto validate_requests = [&](uint64_t max_page, uint64_t cutoff = UINT64_MAX) {
    for (auto& req : requests) {
      if (max_page < req.start || req.start >= cutoff) {
        continue;
      }
      WaitForData(&expected[req.start * zx_system_get_page_size()], req.data.data(),
                  (std::min(max_page, req.end) - req.start) * zx_system_get_page_size());
      // If the whole range has been supplied, also check that the thread has completed, unless we
      // already checked.
      if (max_page >= req.end && req.thread.has_value()) {
        ASSERT_TRUE(req.thread->Wait());
        req.thread.reset();
      }
    }
  };

  // To account for pages potentially getting evicted, and the fact that the validate_requests will
  // be re-scanning the entire VMO start a page fault handler to re-supply anything.
  vmo->SetPageFaultSupplyLimit(0);
  ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

  // Supply some early pages.
  vmo->SetPageFaultSupplyLimit(8);
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 8));
  validate_requests(8);

  // Pre-supply a later page at the start of an overlap.
  ASSERT_TRUE(pager.SupplyPages(vmo, 16, 1));

  // Supply the missing pages.
  vmo->SetPageFaultSupplyLimit(17);
  ASSERT_TRUE(pager.SupplyPages(vmo, 8, 8));
  // Everything up till page 16 should be visible, but some of the overlap requests may not have
  // woken up due to the pre-supply.
  validate_requests(17, 16);

  // Supply the next page in the line, finally waking up the overlap.
  vmo->SetPageFaultSupplyLimit(18);
  ASSERT_TRUE(pager.SupplyPages(vmo, 17, 1));
  validate_requests(18);

  // Pre-supply a later page in this range before the end of an overlapping request.
  ASSERT_TRUE(pager.SupplyPages(vmo, 30, 2));

  // Supply the gap.
  vmo->SetPageFaultSupplyLimit(32);
  ASSERT_TRUE(pager.SupplyPages(vmo, 18, 30 - 18));
  validate_requests(32);

  // Supply into the range of the start of the next overlap.
  vmo->SetPageFaultSupplyLimit(50);
  ASSERT_TRUE(pager.SupplyPages(vmo, 32, 50 - 32));
  validate_requests(50);

  // Supply all the way beyond the first request, completing the original sent page request.
  vmo->SetPageFaultSupplyLimit(100);
  ASSERT_TRUE(pager.SupplyPages(vmo, 50, 100 - 50));
  validate_requests(100);

  // Should have another page request for the remaining portion. Unfortunately this request gets
  // received by the page fault handler we started previously. What we can do instead is expect that
  // any thread that did not complete, should end up waiting for a page request.
  for (auto& req : requests) {
    if (req.thread.has_value()) {
      ASSERT_TRUE(req.thread->WaitForBlocked());
    }
  }

  // Supply the remaining pages.
  vmo->SetPageFaultSupplyLimit(kVmoSize);
  ASSERT_TRUE(pager.SupplyPages(vmo, 100, kVmoSize - 100));

  validate_requests(kVmoSize);
}

TEST(Pager, EarlyWakeWithResize) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  constexpr size_t kNumPages = 16;

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, ZX_VMO_RESIZABLE, &vmo));

  std::vector<uint8_t> data(kNumPages * zx_system_get_page_size(), 0);

  TestThread t([&vmo, &data] {
    return vmo->vmo().read(data.data(), 0, kNumPages * zx_system_get_page_size()) == ZX_OK;
  });

  ASSERT_TRUE(t.Start());

  // Expect a request for all the pages.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kNumPages, ZX_TIME_INFINITE));

  // Supply one page of the request.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // Wait for the data to appear and then for the thread to block again.
  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);
  WaitForData(expected.data(), data.data(), zx_system_get_page_size());
  // Wait for blocked again.
  ASSERT_TRUE(t.WaitForBlocked());

  // Resize the VMO smaller.
  ASSERT_TRUE(vmo->Resize(kNumPages / 2));

  // Supply the missing pages.
  ASSERT_TRUE(pager.SupplyPages(vmo, 1, kNumPages / 2 - 1));

  // Should end with failure.
  ASSERT_TRUE(t.WaitForFailure());
}

TEST(Pager, EarlyWakeCloneRace) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  constexpr size_t kNumPages = 1025;
  constexpr size_t kMiddlePage = kNumPages / 2;
  constexpr size_t kNumOverwritePages = 257;
  constexpr size_t kOvewritePagesStart = kMiddlePage - kNumOverwritePages / 2;

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(kNumPages, &vmo));

  std::unique_ptr<Vmo> clone = vmo->Clone();
  ASSERT_TRUE(clone);

  std::vector<uint8_t> expected(kNumPages * zx_system_get_page_size(), 0);
  vmo->GenerateBufferContents(expected.data(), kNumPages, 0);
  std::vector<uint8_t> data(kNumPages * zx_system_get_page_size(), 0);

  TestThread t([&clone, &data] {
    return clone->vmo().read(data.data(), 0, kNumPages * zx_system_get_page_size()) == ZX_OK;
  });

  // Before starting the test thread, supply an initial page in the middle of the range.
  ASSERT_TRUE(pager.SupplyPages(vmo, kMiddlePage, 1));
  ASSERT_TRUE(t.Start());

  // Expect to see a request just for the first empty run.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, kMiddlePage, ZX_TIME_INFINITE));

  // Zero some pages in the clone. As it operates on whole pages this does not generate a page
  // request.
  EXPECT_OK(clone->vmo().op_range(ZX_VMO_OP_ZERO, kOvewritePagesStart * zx_system_get_page_size(),
                                  kNumOverwritePages * zx_system_get_page_size(), nullptr, 0));

  // Now that they are defined as being zero, we can write to the cloned pages without causing an
  // allocation.
  for (size_t page = kOvewritePagesStart; page < kOvewritePagesStart + kNumOverwritePages; page++) {
    const size_t offset = page * zx_system_get_page_size();
    // Manipulate the data that we expect at this page to detect whether the test thread ends up
    // reading from the root or the forked page.
    memset(&expected[offset], 0, zx_system_get_page_size() / 2);
    EXPECT_OK(clone->vmo().write(&expected[offset], offset, zx_system_get_page_size()));
  }

  // Supply all but 1 of the pages in the original range. As the last page of the range has been
  // allocated in the clone, when the early wake happens for the content we have supplied it should
  // find the forked page and use it instead of continuing to wait for the redundant page in the
  // root.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, kMiddlePage - 1));

  // Expect to get a page request for the missing range in the combination of clone and parent.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, kOvewritePagesStart + kNumOverwritePages,
                                    kNumPages - (kOvewritePagesStart + kNumOverwritePages),
                                    ZX_TIME_INFINITE));

  ASSERT_TRUE(pager.SupplyPages(vmo, kOvewritePagesStart + kNumOverwritePages,
                                kNumPages - (kOvewritePagesStart + kNumOverwritePages)));

  // Validate everything completes and is correct.
  ASSERT_TRUE(t.Wait());

  EXPECT_BYTES_EQ(expected.data(), data.data(), kNumPages * zx_system_get_page_size());
}

TEST(Pager, Prefetch) {
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Test prefetching directly on the root.
  {
    Vmo* root_vmo;
    ASSERT_TRUE(pager.CreateVmo(1, &root_vmo));

    TestThread t([&root_vmo] { return root_vmo->Prefetch(0, 1); });
    ASSERT_TRUE(t.Start());

    EXPECT_TRUE(pager.WaitForPageRead(root_vmo, 0, 1, ZX_TIME_INFINITE));
    EXPECT_TRUE(pager.SupplyPages(root_vmo, 0, 1));
    pager.ReleaseVmo(root_vmo);
  }

  // Prefetch through a slice.
  {
    Vmo* root_vmo;
    ASSERT_TRUE(pager.CreateVmo(2, &root_vmo));
    std::unique_ptr<Vmo> slice =
        root_vmo->Clone(zx_system_get_page_size(), zx_system_get_page_size(), ZX_VMO_CHILD_SLICE);
    ASSERT_NOT_NULL(slice);

    TestThread t([&slice] { return slice->Prefetch(0, 1); });
    ASSERT_TRUE(t.Start());

    EXPECT_TRUE(pager.WaitForPageRead(root_vmo, 1, 1, ZX_TIME_INFINITE));
    EXPECT_TRUE(pager.SupplyPages(root_vmo, 1, 1));
    pager.ReleaseVmo(root_vmo);
  }

  // Prefetch through a clone.
  {
    Vmo* root_vmo;
    ASSERT_TRUE(pager.CreateVmo(2, &root_vmo));
    std::unique_ptr<Vmo> clone = root_vmo->Clone();
    ASSERT_NOT_NULL(clone);

    TestThread t([&clone] {
      // Perform a copy-on-write to the first page of the clone.
      *reinterpret_cast<volatile uint64_t*>(clone->base_addr()) = 42;
      // Now prefetch the entire clone.
      return clone->Prefetch(0, 2);
    });
    ASSERT_TRUE(t.Start());

    // Should expect a read for the copy-on-write page first.
    EXPECT_TRUE(pager.WaitForPageRead(root_vmo, 0, 1, ZX_TIME_INFINITE));
    EXPECT_TRUE(pager.SupplyPages(root_vmo, 0, 1));
    // Now expect a read for just the second page due to the prefetch. Even if the first page got
    // evicted due to a race, we still should not see a request for it because the clone has a CoW
    // view of the first page, and so it should not be requested.
    EXPECT_TRUE(pager.WaitForPageRead(root_vmo, 1, 1, ZX_TIME_INFINITE));
    EXPECT_TRUE(pager.SupplyPages(root_vmo, 1, 1));
  }
}

// Tests thread suspension when blocked on a page request from a page fault in kernel mode
// (usercopy).
TEST(Pager, SuspendInKernelTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Issue a syscall with an uncommitted pager-backed VMO passed in as the user buffer.
  TestThread t([vmo]() -> bool {
    // Create an anonymous VMO for the syscall to operate on.
    zx::vmo anon_vmo;
    if (zx::vmo::create(zx_system_get_page_size(), 0, &anon_vmo) != ZX_OK) {
      return false;
    }
    if (anon_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0) != ZX_OK) {
      return false;
    }
    // Map the pager-backed VMO to provide a user buffer address to the syscall.
    zx_vaddr_t buf;
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &buf) != ZX_OK) {
      return false;
    }
    auto unmap =
        fit::defer([&]() { zx_vmar_unmap(zx_vmar_root_self(), buf, zx_system_get_page_size()); });

    // This will fault on |buf| in the usercopy. zx_object_get_info() uses the variant of usercopy
    // that resolves page faults inline. This is the path we're testing here.
    if (anon_vmo.get_info(ZX_INFO_VMO, reinterpret_cast<void*>(buf), sizeof(zx_info_vmo_t), nullptr,
                          nullptr) != ZX_OK) {
      return false;
    }
    return true;
  });

  ASSERT_TRUE(t.Start());

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(t.WaitForBlocked());

  // Issue the suspend.
  t.Suspend();

  // Wait a bit and verify that the thread is still blocked. We're testing for inability to suspend,
  // so the best we can do is allow some time for the suspend to go through.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Resume the thread.
  t.Resume();

  // The thread is still blocked since the page request hasn't been resolved.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Suspend the thread again.
  t.Suspend();

  // Wait a bit and verify that the thread is still blocked.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Resolve the page request. We did not create the VMO with ZX_VMO_TRAP_DIRTY so the write will
  // proceed without blocking.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // The thread should now suspend on its way back to userspace from the syscall.
  ASSERT_TRUE(t.WaitForSuspend());

  // Resume the thread and wait for it to terminate.
  t.Resume();
  ASSERT_TRUE(t.Wait());

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Same as SuspendInKernelTest but tests the usercopy version that captures faults and lets the
// caller (in the kernel) resolve it.
TEST(Pager, SuspendInKernelCaptureFaultsTest) {
  UserPager pager;

  ASSERT_TRUE(pager.Init());

  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Issue a syscall with an uncommitted pager-backed VMO passed in as the user buffer.
  TestThread t([vmo]() -> bool {
    // Create an anonymous VMO for the syscall to operate on.
    zx::vmo anon_vmo;
    if (zx::vmo::create(zx_system_get_page_size(), 0, &anon_vmo) != ZX_OK) {
      return false;
    }
    if (anon_vmo.op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0) != ZX_OK) {
      return false;
    }
    // Map the pager-backed VMO to provide a user buffer address to the syscall.
    zx_vaddr_t buf;
    if (zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                                   zx_system_get_page_size(), &buf) != ZX_OK) {
      return false;
    }
    auto unmap =
        fit::defer([&]() { zx_vmar_unmap(zx_vmar_root_self(), buf, zx_system_get_page_size()); });

    // This will fault on |buf| in the usercopy. zx_vmo_write() uses the capture faults variant of
    // usercopy, and resolves them with an explicit SoftFault(). This is the path we're testing
    // here.
    if (anon_vmo.write(reinterpret_cast<void*>(buf), 0, zx_system_get_page_size()) != ZX_OK) {
      return false;
    }
    return true;
  });

  ASSERT_TRUE(t.Start());

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(t.WaitForBlocked());

  // Issue the suspend.
  t.Suspend();

  // Wait a bit and verify that the thread is still blocked. We're testing for inability to suspend,
  // so the best we can do is allow some time for the suspend to go through.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Resume the thread.
  t.Resume();

  // The thread is still blocked since the page request hasn't been resolved.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Suspend the thread again.
  t.Suspend();

  // Wait a bit and verify that the thread is still blocked.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(t.WaitForBlocked());

  // Resolve the page request.
  ASSERT_TRUE(pager.SupplyPages(vmo, 0, 1));

  // The thread should now suspend on its way back to userspace from the syscall.
  ASSERT_TRUE(t.WaitForSuspend());

  // Resume the thread and wait for it to terminate.
  t.Resume();
  ASSERT_TRUE(t.Wait());

  // No more page requests seen.
  uint64_t offset, length;
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
}

// Regression test for ensuring that pinning a pager backed VMO with loaned pages correctly replaces
// the loaned pages.
TEST(Pager, PinPagerVmoWithLoanedPages) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoTestCase::CompressedContiguous");

  zx_info_bti_t bti_info;
  EXPECT_OK(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr));

  constexpr size_t kNumPages = 2048;

  // Create single page contiguous VMOs to give us pages to decommit, but avoiding needing to
  // actually find a contiguous run of pages that could fail.
  zx::vmo contig_vmos[kNumPages];
  for (size_t i = 0; i < kNumPages; i++) {
    ASSERT_OK(zx::vmo::create_contiguous(bti, zx_system_get_page_size(), 0, &contig_vmos[i]));
  }

  // Now create the pager ready for the loaned pages.
  UserPager pager;
  ASSERT_TRUE(pager.Init());

  Vmo* pager_vmo;
  ASSERT_TRUE(pager.CreateVmoWithOptions(kNumPages, 0, &pager_vmo));

  // Decommit all the pages from the contiguous VMOs, being tolerant of borrowing potentially not
  // being enabled.
  for (size_t i = 0; i < kNumPages; i++) {
    zx_status_t status =
        contig_vmos[i].op_range(ZX_VMO_OP_DECOMMIT, 0, zx_system_get_page_size(), nullptr, 0);
    if (status == ZX_ERR_NOT_SUPPORTED) {
      printf("Page borrowing not enabled, skipping\n");
      return;
    }
    EXPECT_OK(status);
  }

  // Now supply the pager, hopefully causing some of the recently loaned pages to be used in the
  // supply step.
  EXPECT_TRUE(pager.SupplyPages(pager_vmo, 0, kNumPages));
  // Make sure any evictions just get a re-supply.
  EXPECT_TRUE(pager.StartTaggedPageFaultHandler());
  pager_vmo->SetPageFaultSupplyLimit(kNumPages);

  // Pin a single page from the pager vmo. This will mark the VMO overall as being pinned, and not
  // a candidate for receiving loaned pages.
  zx::pmt pmt;
  uint64_t addr;
  EXPECT_OK(
      bti.pin(ZX_BTI_PERM_READ, pager_vmo->vmo(), 0, zx_system_get_page_size(), &addr, 1, &pmt));

  // Pin the remaining portion of the VMO. Although the VMO is not presently eligible for receiving
  // loaned pages, it might still have loaned pages from the original supply, and these still need
  // to be replaced.
  // Failure mode on regression is a kernel panic finding that a page is still loaned when it
  // attempts to increment the pin count.
  zx::pmt pmt2;
  uint64_t addrs[kNumPages - 1];
  EXPECT_OK(bti.pin(ZX_BTI_PERM_READ, pager_vmo->vmo(), zx_system_get_page_size(),
                    (kNumPages - 1) * zx_system_get_page_size(), addrs, kNumPages - 1, &pmt2));

  // Recommit the contiguous VMOs. These pages should be available, and not pinned.
  for (size_t i = 0; i < kNumPages; i++) {
    EXPECT_OK(contig_vmos[i].op_range(ZX_VMO_OP_COMMIT, 0, zx_system_get_page_size(), nullptr, 0));
  }

  // Must explicitly unpin.
  EXPECT_OK(pmt2.unpin());
  EXPECT_OK(pmt.unpin());
}

// Regression test for https://fxbug.dev/502265328
TEST(Pager, CreateVmoUnboundedInvalidStreamSize) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  // The initial stream size of an unbounded vmo must be within the vmo size, and attempting to set
  // an overly large one shouldn't trigger a kernel panic.
  zx_status_t status = zx_pager_create_vmo(pager.get(), ZX_VMO_UNBOUNDED, port.get(), 0x1234,
                                           0xFFFFFFFFFFFFFFFF, vmo.reset_and_get_address());
  EXPECT_EQ(status, ZX_ERR_OUT_OF_RANGE);
}

// This is a regression test for https://fxbug.dev/498512247 that attempts to trigger a
// use-after-free in PagerProxy.  It is intended to be run under kasan.
TEST(Pager, ProxyUseAfterFree) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  // Close the pager, which will free the PagerDispatcher.
  pager.reset();

  // Create a new pager and query the stats in an attempt to trigger a use-after-free.
  ASSERT_OK(zx::pager::create(0, &pager));
  zx_pager_vmo_stats_t stats;
  zx_status_t status = zx_pager_query_vmo_stats(pager.get(), vmo.get(), 0, &stats, sizeof(stats));
  ASSERT_EQ(status, ZX_ERR_INVALID_ARGS);
}

// Regression test for https://fxbug.dev/505823417 that attempts to detach a pager VMO with pinned
// pages.
TEST(Pager, PagerDetachPinned) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    ZXTEST_SKIP("System resource not available");
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx_iommu_desc_stub_t desc;
  ASSERT_OK(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()));

  zx::bti bti;
  ASSERT_OK(zx::bti::create(iommu, 0, 0xdeadbeef, &bti));

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo pager_vmo;
  ASSERT_OK(
      zx_pager_create_vmo(pager.get(), 0, port.get(), 0, 4096, pager_vmo.reset_and_get_address()));

  zx::vmo aux_vmo;
  ASSERT_OK(zx::vmo::create(4096, 0, &aux_vmo));
  ASSERT_OK(aux_vmo.op_range(ZX_VMO_OP_COMMIT, 0, 4096, nullptr, 0));

  ASSERT_OK(zx_pager_supply_pages(pager.get(), pager_vmo.get(), 0, 4096, aux_vmo.get(), 0));

  zx::pmt pmt;
  zx_paddr_t paddr;
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, pager_vmo, 0, 4096, &paddr, 1, &pmt));

  // Detach the pager VMO.
  ASSERT_EQ(ZX_OK, zx_pager_detach_vmo(pager.get(), pager_vmo.get()));

  // Clean up!
  pmt.unpin();
}

// Test that ZX_VMO_OP_ALWAYS_NEED can be applied to a pager-backed VMO with the pager detached.
// Regression test for https://fxbug.dev/515718774.
TEST(Pager, DetachAndAlwaysNeed) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 1, 2u * zx_system_get_page_size(), &vmo));

  EXPECT_OK(pager.detach_vmo(vmo));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ALWAYS_NEED, 0, 2u * zx_system_get_page_size(), nullptr, 0));
}

}  // namespace pager_tests
