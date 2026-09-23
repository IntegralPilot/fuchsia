// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/iommu.h>
#include <lib/zx/pager.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <zircon/syscalls/iommu.h>

#include <zxtest/zxtest.h>

#include "helpers.h"

namespace vmo_test {

bool AllSameVal(uint8_t *ptr, size_t len, uint8_t val) {
  for (size_t i = 0; i < len; i++) {
    if (ptr[i] != val) {
      return false;
    }
  }
  return true;
}

TEST(VmoZeroTestCase, UnalignedSubPage) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  Mapping mapping;
  EXPECT_OK(mapping.Init(vmo, zx_system_get_page_size()));
  uint8_t *ptr = mapping.bytes();

  memset(ptr, 0xff, zx_system_get_page_size());

  // zero a few words in the middle of the page.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, 42, 91, NULL, 0));

  EXPECT_TRUE(AllSameVal(ptr, 42, 0xff));
  EXPECT_TRUE(AllSameVal(ptr + 42, 91, 0));
  EXPECT_TRUE(AllSameVal(ptr + 42 + 91, zx_system_get_page_size() - 42 - 91, 0xff));
}

TEST(VmoZeroTestCase, UnalignedCommitted) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));

  Mapping mapping;
  EXPECT_OK(mapping.Init(vmo, zx_system_get_page_size() * 2));
  uint8_t *ptr = mapping.bytes();

  memset(ptr, 0xff, zx_system_get_page_size() * 2);

  // zero across both page boundaries
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() / 2, zx_system_get_page_size(),
                         NULL, 0));

  EXPECT_TRUE(AllSameVal(ptr, zx_system_get_page_size() / 2, 0xff));
  EXPECT_TRUE(AllSameVal(ptr + zx_system_get_page_size() / 2, zx_system_get_page_size(), 0));
  EXPECT_TRUE(AllSameVal(ptr + zx_system_get_page_size() + zx_system_get_page_size() / 2,
                         zx_system_get_page_size() / 2, 0xff));
}

TEST(VmoZeroTestCase, UnalignedUnCommitted) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &vmo));

  EXPECT_EQ(0, VmoPopulatedBytes(vmo));

  // zero across both page boundaries. As these are already known zero pages this should not reuslt
  // in any pages being committed.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() / 2, zx_system_get_page_size(),
                         NULL, 0));

  EXPECT_EQ(0, VmoPopulatedBytes(vmo));
}

TEST(VmoZeroTestCase, DecommitMiddle) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 3, 0, &vmo));

  Mapping mapping;
  EXPECT_OK(mapping.Init(vmo, zx_system_get_page_size() * 3));
  uint8_t *ptr = mapping.bytes();

  memset(ptr, 0xff, zx_system_get_page_size() * 3);
  EXPECT_EQ(zx_system_get_page_size() * 3, VmoPopulatedBytes(vmo));

  // zero across all three pages. This should decommit the middle one.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() / 2,
                         zx_system_get_page_size() * 2, NULL, 0));

  // Only two pages should be committed
  EXPECT_EQ(zx_system_get_page_size() * 2, VmoPopulatedBytes(vmo));
}

TEST(VmoZeroTestCase, Contiguous) {
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
  auto final_bti_check = vmo_test::CreateDeferredBtiCheck(bti);

  zx_iommu_desc_stub_t desc;
  EXPECT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  bti = vmo_test::CreateNamedBti(iommu, 0, 0xdeadbeef, "VmoZero Contiguous");

  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create_contiguous(bti, zx_system_get_page_size() * 2, 0, &vmo));
  EXPECT_EQ(zx_system_get_page_size() * 2, VmoPopulatedBytes(vmo));

  // Pin momentarily to retrieve the physical address
  zx_paddr_t phys_addr;
  {
    zx::pmt pmt;
    EXPECT_OK(bti.pin(ZX_BTI_PERM_WRITE | ZX_BTI_CONTIGUOUS, vmo, 0, zx_system_get_page_size() * 2,
                      &phys_addr, 1, &pmt));
    pmt.unpin();
  }

  Mapping mapping;
  EXPECT_OK(mapping.Init(vmo, zx_system_get_page_size() * 2));
  uint8_t *ptr = mapping.bytes();
  memset(ptr, 0xff, zx_system_get_page_size() * 2);

  // Zero a page. should not cause decommit as our VMO must remain contiguous.
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), NULL, 0));
  EXPECT_EQ(zx_system_get_page_size() * 2, VmoPopulatedBytes(vmo));

  EXPECT_TRUE(AllSameVal(ptr, zx_system_get_page_size(), 0));
  EXPECT_TRUE(AllSameVal(ptr + zx_system_get_page_size(), zx_system_get_page_size(), 0xff));

  // Pin again to make sure physical contiguity was preserved.
  zx_paddr_t phys_addr2;
  {
    zx::pmt pmt;
    EXPECT_OK(bti.pin(ZX_BTI_PERM_WRITE | ZX_BTI_CONTIGUOUS, vmo, 0, zx_system_get_page_size() * 2,
                      &phys_addr2, 1, &pmt));
    pmt.unpin();
  }
  EXPECT_EQ(phys_addr, phys_addr2);
}

TEST(VmoZeroTestCase, ContentInParentAndChild) {
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  zx::vmo child;
  // Create a child of both pages, and then just fork the first 1
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &child));
  VmoWrite(child, 2, 0);

  // As page 2 is still CoW with the parent page 1 cannot be decommitted as it would then see old
  // parent data.
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), NULL, 0));

  VmoCheck(child, 0, 0);
}

TEST(VmoZeroTestCase, EmptyCowChildren) {
  // Create a parent VMO and commit the first page by writing to it.
  //
  // Expected attribution:
  //  Page 0 shared 1 time
  //  Page 1 shared 0 times (zero page in parent)
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  // Create a SNAPSHOT child.
  //
  // Parent and child share attribution now.
  // Expected attribution:
  //  Page 0 shared 2 times (both)
  //  Page 1 shared 0 times (zero page in both)
  zx::vmo child;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &child));
  VmoCheck(child, 1, 0);  // Validate child contents.
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child));

  // Zero the child.
  //
  // Should not change total pages committed, but the child now sees the zero page.
  // Expected attribution:
  //  Page 0 shared 1 time (parent, zero page in child)
  //  Page 1 shared 0 times (zero page in both)
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));
  VmoCheck(child, 0, 0);
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(0, VmoPopulatedBytes(child));

  // Now zero the parent.
  //
  // Parent now also sees the zero page, dropping committed.
  // Expected attribution:
  //  Page 0 shared 0 times (zero page in both)
  //  Page 1 shared 0 times (zero page in both)
  EXPECT_OK(parent.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));
  VmoCheck(parent, 0, 0);
  EXPECT_EQ(0, VmoPopulatedBytes(parent));
  EXPECT_EQ(0, VmoPopulatedBytes(child));
}

TEST(VmoZeroTestCase, MergeZeroChildren) {
  // Create a parent VMO and commit the first page by writing to it.
  //
  // Expected attribution:
  //  Page 0 shared 1 time
  //  Page 1 shared 0 times (zero page in parent)
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  // Create a SNAPSHOT child.
  //
  // Parent and child share attribution now.
  // Expected attribution:
  //  Page 0 shared 2 times (both)
  //  Page 1 shared 0 times (zero page in both)
  zx::vmo child;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child));

  // Zero the parent.
  //
  // Committed page should move to the child and the parent sees the zero page.
  // Expected attribution:
  //  Page 0 shared 1 time (child, zero page in parent)
  //  Page 1 shared 0 times (zero page in both)
  EXPECT_OK(parent.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(0, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));

  // Close the child. Other parts of the system may temporarily keep the child alive,
  // so we must poll until it has closed.
  //
  // Page should cease being committed and not move to the parent.
  // Expected attribution:
  //  Page 0 shared 0 times (zero page in parent)
  //  Page 1 shared 0 times (zero page in parent)
  child.reset();
  ASSERT_TRUE(PollVmoPopulatedBytes(parent, 0));
}

// Tests that after merging a child with its hidden parent that child pages are correctly preserved
// and do not get replaced by hidden parents pages.
TEST(VmoZeroTestCase, AllocateAfterMerge) {
  // Create a parent VMO and commit pages by writing to them.
  //
  // Expected attribution:
  //  Page 0 shared 1 time
  //  Page 1 shared 1 time
  zx::vmo parent;
  InitPageTaggedVmo(2, &parent);

  // Create a SNAPSHOT child.
  //
  // Parent and child share attribution now.
  // Expected attribution:
  //  Page 0 shared 2 times (both)
  //  Page 1 shared 2 times (both)
  zx::vmo child;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &child));
  VmoCheck(child, 1, 0);
  VmoCheck(child, 2, zx_system_get_page_size());
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));

  // Zero first page of the child.
  //
  // This doesn't change number of pages committed as the parent is still using it.
  // Expected attribution:
  //  Page 0 shared 1 times (parent, zero page in child)
  //  Page 1 shared 2 times (both)
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(3 * zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child));

  // Close the parent to make the merge happen. Other parts of the system may temporarily
  // keep the child alive, so we must poll until it has closed.
  //
  // Child should still see the zero page for page #0, and only have page #1 attributed.
  // Expected attribution:
  //  Page 0 shared 0 times (zero page in child)
  //  Page 1 shared 1 times (child)
  parent.reset();
  ASSERT_TRUE(PollVmoPopulatedBytes(child, zx_system_get_page_size()));
  VmoCheck(child, 0, 0);
  VmoCheck(child, 2, zx_system_get_page_size());
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));

  // Write to a different byte in child's zero page to ensure we can't uncover parent's old data.
  // Expected attribution:
  //  Page 0 shared 1 times (child)
  //  Page 1 shared 1 times (child)
  VmoWrite(child, 1, 64);
  VmoCheck(child, 0, 0);
  EXPECT_EQ(2 * zx_system_get_page_size(), VmoPopulatedBytes(child));
}

// Similar to AllocateAfterMerge, but with multiple children and more complex sharing.
TEST(VmoZeroTestCase, AllocateAfterMergeMultipleChildren) {
  // Create a parent VMO and commit pages by writing to them.
  //
  // Expected attribution:
  //  Page 0 shared 1 time
  //  Page 1 shared 1 time
  //  Page 2 shared 1 time
  zx::vmo parent;
  InitPageTaggedVmo(3, &parent);

  // Create a SNAPSHOT child.
  //
  // Parent and child share attribution now.
  // Expected attribution:
  //  Page 0 shared 2 times (both)
  //  Page 1 shared 2 times (both)
  //  Page 2 shared 2 times (both)
  zx::vmo child1;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &child1));
  EXPECT_EQ(3 * zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(3 * zx_system_get_page_size() / 2, VmoPopulatedBytes(child1));

  // Zero a page in the parent before creating the next child.
  //
  // This migrates the committed page to the first child.  The common hidden parent sees the zero
  // page. Expected attribution:
  //  Page 0 shared 1 times (child1, zero page in parent)
  //  Page 1 shared 2 times (both)
  //  Page 2 shared 2 times (both)
  EXPECT_OK(parent.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), nullptr, 0));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(2 * zx_system_get_page_size(), VmoPopulatedBytes(child1));

  // Create another SNAPSHOT child.
  //
  // All share attribution now.
  // Expected attribution:
  //  Page 0 shared 1 times (child1, zero page in others)
  //  Page 1 shared 3 times (all)
  //  Page 2 shared 3 times (all)
  zx::vmo child2;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &child2));
  EXPECT_EQ(2 * zx_system_get_page_size() / 3, VmoPopulatedBytes(parent));
  EXPECT_EQ(5 * zx_system_get_page_size() / 3, VmoPopulatedBytes(child1));
  EXPECT_EQ(2 * zx_system_get_page_size() / 3, VmoPopulatedBytes(child2));

  // Zero the middle page of child1.
  //
  // Child1 now sees the zero page here instead of sharing with the others.
  // This leaves the number of comitted pages the same.
  // Expected attribution:
  //  Page 0 shared 1 times (child1, zero page in others)
  //  Page 1 shared 2 times (parent and child2, zero page in child1)
  //  Page 2 shared 3 times (all)
  EXPECT_OK(child1.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(), zx_system_get_page_size(),
                            nullptr, 0));
  EXPECT_EQ(5 * zx_system_get_page_size() / 6, VmoPopulatedBytes(parent));
  EXPECT_EQ(4 * zx_system_get_page_size() / 3, VmoPopulatedBytes(child1));
  EXPECT_EQ(5 * zx_system_get_page_size() / 6, VmoPopulatedBytes(child2));

  // Validate page states.
  VmoCheck(parent, 0, 0);
  VmoCheck(parent, 2, zx_system_get_page_size());
  VmoCheck(parent, 3, zx_system_get_page_size() * 2);
  VmoCheck(child1, 1, 0);
  VmoCheck(child1, 0, zx_system_get_page_size());
  VmoCheck(child1, 3, zx_system_get_page_size() * 2);
  VmoCheck(child2, 0, 0);
  VmoCheck(child2, 2, zx_system_get_page_size());
  VmoCheck(child2, 3, zx_system_get_page_size() * 2);

  // Close the first child.
  //
  // Child1's zero page should be discarded and not overwrite the forked version,
  // and the page we zeroed in the parent should also not get overridden.
  // In other words, parent and child2 should be unaffected.
  // Expected attribution:
  //  Page 0 shared 0 times (zero page in all remaining)
  //  Page 1 shared 2 times (all remaining)
  //  Page 2 shared 2 times (all remaining)
  child1.reset();
  VmoCheck(parent, 0, 0);
  VmoCheck(parent, 2, zx_system_get_page_size());
  VmoCheck(parent, 3, zx_system_get_page_size() * 2);
  VmoCheck(child2, 0, 0);
  VmoCheck(child2, 2, zx_system_get_page_size());
  VmoCheck(child2, 3, zx_system_get_page_size() * 2);
  // The reset of child1 may be ongoing due to another part of the system temporarily holding a
  // reference, so poll the committed bytes individually until we know things are stable before
  // continuing.
  EXPECT_TRUE(PollVmoPopulatedBytes(parent, zx_system_get_page_size()));
  EXPECT_TRUE(PollVmoPopulatedBytes(child2, zx_system_get_page_size()));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child2));

  // Write to a different byte in parent's zero page to ensure we can't uncover child1's old data.
  // Expected attribution:
  //  Page 0 shared 1 times (parent)
  //  Page 1 shared 2 times (both)
  //  Page 2 shared 2 times (both)
  VmoWrite(parent, 1, 64);
  VmoCheck(parent, 0, 0);
  EXPECT_EQ(2 * zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child2));

  // Fork the middle page that child1 zeroed and ensure we CoW the correct underlying page.
  // Expected attribution:
  //  Page 0 shared 1 times (parent)
  //  Page 1 shared 0 times (parent and child2 have unique copies)
  //  Page 2 shared 2 times (both)
  VmoWrite(child2, 5, zx_system_get_page_size() + 64);
  VmoCheck(child2, 2, zx_system_get_page_size());
  VmoCheck(parent, 0, zx_system_get_page_size() + 64);
  VmoCheck(parent, 2, zx_system_get_page_size());
  EXPECT_EQ(5 * zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(3 * zx_system_get_page_size() / 2, VmoPopulatedBytes(child2));
}

TEST(VmoZeroTestCase, WriteCowParent) {
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  zx::vmo child;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &child));

  // Parent should have the page currently attributed to it.
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child));

  // Write to the parent to perform a COW copy.
  VmoCheck(parent, 1, 0);
  VmoWrite(parent, 2, 0);

  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));

  // Zero the child. This should decommit the child page.
  VmoCheck(child, 1, 0);
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), NULL, 0));
  VmoCheck(child, 0, 0);
  VmoCheck(parent, 2, 0);
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(0, VmoPopulatedBytes(child));

  // Close the parent. No pages should get merged.
  parent.reset();
  VmoCheck(child, 0, 0);
  ASSERT_TRUE(PollVmoPopulatedBytes(child, 0));

  // Write to a different byte in child's zero page to ensure we can't uncover parent's old data.
  // Expected attribution:
  //  Page 0 shared 1 times (child)
  //  Page 1 shared 1 times (child)
  VmoWrite(child, 1, 64);
  VmoCheck(child, 0, 0);
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));
}

TEST(VmoZeroTestCase, ChildZeroThenWrite) {
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  zx::vmo child;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 2, &child));

  // Parent should have the page currently attributed to it.
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child));

  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), NULL, 0));

  // Page attribution should be unchanged.
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(0, VmoPopulatedBytes(child));

  // Write to the child, should cause a new page allocation.
  VmoWrite(child, 1, 0);

  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size(), VmoPopulatedBytes(child));

  // Reset the parent. The two committed pages should be different, and the parents page should be
  // dropped.
  parent.reset();
  ASSERT_TRUE(PollVmoPopulatedBytes(child, zx_system_get_page_size()));
}

TEST(VmoZeroTestCase, Nested) {
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 2, 0, &parent));
  VmoWrite(parent, 1, 0);

  // Create two children.
  zx::vmo child1, child2;
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child1));
  EXPECT_OK(parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size(), &child2));

  // Should have 1 page total attributed to the parent.
  EXPECT_EQ(zx_system_get_page_size() / 3, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 3, VmoPopulatedBytes(child1));
  EXPECT_EQ(zx_system_get_page_size() / 3, VmoPopulatedBytes(child2));

  // Zero the parent, this will insert a zero page into the parent and only the 2 children
  // will be able to see the forked page.
  EXPECT_OK(parent.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size(), NULL, 0));

  EXPECT_EQ(0, VmoPopulatedBytes(parent));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child1));
  EXPECT_EQ(zx_system_get_page_size() / 2, VmoPopulatedBytes(child2));
}

TEST(VmoZeroTestCase, ZeroLengths) {
  zx::vmo vmo;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, 0, 0, NULL, 0));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, 10, 0, NULL, 0));
  EXPECT_OK(vmo.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size(), 0, NULL, 0));
}

// Test that the traversal to a hidden parent with no content correctly handles
// the inability to fall back to its pager-backed parent.
//
// This is a regression test for https://fxbug.dev/458734268.
TEST(VmoZeroTestcase, ZeroHiddenNoContent) {
  zx::port port;
  ASSERT_OK(zx::port::create(0, &port));

  zx::pager pager;
  ASSERT_OK(zx::pager::create(0, &pager));

  zx::vmo pager_vmo;
  ASSERT_OK(pager.create_vmo(0, port, 0, 1 * zx_system_get_page_size(), &pager_vmo));

  zx::vmo a;
  ASSERT_OK(
      pager_vmo.create_child(ZX_VMO_CHILD_SNAPSHOT_MODIFIED, 0, 2 * zx_system_get_page_size(), &a));

  zx::vmo b;
  ASSERT_OK(a.create_child(ZX_VMO_CHILD_SNAPSHOT_MODIFIED, 0, 2 * zx_system_get_page_size(), &b));

  EXPECT_OK(b.op_range(ZX_VMO_OP_ZERO, 0, 2 * zx_system_get_page_size(), nullptr, 0));
}

// Test that we handle free pages correctly when both decomitting and allocating new pages in a
// single zero operation.
TEST(VmoZeroTestcase, ZeroFreesAndAllocates) {
  zx::vmo parent;
  EXPECT_OK(zx::vmo::create(zx_system_get_page_size() * 3, 0, &parent));

  // Commit the second page with non-zero data so that we have to fork it later.
  VmoWrite(parent, 1, zx_system_get_page_size());

  // Create two levels of children so we are forced to fork a page when inserting a marker later.
  zx::vmo intermediate;
  EXPECT_OK(
      parent.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &intermediate));
  zx::vmo child;
  EXPECT_OK(
      intermediate.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 3, &child));

  // Commit the first page in the child so we have something to decommit later.
  VmoWrite(child, 1, 0);

  // Now zero the child. The first page gets decommitted, and potentially used to fulfill the page
  // allocation involved in forking the second page into the intermediate.
  EXPECT_OK(child.op_range(ZX_VMO_OP_ZERO, 0, zx_system_get_page_size() * 2, NULL, 0));
}

// Tests that if a hidden parent ends up with markers then when its children perform resize
// operations markers that are still visible to the sibling are not removed from the parent.
TEST(VmoZeroTestCase, ResizeOverHiddenMarkers) {
  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, ZX_VMO_RESIZABLE, &vmo));

  // Commit the second last page with non-zero data so we can place a marker over it in a child
  // later.
  VmoWrite(vmo, 1, zx_system_get_page_size() * 2);

  // Create an intermediate hidden parent, this ensures that when the child is resized the pages in
  // the range cannot simply be freed, as there is still a child of the root that needs them.
  zx::vmo intermediate;
  ASSERT_OK(
      vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, 0, zx_system_get_page_size() * 4, &intermediate));

  // Now zero that second last page slot. As our parent has a page here a marker has to get inserted
  // to prevent seeing back to the parent. We explicitly do not zero the first or last page as in
  // those cases the parent limits could be updated instead.
  ASSERT_OK(vmo.op_range(ZX_VMO_OP_ZERO, zx_system_get_page_size() * 2, zx_system_get_page_size(),
                         nullptr, 0));

  // Create a sibling over this zero page.
  zx::vmo sibling;
  ASSERT_OK(vmo.create_child(ZX_VMO_CHILD_SNAPSHOT, zx_system_get_page_size() * 2,
                             zx_system_get_page_size(), &sibling));

  // The sibling should see the zeros.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(sibling, 0, 0));

  // Finally resize the VMO such that only our sibling sees the range in the parent that contains
  // that zero marker. In doing this resize the marker should not be freed.
  ASSERT_OK(vmo.set_size(zx_system_get_page_size()));

  // Check that the sibling still correctly sees zero.
  ASSERT_NO_FATAL_FAILURE(VmoCheck(sibling, 0, 0));

  // Writing to the sibling should commit a fresh zero page due to the marker, and should not
  // attempt to refork the page from the root.
  VmoWrite(sibling, 1, 0);
}

}  // namespace vmo_test
