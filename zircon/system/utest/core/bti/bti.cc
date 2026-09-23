// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/process.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/types.h>

#include <atomic>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

namespace {

TEST(Bti, Create) {
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;

  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);
}

TEST(Bti, NameSupport) {
  zx::iommu iommu;
  zx::bti bti;

  zx_iommu_desc_stub_t desc;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  static char name_buffer[ZX_MAX_NAME_LEN];

  // Initially, there should be no name assigned to the BTI
  ASSERT_OK(bti.get_property(ZX_PROP_NAME, name_buffer, sizeof(name_buffer)));
  ASSERT_EQ(0, strlen(name_buffer));

  // Setting the name to normal name length should succeed.
  const char normal_name[] = "Core Test BTI";
  ASSERT_LE(strlen(normal_name), (ZX_MAX_NAME_LEN - 1), "normal_name would be truncated");
  ASSERT_OK(bti.set_property(ZX_PROP_NAME, normal_name, sizeof(normal_name)));
  ASSERT_OK(bti.get_property(ZX_PROP_NAME, name_buffer, sizeof(name_buffer)));
  ASSERT_STREQ(normal_name, name_buffer);

  // Setting the name to long_name should succeed, but the result will be truncated.
  const char long_name[] =
      "0123456789012345678901234567890123456789"
      "0123456789012345678901234567890123456789";
  ASSERT_GT(strlen(long_name), (ZX_MAX_NAME_LEN - 1), "long_name would not be truncated");
  ASSERT_OK(bti.set_property(ZX_PROP_NAME, long_name, sizeof(long_name)));
  ASSERT_OK(bti.get_property(ZX_PROP_NAME, name_buffer, sizeof(name_buffer)));
  ASSERT_EQ(0, name_buffer[sizeof(name_buffer) - 1]);
  ASSERT_BYTES_EQ(long_name, name_buffer, sizeof(name_buffer) - 1);

  // Setting the name to an empty string should be OK.
  const char empty_name[] = "";
  ASSERT_LE(strlen(empty_name), (ZX_MAX_NAME_LEN - 1), "empty_name would be truncated");
  ASSERT_OK(bti.set_property(ZX_PROP_NAME, empty_name, sizeof(empty_name)));
  ASSERT_OK(bti.get_property(ZX_PROP_NAME, name_buffer, sizeof(name_buffer)));
  ASSERT_STREQ(empty_name, name_buffer);
}

enum class VmoType : bool { Contiguous, NonContiguous };

void bti_pin_test_helper(VmoType vmo_type) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  static constexpr uint64_t kPageCount = 256;
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx::vmo vmo;
  if (vmo_type == VmoType::Contiguous) {
    ASSERT_EQ(zx::vmo::create_contiguous(bti, kVmoSize, 0, &vmo), ZX_OK);
  } else {
    ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);
  }

  zx_paddr_t paddrs[kPageCount];
  zx::pmt pmt;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt), ZX_OK);

  ASSERT_EQ(pmt.unpin(), ZX_OK);

  if (vmo_type == VmoType::Contiguous) {
    for (unsigned i = 1; i < kPageCount; i++) {
      ASSERT_EQ(paddrs[i], paddrs[0] + i * zx_system_get_page_size());
    }
  }
}

TEST(Bti, Pin) { bti_pin_test_helper(VmoType::NonContiguous); }

TEST(Bti, PinContiguous) { bti_pin_test_helper(VmoType::Contiguous); }

TEST(Bti, PinContigFlag) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  static constexpr uint64_t kPageCount = 256;
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create_contiguous(bti, kVmoSize, 0, &vmo), ZX_OK);

  zx_paddr_t paddr;
  zx::pmt pmt;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS, vmo, 0, kVmoSize, &paddr, 1, &pmt),
            ZX_OK);

  ASSERT_EQ(pmt.unpin(), ZX_OK);
}

TEST(Bti, Resize) {
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo), ZX_OK);

  zx_paddr_t paddrs;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, zx_system_get_page_size(), &paddrs, 1, &pmt), ZX_OK);

  EXPECT_EQ(vmo.set_size(0), ZX_ERR_BAD_STATE);

  pmt.unpin();
}

TEST(Bti, Clone) {
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  zx::vmo vmo, clone;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo), ZX_OK);
  ASSERT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone), ZX_OK);

  zx_paddr_t paddrs;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, clone, 0, zx_system_get_page_size(), &paddrs, 1, &pmt),
            ZX_OK);

  clone.reset();

  zx_signals_t o;
  EXPECT_EQ(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o), ZX_ERR_TIMED_OUT);

  pmt.unpin();

  EXPECT_EQ(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o), ZX_OK);
}

TEST(Bti, GetInfoTest) {
  zx::iommu iommu;
  zx::bti bti;
  zx::pmt pmt;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);
  // Query the info on the bti. It should have no pmos, and no quarantines:
  zx_info_bti_t bti_info;
  EXPECT_EQ(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr), ZX_OK);
  EXPECT_EQ(bti_info.pmo_count, 0);
  EXPECT_EQ(bti_info.quarantine_count, 0);

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo), ZX_OK);

  zx_paddr_t paddrs;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, zx_system_get_page_size(), &paddrs, 1, &pmt), ZX_OK);

  // Now our bti should have one pmo, and no quarantines:
  EXPECT_EQ(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr), ZX_OK);
  EXPECT_EQ(bti_info.pmo_count, 1);
  EXPECT_EQ(bti_info.quarantine_count, 0);

  // Delete pmt without unpinning. This should trigger a quarantine.
  pmt.reset();

  // Now our bti should have one pmo, and one quarantines:
  EXPECT_EQ(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr), ZX_OK);
  EXPECT_EQ(bti_info.pmo_count, 1);
  EXPECT_EQ(bti_info.quarantine_count, 1);

  EXPECT_EQ(bti.release_quarantine(), ZX_OK);
  // Now our bti should have no pmo, and no quarantines:
  EXPECT_EQ(bti.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr), ZX_OK);
  EXPECT_EQ(bti_info.pmo_count, 0);
  EXPECT_EQ(bti_info.quarantine_count, 0);
}

TEST(Bti, NoDelayedUnpin) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  // Create the VMO we will pin+unpin
  static constexpr uint64_t kPageCount = 4;
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  // Spin up a helper that will query handle information of the process. This helper should not
  // cause our unpins to be delayed.
  std::atomic<bool> running = true;

  std::thread thread = std::thread([&running] {
    // Create a vmo and clone it a few times with a semi random hierarchy. Vmo shall have a lot of
    // pages so that we can do long running writes to it.
    static constexpr uint64_t kPageCount = 4096;
    const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
    zx::vmo vmo;
    zx::vmo::create(kVmoSize, 0, &vmo);

    // Size clones so that our get_info call takes longer, but not too many as only the clone
    // handles that fall into the same batch (batches are currently 32 handles) as our pmt will
    // actually be useful.
    static constexpr int kClones = 16;
    zx::vmo clones[kClones];
    vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, &clones[0]);
    for (int i = 1; i < kClones; i++) {
      clones[rand() % i].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, &clones[i]);
    }
    // To ensure our info querying is slow, spin up another thread to do long running operations on
    // our vmo chain. When tested this made the get_info call take around 500ms.
    std::thread thread = std::thread([&running, &vmo, kVmoSize] {
      std::vector<uint8_t> buffer(kVmoSize);
      while (running) {
        vmo.write(buffer.data(), 0, kVmoSize);
      }
    });

    zx::unowned_process self_process{zx::process::self()};
    static constexpr int kMaxInfo = 1024;
    std::vector<zx_info_vmo_t> vmo_info(kMaxInfo);
    while (running) {
      size_t actual, avail;
      self_process->get_info(ZX_INFO_PROCESS_VMOS, vmo_info.data(),
                             kMaxInfo * sizeof(zx_info_vmo_t), &actual, &avail);
    }

    thread.join();
  });

  zx_paddr_t paddrs[kPageCount];

  // Perform pin+unpin+clone some arbitrary number of times to see if we hit the race condition.
  // This part of the test could spuriously succeed, but in my testing that never happened and
  // would typically fail around 1000 iterations in. Do 20000 iterations anyway since these
  // iterations are very fast and do not make the test take any noticeable time.
  for (int i = 0; i < 20000; i++) {
    zx::pmt pmt;
    ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt), ZX_OK);
    ASSERT_EQ(pmt.unpin(), ZX_OK);

    // After unpinning we should be able to make a clone.
    zx::vmo clone;
    ASSERT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, &clone), ZX_OK);
  }

  running = false;
  thread.join();
}

TEST(Bti, DecommitRace) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  // Create the VMO we will pin/decommit.
  constexpr uint64_t kPageCount = 64;
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &vmo), ZX_OK);

  // Spin up a helper that will perform the decommits.
  std::atomic<bool> running = true;

  // Flag that indicates the helper thread is up and running in case it takes a bit.
  std::atomic<bool> done_one_iteration = false;
  std::thread thread = std::thread([&running, &done_one_iteration, &vmo, kVmoSize] {
    while (running) {
      vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, kVmoSize, nullptr, 0);
      done_one_iteration = true;
    }
  });

  zx_paddr_t paddrs[kPageCount];

  // Wait until at least one iteration of the helper thread is done. Shouldn't take long so no need
  // to yield or sleep.
  while (!done_one_iteration)
    ;

  // Perform pin+unpin for 10 seconds or 20000 iterations (whichever happens
  // first) to see if we hit the race condition.
  const int max_iterations = 20000;
  const zx_duration_mono_ticks_t max_elapsed_ticks = 10 * zx::ticks::per_second().get();
  const zx::ticks start = zx::ticks::now();

  zx_duration_mono_ticks_t elapsed_ticks = 0;
  int iterations = 0;
  while (elapsed_ticks < max_elapsed_ticks && iterations < max_iterations) {
    zx::pmt pmt;
    ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt), ZX_OK);
    ASSERT_EQ(pmt.unpin(), ZX_OK);
    elapsed_ticks = (zx::ticks::now() - start).get();
    iterations++;
  }

  running = false;
  thread.join();
}

TEST(Bti, PinTooManyAddresses) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  const size_t page_size = zx_system_get_page_size();
  const uint64_t kPageCount = (page_size * 64) / sizeof(uint64_t) + 1;
  const uint64_t kVmoSize = page_size * kPageCount;

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  std::vector<zx_paddr_t> paddrs(kPageCount);
  zx::pmt pmt;
  ASSERT_EQ(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs.data(), kPageCount, &pmt),
            ZX_ERR_INVALID_ARGS);
}

TEST(Bti, QuarantineDisallowsPin) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  // Create and pin a VMO, then allow the pinned VMO to leak while still pinned.
  // Its pages will be added to the quarantine list for the BTI.
  constexpr uint64_t kPageCount = 4;
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx_paddr_t paddrs[kPageCount];
  {
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_OK(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
  }

  // Now that our BTI has a non-empty quarantine list, new pin operations should
  // fail with ZX_ERR_BAD_STATE.
  {
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_BAD_STATE,
                  bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
  }

  // Release the quarantine on our BTI, sending the quarantined pages back to
  // the page pool
  EXPECT_OK(bti.release_quarantine());

  // Try to pin some pages again.  Now that the quarantine list is clear, this
  // should be allowed again.  Don't forget to unpin the pages we had pinned.
  {
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_OK(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
    EXPECT_OK(pmt.unpin());
  }
}

// Test that various combinations of invalid arguments all fail with
// ZX_ERR_INVALID_ARGS.  This test also serves as a regression test for
// https://fxbug.dev/507926838
TEST(Bti, InvalidPinArgs) {
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;

  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_EQ(zx_iommu_create(iommu_resource.get(), ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc),
                            iommu.reset_and_get_address()),
            ZX_OK);
  ASSERT_EQ(zx::bti::create(iommu, 0, 0xdeadbeef, &bti), ZX_OK);

  constexpr uint64_t kPageCount = 4;
  const uint64_t kPageSize = zx_system_get_page_size();
  const uint64_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx_paddr_t paddrs[kPageCount];

  // Invalid argument flag.  Combine a valid flag, with a flag that is undefined
  // (bit 31).
  {
    constexpr uint32_t kInvalidFlag = ZX_BTI_PERM_READ | (uint32_t{1} << 31);
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                  bti.pin(kInvalidFlag, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
  }

  // Zero options.  Passing a zero for our options is invalid as it does not
  // specify at least one valid access flag (read, write, execute).
  {
    constexpr uint32_t kZeroFlag = 0;
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                  bti.pin(kZeroFlag, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
  }

  // Demand contiguous with non-contiguous VMO.  When using a Stub IOMMU, it is
  // an error to demand a contiguous pin when the VMO being supplied is not
  // explicitly contiguous.
  {
    constexpr uint32_t kContigFlag = ZX_BTI_CONTIGUOUS | ZX_BTI_PERM_READ;
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                  bti.pin(kContigFlag, vmo, 0, kVmoSize, paddrs, kPageCount, &pmt));
  }

  // Unaligned VMO offset.  When pinning VMOs, all offsets must be a multiple of
  // page size.
  {
    constexpr uint32_t kInvalidOffset = 13;
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS, bti.pin(ZX_BTI_PERM_READ, vmo, kInvalidOffset, kPageSize,
                                               paddrs, kPageCount, &pmt));
  }

  // Invalid non-zero size.  The size of a pin operation needs to be a multiple
  // of page size.
  {
    constexpr uint32_t kInvalidSize = 13;
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                  bti.pin(ZX_BTI_PERM_READ, vmo, 0, kInvalidSize, paddrs, kPageCount, &pmt));
  }

  // Invalid zero size.  The minimum that a user can pin is one page.  Zero
  // should be explicitly rejected.
  {
    zx::vmo vmo;
    zx::pmt pmt;
    EXPECT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
    EXPECT_STATUS(ZX_ERR_INVALID_ARGS,
                  bti.pin(ZX_BTI_PERM_READ, vmo, 0, 0, paddrs, kPageCount, &pmt));
  }
}

}  // namespace
