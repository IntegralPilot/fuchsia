// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <elf.h>
#include <lib/arch/intrin.h>
#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/pager.h>
#include <lib/zx/port.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <link.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/iommu.h>

#include <thread>
#include <utility>
#include <vector>

#include <zxtest/zxtest.h>

#include "helpers.h"

namespace vmo_test {
namespace {

// Some tests below rely on sampling the memory statistics and having only the
// page allocations directly incurred by the test code happen during the test.
// Those samples can be polluted by any COW faults taken by this program itself
// for touching its own data pages.  So avoid the pollution by preemptively
// faulting in all the static data pages beforehand.
class VmoClone2TestCase : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
    if (system_resource->is_valid()) {
      ASSERT_EQ(dl_iterate_phdr(&DlIterpatePhdrCallback, nullptr), 0);
    }
  }

 private:
  // Touch every page in the region to make sure it's been COW'd.
  __attribute__((no_sanitize("all"))) static void PrefaultPages(uintptr_t start, uintptr_t end) {
    while (start < end) {
      auto ptr = reinterpret_cast<volatile uintptr_t*>(start);
      *ptr = *ptr;
      start += zx_system_get_page_size();
    }
  }

  // Called on each loaded module to collect the bounds of its data pages.
  static void PrefaultData(const Elf64_Phdr* const phdrs, uint16_t phnum, uintptr_t bias) {
    // First find the RELRO segment, which may span part or all
    // of a writable segment (that's thus no longer actually writable).
    const Elf64_Phdr* relro = nullptr;
    for (uint_fast16_t i = 0; i < phnum; ++i) {
      const Elf64_Phdr* ph = &phdrs[i];
      if (ph->p_type == PT_GNU_RELRO) {
        relro = ph;
        break;
      }
    }

    // Now process each writable segment.
    for (uint_fast16_t i = 0; i < phnum; ++i) {
      const Elf64_Phdr* const ph = &phdrs[i];
      if (ph->p_type != PT_LOAD || !(ph->p_flags & PF_W)) {
        continue;
      }
      uintptr_t start = ph->p_vaddr;
      uintptr_t end = ph->p_vaddr + ph->p_memsz;
      ASSERT_LE(start, end);
      if (relro && relro->p_vaddr >= start && relro->p_vaddr < end) {
        start = relro->p_vaddr + relro->p_memsz;
        ASSERT_GE(start, ph->p_vaddr);
        if (start >= end) {
          continue;
        }
      }
      start = (start + zx_system_get_page_size() - 1) & -uintptr_t{zx_system_get_page_size()};
      end &= -uintptr_t{zx_system_get_page_size()};
      PrefaultPages(bias + start, bias + end);
    }
  }

  static int DlIterpatePhdrCallback(dl_phdr_info* info, size_t, void*) {
    PrefaultData(info->dlpi_phdr, info->dlpi_phnum, info->dlpi_addr);
    return 0;
  }
};

// Helper function which checks that the give vmo is contiguous.
template <size_t N>
void CheckContigState(const zx::bti& bti, const zx::vmo& vmo) {
  zx::pmt pmt;
  zx_paddr_t addrs[N];
  zx_status_t status =
      bti.pin(ZX_BTI_PERM_READ, vmo, 0, N * zx_system_get_page_size(), addrs, N, &pmt);
  ASSERT_OK(status, "pin failed");
  pmt.unpin();

  for (unsigned i = 0; i < N - 1; i++) {
    ASSERT_EQ(addrs[i] + zx_system_get_page_size(), addrs[i + 1]);
  }
}

// Helper function for CallPermutations
using PermutationsTestFn = void(uint32_t[]);
void CallPermutationsHelper(PermutationsTestFn* fn, uint32_t count, std::vector<uint32_t>& perm,
                            std::vector<bool>& elts, uint32_t idx) {
  if (idx == count) {
    ASSERT_NO_FATAL_FAILURE(fn(perm.data()));
    return;
  }
  for (unsigned i = 0; i < count; i++) {
    if (elts[i]) {
      continue;
    }

    elts[i] = true;
    perm[idx] = i;

    ASSERT_NO_FATAL_FAILURE(CallPermutationsHelper(fn, count, perm, elts, idx + 1));

    elts[i] = false;
  }
}

// Function which invokes |fn| with all the permutations of [0...count-1].
void CallPermutations(PermutationsTestFn* fn, uint32_t count) {
  std::vector<uint32_t> perm(count);
  std::vector<bool> elts(count);

  for (unsigned i = 0; i < count; i++) {
    perm[i] = 0;
    elts[i] = false;
  }

  ASSERT_NO_FATAL_FAILURE(CallPermutationsHelper(fn, count, perm, elts, 0));
}

// Checks the correctness of various zx_info_vmo_t properties.
TEST_F(VmoClone2TestCase, Info) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_info_vmo_t orig_info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &orig_info, sizeof(orig_info), nullptr, nullptr));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  zx_info_vmo_t new_info;
  EXPECT_OK(vmo.get_info(ZX_INFO_VMO, &new_info, sizeof(new_info), nullptr, nullptr));

  zx_info_vmo_t clone_info;
  EXPECT_OK(clone.get_info(ZX_INFO_VMO, &clone_info, sizeof(clone_info), nullptr, nullptr));

  // Check for consistency of koids.
  ASSERT_EQ(orig_info.koid, new_info.koid);
  ASSERT_NE(orig_info.koid, clone_info.koid);
  ASSERT_EQ(clone_info.parent_koid, orig_info.koid);

  // Check that flags are properly set.
  constexpr uint32_t kOriginalFlags = ZX_INFO_VMO_TYPE_PAGED | ZX_INFO_VMO_VIA_HANDLE;
  constexpr uint32_t kCloneFlags =
      ZX_INFO_VMO_TYPE_PAGED | ZX_INFO_VMO_IS_COW_CLONE | ZX_INFO_VMO_VIA_HANDLE;
  ASSERT_EQ(orig_info.flags, kOriginalFlags);
  ASSERT_EQ(new_info.flags, kOriginalFlags);
  ASSERT_EQ(clone_info.flags, kCloneFlags);
}

// Tests that reading from a clone gets the correct data.
TEST_F(VmoClone2TestCase, Read) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  static constexpr uint32_t kOriginalData = 0xdeadbeef;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, kOriginalData));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, kOriginalData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, kOriginalData));
}

// Tests that zx_vmo_write into the (clone|parent) doesn't affect the other.
void VmoWriteTestHelper(bool clone_write) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  static constexpr uint32_t kOriginalData = 0xdeadbeef;
  static constexpr uint32_t kNewData = 0xc0ffee;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, kOriginalData));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone_write ? clone : vmo, kNewData));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, clone_write ? kOriginalData : kNewData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, clone_write ? kNewData : kOriginalData));
}

TEST_F(VmoClone2TestCase, CloneVmoWrite) { ASSERT_NO_FATAL_FAILURE(VmoWriteTestHelper(true)); }

TEST_F(VmoClone2TestCase, ParentVmoWrite) { ASSERT_NO_FATAL_FAILURE(VmoWriteTestHelper(false)); }

// Tests that writing into the mapped (clone|parent) doesn't affect the other.
void VmarWriteTestHelper(bool clone_write) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  Mapping vmo_mapping;
  ASSERT_OK(vmo_mapping.Init(vmo, zx_system_get_page_size()));

  static constexpr uint32_t kOriginalData = 0xdeadbeef;
  static constexpr uint32_t kNewData = 0xc0ffee;
  *vmo_mapping.ptr() = kOriginalData;

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  Mapping clone_mapping;
  ASSERT_OK(clone_mapping.Init(clone, zx_system_get_page_size()));

  *(clone_write ? clone_mapping.ptr() : vmo_mapping.ptr()) = kNewData;

  ASSERT_EQ(*vmo_mapping.ptr(), clone_write ? kOriginalData : kNewData);
  ASSERT_EQ(*clone_mapping.ptr(), clone_write ? kNewData : kOriginalData);
}

TEST_F(VmoClone2TestCase, CloneVmarWrite) { ASSERT_NO_FATAL_FAILURE(VmarWriteTestHelper(true)); }

TEST_F(VmoClone2TestCase, ParentVmarWrite) { ASSERT_NO_FATAL_FAILURE(VmarWriteTestHelper(false)); }

// Tests that closing the (parent|clone) doesn't affect the other.
void CloseTestHelper(bool close_orig) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  static constexpr uint32_t kOriginalData = 0xdeadbeef;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, kOriginalData));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  (close_orig ? vmo : clone).reset();

  ASSERT_NO_FATAL_FAILURE(VmoCheck(close_orig ? clone : vmo, kOriginalData));
}

TEST_F(VmoClone2TestCase, CloseOriginal) {
  constexpr bool kCloseOriginal = true;
  ASSERT_NO_FATAL_FAILURE(CloseTestHelper(kCloseOriginal));
}

TEST_F(VmoClone2TestCase, CloseClone) {
  constexpr bool kCloseClone = false;
  ASSERT_NO_FATAL_FAILURE(CloseTestHelper(kCloseClone));
}

// Basic memory accounting test that checks vmo memory attribution.
TEST_F(VmoClone2TestCase, ObjMemAccounting) {
  const uint64_t kOriginalSize = 2ul * zx_system_get_page_size();
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();

  // Create a vmo, write to both pages, and check the committed stats.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kOriginalSize, 0, &vmo));

  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 1, 0));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 1, zx_system_get_page_size()));

  EXPECT_EQ(VmoPopulatedBytes(vmo), kOriginalSize);

  // Create a clone and check the initial committed stats.
  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the shared pages fronm the original VMO.
  // No VMO can be blamed for more pages than its total size.
  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kCloneSize, &clone));

  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginalSplit = zx_system_get_page_size();  // 1/2 + 1/2
  const uint64_t kImplCostCloneSplit = zx_system_get_page_size();     // 1/2 + 1/2
  ASSERT_LE(kImplCostOriginalSplit, kOriginalSize);
  ASSERT_LE(kImplCostCloneSplit, kCloneSize);
  ASSERT_EQ(kImplCostOriginalSplit + kImplCostCloneSplit, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginalSplit);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostCloneSplit);

  // Write to the original and check that forks a page into the clone.
  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the shared pages fronm the original VMO plus a
  // forked page.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 2, 0));

  const uint64_t kImplCostAll2 = kOriginalSize + zx_system_get_page_size();
  const uint64_t kImplCostOriginalSplit2 = 3ul * zx_system_get_page_size() / 2ul;  // 1 + 1/2
  const uint64_t kImplCostCloneSplit2 = 3ul * zx_system_get_page_size() / 2ul;     // 1 + 1/2
  ASSERT_LE(kImplCostOriginalSplit2, kOriginalSize);
  ASSERT_LE(kImplCostCloneSplit2, kCloneSize);
  ASSERT_EQ(kImplCostOriginalSplit2 + kImplCostCloneSplit2, kImplCostAll2);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginalSplit2);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostCloneSplit2);

  // Write to the clone and check that forks a page into the clone.
  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the shared pages fronm the original VMO plus both
  // forked pages.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 2, zx_system_get_page_size()));

  const uint64_t kImplCostAll3 = kOriginalSize + 2ul * zx_system_get_page_size();
  const uint64_t kImplCostOriginalSplit3 = 2ul * zx_system_get_page_size();  // 1 + 1
  const uint64_t kImplCostCloneSplit3 = 2ul * zx_system_get_page_size();     // 1 + 1
  ASSERT_LE(kImplCostOriginalSplit3, kOriginalSize);
  ASSERT_LE(kImplCostCloneSplit3, kCloneSize);
  ASSERT_EQ(kImplCostOriginalSplit3 + kImplCostCloneSplit3, kImplCostAll3);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginalSplit3);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostCloneSplit3);

  // Write to the other pages, which shouldn't affect accounting.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 2, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 2, 0));

  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginalSplit3);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostCloneSplit3);
}

// Tests that writes to a COW'ed zero page work and don't require redundant allocations.
TEST_F(VmoClone2TestCase, ZeroPageWrite) {
  const uint64_t kVmoSize = zx_system_get_page_size();

  zx::vmo vmos[4];

  // Create A VMO, two clones of the original, and one clone of one of those clones.
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, vmos));
  ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, vmos + 1));
  ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, vmos + 2));
  ASSERT_OK(vmos[1].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, vmos + 3));

  // Write to each VMO.
  // Each VMO has a private page.
  // Each VMO has exactly its total size in pages.
  for (unsigned i = 0; i < 4; i++) {
    ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[i], i + 1));
    for (unsigned j = 0; j < 4; j++) {
      ASSERT_NO_FATAL_FAILURE(VmoCheck(vmos[j], j <= i ? j + 1 : 0));
      ASSERT_EQ(VmoPopulatedBytes(vmos[j]), (j <= i ? 1u : 0u) * kVmoSize);
    }
  }
}

// Tests closing a vmo with the last reference to a mostly forked page.
TEST_F(VmoClone2TestCase, SplitPageClosure) {
  const uint64_t kOriginalPageCount = 1ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kClone1Size = zx_system_get_page_size();
  const uint64_t kClone2Size = zx_system_get_page_size();

  // Create a chain of clones.
  zx::vmo vmo, clone1, clone2;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone1Size, &clone1));
  ASSERT_OK(clone1.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone2Size, &clone2));

  // Fork the page into the two clones.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone1, 3));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, 4));

  // Each VMO has a private page.
  // Each VMO has exactly its total size in pages.
  ASSERT_EQ(VmoPopulatedBytes(vmo), kOriginalSize);
  ASSERT_EQ(VmoPopulatedBytes(clone1), kClone1Size);
  ASSERT_EQ(VmoPopulatedBytes(clone2), kClone2Size);

  // Close the original vmo, check that data is correct and things were freed.
  vmo.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, 3));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4));

  // The page should still be private in each remaining vmo.
  // Each VMO has exactly its total size in pages.
  ASSERT_TRUE(PollVmoPopulatedBytes(clone1, kClone1Size));
  ASSERT_TRUE(PollVmoPopulatedBytes(clone2, kClone2Size));

  // Close the first clone, check that data is correct and things were freed.
  clone1.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4));

  // The page should still be private in the last remaining vmo.
  // Each VMO has exactly its total size in pages.
  ASSERT_TRUE(PollVmoPopulatedBytes(clone2, kClone2Size));
}

// Tests that a clone with an offset accesses the right data and doesn't
// unnecessarily retain pages when the parent is closed.
TEST_F(VmoClone2TestCase, Offset) {
  const uint64_t kOriginalPageCount = 3ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = 3ul * zx_system_get_page_size();

  // Create a parent and child clone.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kCloneSize, &clone));

  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the shared pages from the original VMO.
  // No VMO can be blamed for more pages than its total size.
  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginalSplit = 2ul * zx_system_get_page_size();  // 1 + 1/2 + 1/2
  const uint64_t kImplCostCloneSplit = zx_system_get_page_size();           // 1/2 + 1/2
  ASSERT_LE(kImplCostOriginalSplit, kOriginalSize);
  ASSERT_LE(kImplCostCloneSplit, kCloneSize);
  ASSERT_EQ(kImplCostOriginalSplit + kImplCostCloneSplit, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginalSplit);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostCloneSplit);

  // Check that the child has the right data.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 3, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, 2 * zx_system_get_page_size()));

  // Write to one of the pages in the clone, close the original, and check that
  // things are still correct.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 4, zx_system_get_page_size()));
  vmo.reset();

  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 4, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, 2 * zx_system_get_page_size()));

  // The clone shouldn't retain the last page (it is a zero page) - the clone's
  // attributed memory is the size of the clone minus the last page.
  const uint64_t kImplCostCloneNoSplit = kCloneSize - zx_system_get_page_size();
  EXPECT_TRUE(PollVmoPopulatedBytes(clone, kImplCostCloneNoSplit));
}

// Tests writing to the clones of a clone created with an offset.
TEST_F(VmoClone2TestCase, OffsetTest2) {
  const uint64_t kOriginalPageCount = 4ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kOffsetSize = 3ul * zx_system_get_page_size();
  const uint64_t kClone1Size = 2ul * zx_system_get_page_size();
  const uint64_t kClone2Size = zx_system_get_page_size();

  // Create a parent and a child clone at an offset.
  // Create two clones-of-a-clone to fully divide the offset clone.
  zx::vmo vmo, offset_clone, clone1, clone2;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kOffsetSize,
                             &offset_clone));
  ASSERT_OK(offset_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone1Size, &clone1));
  ASSERT_OK(offset_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, kClone1Size, kClone2Size, &clone2));

  // Check that the clones have the right data.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, 2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, 3, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4));

  // Write to one of the pages in the offset clone, close the clone, and check that
  // things are still correct.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(offset_clone, 4, zx_system_get_page_size()));
  offset_clone.reset();

  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, 2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, 3, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4));

  // The original and clones split shared pages equally and each VMO's attributed
  // memory sums to the total - the shared pages fronm the original VMO.
  // No VMO can be blamed for more pages than its total size.
  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginalFullSplit =
      5ul * zx_system_get_page_size() / 2ul;                         // 1 + 1/2 + 1/2 + 1/2
  const uint64_t kImplCostClone1 = zx_system_get_page_size();        // 1/2 + 1/2
  const uint64_t kImplCostClone2 = zx_system_get_page_size() / 2ul;  // 1/2
  ASSERT_LE(kImplCostOriginalFullSplit, kOriginalSize);
  ASSERT_LE(kImplCostClone1, kClone1Size);
  ASSERT_LE(kImplCostClone2, kClone2Size);
  ASSERT_EQ(kImplCostOriginalFullSplit + kImplCostClone1 + kImplCostClone2, kImplCostAll);
  EXPECT_TRUE(PollVmoPopulatedBytes(vmo, kImplCostOriginalFullSplit));
  EXPECT_TRUE(PollVmoPopulatedBytes(clone1, kImplCostClone1));
  EXPECT_TRUE(PollVmoPopulatedBytes(clone2, kImplCostClone2));

  // Close the first clone and check that the total amount of allocated memory is still correct.
  clone1.reset();
  const uint64_t kImplCostOriginalPartialSplit =
      7ul * zx_system_get_page_size() / 2ul;  // 1 + 1 + 1 + 1/2
  ASSERT_LE(kImplCostOriginalPartialSplit, kOriginalSize);
  ASSERT_EQ(kImplCostOriginalPartialSplit + kImplCostClone2, kImplCostAll);
  EXPECT_TRUE(PollVmoPopulatedBytes(vmo, kImplCostOriginalPartialSplit));
  EXPECT_TRUE(PollVmoPopulatedBytes(clone2, kImplCostClone2));

  // Close the second clone and check that the total amount of allocated memory is still correct.
  clone2.reset();
  const uint64_t kImplCostOriginalNoSplit = 4ul * zx_system_get_page_size();  // 1 + 1 + 1 + 1
  ASSERT_EQ(kImplCostOriginalNoSplit, kOriginalSize);
  EXPECT_TRUE(PollVmoPopulatedBytes(vmo, kImplCostOriginalNoSplit));
}

// Tests writes to a page in a clone that is offset from the original and has a clone itself.
TEST_F(VmoClone2TestCase, OffsetProgressiveWrite) {
  const uint64_t kOriginalPageCount = 2ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();
  const uint64_t kClone2Size = zx_system_get_page_size();

  // Create a parent and a child clone at an offset.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kCloneSize, &clone));

  // Check that the child has the right data.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));

  // Write to the clone and check that everything still has the correct data.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 3));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 3));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 1));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 2, zx_system_get_page_size()));

  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the first private page from the original VMO, and
  // the 2 forked private copies of the second page from the original VMO.
  // No VMO can be blamed for more pages than its total size.
  const uint64_t kImplCostAll = kOriginalSize + zx_system_get_page_size();
  const uint64_t kImplCostOriginal = 2 * zx_system_get_page_size();  // 1 + 1
  const uint64_t kImplCostClone = zx_system_get_page_size();         // 1 + 0
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone, kImplCostAll);
  ASSERT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  ASSERT_EQ(VmoPopulatedBytes(clone), kImplCostClone);

  // Create a clone-of-a-clone at an offset.
  zx::vmo clone2;
  ASSERT_OK(
      clone.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kClone2Size, &clone2));

  // Write to the first clone again, and check that the write doesn't consume any
  // extra pages as the page isn't accessible by clone2.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 4));

  // The original and clone split shared pages equally and each VMO's attributed
  // memory sums to the total - the first private page from the original VMO, and
  // the 2 forked private copies of the second page from the original VMO.
  // No VMO can be blamed for more pages than its total size.
  const uint64_t kImplCostClone2 = 0;
  ASSERT_LE(kImplCostClone2, kClone2Size);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone + kImplCostClone2, kImplCostAll);
  ASSERT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  ASSERT_EQ(VmoPopulatedBytes(clone), kImplCostClone);
  ASSERT_EQ(VmoPopulatedBytes(clone2), kImplCostClone2);

  // Reset the original vmo and clone2, and make sure that the clone stays correct.
  vmo.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 4));
  clone2.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 4));

  // Check that the clone doesn't unnecessarily retain pages.
  // The clone shouldn't retain the last page (it is a zero page) - the clone's
  // attributed memory is the size of the clone minus the last page.
  const uint64_t kImplCost2Clone = kCloneSize - zx_system_get_page_size();
  ASSERT_TRUE(PollVmoPopulatedBytes(clone, kImplCost2Clone));
}

// Tests that a clone of a clone which overflows its parent properly interacts with
// both of its ancestors (i.e. the original vmo and the first clone).
TEST_F(VmoClone2TestCase, Overflow) {
  const uint64_t kOriginalPageCount = 1ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();
  const uint64_t kClone2Size = 3ul * zx_system_get_page_size();

  // Create a parent and a child clone.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kCloneSize, &clone));

  // Check that the child has the right data, and check accounting.
  // Total pages are the shared pages from the original VMO.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 1));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, zx_system_get_page_size()));

  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginal = zx_system_get_page_size() / 2;  // 1/2
  const uint64_t kImplCostClone = zx_system_get_page_size() / 2;     // 1/2 + 0
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);

  // Write to the child and then clone it.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 2, zx_system_get_page_size()));
  zx::vmo clone2;
  ASSERT_OK(clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone2Size, &clone2));

  // Check that the second clone is correct, including accounting.
  // Total pages are the shared page from the original VMO, plus the shared page
  // from the first clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 1));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 2, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 0, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost2All = kOriginalSize + zx_system_get_page_size();
  const uint64_t kImplCost2Original = zx_system_get_page_size() / 3;    // 1/3
  const uint64_t kImplCost2Clone = 5 * zx_system_get_page_size() / 6;   // 1/3 + 1/2
  const uint64_t kImplCost2Clone2 = 5 * zx_system_get_page_size() / 6;  // 1/3 + 1/2 + 0
  ASSERT_LE(kImplCost2Original, kOriginalSize);
  ASSERT_LE(kImplCost2Clone, kCloneSize);
  ASSERT_LE(kImplCost2Clone2, kClone2Size);
  // A remaining byte gets spread 1/3 between each of the VMOs, and we can check the fractional
  // bytes field for this. For this simple case we can do the fixed point arithmetic directly and
  // calculate a third of a whole byte for comparison.
  constexpr uint64_t kFractionalWholeByte = 0x8000000000000000;
  constexpr uint64_t kFractionalThirdByte = kFractionalWholeByte / 3u;
  EXPECT_EQ(kImplCost2Original + kImplCost2Clone + kImplCost2Clone2 + 1ul, kImplCost2All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost2Original);
  EXPECT_EQ(VmoPopulatedFractionalBytes(vmo), kFractionalThirdByte);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost2Clone);
  EXPECT_EQ(VmoPopulatedFractionalBytes(clone), kFractionalThirdByte);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost2Clone2);
  EXPECT_EQ(VmoPopulatedFractionalBytes(clone2), kFractionalThirdByte);

  // Write the private page in 2nd clone and then check that accounting is correct.
  // Total pages are the shared page from the original VMO, the shared page
  // from the first clone, and the private page in the 2nd clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 3, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost3All = kOriginalSize + 2ul * zx_system_get_page_size();
  const uint64_t kImplCost3Original = zx_system_get_page_size() / 3ul;       // 1/3
  const uint64_t kImplCost3Clone = 5ul * zx_system_get_page_size() / 6ul;    // 1/3 + 1/2
  const uint64_t kImplCost3Clone2 = 11ul * zx_system_get_page_size() / 6ul;  // 1/3 + 1/2 + 1
  ASSERT_LE(kImplCost3Original, kOriginalSize);
  ASSERT_LE(kImplCost3Clone, kCloneSize);
  ASSERT_LE(kImplCost3Clone2, kClone2Size);
  EXPECT_EQ(kImplCost3Original + kImplCost3Clone + kImplCost3Clone2 + 1ul, kImplCost3All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost3Original);
  EXPECT_EQ(VmoPopulatedFractionalBytes(vmo), kFractionalThirdByte);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost3Clone);
  EXPECT_EQ(VmoPopulatedFractionalBytes(clone), kFractionalThirdByte);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost3Clone2);
  EXPECT_EQ(VmoPopulatedFractionalBytes(clone2), kFractionalThirdByte);

  // Completely fork the final clone and check that things are correct, including accounting.
  // Total pages are the shared page from the original VMO, the private page
  // from the first clone, and the 3 private pages in the 2nd clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, 4, 0));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, 5, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 1, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 1, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 5, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 3, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost4All = kOriginalSize + 4ul * zx_system_get_page_size();
  const uint64_t kImplCost4Original = zx_system_get_page_size() / 2ul;     // 1/2
  const uint64_t kImplCost4Clone = 3ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1
  const uint64_t kImplCost4Clone2 = 3ul * zx_system_get_page_size();       // 1 + 1 + 1
  ASSERT_LE(kImplCost4Original, kOriginalSize);
  ASSERT_LE(kImplCost4Clone, kCloneSize);
  ASSERT_LE(kImplCost4Clone2, kClone2Size);
  ASSERT_EQ(kImplCost4Original + kImplCost4Clone + kImplCost4Clone2, kImplCost4All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost4Original);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost4Clone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost4Clone2);

  // Close the middle clone and check that things are still correct.
  // Total pages are the now-private page from the original VMO and the 3 private
  // pages in the 2nd clone.
  // No VMO can be blamed for more pages than its total size.
  clone.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 1, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 5, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 3, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost5All = kOriginalSize + 3ul * zx_system_get_page_size();
  const uint64_t kImplCost5Original = zx_system_get_page_size();      // 1
  const uint64_t kImplCost5Clone2 = 3ul * zx_system_get_page_size();  // 1 + 1 + 1
  ASSERT_LE(kImplCost5Original, kOriginalSize);
  ASSERT_LE(kImplCost5Clone2, kClone2Size);
  ASSERT_EQ(kImplCost5Original + kImplCost5Clone2, kImplCost5All);
  EXPECT_TRUE(PollVmoPopulatedBytes(vmo, kImplCost5Original));
  EXPECT_TRUE(PollVmoPopulatedBytes(clone2, kImplCost5Clone2));
}

// Test that a clone that does not overlap the parent at all behaves correctly.
TEST_F(VmoClone2TestCase, OutOfBounds) {
  const uint64_t kOriginalPageCount = 1ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();
  const uint64_t kClone2Size = 3ul * zx_system_get_page_size();

  // Create a parent and a child clone which does not overlap the parent.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 2 * zx_system_get_page_size(), kCloneSize, &clone));

  // Check that the child has the right data, and check accounting.
  // Total pages are the private page from the original VMO.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, zx_system_get_page_size()));

  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginal = zx_system_get_page_size();  // 1
  const uint64_t kImplCostClone = 0;
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);

  // Write to the child and then clone it.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 2, zx_system_get_page_size()));
  zx::vmo clone2;
  ASSERT_OK(clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone2Size, &clone2));

  // Check that the second clone is correct, including accounting.
  // Total pages are the private page from the original VMO, plus the shared page
  // from the first clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 0));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 2, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 0, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost2All = kOriginalSize + zx_system_get_page_size();
  const uint64_t kImplCost2Original = zx_system_get_page_size();      // 1
  const uint64_t kImplCost2Clone = zx_system_get_page_size() / 2ul;   // 0 + 1/2
  const uint64_t kImplCost2Clone2 = zx_system_get_page_size() / 2ul;  // 0 + 1/2 + 0
  ASSERT_LE(kImplCost2Original, kOriginalSize);
  ASSERT_LE(kImplCost2Clone, kCloneSize);
  ASSERT_LE(kImplCost2Clone2, kClone2Size);
  ASSERT_EQ(kImplCost2Original + kImplCost2Clone + kImplCost2Clone2, kImplCost2All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost2Original);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost2Clone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost2Clone2);

  // Write the dedicated page in 2nd child and then check that accounting is correct.
  // Total pages are the private page from the original VMO, the shared page from
  // the first clone, and the private page from the second clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 3, 2 * zx_system_get_page_size()));

  const uint64_t kImplCost3All = kOriginalSize + 2ul * zx_system_get_page_size();
  const uint64_t kImplCost3Original = zx_system_get_page_size();            // 1
  const uint64_t kImplCost3Clone = zx_system_get_page_size() / 2ul;         // 0 + 1/2
  const uint64_t kImplCost3Clone2 = 3ul * zx_system_get_page_size() / 2ul;  // 0 + 1/2 + 1
  ASSERT_LE(kImplCost3Original, kOriginalSize);
  ASSERT_LE(kImplCost3Clone, kCloneSize);
  ASSERT_LE(kImplCost3Clone2, kClone2Size);
  ASSERT_EQ(kImplCost3Original + kImplCost3Clone + kImplCost3Clone2, kImplCost3All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost3Original);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost3Clone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost3Clone2);
}

// Tests that a small clone doesn't require allocations for pages which it doesn't
// have access to and that unneeded pages get freed if the original vmo is closed.
TEST_F(VmoClone2TestCase, SmallClone) {
  const uint64_t kOriginalPageCount = 3ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = zx_system_get_page_size();

  // Create a parent and a child clone which does not overlap the parent.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kCloneSize, &clone));

  // Check that the child has the right data, and check accounting.
  // Total pages are the pages from the original VMO - one shared with the clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));

  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginal = 5ul * zx_system_get_page_size() / 2ul;  // 1 + 1/2 + 1
  const uint64_t kImplCostClone = zx_system_get_page_size() / 2ul;           // 1/2
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);

  // Check that a write into the original VMO out of bounds of the first clone
  // doesn't allocate any memory.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 4, 0));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 5, 2 * zx_system_get_page_size()));
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);

  // Close the parent and check that clone has the right data and accounting.
  // Total pages are the shared page from the parent which migrated into the clone.
  // No VMO can be blamed for more pages than its total size.
  vmo.reset();
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));

  const uint64_t kImplCost2Clone = zx_system_get_page_size();  // 1
  ASSERT_LE(kImplCost2Clone, kCloneSize);
  EXPECT_TRUE(PollVmoPopulatedBytes(clone, kImplCost2Clone));
}

// Tests that a small clone properly interrupts access into the parent.
TEST_F(VmoClone2TestCase, SmallCloneChild) {
  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(3, &vmo));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                             zx_system_get_page_size(), &clone));

  // Check that the child has the right data.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));

  // Create a clone of the first clone and check that it has the right data (incl. that
  // it can't access the original vmo).
  zx::vmo clone2;
  ASSERT_OK(clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, 2 * zx_system_get_page_size(), &clone2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 2));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 0, zx_system_get_page_size()));
}

// Tests that closing a vmo with multiple small clones properly frees pages.
TEST_F(VmoClone2TestCase, SmallClones) {
  const uint64_t kOriginalPageCount = 3ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();
  const uint64_t kClone2Size = zx_system_get_page_size();

  // Create a parent and a child clone.  Populate a page in the child.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(), kCloneSize, &clone));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 4, zx_system_get_page_size()));

  // Create a second clone of the original.
  zx::vmo clone2;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone2Size, &clone2));

  // Check that the child has the right data, and check accounting.
  // Total pages are the shared pages from the original VMO and one private page in
  // the first clone.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));

  const uint64_t kImplCostAll = kOriginalSize + zx_system_get_page_size();
  const uint64_t kImplCostOriginal = 2ul * zx_system_get_page_size();     // 1/2 + 1/2 + 1
  const uint64_t kImplCostClone = 3ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1
  const uint64_t kImplCostClone2 = zx_system_get_page_size() / 2ul;       // 1/2
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_LE(kImplCostClone2, kClone2Size);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone + kImplCostClone2, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCostClone2);

  // Close the original VMO and then check accounting.
  // The original's inaccessible 3rd page should be freed, and original's page 2
  // should havce migrated to the first clone.
  // Total pages are the now-private pages in each clone.
  vmo.reset();

  const uint64_t kImplCost2All = 3ul * zx_system_get_page_size();
  const uint64_t kImplCost2Clone = 2ul * zx_system_get_page_size();  // 1 + 1
  const uint64_t kImplCost2Clone2 = zx_system_get_page_size();       // 1
  ASSERT_LE(kImplCost2Clone, kCloneSize);
  ASSERT_LE(kImplCost2Clone2, kClone2Size);
  ASSERT_EQ(kImplCost2Clone + kImplCost2Clone2, kImplCost2All);
  EXPECT_TRUE(PollVmoPopulatedBytes(clone, kImplCost2Clone));
  EXPECT_TRUE(PollVmoPopulatedBytes(clone2, kImplCost2Clone2));
}

// Tests that disjoint clones work (i.e. create multiple clones, none of which
// overlap) and that they don't unnecessarily retain/allocate memory after
// closing the original VMO. This tests two cases - resetting the original vmo
// before writing to the clones and resetting the original vmo after writing to
// the clones.
struct VmoCloneDisjointClonesTests : public VmoClone2TestCase {
  static void DisjointClonesTest(bool early_close) {
    zx::vmo vmo;
    ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(4, &vmo));

    // Create a disjoint clone for each page in the original vmo: 2 direct and 2 through another
    // intermediate COW clone.
    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 1 * zx_system_get_page_size(),
                               2 * zx_system_get_page_size(), &clone));

    zx::vmo leaf_clones[4];
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), leaf_clones));
    ASSERT_OK(
        clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), leaf_clones + 1));
    ASSERT_OK(clone.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                                 zx_system_get_page_size(), leaf_clones + 2));
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 3 * zx_system_get_page_size(),
                               zx_system_get_page_size(), leaf_clones + 3));

    if (early_close) {
      vmo.reset();
      clone.reset();
    }

    // Check that each clone's has the correct data and then write to the clone.
    for (unsigned i = 0; i < 4; i++) {
      ASSERT_NO_FATAL_FAILURE(VmoCheck(leaf_clones[i], i + 1));
      ASSERT_NO_FATAL_FAILURE(VmoWrite(leaf_clones[i], i + 5));
    }

    if (!early_close) {
      // The number of allocated pages is implementation dependent, but it must be less
      // than the total user-visible vmo size.
      constexpr uint32_t kImplTotalPages = 10;
      static_assert(kImplTotalPages <= 10);
      vmo.reset();
      clone.reset();
    }

    // Check that the clones have the correct data and that nothing
    // is unnecessary retained/allocated.
    for (unsigned i = 0; i < 4; i++) {
      ASSERT_NO_FATAL_FAILURE(VmoCheck(leaf_clones[i], i + 5));
      ASSERT_TRUE(PollVmoPopulatedBytes(leaf_clones[i], zx_system_get_page_size()));
    }
  }
};

TEST_F(VmoCloneDisjointClonesTests, DisjointCloneEarlyClose) {
  ASSERT_NO_FATAL_FAILURE(DisjointClonesTest(true));
}

TEST_F(VmoCloneDisjointClonesTests, DisjointCloneLateClose) {
  ASSERT_NO_FATAL_FAILURE(DisjointClonesTest(false));
}

// A second disjoint clone test that checks that closing the disjoint clones which haven't
// yet been written to doesn't affect the contents of other disjoint clones.
TEST_F(VmoClone2TestCase, DisjointCloneTest2) {
  auto test_fn = [](uint32_t perm[]) -> void {
    zx::vmo vmo;
    ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(4, &vmo));

    // Create a disjoint clone for each page in the original vmo: 2 direct and 2 through another
    // intermediate COW clone.
    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 1 * zx_system_get_page_size(),
                               2 * zx_system_get_page_size(), &clone));

    zx::vmo leaf_clones[4];
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), leaf_clones));
    ASSERT_OK(
        clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), leaf_clones + 1));
    ASSERT_OK(clone.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                                 zx_system_get_page_size(), leaf_clones + 2));
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 3 * zx_system_get_page_size(),
                               zx_system_get_page_size(), leaf_clones + 3));

    vmo.reset();
    clone.reset();

    // Check that each clone's has the correct data and then write to the clone.
    for (unsigned i = 0; i < 4; i++) {
      ASSERT_NO_FATAL_FAILURE(VmoCheck(leaf_clones[i], i + 1));
    }

    // Close the clones in the order specified by |perm|, and at each step
    // check the rest of the clones.
    bool closed[4] = {};
    for (unsigned i = 0; i < 4; i++) {
      leaf_clones[perm[i]].reset();
      closed[perm[i]] = true;

      for (unsigned j = 0; j < 4; j++) {
        if (!closed[j]) {
          ASSERT_NO_FATAL_FAILURE(VmoCheck(leaf_clones[j], j + 1));
          ASSERT_TRUE(PollVmoPopulatedBytes(leaf_clones[j], zx_system_get_page_size()));
        }
      }
    }
  };

  ASSERT_NO_FATAL_FAILURE(CallPermutations(test_fn, 4));
}

// Tests a case where a clone is written to and then a series of subsequent clones
// are created with various offsets and sizes. This test is constructed to catch issues
// due to partial COW releases in the current implementation.
TEST_F(VmoClone2TestCase, DisjointCloneProgressive) {
  zx::vmo vmo, main_clone, clone1, clone2, clone3, clone4;

  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(6, &vmo));

  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                             5 * zx_system_get_page_size(), &main_clone));

  ASSERT_NO_FATAL_FAILURE(VmoWrite(main_clone, 7, 3 * zx_system_get_page_size()));

  // A clone which references the written page.
  ASSERT_OK(main_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 1 * zx_system_get_page_size(),
                                    4 * zx_system_get_page_size(), &clone1));
  // A clone after the written page.
  ASSERT_OK(main_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 4 * zx_system_get_page_size(),
                                    1 * zx_system_get_page_size(), &clone2));
  // A clone before the written page.
  ASSERT_OK(main_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 2 * zx_system_get_page_size(),
                                    1 * zx_system_get_page_size(), &clone3));
  // A clone which doesn't reference any pages, but it needs to be in the clone tree.
  ASSERT_OK(main_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 10 * zx_system_get_page_size(),
                                    1 * zx_system_get_page_size(), &clone4));

  main_clone.reset();
  clone1.reset();
  clone3.reset();
  clone4.reset();
  clone2.reset();

  zx::vmo last_clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, 6 * zx_system_get_page_size(), &last_clone));
  for (unsigned i = 0; i < 6; i++) {
    ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, i + 1, i * zx_system_get_page_size()));
    ASSERT_NO_FATAL_FAILURE(VmoCheck(last_clone, i + 1, i * zx_system_get_page_size()));
  }

  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 8, 4 * zx_system_get_page_size()));

  for (unsigned i = 0; i < 6; i++) {
    ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, i == 4 ? 8 : i + 1, i * zx_system_get_page_size()));
    ASSERT_NO_FATAL_FAILURE(VmoCheck(last_clone, i + 1, i * zx_system_get_page_size()));
  }
}

enum class Contiguity {
  Contig,
  NonContig,
};

enum class ResizeTarget {
  Parent,
  Child,
};

// Tests that resizing a (clone|cloned) vmo frees unnecessary pages.
class VmoCloneResizeTests : public VmoClone2TestCase {
 protected:
  static void ResizeTest(Contiguity contiguity, ResizeTarget target) {
    bool contiguous = contiguity == Contiguity::Contig;
    bool resize_child = target == ResizeTarget::Child;
    zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
    if (contiguous && !system_resource->is_valid()) {
      printf("System resource not available, skipping\n");
      return;
    }

    // Create a vmo and a clone of the same size.
    zx::iommu iommu;
    zx::bti bti;
    zx::vmo vmo;
    auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

    if (contiguous) {
      zx_iommu_desc_stub_t desc;

      zx::result<zx::resource> result =
          maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
      ASSERT_OK(result.status_value());
      zx::resource iommu_resource = std::move(result.value());

      ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
      ASSERT_NO_FAILURES(bti =
                             vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoCloneResizeTests"));
      ASSERT_OK(zx::vmo::create_contiguous(bti, 4 * zx_system_get_page_size(), 0, &vmo));
    } else {
      ASSERT_OK(zx::vmo::create(4 * zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));
    }

    for (unsigned i = 0; i < 4; i++) {
      ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, i + 1, i * zx_system_get_page_size()));
    }

    zx::vmo clone;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE, 0,
                               4 * zx_system_get_page_size(), &clone));

    // Write to one page in each vmo.
    ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 5, zx_system_get_page_size()));
    ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 5, 2 * zx_system_get_page_size()));

    // Check attribution for the vmos.
    // The 2 pages written after cloning are each private to both the original
    // vmo and the clone.
    // The 2 pages unwritten after cloning are both shared equally.
    const uint64_t kOriginalAttrBytes = 3ul * zx_system_get_page_size();  // 1/2 + 1 + 1 + 1/2
    const uint64_t kCloneAttrBytes = 3ul * zx_system_get_page_size();     // 1/2 + 1 + 1 + 1/2
    EXPECT_EQ(VmoPopulatedBytes(vmo), kOriginalAttrBytes);
    EXPECT_EQ(VmoPopulatedBytes(clone), kCloneAttrBytes);

    const zx::vmo& resize_target = resize_child ? clone : vmo;
    const zx::vmo& original_size_vmo = resize_child ? vmo : clone;

    if (contiguous && !resize_child) {
      // Contiguous vmos can't be resizable.
      ASSERT_EQ(resize_target.set_size(zx_system_get_page_size()), ZX_ERR_UNAVAILABLE);
      return;
    } else {
      ASSERT_OK(resize_target.set_size(zx_system_get_page_size()));
    }

    // Check that the data in both vmos is correct.
    for (unsigned i = 0; i < 4; i++) {
      // The index of original_size_vmo's page we wrote to depends on which vmo it is
      uint32_t written_page_idx = resize_child ? 1 : 2;
      // If we're checking the page we wrote to, look for 5, otherwise look for the tagged value.
      uint32_t expected_val = i == written_page_idx ? 5 : i + 1;
      ASSERT_NO_FATAL_FAILURE(
          VmoCheck(original_size_vmo, expected_val, i * zx_system_get_page_size()));
    }
    ASSERT_NO_FATAL_FAILURE(VmoCheck(resize_target, 1));

    // Check attribution for the vmos.
    // The first page is unwritten after the cloning and shared equally.
    // The non-resized vmo has 3 more private pages after the first.
    const uint64_t kNonresizedAttrBytes = 7ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1 + 1 + 1
    const uint64_t kResizedAttrBytes = zx_system_get_page_size() / 2ul;           // 1/2
    EXPECT_EQ(VmoPopulatedBytes(vmo), resize_child ? kNonresizedAttrBytes : kResizedAttrBytes);
    EXPECT_EQ(VmoPopulatedBytes(clone), resize_child ? kResizedAttrBytes : kNonresizedAttrBytes);

    // Check that growing the shrunk vmo doesn't expose anything.
    ASSERT_OK(resize_target.set_size(2 * zx_system_get_page_size()));
    ASSERT_NO_FATAL_FAILURE(VmoCheck(resize_target, 0, zx_system_get_page_size()));

    // Check attribution for the vmos.
    // Writes into the non-resized vmo don't require allocating pages.
    ASSERT_NO_FATAL_FAILURE(VmoWrite(original_size_vmo, 6, 3ul * zx_system_get_page_size()));
    EXPECT_EQ(VmoPopulatedBytes(vmo), resize_child ? kNonresizedAttrBytes : kResizedAttrBytes);
    EXPECT_EQ(VmoPopulatedBytes(clone), resize_child ? kResizedAttrBytes : kNonresizedAttrBytes);

    // Check that closing the non-resized vmo frees the inaccessible pages.
    if (contiguous) {
      ASSERT_NO_FATAL_FAILURE(CheckContigState<4>(bti, vmo));
    }
    if (resize_child) {
      vmo.reset();
    } else {
      clone.reset();
    }

    // Check attribution for the vmos.
    // The resized vmo should have 1 private page and another zero page.
    const uint64_t kResizedAttrBytes2 = zx_system_get_page_size();  // 1 + 0
    ASSERT_NO_FATAL_FAILURE(VmoCheck(resize_target, 1));
    ASSERT_TRUE(PollVmoPopulatedBytes(resize_target, kResizedAttrBytes2));
  }
};

TEST_F(VmoCloneResizeTests, ResizeChild) {
  ASSERT_NO_FATAL_FAILURE(ResizeTest(Contiguity::NonContig, ResizeTarget::Child));
}

TEST_F(VmoCloneResizeTests, ResizeOriginal) {
  ASSERT_NO_FATAL_FAILURE(ResizeTest(Contiguity::NonContig, ResizeTarget::Parent));
}

// Tests that growing a clone exposes zeros and doesn't consume memory on parent writes.
TEST_F(VmoClone2TestCase, ResizeGrow) {
  const uint64_t kOriginalPageCount = 2ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kCloneSize = zx_system_get_page_size();
  const uint64_t kCloneNewSize = 2ul * zx_system_get_page_size();

  // Create a parent and a resizable child clone.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE, 0, kCloneSize, &clone));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 1));

  // Resize the clone and check that the new page in the clone is 0.
  ASSERT_OK(clone.set_size(kCloneNewSize));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, zx_system_get_page_size()));

  // Write to the second page of the original VMO.
  // This doesn't require forking a page and doesn't affect the clone.
  // Total pages are the shared page from the original VMO and the private page
  // in the original VMO.
  // No VMO can be blamed for more pages than its total size.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 3, zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 0, zx_system_get_page_size()));

  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginal = 3ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1
  const uint64_t kImplCostClone = zx_system_get_page_size() / 2ul;           // 1/2 + 0
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneSize);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);
}

// Tests that a vmo with a child that has a non-zero offset can be truncated without
// affecting the child.
TEST_F(VmoClone2TestCase, ResizeOffsetChild) {
  zx::vmo vmo;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(3, &vmo));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                             zx_system_get_page_size(), &clone));

  ASSERT_OK(vmo.set_size(0));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 2));
  ASSERT_EQ(VmoPopulatedBytes(vmo), 0);
  ASSERT_EQ(VmoPopulatedBytes(clone), zx_system_get_page_size());
}

// Tests that resize works with multiple disjoint children.
TEST_F(VmoClone2TestCase, ResizeDisjointChild) {
  auto test_fn = [](uint32_t perm[]) -> void {
    const uint64_t kOriginalPageCount = 3ul;
    const uint64_t kCloneSize = zx_system_get_page_size();

    // Create a parent and resizable clones for each page in the parent.
    zx::vmo vmo;
    zx::vmo clones[3];
    ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
    for (uint32_t i = 0; i < 3; i++) {
      ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE,
                                 i * zx_system_get_page_size(), kCloneSize, clones + i));
      ASSERT_NO_FATAL_FAILURE(VmoCheck(clones[i], i + 1));
    }

    // Nothing new should have been allocated.  Each page in the original is
    // shared with a clone.
    const uint64_t kImplCostOriginal = 3ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1/2 + 1/2
    const uint64_t kImplCostClone = zx_system_get_page_size() / 2ul;           // 1/2
    EXPECT_EQ(VmoPopulatedBytes(vmo),

              kImplCostOriginal);
    for (uint32_t i = 0; i < 3; i++) {
      EXPECT_EQ(VmoPopulatedBytes(clones[i]), kImplCostClone);
    }

    // Shrink two of the clones and then the original, and then check that the
    // remaining clone is okay.
    ASSERT_OK(clones[perm[0]].set_size(0));
    ASSERT_OK(clones[perm[1]].set_size(0));
    ASSERT_OK(vmo.set_size(0));

    const uint64_t kImplCost2Clone = zx_system_get_page_size();  // 1
    ASSERT_NO_FATAL_FAILURE(VmoCheck(clones[perm[2]], perm[2] + 1));
    EXPECT_EQ(VmoPopulatedBytes(vmo), 0);
    EXPECT_EQ(VmoPopulatedBytes(clones[perm[0]]), 0);
    EXPECT_EQ(VmoPopulatedBytes(clones[perm[1]]), 0);
    EXPECT_EQ(VmoPopulatedBytes(clones[perm[2]]), kImplCost2Clone);

    ASSERT_OK(clones[perm[2]].set_size(0));

    EXPECT_EQ(VmoPopulatedBytes(clones[perm[2]]), 0);
  };

  ASSERT_NO_FATAL_FAILURE(CallPermutations(test_fn, 3));
}

// Tests that resize works when with progressive writes.
TEST_F(VmoClone2TestCase, ResizeMultipleProgressive) {
  const uint64_t kOriginalPageCount = 3ul;
  const uint64_t kOriginalSize = kOriginalPageCount * zx_system_get_page_size();
  const uint64_t kOriginalNewSize = 0;
  const uint64_t kCloneSize = 2ul * zx_system_get_page_size();
  const uint64_t kCloneNewSize = 0;
  const uint64_t kClone2Size = zx_system_get_page_size();

  // Create a parent and a resizable child clone.
  zx::vmo vmo, clone;
  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kOriginalPageCount, &vmo));
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE, 0, kCloneSize, &clone));

  // Fork a page into both and check contents.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 4, 0 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone, 5, 1 * zx_system_get_page_size()));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 4, 0 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 2, 1 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 1, 0 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone, 5, 1 * zx_system_get_page_size()));

  // Create another clone of the original VMO and check contents.
  zx::vmo clone2;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kClone2Size, &clone2));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 4, 0 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 2, 1 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4, 0 * zx_system_get_page_size()));

  // Resize the first clone, check the contents and allocations.
  ASSERT_OK(clone.set_size(kCloneNewSize));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 4, 0 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 2, 1 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4, 0 * zx_system_get_page_size()));

  // Nothing new should have been allocated.
  // Total pages are the first page from the original VMO shared with the remaining
  // clone, and the other private pages from the original VMO.
  // VMO and the second clone.
  const uint64_t kImplCostAll = kOriginalSize;
  const uint64_t kImplCostOriginal = 5ul * zx_system_get_page_size() / 2ul;  // 1/2 + 1 + 1
  const uint64_t kImplCostClone = 0;
  const uint64_t kImplCostClone2 = zx_system_get_page_size() / 2ul;  // 1/2
  ASSERT_LE(kImplCostOriginal, kOriginalSize);
  ASSERT_LE(kImplCostClone, kCloneNewSize);
  ASSERT_LE(kImplCostClone2, kClone2Size);
  ASSERT_EQ(kImplCostOriginal + kImplCostClone + kImplCostClone2, kImplCostAll);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCostOriginal);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCostClone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCostClone2);

  // Resize the original vmo and make sure it frees the necessary pages.
  // Total pages are the now-private page in the 2nd VMO.
  ASSERT_OK(vmo.set_size(kOriginalNewSize));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, 4, 0 * zx_system_get_page_size()));

  const uint64_t kImplCost2All = zx_system_get_page_size();
  const uint64_t kImplCost2Original = 0;
  const uint64_t kImplCost2Clone = 0;
  const uint64_t kImplCost2Clone2 = zx_system_get_page_size();  // 1
  ASSERT_LE(kImplCost2Original, kOriginalNewSize);
  ASSERT_LE(kImplCost2Clone, kCloneNewSize);
  ASSERT_LE(kImplCost2Clone2, kClone2Size);
  ASSERT_EQ(kImplCost2Original + kImplCost2Clone + kImplCost2Clone2, kImplCost2All);
  EXPECT_EQ(VmoPopulatedBytes(vmo), kImplCost2Original);
  EXPECT_EQ(VmoPopulatedBytes(clone), kImplCost2Clone);
  EXPECT_EQ(VmoPopulatedBytes(clone2), kImplCost2Clone2);
}

// This is a regression test for bug 53710 and checks that when a COW child is resized its
// parent_limit_ is correctly updated when the resize goes over the range of its sibling.
TEST_F(VmoClone2TestCase, ResizeOverSiblingRange) {
  zx::vmo vmo;

  ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(4, &vmo));

  // Create an intermediate hidden parent, this ensures that when the child is resized the pages in
  // the range cannot simply be freed, as there is still a child of the root that needs them.
  zx::vmo intermediate;
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 4, &intermediate));

  // Create the sibling as a one page hole. This means that vmo has its range divided into 3 pieces
  // Private view of the parent | Shared view with sibling | Private view of the parent
  zx::vmo sibling;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE,
                             zx_system_get_page_size() * 2, zx_system_get_page_size(), &sibling));

  // Resize the vmo such that there is a gap between the end of our range, and the start of the
  // siblings view. This gap means the resize operation has to process three distinct ranges. Two
  // ranges where only we see the parent, and one range in the middle where we both see the parent.
  // For the ranges where only we see the parent this resize should get propagated to our parents
  // parents and pages in that range get marked now being uniaccessible to our parents sibling
  // (that is the intermediate vmo). Although marked as uniaccessible, migrating them is done lazily
  // once intermediate uses them.
  ASSERT_OK(vmo.set_size(zx_system_get_page_size()));

  // Now set the vmos size back to what it was. The result should be identical to if we had started
  // with a clone of size 1, and then grown it to size 4. That is, all the 'new' pages should be
  // zero and we should *not* see through to our parent.
  ASSERT_OK(vmo.set_size(zx_system_get_page_size() * 4));
  // The part we didn't resize over should be original value.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 1, 0 * zx_system_get_page_size()));
  // Rest should be zero.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 0, 1 * zx_system_get_page_size()));
  // For regression of 53710 only the previous read causes issues as it is the gap between our
  // temporary reduced size and our siblings start that becomes the window we can incorrectly
  // retain access to. Nevertheless, for completeness we might as well validate the rest of the
  // pages as well. This is also true for the write tests below as well.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 0, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 0, 3 * zx_system_get_page_size()));

  // Writing to the newly visible pages should just fork off a new zero page, and we should *not*
  // attempt to the pages from the root, as they are uniaccessible to intermediate. If we fork
  // uniaccessible pages in the root we will trip an assertion in the kernel.
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 2, 1 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 3, 2 * zx_system_get_page_size()));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 4, 3 * zx_system_get_page_size()));
}

// Tests the basic operation of the ZX_VMO_ZERO_CHILDREN signal.
TEST_F(VmoClone2TestCase, Children) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_signals_t o;
  ASSERT_OK(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o));

  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));

  ASSERT_EQ(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o), ZX_ERR_TIMED_OUT);
  ASSERT_OK(clone.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o));

  clone.reset();

  ASSERT_OK(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o));
}

// Tests that child count and zero child signals for when there are many children. Tests
// with closing the children both in the order they were created and the reverse order.
void ManyChildrenTestHelper(bool reverse_close) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  static constexpr uint32_t kCloneCount = 5;
  zx::vmo clones[kCloneCount];

  for (unsigned i = 0; i < kCloneCount; i++) {
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), clones + i));
    ASSERT_TRUE(PollVmoNumChildren(vmo, i + 1));
  }

  if (reverse_close) {
    for (unsigned i = kCloneCount - 1; i != UINT32_MAX; i--) {
      clones[i].reset();
      ASSERT_TRUE(PollVmoNumChildren(vmo, i));
    }
  } else {
    for (unsigned i = 0; i < kCloneCount; i++) {
      clones[i].reset();
      ASSERT_TRUE(PollVmoNumChildren(vmo, kCloneCount - (i + 1)));
    }
  }

  zx_signals_t o;
  ASSERT_OK(vmo.wait_one(ZX_VMO_ZERO_CHILDREN, zx::time::infinite_past(), &o));
}

TEST_F(VmoClone2TestCase, ManyChildren) {
  bool kForwardClose = false;
  ASSERT_NO_FATAL_FAILURE(ManyChildrenTestHelper(kForwardClose));
}

TEST_F(VmoClone2TestCase, ManyChildrenRevClose) {
  bool kReverseClose = true;
  ASSERT_NO_FATAL_FAILURE(ManyChildrenTestHelper(kReverseClose));
}

// Creates a collection of clones and writes to their mappings in every permutation order
// to make sure that no order results in a bad read.
TEST_F(VmoClone2TestCase, ManyCloneMapping) {
  constexpr uint32_t kNumElts = 4;

  auto test_fn = [](uint32_t perm[]) -> void {
    zx::vmo vmos[kNumElts];
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, vmos));

    constexpr uint32_t kOriginalData = 0xdeadbeef;
    constexpr uint32_t kNewData = 0xc0ffee;

    ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[0], kOriginalData));

    ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), vmos + 1));
    ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), vmos + 2));
    ASSERT_OK(vmos[1].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), vmos + 3));

    Mapping mappings[kNumElts] = {};

    // Map the vmos and make sure they're all correct.
    for (unsigned i = 0; i < kNumElts; i++) {
      ASSERT_OK(mappings[i].Init(vmos[i], zx_system_get_page_size()));
      ASSERT_EQ(*mappings[i].ptr(), kOriginalData);
    }

    // Write to the pages in the order specified by |perm| and validate.
    bool written[kNumElts] = {};
    for (unsigned i = 0; i < kNumElts; i++) {
      uint32_t cur_idx = perm[i];
      *mappings[cur_idx].ptr() = kNewData;
      written[cur_idx] = true;

      for (unsigned j = 0; j < kNumElts; j++) {
        ASSERT_EQ(written[j] ? kNewData : kOriginalData, *mappings[j].ptr());
      }
    }
  };

  ASSERT_NO_FATAL_FAILURE(CallPermutations(test_fn, kNumElts));
}

// Tests that a chain of clones where some have offsets works.
TEST_F(VmoClone2TestCase, ManyCloneOffset) {
  zx::vmo vmo;
  zx::vmo clone1;
  zx::vmo clone2;

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, 1));

  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone1));
  ASSERT_OK(clone1.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                                zx_system_get_page_size(), &clone2));

  VmoWrite(clone1, 1);

  clone1.reset();

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, 1));
}

// Tests that a chain of clones where some have offsets doesn't mess up
// the page migration logic.
TEST_F(VmoClone2TestCase, ManyCloneMappingOffset) {
  zx::vmo vmos[4];
  ASSERT_OK(zx::vmo::create(2 * zx_system_get_page_size(), 0, vmos));

  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[0], 1));

  ASSERT_OK(
      vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, 2 * zx_system_get_page_size(), vmos + 1));
  ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                                 zx_system_get_page_size(), vmos + 2));
  ASSERT_OK(
      vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT, 0, 2 * zx_system_get_page_size(), vmos + 3));

  Mapping mappings[4] = {};

  // Map the vmos and make sure they're all correct.
  for (unsigned i = 0; i < 4; i++) {
    ASSERT_OK(mappings[i].Init(vmos[i], zx_system_get_page_size()));
    if (i != 2) {
      ASSERT_EQ(*mappings[i].ptr(), 1);
    }
  }

  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[3], 2));
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[1], 3));

  ASSERT_EQ(*mappings[1].ptr(), 3);
  ASSERT_EQ(*mappings[3].ptr(), 2);
  ASSERT_EQ(*mappings[0].ptr(), 1);

  for (unsigned i = 0; i < 4; i++) {
    ASSERT_EQ(VmoPopulatedBytes(vmos[i]), (i != 2) * zx_system_get_page_size());
  }
}

// Tests the correctness and memory consumption of a chain of progressive clones, and
// ensures that memory is properly discarded by closing/resizing the vmos.
struct ProgressiveCloneDiscardTests : public VmoClone2TestCase {
  static void ProgressiveCloneDiscardTest(bool close) {
    constexpr uint64_t kNumClones = 6;
    zx::vmo vmos[kNumClones];
    ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(kNumClones, vmos));

    ASSERT_EQ(VmoPopulatedBytes(vmos[0]), kNumClones * zx_system_get_page_size());

    // Repeatedly clone the vmo and fork a different page in each clone.
    for (unsigned i = 1; i < kNumClones; i++) {
      ASSERT_OK(vmos[0].create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE, 0,
                                     kNumClones * zx_system_get_page_size(), vmos + i));
      ASSERT_NO_FATAL_FAILURE(VmoWrite(vmos[i], kNumClones + 2, i * zx_system_get_page_size()));
    }

    // Check that the vmos have the right content.
    for (unsigned i = 0; i < kNumClones; i++) {
      for (unsigned j = 0; j < kNumClones; j++) {
        uint32_t expected = (i != 0 && j == i) ? kNumClones + 2 : j + 1;
        ASSERT_NO_FATAL_FAILURE(VmoCheck(vmos[i], expected, j * zx_system_get_page_size()));
      }
    }

    // Check the total memory attribution.
    // The amount is less than manually duplicating the original vmo, with each
    // vmo forking one page and sharing the other `kNumClones -1` pages it can see.
    // The amount attributed should match the amount allocated.
    //
    // To compute `kAttrBytesTotal`:
    // Page attributions orig: 1 / (#clones) + (#clones - 1) * 1 / (#clones - 1)
    // Page attributions others: 1 / (#clones) + 1 + (#clones - 2) * 1 / (#clones - 1)
    // Page attributions: 1 + (#clones - 1) * 1 / (#clones - 1)
    // which simplifies to: 2
    // multiply by (#clones) vmos: 2 * #clones
    // 9/5 + 1/6 = 54/30 + 5/30 = 59/30 * 5 = 295/30
    // Orig 35 / 30 total = 330 / 30 = 11
    const uint64_t kAttrBytesOriginal =
        7ul * zx_system_get_page_size() / 6ul;  // 1/6 + 1/5 + 1/5 ...
    const uint64_t kAttrBytesAllClones =
        59ul * zx_system_get_page_size() / 6ul;  // 5 * (1 + 1/6 + 1/5 + 1/5 ...)
    const uint64_t kAttrBytesClone = kAttrBytesAllClones / (kNumClones - 1);
    const uint64_t kAttrBytesTotal = kAttrBytesAllClones + kAttrBytesOriginal;
    const uint64_t kAttrBytesWorstCase = kNumClones * kNumClones * zx_system_get_page_size();
    ASSERT_LT(kAttrBytesTotal, kAttrBytesWorstCase);
    EXPECT_EQ(kAttrBytesOriginal, VmoPopulatedBytes(vmos[0]));
    for (unsigned i = 1; i < kNumClones; i++) {
      EXPECT_EQ(kAttrBytesClone, VmoPopulatedBytes(vmos[i]));
    }

    // Close the original vmo and check for correctness.
    if (close) {
      vmos[0].reset();
    } else {
      ASSERT_OK(vmos[0].set_size(0));
    }

    for (unsigned i = 1; i < kNumClones; i++) {
      for (unsigned j = 0; j < kNumClones; j++) {
        ASSERT_NO_FATAL_FAILURE(
            VmoCheck(vmos[i], j == i ? kNumClones + 2 : j + 1, j * zx_system_get_page_size()));
      }
    }

    // Check the total memory attribution.
    // Some memory was freed; the remaining allocated memory should be accounted for.
    // The amount is less than manually duplicating the original vmo, with each
    // vmo forking one page and sharing the other `kNumClones - 1` pages it can see.
    // The amount attributed should match the amount allocated.
    //
    // To compute `kAttrBytesTotal2`:
    // Page attributions: 1 + 1 / (#clones - 1) + (#clones - 2) * 1 / (#clones - 2)
    // which simplifies to: 2 + 1 / (#clones - 1)
    // which simplifies to: (2 * #clones - 2 + 1) / (#clones - 1)
    // multiply by (#clones - 1) vmos: 2 * #clones - 1
    const uint64_t kAttrBytesTotal2 = (2ul * kNumClones - 1ul) * zx_system_get_page_size();
    const uint64_t kAttrBytesVmo2 = kAttrBytesTotal2 / (kNumClones - 1ul);
    const uint64_t kAttrBytesWorstCase2 =
        (kNumClones - 1ul) * kNumClones * zx_system_get_page_size();
    ASSERT_LT(kAttrBytesTotal2, kAttrBytesWorstCase2);
    for (unsigned i = 1; i < kNumClones; i++) {
      EXPECT_EQ(kAttrBytesVmo2, VmoPopulatedBytes(vmos[i]));
    }

    // Close all but the last two vmos and check for correctness.
    for (unsigned i = 1; i < kNumClones - 2; i++) {
      if (close) {
        vmos[i].reset();
      } else {
        ASSERT_OK(vmos[i].set_size(0));
      }
    }

    for (unsigned i = kNumClones - 2; i < kNumClones; i++) {
      for (unsigned j = 0; j < kNumClones; j++) {
        ASSERT_NO_FATAL_FAILURE(
            VmoCheck(vmos[i], j == i ? kNumClones + 2 : j + 1, j * zx_system_get_page_size()));
      }
    }

    // Check the total memory attribution.
    // Some memory was freed; the remaining allocated memory should be accounted for.
    // The amount is less than manually duplicating the original vmo, with each
    // vmo forking two pages and sharing the other pages it can see.
    // The amount attributed should match the amount allocated.
    //
    // To compute `kAttrBytesTotal3`:
    // Page attributions: (kNumClones - 2) * 1/2 + 2
    // multiply by 2 clones left: kNumClones - 2 + 4
    // which simplifies to: kNumClones + 2
    const uint64_t kAttrBytesTotal3 = (kNumClones + 2ul) * zx_system_get_page_size();
    const uint64_t kAttrBytesVmo3 = kAttrBytesTotal3 / 2ul;
    const uint64_t kAttrBytesWorstCase3 = 2ul * kNumClones * zx_system_get_page_size();
    ASSERT_LT(kAttrBytesTotal3, kAttrBytesWorstCase3);
    for (unsigned i = kNumClones - 2; i < kNumClones; i++) {
      EXPECT_EQ(kAttrBytesVmo3, VmoPopulatedBytes(vmos[i]));
    }
  }
};

TEST_F(ProgressiveCloneDiscardTests, ProgressiveCloneClose) {
  constexpr bool kClose = true;
  ASSERT_NO_FATAL_FAILURE(ProgressiveCloneDiscardTest(kClose));
}

TEST_F(ProgressiveCloneDiscardTests, ProgressiveCloneTruncate) {
  constexpr bool kTruncate = false;
  ASSERT_NO_FATAL_FAILURE(ProgressiveCloneDiscardTest(kTruncate));
}

TEST_F(VmoClone2TestCase, ForbidContiguousVmo) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  ASSERT_NO_FAILURES(bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "ForbidContiguousVmo"));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create_contiguous(bti, zx_system_get_page_size(), 0, &vmo));

  // Any kind of copy-on-write child should copy.
  zx::vmo child;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));

  ASSERT_NO_FATAL_FAILURE(CheckContigState<1>(bti, vmo));
}

TEST_F(VmoClone2TestCase, PinBeforeCreateFailure) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  ASSERT_NO_FAILURES(bti =
                         vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "PinBeforeCreateFailure"));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::pmt pmt;
  zx_paddr_t addr;
  zx_status_t status = bti.pin(ZX_BTI_PERM_READ, vmo, 0, zx_system_get_page_size(), &addr, 1, &pmt);
  ASSERT_OK(status, "pin failed");

  // Fail to clone if pages are pinned.
  zx::vmo clone;
  EXPECT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone),
            ZX_ERR_BAD_STATE);
  pmt.unpin();

  // Clone successfully after pages are unpinned.
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone));
}

TEST_F(VmoClone2TestCase, PinClonePages) {
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  // Create the stub IOMMU and fake BTI we will need for this test.
  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());
  ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  ASSERT_NO_FAILURES(bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "PinClonePages"));
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  constexpr size_t kPageCount = 4;
  const size_t kVmoSize = kPageCount * zx_system_get_page_size();
  constexpr uint32_t kTestPattern = 0x73570f00;

  // Create a VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  // Write a test pattern to each of these pages.  This should force them to
  // become committed.
  for (size_t i = 0; i < kPageCount; ++i) {
    VmoWrite(vmo, static_cast<uint32_t>(kTestPattern + i), zx_system_get_page_size() * i);
  }

  // Make a COW clone of this VMO.
  zx::vmo clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kVmoSize, &clone));

  // Confirm that we see the test pattern that we wrote to our parent.  At this
  // point in time, we should be sharing pages.
  for (size_t i = 0; i < kPageCount; ++i) {
    const uint32_t expected = static_cast<uint32_t>(kTestPattern + i);
    uint32_t observed = VmoRead(vmo, zx_system_get_page_size() * i);
    EXPECT_EQ(expected, observed);
  }

  // OK, now pin both of the VMOs.  After pinning, the VMOs should not longer be
  // sharing any physical pages (even though they were sharing pages up until
  // now).
  zx::pmt parent_pmt, clone_pmt;
  auto unpin = fit::defer([&parent_pmt, &clone_pmt]() {
    if (parent_pmt.is_valid()) {
      parent_pmt.unpin();
    }

    if (clone_pmt.is_valid()) {
      clone_pmt.unpin();
    }
  });

  zx_paddr_t parent_paddrs[kPageCount] = {0};
  zx_paddr_t clone_paddrs[kPageCount] = {0};

  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, vmo, 0, kVmoSize, parent_paddrs, std::size(parent_paddrs),
                    &parent_pmt));
  ASSERT_OK(bti.pin(ZX_BTI_PERM_READ, clone, 0, kVmoSize, clone_paddrs, std::size(clone_paddrs),
                    &clone_pmt));

  for (size_t i = 0; i < std::size(parent_paddrs); ++i) {
    for (size_t j = 0; j < std::size(clone_paddrs); ++j) {
      EXPECT_NE(parent_paddrs[i], clone_paddrs[j]);
    }
  }

  // Verify that the test pattern is still present in each of the VMOs, even
  // though they are now backed by different pages.
  for (size_t i = 0; i < kPageCount; ++i) {
    const uint32_t expected = static_cast<uint32_t>(kTestPattern + i);
    uint32_t observed = VmoRead(vmo, zx_system_get_page_size() * i);
    EXPECT_EQ(expected, observed);

    observed = VmoRead(clone, zx_system_get_page_size() * i);
    EXPECT_EQ(expected, observed);
  }

  // Everything went great.  Simply unwind and let our various deferred actions
  // clean up and do final sanity checks for us.
}

// Tests that clones based on physical vmos can't be created.
TEST_F(VmoClone2TestCase, NoPhysical) {
  vmo_test::PhysVmo phys;
  if (auto res = vmo_test::GetTestPhysVmo(); !res.is_ok()) {
    if (res.error_value() == ZX_ERR_NOT_SUPPORTED) {
      printf("Root resource not available, skipping\n");
    }
    return;
  } else {
    phys = std::move(res.value());
  }

  zx::vmo clone;
  ASSERT_EQ(phys.vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone),
            ZX_ERR_NOT_SUPPORTED);
}

// Tests that snapshots based on pager vmos can't be created.
TEST_F(VmoClone2TestCase, NoSnapshotPager) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  zx::vmo uni_clone;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, zx_system_get_page_size(),
                             &uni_clone));

  zx::vmo clone;
  ASSERT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone),
            ZX_ERR_NOT_SUPPORTED);
  ASSERT_EQ(uni_clone.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone),
            ZX_ERR_NOT_SUPPORTED);
}

// Tests that clones of uncached memory can't be created.
TEST_F(VmoClone2TestCase, Uncached) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  ASSERT_OK(vmo.set_cache_policy(ZX_CACHE_POLICY_UNCACHED));

  Mapping vmo_mapping;
  ASSERT_OK(vmo_mapping.Init(vmo, zx_system_get_page_size()));

  static constexpr uint32_t kOriginalData = 0xdeadbeef;
  *vmo_mapping.ptr() = kOriginalData;

  zx::vmo clone;
  ASSERT_EQ(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &clone),
            ZX_ERR_BAD_STATE);

  ASSERT_EQ(*vmo_mapping.ptr(), kOriginalData);
}

// This test case is derived from a failure found by the kstress tool and exists to prevent
// regressions. The comments here describe a failure path that no longer exists, but could be useful
// should this test ever regress. As such it describes specific kernel implementation details at
// time of writing.
TEST_F(VmoClone2TestCase, ParentStartLimitRegression) {
  // This is validating that when merging a hidden VMO with a remaining child that parent start
  // limits are updated correctly. Specifically if both the VMO being merged and its sibling have
  // a non-zero parent offset, then when we recursively free unused ranges up through into the
  // parent we need to calculate the correct offset for parent_start_limit. More details after a
  // diagram:
  //
  //         R
  //         |
  //     |-------|
  //     M       S
  //     |
  //  |-----|
  //  C     H
  //
  // Here R is the hidden root, M is the hidden VMO being merged with a child and S is its sibling.
  // When we close C and merge M with H there may be a portion of R that is now no longer
  // referenced, i.e. neither H nor S referenced it. Lets give some specific values (in pages) of:
  //  S has offset 2 (in R), length 1
  //  M has offset 1 (in R), length 2
  //  C has offset 0 (in M), length 1
  //  H has offset 1 (in M), length 1
  // In this setup page 0 is already (due to lack of reference) in R, and when C is closed page 1
  // can also be closed, as both H and S share the same view of just page 2.
  //
  // Before M and H are merged the unused pages are first freed. This frees page 1 in R and attempts
  // to update parent_start_limit in M. As H has offset 1, and C is gone, M should gain a
  // parent_start_limit of 1. Previously the new parent_start_limit of M was calculated as an offset
  // in R (the parent) and not M. As M is offset by 1 in R this led to parent_start_limit of 2 and
  // not 1.
  //
  // Although M is going away its parent_start_limit still matters as it effects the merge with the
  // child, and the helper that has the bug is used in many other locations.
  //
  // As a final detail the vmo H also needs to be a hidden VMO (i.e. it needs to have 2 children)
  // in order to trigger the correct path when merging that has this problem.

  // Create the root R.
  zx::vmo vmo_r;

  ASSERT_OK(zx::vmo::create(0x3000, 0, &vmo_r));

  zx::vmo vmo_m;
  ASSERT_OK(vmo_r.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x1000, 0x2000, &vmo_m));

  zx::vmo vmo_c;
  ASSERT_OK(vmo_m.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x0, 0x1000, &vmo_c));

  // R is in the space where want S, create the range we want and close R to end up with S as the
  // child of the hidden parent.
  zx::vmo vmo_s;
  ASSERT_OK(vmo_r.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x2000, 0x1000, &vmo_s));
  vmo_r.reset();

  // Same as turning s->r turn m->h.
  zx::vmo vmo_h;
  ASSERT_OK(vmo_m.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x1000, 0x1000, &vmo_h));
  vmo_m.reset();

  // Turn H into a hidden parent by creating a child.
  zx::vmo vmo_hc;
  ASSERT_OK(vmo_h.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x0, 0x1000, &vmo_hc));

  // This is where it might explode.
  vmo_c.reset();
}

// This is a regression test for https://fxbug.dev/42133843 and checks that if both children of a
// hidden parent are dropped 'at the same time', then there are no races with their parallel
// destruction.
TEST_F(VmoClone2TestCase, DropChildrenInParallel) {
  // Try some N times and hope that if there is a bug we get the right timing. Prior to fixing
  // https://fxbug.dev/42133843 this was enough iterations to reliably trigger.
  for (size_t i = 0; i < 1000; i++) {
    zx::vmo vmo;

    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

    zx::vmo child;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));

    // Use a three step ready protocol to ensure both threads can issue their requests at close to
    // the same time.
    std::atomic<bool> ready = true;

    std::thread thread{[&ready, &child] {
      ready = false;
      while (!ready) {
        arch::Yield();
      }
      child.reset();
    }};
    while (ready) {
      arch::Yield();
    }
    ready = true;
    vmo.reset();
    thread.join();
  }
}

TEST_F(VmoClone2TestCase, NoAccumulatedOverflow) {
  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(0, 0, &vmo));

  zx::vmo child1;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0xffffffffffff8000, 0x0, &child1));

  zx::vmo child2;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, child1.create_child(ZX_VMO_CHILD_SNAPSHOT, 0x8000, 0, &child2));

  ASSERT_OK(
      child1.create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_RESIZABLE, 0x4000, 0, &child2));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, child2.set_size(0x8000));
}

TEST_F(VmoClone2TestCase, MarkerClearsSplitBits) {
  zx::vmo vmo;

  // Need three pages so that we can have a three page child allowing us to zero without being able
  // to adjust parent limits;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 3, 0, &vmo));

  uint64_t val = 42;
  // Commit a page in what will become the hidden parent so we have something to fork.
  EXPECT_OK(vmo.write(&val, zx_system_get_page_size(), sizeof(val)));

  zx::vmo child;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &child));

  // Fork the page into this child
  EXPECT_OK(child.write(&val, zx_system_get_page_size(), sizeof(val)));
  // By zeroing the middle page we ensure that the zero cannot be 'faked' by adjusting the parent
  // limits and that a marker really has to be inserted.
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(), zx_system_get_page_size(),
                           nullptr, 0));

  // Reset the child merging the hidden parent back into our sibling. This should update any pages
  // that we forked (event if we later turned them into a marker) to no longer being forked as it
  // is a leaf vmo again.
  child.reset();

  // Create another child and attempt to fork the same page again. This should succeed as this page
  // should have been updated as not forked in the reset() above.
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &child));
  EXPECT_OK(child.write(&val, zx_system_get_page_size(), sizeof(val)));
}

// Test that creating a read-only mapping with ZX_VM_MAP_RANGE does not commit pages in the clone.
TEST_F(VmoClone2TestCase, MapRangeReadOnly) {
  zx::vmo vmo;

  const uint64_t kNumPages = 5;
  ASSERT_OK(zx::vmo::create(kNumPages * zx_system_get_page_size(), 0, &vmo));

  // Write non-zero pages so they are not deduped by the zero scanner. We do this so we can get an
  // accurate committed bytes count.
  for (uint64_t i = 0; i < kNumPages; i++) {
    uint64_t data = 77;
    ASSERT_OK(vmo.write(&data, i * zx_system_get_page_size(), sizeof(data)));
  }

  // All pages in vmo should now be committed.
  zx_info_vmo_t info;
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);

  // Create a clone that sees all parent pages.
  zx::vmo clone;
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, kNumPages * zx_system_get_page_size(), &clone));

  // Read only map the clone, populating all mappings. This should not commit any pages in the
  // clone.
  zx_vaddr_t clone_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_MAP_RANGE | ZX_VM_PERM_READ, 0, clone, 0,
                                       kNumPages * zx_system_get_page_size(), &clone_addr));

  auto unmap = fit::defer([&]() {
    // Cleanup the mapping we created.
    zx::vmar::root_self()->unmap(clone_addr, kNumPages * zx_system_get_page_size());
  });

  // No private pages committed in the clone.
  ASSERT_OK(clone.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(0u, info.populated_private_bytes);

  // Committed pages in the parent are unchanged.
  ASSERT_OK(vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(kNumPages * zx_system_get_page_size(), info.populated_bytes);
}

// Regression test for https://fxbug.dev/42080199. The hierarchy generation count was previously
// incremented in the VmObjectPaged destructor, not in the VmCowPages destructor. But the actual
// changes to the page list take place in the VmCowPages destructor, which would affect attribution
// counts. We drop the lock between invoking the two destructors, so it was possible for someone to
// query the attribution count in between and see an old cached count.
TEST_F(VmoClone2TestCase, DropParentCommittedBytes) {
  // Try some N times and hope that if there is a bug we get the right timing. Prior to fixing
  // https://fxbug.dev/42080199 this was enough iterations to reliably trigger.
  for (size_t i = 0; i < 1000; i++) {
    zx::vmo vmo;
    ASSERT_NO_FATAL_FAILURE(InitPageTaggedVmo(3, &vmo));

    // Create a child that sees the parent partially.
    zx::vmo child;
    ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size(),
                               2 * zx_system_get_page_size(), &child));

    // Check that the child has the right data.
    ASSERT_NO_FATAL_FAILURE(VmoCheck(child, 2));
    ASSERT_NO_FATAL_FAILURE(VmoCheck(child, 3, zx_system_get_page_size()));

    // Fork a page in the child.
    ASSERT_NO_FATAL_FAILURE(VmoWrite(child, 4, zx_system_get_page_size()));

    // The child shares a page with the parent, and has a forked page.
    const uint64_t kAttrBytesCloneSplit = 3ul * zx_system_get_page_size() / 2ul;
    const uint64_t kAttrBytesCloneNoSplit = 2ul * zx_system_get_page_size();
    EXPECT_EQ(kAttrBytesCloneSplit, VmoPopulatedBytes(child));

    // Use a three step ready protocol to ensure both threads can issue their requests at close to
    // the same time.
    std::atomic<bool> ready = true;

    std::thread thread{[&ready, &child, &kAttrBytesCloneSplit, &kAttrBytesCloneNoSplit] {
      ready = false;
      while (!ready) {
        arch::Yield();
      }
      size_t committed = VmoPopulatedBytes(child);
      // Depending on who wins the race between this thread and the thread destroying the parent,
      // the child will either continue sharing a page with the parent, or it will have both
      // pages attributed to it.
      EXPECT_TRUE(committed == kAttrBytesCloneSplit || committed == kAttrBytesCloneNoSplit,
                  "committed bytes in child: %zu\n", committed);
    }};
    while (ready) {
      arch::Yield();
    }
    ready = true;
    // Drop the parent.
    vmo.reset();
    thread.join();

    // Check that we don't change the child.
    ASSERT_NO_FATAL_FAILURE(VmoCheck(child, 2));
    ASSERT_NO_FATAL_FAILURE(VmoCheck(child, 4, zx_system_get_page_size()));

    // The parent is gone now, so the remaining page that the child could see should also have
    // moved to the child. We might need to poll a few times in case the vmo.reset() above did not
    // destroy the parent. When run as a component test, memory_monitor might be querying the
    // parent's attribution, keeping it alive. That should only be a small window though and the
    // parent should eventually be destroyed.
    ASSERT_TRUE(PollVmoPopulatedBytes(child, kAttrBytesCloneNoSplit));
  }
}

// Tests that creating a SNAPSHOT_AT_LEAST_ON_WRITE child of a slice in the middle of a
// unidirectional chain works
TEST_F(VmoClone2TestCase, SnapshotAtLeastOnWriteSliceInChain) {
  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::vmo vmo_src;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo_src));

  // Make unidirectional chain
  zx::vmo vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, zx_system_get_page_size(), &vmo));

  pager.supply_pages(vmo, 0, zx_system_get_page_size(), vmo_src, 0);
  static constexpr uint32_t kOriginalData = 0xdead1eaf;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(vmo, kOriginalData));

  zx::vmo clone1;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0, zx_system_get_page_size(),
                             &clone1));
  static constexpr uint32_t kNewData = 0xc0ffee;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone1, kNewData));

  zx::vmo clone2;
  ASSERT_OK(clone1.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                                zx_system_get_page_size(), &clone2));

  static constexpr uint32_t kNewerData = 0x1eaf;
  ASSERT_NO_FATAL_FAILURE(VmoWrite(clone2, kNewerData));

  // Slice the middle of the chain
  zx::vmo slice;
  ASSERT_OK(clone1.create_child(ZX_VMO_CHILD_SLICE, 0, zx_system_get_page_size(), &slice));

  // Snapshot-at-least-on-write the slice.
  zx::vmo snapshot;
  ASSERT_OK(slice.create_child(ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, 0,
                               zx_system_get_page_size(), &snapshot));

  ASSERT_NO_FATAL_FAILURE(VmoCheck(vmo, kOriginalData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone1, kNewData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(slice, kNewData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(snapshot, kNewData));
  ASSERT_NO_FATAL_FAILURE(VmoCheck(clone2, kNewerData));
}

}  // namespace
}  // namespace vmo_test
