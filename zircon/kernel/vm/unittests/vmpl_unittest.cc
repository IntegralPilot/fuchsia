// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page/size.h>

#include "test_helper.h"

namespace vm_unittest {

namespace {

template <size_t count>
ktl::array<vm_page_t*, count> GetPages() {
  VmPageDoublyLinkedList pmm_page_list;
  ktl::array<vm_page_t*, count> pages;

  if (pmm_alloc_pages(count, 0, &pmm_page_list) != ZX_OK) {
    // Out of memory.
    ASSERT(false);
  }

  uint32_t ix = 0;
  for (auto& page : pmm_page_list) {
    pages[ix] = &page;
    ix += 1;
  }
  pmm_page_list.clear();

  return pages;
}

template <size_t count>
void UnlinkAndFreePages(const ktl::array<vm_page_t*, count>& pages) {
  for (auto& p : pages) {
    if (p->queue_node.InContainer()) {
      p->queue_node.template RemoveFromContainer<vm_page_node_traits>();
    }
    pmm_free_page(p);
  }
}

bool AddPage(VmPageList* pl, vm_page_t* page, uint64_t offset) {
  if (!pl) {
    return false;
  }
  auto [slot, is_interval] =
      pl->LookupOrAllocate(offset, VmPageList::IntervalHandling::SplitInterval);
  if (!slot) {
    return false;
  }
  if (!slot->IsEmpty() && !slot->IsIntervalSlot()) {
    return false;
  }
  ASSERT(slot->IsEmpty() || is_interval);
  *slot = VmPageOrMarker::Page(page);
  return true;
}

bool AddMarker(VmPageList* pl, uint64_t offset) {
  if (!pl) {
    return false;
  }
  auto [slot, is_interval] =
      pl->LookupOrAllocate(offset, VmPageList::IntervalHandling::SplitInterval);
  if (!slot) {
    return false;
  }
  if (!slot->IsEmpty() && !slot->IsIntervalSlot()) {
    return false;
  }
  ASSERT(slot->IsEmpty() || is_interval);
  *slot = VmPageOrMarker::Marker();
  return true;
}

bool AddReference(VmPageList* pl, VmPageOrMarker::ReferenceValue ref, uint64_t offset) {
  if (!pl) {
    return false;
  }
  auto [slot, is_interval] =
      pl->LookupOrAllocate(offset, VmPageList::IntervalHandling::SplitInterval);
  if (!slot) {
    return false;
  }
  if (!slot->IsEmpty() && !slot->IsIntervalSlot()) {
    return false;
  }
  ASSERT(slot->IsEmpty() || is_interval);
  *slot = VmPageOrMarker::Reference(ref);
  return true;
}

constexpr uint64_t TestReference(uint64_t v) {
  return v << VmPageOrMarker::ReferenceValue::kAlignBits;
}

}  // namespace

// Basic test that checks adding/removing a page
static bool vmpl_add_remove_page_test() {
  BEGIN_TEST;

  VmPageList pl;

  vm_page_t* test_page;
  ASSERT_OK(pmm_alloc_page(0, &test_page));

  EXPECT_TRUE(AddPage(&pl, test_page, 0));

  EXPECT_EQ(test_page, pl.Lookup(0)->Page(), "unexpected page\n");
  EXPECT_FALSE(pl.IsEmpty());
  EXPECT_FALSE(pl.HasNoPageOrRef());

  vm_page* remove_page = pl.RemoveContent(0).ReleasePage();
  EXPECT_EQ(test_page, remove_page, "unexpected page\n");
  EXPECT_TRUE(pl.RemoveContent(0).IsEmpty(), "unexpected page\n");

  EXPECT_TRUE(pl.IsEmpty());
  EXPECT_TRUE(pl.HasNoPageOrRef());

  pmm_free_page(test_page);

  END_TEST;
}

// Basic test of setting and getting markers.
static bool vmpl_basic_marker_test() {
  BEGIN_TEST;

  VmPageList pl;

  EXPECT_TRUE(pl.IsEmpty());
  EXPECT_TRUE(pl.HasNoPageOrRef());

  EXPECT_TRUE(AddMarker(&pl, 0));

  EXPECT_TRUE(pl.Lookup(0)->IsMarker());

  EXPECT_FALSE(pl.IsEmpty());
  EXPECT_TRUE(pl.HasNoPageOrRef());

  VmPageOrMarker removed = pl.RemoveContent(0);
  EXPECT_TRUE(removed.IsMarker());

  EXPECT_TRUE(pl.HasNoPageOrRef());
  EXPECT_TRUE(pl.IsEmpty());

  END_TEST;
}

static bool vmpl_basic_reference_test() {
  BEGIN_TEST;

  VmPageList pl;

  EXPECT_TRUE(pl.IsEmpty());
  EXPECT_TRUE(pl.HasNoPageOrRef());

  // The zero ref is valid.
  const VmPageOrMarker::ReferenceValue ref0(0);
  EXPECT_TRUE(AddReference(&pl, ref0, 0));

  EXPECT_FALSE(pl.IsEmpty());
  EXPECT_FALSE(pl.HasNoPageOrRef());

  // A non-zero ref.
  const VmPageOrMarker::ReferenceValue ref1(TestReference(1));
  EXPECT_TRUE(AddReference(&pl, ref1, kPageSize));

  VmPageOrMarker removed = pl.RemoveContent(0);
  EXPECT_EQ(removed.ReleaseReference().value(), ref0.value());

  EXPECT_FALSE(pl.IsEmpty());
  EXPECT_FALSE(pl.HasNoPageRefOrMarker());

  removed = pl.RemoveContent(kPageSize);
  EXPECT_EQ(removed.ReleaseReference().value(), ref1.value());

  EXPECT_TRUE(pl.IsEmpty());
  EXPECT_TRUE(pl.HasNoPageRefOrMarker());

  END_TEST;
}

// Test for freeing a range of pages
static bool vmpl_free_pages_test() {
  BEGIN_TEST;

  VmPageList pl;
  constexpr uint32_t kCount = 3 * VmPageListNode::kPageFanOut;

  const auto test_pages = GetPages<kCount>();

  // Install alternating pages and markers.
  for (uint32_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&pl, test_pages[i], i * 2 * kPageSize));
    EXPECT_TRUE(AddMarker(&pl, (i * 2 + 1) * kPageSize));
  }

  VmPageDoublyLinkedList list;
  pl.RemovePages(
      [&list](VmPageOrMarker* page_or_marker, uint64_t off) {
        if (page_or_marker->IsPage()) {
          vm_page_t* p = page_or_marker->ReleasePage();
          list.push_back(p);
        }
        *page_or_marker = VmPageOrMarker::Empty();
        return ZX_ERR_NEXT;
      },
      kPageSize * 2, (kCount - 1) * 2 * kPageSize);
  for (unsigned i = 1; i < kCount - 2; i++) {
    EXPECT_TRUE(test_pages[i]->queue_node.InContainer(), "Not in free list");
  }

  for (uint32_t i = 0; i < kCount; i++) {
    VmPageOrMarker remove_page = pl.RemoveContent(i * 2 * kPageSize);
    VmPageOrMarker remove_marker = pl.RemoveContent((i * 2 + 1) * kPageSize);
    if (i == 0 || i == kCount - 1) {
      EXPECT_TRUE(remove_page.IsPage(), "missing page\n");
      EXPECT_TRUE(remove_marker.IsMarker(), "missing marker\n");
      EXPECT_EQ(test_pages[i], remove_page.ReleasePage(), "unexpected page\n");
    } else {
      EXPECT_TRUE(remove_page.IsEmpty(), "extra page\n");
      EXPECT_TRUE(remove_marker.IsEmpty(), "extra marker\n");
    }
  }

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

// Tests freeing the last page in a list
static bool vmpl_free_pages_last_page_test() {
  BEGIN_TEST;

  vm_page_t* page;
  ASSERT_OK(pmm_alloc_page(0, &page));

  VmPageList pl;
  EXPECT_TRUE(AddPage(&pl, page, 0));

  EXPECT_EQ(page, pl.Lookup(0)->Page(), "unexpected page\n");

  VmPageDoublyLinkedList list;
  pl.RemoveAllContent([&list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      list.push_back(p.ReleasePage());
    }
  });
  EXPECT_TRUE(pl.IsEmpty(), "not empty\n");

  EXPECT_EQ(list.size_slow(), 1u, "too many pages");
  EXPECT_EQ(list.pop_front(), page, "wrong page");

  pmm_free_page(page);

  END_TEST;
}

static bool vmpl_near_last_offset_free() {
  BEGIN_TEST;

  vm_page_t* page;
  ASSERT_OK(pmm_alloc_page(0, &page));

  bool at_least_one = false;
  for (uint64_t addr = 0xfffffffffff00000; addr != 0; addr += kPageSize) {
    VmPageList pl;
    if (AddPage(&pl, page, addr)) {
      at_least_one = true;
      EXPECT_EQ(page, pl.Lookup(addr)->Page(), "unexpected page\n");

      VmPageDoublyLinkedList list;
      pl.RemoveAllContent([&list](VmPageOrMarker&& p) {
        if (p.IsPage()) {
          list.push_back(p.ReleasePage());
        }
      });

      EXPECT_EQ(list.size_slow(), 1u, "too many pages");
      EXPECT_EQ(list.pop_front(), page, "wrong page");
      EXPECT_TRUE(pl.IsEmpty(), "non-empty list\n");
    }
  }
  EXPECT_TRUE(at_least_one, "starting address too large");

  VmPageList pl2;
  EXPECT_NULL(
      pl2.LookupOrAllocate(0xffffffffffff0000, VmPageList::IntervalHandling::NoIntervals).first,
      "unexpected offset addable\n");

  pmm_free_page(page);

  END_TEST;
}

// Tests taking a page from the start of a VmPageListNode
static bool vmpl_take_single_page_even_test() {
  BEGIN_TEST;

  VmPageList pl;

  vm_page_t* test_page;
  vm_page_t* test_page2;
  ASSERT_OK(pmm_alloc_page(0, &test_page));
  ASSERT_OK(pmm_alloc_page(0, &test_page2));

  EXPECT_TRUE(AddPage(&pl, test_page, 0));
  EXPECT_TRUE(AddPage(&pl, test_page2, kPageSize));

  VmPageSpliceList splice(kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl, 0);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_EQ(test_page, splice.Pop().ReleasePage(), "wrong page\n");
  EXPECT_TRUE(splice.IsProcessed(), "extra page\n");
  EXPECT_TRUE(pl.Lookup(0) == nullptr || pl.Lookup(0)->IsEmpty(), "duplicate page\n");

  EXPECT_EQ(test_page2, pl.RemoveContent(kPageSize).ReleasePage(), "remove failure\n");

  pmm_free_page(test_page);
  pmm_free_page(test_page2);

  END_TEST;
}

// Tests taking a page from the middle of a VmPageListNode
static bool vmpl_take_single_page_odd_test() {
  BEGIN_TEST;

  VmPageList pl;
  vm_page_t* test_page;
  vm_page_t* test_page2;
  ASSERT_OK(pmm_alloc_page(0, &test_page));
  ASSERT_OK(pmm_alloc_page(0, &test_page2));

  EXPECT_TRUE(AddPage(&pl, test_page, 0));
  EXPECT_TRUE(AddPage(&pl, test_page2, kPageSize));

  VmPageSpliceList splice(kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl,
      kPageSize);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_EQ(test_page2, splice.Pop().ReleasePage(), "wrong page\n");
  EXPECT_TRUE(splice.IsProcessed(), "extra page\n");
  EXPECT_TRUE(pl.Lookup(kPageSize) == nullptr || pl.Lookup(kPageSize)->IsEmpty(),
              "duplicate page\n");

  EXPECT_EQ(test_page, pl.RemoveContent(0).ReleasePage(), "remove failure\n");

  pmm_free_page(test_page);
  pmm_free_page(test_page2);

  END_TEST;
}

// Tests taking all the pages from a range of VmPageListNodes
static bool vmpl_take_all_pages_test() {
  BEGIN_TEST;

  VmPageList pl;
  constexpr uint32_t kCount = 3 * VmPageListNode::kPageFanOut;
  const auto test_pages = GetPages<kCount>();

  for (uint32_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&pl, test_pages[i], i * 2 * kPageSize));
    EXPECT_TRUE(AddMarker(&pl, (i * 2 + 1) * kPageSize));
  }

  VmPageSpliceList splice(kCount * 2 * kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl, 0);
  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_TRUE(pl.IsEmpty(), "non-empty list\n");

  for (uint32_t i = 0; i < kCount; i++) {
    EXPECT_EQ(test_pages[i], splice.Pop().ReleasePage(), "wrong page\n");
    EXPECT_TRUE(splice.Pop().IsMarker(), "expected marker\n");
  }
  EXPECT_TRUE(splice.IsProcessed(), "extra pages\n");

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

// Tests taking the middle pages from a range of VmPageListNodes
static bool vmpl_take_middle_pages_test() {
  BEGIN_TEST;

  VmPageList pl;
  constexpr uint32_t kCount = 3 * VmPageListNode::kPageFanOut;
  const auto test_pages = GetPages<kCount>();

  for (uint32_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&pl, test_pages[i], i * kPageSize));
  }

  constexpr uint32_t kTakeOffset = VmPageListNode::kPageFanOut - 1;
  constexpr uint32_t kTakeCount = VmPageListNode::kPageFanOut + 2;
  VmPageSpliceList splice(kTakeCount * kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl,
      kTakeOffset * kPageSize);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_FALSE(pl.IsEmpty(), "non-empty list\n");

  for (uint32_t i = 0; i < kCount; i++) {
    if (kTakeOffset <= i && i < kTakeOffset + kTakeCount) {
      EXPECT_EQ(test_pages[i], splice.Pop().ReleasePage(), "wrong page\n");
    } else {
      EXPECT_EQ(test_pages[i], pl.RemoveContent(i * kPageSize).ReleasePage(), "remove failure\n");
    }
  }
  EXPECT_TRUE(splice.IsProcessed(), "extra pages\n");

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

// Tests that gaps are preserved in the list
static bool vmpl_take_gap_test() {
  BEGIN_TEST;

  VmPageList pl;
  constexpr uint32_t kCount = VmPageListNode::kPageFanOut;
  constexpr uint32_t kGapSize = 2;

  const auto test_pages = GetPages<kCount>();

  for (uint32_t i = 0; i < kCount; i++) {
    uint64_t offset = (i * (kGapSize + 1)) * kPageSize;
    EXPECT_TRUE(AddPage(&pl, test_pages[i], offset));
  }

  constexpr uint32_t kListStart = kPageSize;
  constexpr uint32_t kListLen = (kCount * (kGapSize + 1) - 2) * kPageSize;
  VmPageSpliceList splice(kListLen);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl,
      kListStart);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_EQ(test_pages[0], pl.RemoveContent(0).ReleasePage(), "wrong page\n");
  EXPECT_TRUE(pl.Lookup(kListLen) == nullptr || pl.Lookup(kListLen)->IsEmpty(), "wrong page\n");

  for (uint64_t offset = kListStart; offset < kListStart + kListLen; offset += kPageSize) {
    auto page_idx = offset / kPageSize;
    if (page_idx % (kGapSize + 1) == 0) {
      EXPECT_EQ(test_pages[(page_idx / (kGapSize + 1))], splice.Pop().ReleasePage(),
                "wrong page\n");
    } else {
      EXPECT_TRUE(splice.Pop().IsEmpty(), "wrong page\n");
    }
  }
  EXPECT_TRUE(splice.IsProcessed(), "extra pages\n");

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

// Tests that an empty page splice list can be created.
static bool vmpl_take_empty_test() {
  BEGIN_TEST;

  VmPageList pl;

  VmPageSpliceList splice(kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl,
      kPageSize);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_FALSE(splice.IsProcessed());
  EXPECT_TRUE(splice.Pop().IsEmpty());
  EXPECT_TRUE(splice.IsProcessed());

  END_TEST;
}

// Tests that appending to a splice list works.
static bool vmpl_append_to_splice_list_test() {
  BEGIN_TEST;

  const uint8_t kNumPages = 5;
  VmPageSpliceList splice(kNumPages * kPageSize);

  // Append kNumPages to the splice list.
  vm_page_t* pages[kNumPages];

  for (uint8_t i = 0; i < kNumPages; i++) {
    paddr_t pa;
    ASSERT_OK(pmm_alloc_page(0, &pages[i], &pa));
    EXPECT_OK(splice.Insert(i * kPageSize, VmPageOrMarker::Page(pages[i])));
  }

  // Finalize the splice list and verify that it worked.
  splice.Finalize();
  EXPECT_TRUE(splice.IsFinalized());

  // Pop all of the pages out of the splice list and validate that it contains
  // the expected pages.
  for (uint8_t i = 0; i < kNumPages; i++) {
    VmPageOrMarker page = splice.Pop();
    EXPECT_EQ(pages[i], page.Page());
    vm_page_t* p = page.ReleasePage();
    pmm_free_page(p);
  }

  END_TEST;
}

// Tests that cleaning up a splice list doesn't blow up
static bool vmpl_take_cleanup_test() {
  BEGIN_TEST;

  paddr_t pa;
  vm_page_t* page;

  zx_status_t status = pmm_alloc_page(0, &page, &pa);
  ASSERT_EQ(ZX_OK, status, "pmm_alloc single page");
  ASSERT_NONNULL(page, "pmm_alloc single page");
  ASSERT_NE(0u, pa, "pmm_alloc single page");

  page->set_state(vm_page_state::OBJECT);
  page->object.pin_count = 0;

  VmPageList pl;
  EXPECT_TRUE(AddPage(&pl, page, 0));

  VmPageSpliceList splice(kPageSize);
  splice.AddPagesFrom(
      [](VmPageOrMarker* src, VmPageOrMarker* dst, uint64_t) { *dst = ktl::move(*src); }, pl, 0);

  EXPECT_TRUE(splice.IsFinalized());
  EXPECT_TRUE(!splice.IsProcessed(), "missing page\n");

  END_TEST;
}

// Helper function which takes an array of pages, builds a VmPageList, and then verifies that
// ForEveryPageInRange is correct when ZX_ERR_NEXT is returned for the |stop_idx|th entry.
static bool vmpl_page_gap_iter_test_body(vm_page_t** pages, uint32_t count, uint32_t stop_idx) {
  BEGIN_TEST;

  VmPageList list;
  for (uint32_t i = 0; i < count; i++) {
    if (pages[i]) {
      EXPECT_TRUE(AddPage(&list, pages[i], i * kPageSize));
    }
  }

  uint32_t idx = 0;
  zx_status_t s = list.ForEveryPageAndGapInRange(
      [pages, stop_idx, &idx](const VmPageOrMarker* p, auto off) {
        if (off != idx * kPageSize || !p->IsPage() || pages[idx] != p->Page()) {
          return ZX_ERR_INTERNAL;
        }
        if (idx == stop_idx) {
          return ZX_ERR_STOP;
        }
        idx++;
        return ZX_ERR_NEXT;
      },
      [pages, stop_idx, &idx](uint64_t gap_start, uint64_t gap_end) {
        for (auto o = gap_start; o < gap_end; o += kPageSize) {
          if (o != idx * kPageSize || pages[idx] != nullptr) {
            return ZX_ERR_INTERNAL;
          }
          if (idx == stop_idx) {
            return ZX_ERR_STOP;
          }
          idx++;
        }
        return ZX_ERR_NEXT;
      },
      0, count * kPageSize);
  ASSERT_EQ(ZX_OK, s);
  ASSERT_EQ(stop_idx, idx);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) { free_list.push_back(p.ReleasePage()); });
  ASSERT_TRUE(list.IsEmpty());
  free_list.clear();

  END_TEST;
}

// Test ForEveryPageInRange against all lists of size 4
static bool vmpl_page_gap_iter_test() {
  static constexpr uint32_t kCount = 4;
  static_assert((kCount & (kCount - 1)) == 0);

  const auto pages = GetPages<kCount>();

  vm_page_t* list[kCount] = {};
  for (unsigned i = 0; i < kCount; i++) {
    for (unsigned j = 0; j < (1 << kCount); j++) {
      for (unsigned k = 0; k < kCount; k++) {
        if (j & (1 << k)) {
          // Ensure pages are ready to be added to a list in every iteration.
          ASSERT(!pages[i]->queue_node.InContainer());
          list[k] = pages[k];
        } else {
          list[k] = nullptr;
        }
      }

      if (!vmpl_page_gap_iter_test_body(list, kCount, i)) {
        return false;
      }
    }
  }

  UnlinkAndFreePages(pages);
  return true;
}

static bool vmpl_for_every_page_test() {
  BEGIN_TEST;

  VmPageList list;

  static constexpr uint32_t kCount = 5;
  const auto test_pages = GetPages<kCount>();

  uint64_t offsets[kCount] = {
      0,
      kPageSize,
      VmPageListNode::kPageFanOut * kPageSize - kPageSize,
      VmPageListNode::kPageFanOut * kPageSize,
      VmPageListNode::kPageFanOut * kPageSize + kPageSize,
  };

  for (unsigned i = 0; i < kCount; i++) {
    if (i % 2) {
      EXPECT_TRUE(AddPage(&list, test_pages[i], offsets[i]));
    } else {
      EXPECT_TRUE(AddMarker(&list, offsets[i]));
    }
  }

  uint32_t idx = 0;
  auto iter_fn = [&](const auto* p, uint64_t off) -> zx_status_t {
    EXPECT_EQ(off, offsets[idx]);

    if (idx % 2) {
      EXPECT_TRUE(p->IsPage());
      EXPECT_EQ(p->Page(), test_pages[idx]);
    } else {
      EXPECT_TRUE(p->IsMarker());
    }

    idx++;

    return ZX_ERR_NEXT;
  };

  list.ForEveryPage(iter_fn);
  ASSERT_EQ(idx, ktl::size(offsets));

  idx = 1;
  list.ForEveryPageInRange(iter_fn, offsets[1], offsets[ktl::size(test_pages) - 1]);
  ASSERT_EQ(idx, ktl::size(offsets) - 1);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

static bool vmpl_skip_last_gap_test() {
  BEGIN_TEST;

  VmPageList list;

  vm_page_t* test_page;
  ASSERT_OK(pmm_alloc_page(0, &test_page));

  EXPECT_TRUE(AddPage(&list, test_page, kPageSize));

  uint64_t saw_gap_start = 0;
  uint64_t saw_gap_end = 0;
  int gaps_seen = 0;
  list.ForEveryPageAndGapInRange(
      [](const VmPageOrMarker* slot, uint64_t offset) { return ZX_ERR_STOP; },
      [&](uint64_t gap_start, uint64_t gap_end) {
        saw_gap_start = gap_start;
        saw_gap_end = gap_end;
        gaps_seen++;
        return ZX_ERR_NEXT;
      },
      0, kPageSize * 3);

  // Validate we saw one gap, and it was the correct gap.
  EXPECT_EQ(gaps_seen, 1);
  EXPECT_EQ(saw_gap_start, 0u);
  EXPECT_EQ(saw_gap_end, 1ul * kPageSize);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });

  free_list.clear();
  ASSERT(!test_page->queue_node.InContainer());
  pmm_free_page(test_page);

  END_TEST;
}

static bool vmpl_contiguous_run_test() {
  BEGIN_TEST;

  VmPageList list;

  static constexpr uint32_t kCount = 6;
  const auto test_pages = GetPages<kCount>();

  // Add test pages, some in the same node, and some in different nodes.
  // This is so that the code below adds pages in new nodes as expected.
  ASSERT_GT(VmPageListNode::kPageFanOut, 4u);
  // single page, then gap
  EXPECT_TRUE(AddPage(&list, test_pages[0], 0));
  // gap in the same node, then two pages
  EXPECT_TRUE(AddPage(&list, test_pages[1], 2 * kPageSize));
  EXPECT_TRUE(AddPage(&list, test_pages[2], 3 * kPageSize));
  // gap moving to the next node, then three pages spanning the node boundary
  EXPECT_TRUE(AddPage(&list, test_pages[3], (VmPageListNode::kPageFanOut * 2 - 1) * kPageSize));
  EXPECT_TRUE(AddPage(&list, test_pages[4], VmPageListNode::kPageFanOut * 2 * kPageSize));
  EXPECT_TRUE(AddPage(&list, test_pages[5], (VmPageListNode::kPageFanOut * 2 + 1) * kPageSize));

  // Perform a basic iteration to see if we can list the ranges correctly.
  uint64_t range_offsets[kCount] = {};
  uint64_t expected_offsets[kCount] = {
      0, 1, 2, 4, VmPageListNode::kPageFanOut * 2 - 1, VmPageListNode::kPageFanOut * 2 + 2};
  size_t index = 0;
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [](const VmPageOrMarker* p, uint64_t off) { return ZX_ERR_NEXT; },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, VmPageListNode::kPageFanOut * 3 * kPageSize);

  EXPECT_OK(status);
  EXPECT_EQ(6u, index);
  for (size_t i = 0; i < kCount; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(6u, free_list.size_slow());

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

static bool vmpl_contiguous_run_compare_test() {
  BEGIN_TEST;

  VmPageList list;

  static constexpr uint32_t kCount = 5;
  const auto test_pages = GetPages<kCount>();

  // Add 5 consecutive pages. The ranges will be divided up based on the compare function.
  for (size_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&list, test_pages[i], i * kPageSize));
  }

  // Random bools to use as results of comparison for each page.
  bool compare_results[kCount] = {false, true, true, false, true};
  bool page_visited[kCount] = {};
  // Expected ranges based on the compare function.
  uint64_t expected_offsets[4] = {1, 3, 4, 5};
  uint64_t range_offsets[4] = {};
  size_t index = 0;

  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [&compare_results](const VmPageOrMarker* p, uint64_t off) {
        return compare_results[off / kPageSize];
      },
      [&page_visited](const VmPageOrMarker* p, uint64_t off) {
        page_visited[off / kPageSize] = true;
        return ZX_ERR_NEXT;
      },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, VmPageListNode::kPageFanOut * kPageSize);

  EXPECT_OK(status);

  for (size_t i = 0; i < kCount; i++) {
    EXPECT_EQ(compare_results[i], page_visited[i]);
  }
  EXPECT_EQ(4u, index);
  for (size_t i = 0; i < 4; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(5u, free_list.size_slow());

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

static bool vmpl_contiguous_traversal_end_test() {
  BEGIN_TEST;

  VmPageList list;

  static constexpr uint32_t kCount = 3;
  const auto test_pages = GetPages<kCount>();

  // Add 3 consecutive pages.
  for (size_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&list, test_pages[i], i * kPageSize));
  }

  bool page_visited[3] = {};
  uint64_t expected_offsets[2] = {0, 2};
  uint64_t range_offsets[2] = {};
  size_t index = 0;
  // The compare function evaluates to true for all pages, but the traversal ends early due to
  // ZX_ERR_STOP in the per-page function.
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&page_visited](const VmPageOrMarker* p, uint64_t off) {
        page_visited[off / kPageSize] = true;
        // Stop the traversal at page 1. This means the last page processed should be page 1 and
        // should be included in the contiguous range. Traversal will stop *after* this page.
        return (off / kPageSize < 1) ? ZX_ERR_NEXT : ZX_ERR_STOP;
      },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, VmPageListNode::kPageFanOut * kPageSize);

  EXPECT_OK(status);
  // Should have visited the first two pages.
  EXPECT_TRUE(page_visited[0]);
  EXPECT_TRUE(page_visited[1]);
  EXPECT_FALSE(page_visited[2]);

  EXPECT_EQ(2u, index);
  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  // Attempt another traversal. This time it ends early because of ZX_ERR_STOP in the contiguous
  // range function.
  index = 0;
  page_visited[0] = false;
  page_visited[1] = false;
  page_visited[2] = false;
  status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) {
        // Include even indexed pages in the range.
        return (off / kPageSize) % 2 == 0;
      },
      [&page_visited](const VmPageOrMarker* p, uint64_t off) {
        page_visited[off / kPageSize] = true;
        return ZX_ERR_NEXT;
      },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        // End traversal after the first range.
        return ZX_ERR_STOP;
      },
      0, VmPageListNode::kPageFanOut * kPageSize);

  EXPECT_OK(status);
  // Should only have visited the first page.
  EXPECT_TRUE(page_visited[0]);
  EXPECT_FALSE(page_visited[1]);
  EXPECT_FALSE(page_visited[2]);

  expected_offsets[0] = 0;
  expected_offsets[1] = 1;
  EXPECT_EQ(2u, index);
  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(3u, free_list.size_slow());

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

static bool vmpl_contiguous_traversal_error_test() {
  BEGIN_TEST;

  VmPageList list;

  static constexpr uint32_t kCount = 3;
  const auto test_pages = GetPages<kCount>();

  // Add 3 consecutive pages.
  for (size_t i = 0; i < kCount; i++) {
    EXPECT_TRUE(AddPage(&list, test_pages[i], i * kPageSize));
  }

  bool page_visited[3] = {};
  uint64_t range_offsets[2] = {};
  size_t index = 0;
  // The compare function evaluates to true for all pages, but the traversal ends early due to
  // an error returned by the per-page function.
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&page_visited](const VmPageOrMarker* p, uint64_t off) {
        page_visited[off / kPageSize] = true;
        // Only page 0 returns success.
        return (off / kPageSize < 1) ? ZX_ERR_NEXT : ZX_ERR_BAD_STATE;
      },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, VmPageListNode::kPageFanOut * kPageSize);

  EXPECT_EQ(ZX_ERR_BAD_STATE, status);
  // Should have visited the first two pages.
  EXPECT_TRUE(page_visited[0]);
  EXPECT_TRUE(page_visited[1]);
  EXPECT_FALSE(page_visited[2]);

  EXPECT_EQ(2u, index);
  // Should have been able to process the contiguous range till right before the page that failed.
  uint64_t expected_offsets[2] = {0, 1};
  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  // Attempt another traversal. This time it ends early because of an error returned by the
  // contiguous range function.
  index = 0;
  page_visited[0] = false;
  page_visited[1] = false;
  page_visited[2] = false;
  status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) {
        // Include even indexed pages in the range.
        return (off / kPageSize) % 2 == 0;
      },
      [&page_visited](const VmPageOrMarker* p, uint64_t off) {
        page_visited[off / kPageSize] = true;
        return ZX_ERR_NEXT;
      },
      [&range_offsets, &index](uint64_t start, uint64_t end, bool is_interval) {
        if (is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        range_offsets[index++] = start;
        range_offsets[index++] = end;
        // Error after the first range.
        return ZX_ERR_BAD_STATE;
      },
      0, VmPageListNode::kPageFanOut * kPageSize);

  EXPECT_EQ(ZX_ERR_BAD_STATE, status);
  // Should only have visited the first page.
  EXPECT_TRUE(page_visited[0]);
  EXPECT_FALSE(page_visited[1]);
  EXPECT_FALSE(page_visited[2]);

  expected_offsets[0] = 0;
  expected_offsets[1] = 1;
  EXPECT_EQ(2u, index);
  for (size_t i = 0; i < 2; i++) {
    EXPECT_EQ(expected_offsets[i] * kPageSize, range_offsets[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(3u, free_list.size_slow());

  UnlinkAndFreePages(test_pages);

  END_TEST;
}

static bool vmpl_cursor_test() {
  BEGIN_TEST;

  VmPageList list;

  // Add some entries to produce some contiguous and non-contiguous nodes.
  constexpr uint64_t off1 = VmPageListNode::kPageFanOut * 3 + 4;
  constexpr uint64_t off2 = VmPageListNode::kPageFanOut * 5 + 4;
  constexpr uint64_t off3 = VmPageListNode::kPageFanOut * 6 + 1;
  constexpr uint64_t off4 = VmPageListNode::kPageFanOut * 6 + 2;
  constexpr uint64_t off5 = VmPageListNode::kPageFanOut * 8 + 1;

  EXPECT_TRUE(AddMarker(&list, off1 * kPageSize));
  EXPECT_TRUE(AddMarker(&list, off2 * kPageSize));
  EXPECT_TRUE(AddMarker(&list, off3 * kPageSize));
  EXPECT_TRUE(AddMarker(&list, off4 * kPageSize));
  EXPECT_TRUE(AddMarker(&list, off5 * kPageSize));

  // Looking up offsets that fall completely out of a node should yield an invalid cursor.
  VMPLCursor cursor = list.LookupMutableCursor((off1 - VmPageListNode::kPageFanOut) * kPageSize);
  EXPECT_FALSE(cursor.current());
  cursor = list.LookupMutableCursor((off1 + VmPageListNode::kPageFanOut) * kPageSize);
  EXPECT_FALSE(cursor.current());

  // Looking up in a node should yield a cursor, even if nothing at the exact entry.
  cursor = list.LookupMutableCursor((off1 - 1) * kPageSize);
  EXPECT_TRUE(cursor.current());
  EXPECT_TRUE(cursor.current()->IsEmpty());

  // Cursor should iterate into the marker though.
  cursor.step();
  EXPECT_TRUE(cursor.current());
  EXPECT_TRUE(cursor.current()->IsMarker());

  // Further iteration should terminate at the end of the this node, as the next node is not
  // contiguous.
  cursor.step();
  while (cursor.current()) {
    EXPECT_TRUE(cursor.current()->IsEmpty());
    cursor.step();
  }

  // Should be able to iterate across contiguous nodes.
  cursor = list.LookupMutableCursor((off2 * kPageSize));
  EXPECT_TRUE(cursor.current());
  EXPECT_TRUE(cursor.current()->IsMarker());
  cursor.step();

  // Iterate to the next marker, which is in a different node, and count the number of items.
  uint64_t items = 0;
  cursor.ForEveryContiguous([&items](const VmPageOrMarker* page_or_marker) {
    items++;
    return page_or_marker->IsMarker() ? ZX_ERR_STOP : ZX_ERR_NEXT;
  });
  EXPECT_EQ(off3 - off2, items);

  // ForEveryContiguous will have left us at off3 when we stopped, so the next item should be off4,
  // which is also a marker.
  cursor.step();
  EXPECT_TRUE(cursor.current());
  EXPECT_TRUE(cursor.current()->IsMarker());

  // Attempting to do this again should fail as next item is in the next node.
  items = 0;
  cursor.step();
  cursor.ForEveryContiguous([&items](const VmPageOrMarker* page_or_marker) {
    items++;
    return page_or_marker->IsMarker() ? ZX_ERR_STOP : ZX_ERR_NEXT;
  });
  EXPECT_FALSE(cursor.current());
  // Should have iterated the remaining items in a node after off4
  EXPECT_EQ(VmPageListNode::kPageFanOut - (off4 % VmPageListNode::kPageFanOut) - 1, items);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_single_node_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval [1, 3] in a single page list node.
  constexpr uint64_t expected_start = 1, expected_end = 3;
  constexpr uint64_t size = VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize))

  uint64_t start, end;
  zx_status_t status = list.ForEveryPage([&](const VmPageOrMarker* p, uint64_t off) {
    if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
      return ZX_ERR_BAD_STATE;
    }
    if (!p->IsZeroIntervalDirty()) {
      return ZX_ERR_BAD_STATE;
    }
    if (p->IsIntervalStart()) {
      start = off;
    } else if (p->IsIntervalEnd()) {
      end = off;
    }
    return ZX_ERR_NEXT;
  });
  EXPECT_OK(status);
  EXPECT_EQ(expected_start * kPageSize, start);
  EXPECT_EQ(expected_end * kPageSize, end);

  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t gaps[4];
  size_t index = 0;
  start = 0;
  end = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(expected_start * kPageSize, start);
  EXPECT_EQ(expected_end * kPageSize, end);

  EXPECT_EQ(4u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_multiple_nodes_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  uint64_t start, end;
  zx_status_t status = list.ForEveryPage([&](const VmPageOrMarker* p, uint64_t off) {
    if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
      return ZX_ERR_BAD_STATE;
    }
    if (!p->IsZeroIntervalDirty()) {
      return ZX_ERR_BAD_STATE;
    }
    if (p->IsIntervalStart()) {
      start = off;
    } else if (p->IsIntervalEnd()) {
      end = off;
    }
    return ZX_ERR_NEXT;
  });
  EXPECT_OK(status);
  EXPECT_EQ(expected_start * kPageSize, start);
  EXPECT_EQ(expected_end * kPageSize, end);

  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t gaps[4];
  size_t index = 0;
  start = 0;
  end = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(expected_start * kPageSize, start);
  EXPECT_EQ(expected_end * kPageSize, end);

  EXPECT_EQ(4u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_traversal_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // End traversal partway into the interval.
  // Should only see the gap before the interval start.
  uint64_t expected_gaps[2] = {0, expected_start};
  uint64_t gaps[2];
  size_t index = 0;
  uint64_t start = 0, end = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, (expected_end - 1) * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(expected_start * kPageSize, start);
  // We should not have seen the end of the interval.
  EXPECT_EQ(0u, end);

  EXPECT_EQ(2u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Start traversal partway into the interval.
  // Should only see the gap after the interval end.
  expected_gaps[0] = expected_end + 1;
  expected_gaps[1] = size;
  index = 0;
  start = 0;
  end = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      (expected_start + 1) * kPageSize, size * kPageSize);
  EXPECT_OK(status);

  // We should not have seen the start of the interval.
  EXPECT_EQ(0u, start);
  EXPECT_EQ(expected_end * kPageSize, end);

  EXPECT_EQ(2u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Start traversal partway into the interval, and also end before the interval end.
  // Should not see any gaps or pages either.
  index = 0;
  start = 0;
  end = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      (expected_start + 1) * kPageSize, (expected_end - 1) * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(0u, start);
  EXPECT_EQ(0u, end);
  EXPECT_EQ(0u, index);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_merge_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval [7, 12].
  constexpr uint64_t expected_start = 7, expected_end = 12;
  constexpr uint64_t size = 2 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize))

  // Add intervals to the left and right of the existing interval and verify that they are merged
  // into a single interval.
  constexpr uint64_t new_expected_start = 3;
  constexpr uint64_t new_expected_end = 20;
  ASSERT_GT(size, new_expected_end);
  // Interval [3, 6].
  ASSERT_OK(list.AddZeroInterval(new_expected_start * kPageSize, expected_start * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  // Interval [13, 20].
  ASSERT_OK(list.AddZeroInterval((expected_end + 1) * kPageSize, (new_expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  uint64_t start, end;
  zx_status_t status = list.ForEveryPage([&](const VmPageOrMarker* p, uint64_t off) {
    if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
      return ZX_ERR_BAD_STATE;
    }
    if (!p->IsZeroIntervalDirty()) {
      return ZX_ERR_BAD_STATE;
    }
    if (p->IsIntervalStart()) {
      start = off;
    } else if (p->IsIntervalEnd()) {
      end = off;
    }
    return ZX_ERR_NEXT;
  });
  EXPECT_OK(status);
  EXPECT_EQ(new_expected_start * kPageSize, start);
  EXPECT_EQ(new_expected_end * kPageSize, end);

  constexpr uint64_t expected_gaps[4] = {0, new_expected_start, new_expected_end + 1, size};
  uint64_t gaps[4];
  size_t index = 0;
  start = 0;
  end = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          start = off;
        } else if (p->IsIntervalEnd()) {
          end = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(new_expected_start * kPageSize, start);
  EXPECT_EQ(new_expected_end * kPageSize, end);

  EXPECT_EQ(4u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_add_page_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Adding a page in the interval should split the interval.
  vm_page_t* page;
  ASSERT_OK(pmm_alloc_page(0, &page));

  constexpr uint64_t page_offset = VmPageListNode::kPageFanOut;
  EXPECT_TRUE(AddPage(&list, page, page_offset * kPageSize));

  constexpr uint64_t expected_intervals[4] = {expected_start, page_offset - 1, page_offset + 1,
                                              expected_end};
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t intervals[4];
  uint64_t gaps[4];
  uint64_t interval_index = 0, gap_index = 0;
  uint64_t page_off = 0;

  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          if (interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsIntervalEnd()) {
          if (interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_off = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }

  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(page_offset * kPageSize, page_off);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(1u, free_list.size_slow());

  free_list.clear();
  ASSERT(!page->queue_node.InContainer());
  pmm_free_page(page);

  END_TEST;
}

static bool vmpl_interval_add_page_slots_test() {
  BEGIN_TEST;

  VmPageList list;

  // 3 page interval such that adding a page in the middle creates two distinct slots.
  constexpr uint64_t expected_start = 0, expected_end = 2;
  constexpr uint64_t size = VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Adding a page in the interval should split the interval.
  vm_page_t* page;
  ASSERT_OK(pmm_alloc_page(0, &page));

  constexpr uint64_t page_offset = 1;
  EXPECT_TRUE(AddPage(&list, page, page_offset * kPageSize));

  constexpr uint64_t expected_intervals[2] = {expected_start, expected_end};
  constexpr uint64_t expected_gaps[2] = {expected_end + 1, size};
  uint64_t intervals[2];
  uint64_t gaps[2];
  uint64_t interval_index = 0, gap_index = 0;
  uint64_t page_off = 0;

  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalSlot() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalSlot()) {
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_off = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(page_offset * kPageSize, page_off);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(1u, free_list.size_slow());

  free_list.clear();
  ASSERT(!page->queue_node.InContainer());
  pmm_free_page(page);

  END_TEST;
}

static bool vmpl_interval_add_page_start_test() {
  BEGIN_TEST;

  VmPageList list;

  constexpr uint64_t expected_start = 0, expected_end = 2;
  constexpr uint64_t size = VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  static constexpr uint32_t kCount = 2;
  const auto pages = GetPages<kCount>();

  // Add a page at the start of the interval.
  EXPECT_TRUE(AddPage(&list, pages[0], expected_start * kPageSize));

  const uint64_t expected_intervals[2] = {expected_start + 1, expected_end};
  const uint64_t expected_gaps[2] = {expected_end + 1, size};
  uint64_t intervals[2];
  uint64_t gaps[2];
  uint64_t interval_index = 0, gap_index = 0;
  uint64_t page_off = size * kPageSize;

  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          if (interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsIntervalEnd()) {
          if (interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_off = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(expected_start * kPageSize, page_off);

  // Add another page at the start of the new interval.
  EXPECT_TRUE(AddPage(&list, pages[1], (expected_start + 1) * kPageSize));

  const uint64_t expected_pages[2] = {expected_start, expected_start + 1};
  uint64_t page_offsets[2] = {};
  uint64_t page_index = 0;
  interval_index = 0;
  gap_index = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalSlot() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalSlot()) {
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_offsets[page_index++] = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(1u, interval_index);
  EXPECT_EQ(expected_end * kPageSize, intervals[0]);

  EXPECT_EQ(2u, page_index);
  for (size_t i = 0; i < page_index; i++) {
    EXPECT_EQ(expected_pages[i] * kPageSize, page_offsets[i]);
  }

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(2u, free_list.size_slow());

  UnlinkAndFreePages(pages);

  END_TEST;
}

static bool vmpl_interval_add_page_end_test() {
  BEGIN_TEST;

  VmPageList list;

  constexpr uint64_t expected_start = 0;
  uint64_t expected_end = 2;
  constexpr uint64_t size = VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  static constexpr uint32_t kCount = 2;
  const auto pages = GetPages<kCount>();

  // Add a page at the end of the interval.
  EXPECT_TRUE(AddPage(&list, pages[0], expected_end * kPageSize));

  const uint64_t expected_intervals[2] = {expected_start, expected_end - 1};
  const uint64_t expected_gaps[2] = {expected_end + 1, size};
  uint64_t intervals[2];
  uint64_t gaps[2];
  uint64_t interval_index = 0, gap_index = 0;
  uint64_t page_off = 0;

  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          if (interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsIntervalEnd()) {
          if (interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_off = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(expected_end * kPageSize, page_off);

  // Add another page at the end of the new interval.
  EXPECT_TRUE(AddPage(&list, pages[1], (expected_end - 1) * kPageSize));

  const uint64_t expected_pages[2] = {expected_end - 1, expected_end};
  uint64_t page_offsets[2] = {};
  uint64_t page_index = 0;
  interval_index = 0;
  gap_index = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalSlot() || p->IsPage())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalSlot()) {
          intervals[interval_index++] = off;
        } else if (p->IsPage()) {
          page_offsets[page_index++] = off;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(1u, interval_index);
  EXPECT_EQ(expected_start * kPageSize, intervals[0]);

  EXPECT_EQ(2u, page_index);
  for (size_t i = 0; i < page_index; i++) {
    EXPECT_EQ(expected_pages[i] * kPageSize, page_offsets[i]);
  }

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(2u, free_list.size_slow());

  UnlinkAndFreePages(pages);

  END_TEST;
}

static bool vmpl_interval_replace_slot_test() {
  BEGIN_TEST;

  VmPageList list;

  constexpr uint64_t expected_interval = 0;
  constexpr uint64_t size = VmPageListNode::kPageFanOut;
  ASSERT_OK(list.AddZeroInterval(expected_interval * kPageSize, (expected_interval + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  constexpr uint64_t expected_gaps[2] = {expected_interval + 1, size};
  uint64_t interval = size * kPageSize;
  uint64_t gaps[2];
  uint64_t gap_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalSlot()) {
          return ZX_ERR_BAD_STATE;
        }
        interval = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(expected_interval * kPageSize, interval);

  // Add a page in the interval slot.
  vm_page_t* page;
  ASSERT_OK(pmm_alloc_page(0, &page));

  EXPECT_TRUE(AddPage(&list, page, expected_interval * kPageSize));

  uint64_t page_off = size * kPageSize;
  gap_index = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsPage()) {
          return ZX_ERR_BAD_STATE;
        }
        page_off = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  EXPECT_EQ(expected_interval * kPageSize, page_off);

  VmPageDoublyLinkedList free_list;
  list.RemoveAllContent([&free_list](VmPageOrMarker&& p) {
    if (p.IsPage()) {
      free_list.push_back(p.ReleasePage());
    }
  });
  EXPECT_EQ(1u, free_list.size_slow());

  free_list.clear();
  ASSERT(!page->queue_node.InContainer());
  pmm_free_page(page);

  END_TEST;
}

static bool vmpl_interval_contig_full_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  constexpr uint64_t expected_pages[2] = {expected_start, expected_end};
  constexpr uint64_t expected_contig[2] = {expected_start, expected_end + 1};
  uint64_t pages[2];
  uint64_t contig[2];
  uint64_t page_index = 0, contig_index = 0;
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart()) {
          if (page_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
        } else if (p->IsIntervalEnd()) {
          if (page_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
        }
        pages[page_index++] = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        if (!is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        contig[contig_index++] = begin;
        contig[contig_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);

  EXPECT_EQ(2u, page_index);
  for (size_t i = 0; i < page_index; i++) {
    EXPECT_EQ(expected_pages[i] * kPageSize, pages[i]);
  }

  EXPECT_EQ(2u, contig_index);
  for (size_t i = 0; i < contig_index; i++) {
    EXPECT_EQ(expected_contig[i] * kPageSize, contig[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_contig_partial_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  uint64_t page;
  uint64_t contig[2];
  uint64_t contig_index = 0;
  // Start the traversal partway into the interval.
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalEnd()) {
          return ZX_ERR_BAD_STATE;
        }
        page = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        if (!is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        contig[contig_index++] = begin;
        contig[contig_index++] = end;
        return ZX_ERR_NEXT;
      },
      (expected_start + 1) * kPageSize, size * kPageSize);
  EXPECT_OK(status);

  // Should only have visited the end.
  uint64_t expected_page = expected_end;
  uint64_t expected_contig[2] = {expected_start + 1, expected_end + 1};
  EXPECT_EQ(expected_page * kPageSize, page);
  EXPECT_EQ(2u, contig_index);
  for (size_t i = 0; i < contig_index; i++) {
    EXPECT_EQ(expected_contig[i] * kPageSize, contig[i]);
  }

  contig_index = 0;
  // End the traversal partway into the interval.
  status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalStart()) {
          return ZX_ERR_BAD_STATE;
        }
        page = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        if (!is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        contig[contig_index++] = begin;
        contig[contig_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, (expected_end - 1) * kPageSize);
  EXPECT_OK(status);

  // Should only have visited the start.
  expected_page = expected_start;
  expected_contig[0] = expected_start;
  expected_contig[1] = expected_end - 1;
  EXPECT_EQ(expected_page * kPageSize, page);
  EXPECT_EQ(2u, contig_index);
  for (size_t i = 0; i < contig_index; i++) {
    EXPECT_EQ(expected_contig[i] * kPageSize, contig[i]);
  }

  contig_index = 0;
  // Start and end the traversal partway into the interval.
  status = list.ForEveryPageAndContiguousRunInRange(
      [](const VmPageOrMarker* p, uint64_t off) { return true; },
      [&](const VmPageOrMarker* p, uint64_t off) {
        // Should not visit any slot.
        return ZX_ERR_BAD_STATE;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        if (!is_interval) {
          return ZX_ERR_BAD_STATE;
        }
        contig[contig_index++] = begin;
        contig[contig_index++] = end;
        return ZX_ERR_NEXT;
      },
      (expected_start + 1) * kPageSize, (expected_end - 1) * kPageSize);
  EXPECT_OK(status);

  // Should have seen the requested contiguous range, even though neither the start nor the end was
  // visited.
  expected_contig[0] = expected_start + 1;
  expected_contig[1] = expected_end - 1;
  EXPECT_EQ(2u, contig_index);
  for (size_t i = 0; i < contig_index; i++) {
    EXPECT_EQ(expected_contig[i] * kPageSize, contig[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});
  END_TEST;
}

static bool vmpl_interval_contig_compare_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  uint64_t page;
  // Start the traversal partway into the interval.
  zx_status_t status = list.ForEveryPageAndContiguousRunInRange(
      // Interval start evaluates to false.
      [](const VmPageOrMarker* p, uint64_t off) { return !p->IsIntervalStart(); },
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalEnd()) {
          return ZX_ERR_BAD_STATE;
        }
        page = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        // The start does not fulfill the condition, so we should not find a valid contiguous run.
        return ZX_ERR_INVALID_ARGS;
      },
      (expected_start + 1) * kPageSize, size * kPageSize);
  EXPECT_OK(status);

  // Should only have visited the end.
  uint64_t expected_page = expected_end;
  EXPECT_EQ(expected_page * kPageSize, page);

  // End the traversal partway into the interval.
  status = list.ForEveryPageAndContiguousRunInRange(
      // Interval end evaluates to false.
      [](const VmPageOrMarker* p, uint64_t off) { return !p->IsIntervalEnd(); },
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalStart()) {
          return ZX_ERR_BAD_STATE;
        }
        page = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end, bool is_interval) {
        // The end does not fulfill the condition, so we should not find a valid contiguous run.
        return ZX_ERR_INVALID_ARGS;
      },
      0, (expected_end - 1) * kPageSize);
  EXPECT_OK(status);

  // Should only have visited the start.
  expected_page = expected_start;
  EXPECT_EQ(expected_page * kPageSize, page);

  list.RemoveAllContent([](VmPageOrMarker&&) {});
  END_TEST;
}

static bool vmpl_interval_populate_full_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 5 nodes, with the middle ones unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 4 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 5 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Populate the entire interval.
  ASSERT_OK(
      list.PopulateSlotsInInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize));

  uint64_t next_off = expected_start * kPageSize;
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t gaps[4];
  size_t index = 0;
  // We should only see interval slots.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalSlot()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (off != next_off) {
          return ZX_ERR_OUT_OF_RANGE;
        }
        next_off += kPageSize;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[index++] = begin;
        gaps[index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ((expected_end + 1) * kPageSize, next_off);
  EXPECT_EQ(4u, index);
  for (size_t i = 0; i < index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_populate_partial_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Populate some slots in the middle of the interval.
  constexpr uint64_t slot_start = expected_start + 2;
  constexpr uint64_t slot_end = expected_end - 2;
  ASSERT_GT(slot_end, slot_start);
  ASSERT_OK(list.PopulateSlotsInInterval(slot_start * kPageSize, (slot_end + 1) * kPageSize));

  constexpr uint64_t expected_intervals[4] = {expected_start, slot_start - 1, slot_end + 1,
                                              expected_end};
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t intervals[4];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  uint64_t slot = slot_start * kPageSize;
  // We should see interval slots in the range we populated.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsInterval()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() || p->IsIntervalEnd()) {
          if (p->IsIntervalStart() && interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          if (p->IsIntervalEnd() && interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
          return ZX_ERR_NEXT;
        }
        if (off != slot) {
          return ZX_ERR_BAD_STATE;
        }
        slot += kPageSize;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ((slot_end + 1) * kPageSize, slot);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_populate_start_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Populate some slots beginning at the start of the interval.
  constexpr uint64_t slot_start = expected_start;
  constexpr uint64_t slot_end = expected_end - 2;
  ASSERT_GT(slot_end, slot_start);
  ASSERT_OK(list.PopulateSlotsInInterval(slot_start * kPageSize, (slot_end + 1) * kPageSize));

  constexpr uint64_t expected_intervals[2] = {slot_end + 1, expected_end};
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t intervals[2];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  uint64_t slot = slot_start * kPageSize;
  // We should see interval slots in the range we populated.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsInterval()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() || p->IsIntervalEnd()) {
          if (p->IsIntervalStart() && interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          if (p->IsIntervalEnd() && interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
          return ZX_ERR_NEXT;
        }
        if (off != slot) {
          return ZX_ERR_BAD_STATE;
        }
        slot += kPageSize;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ((slot_end + 1) * kPageSize, slot);
  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_populate_end_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Populate some slots ending at the end of the interval.
  constexpr uint64_t slot_start = expected_start + 2;
  constexpr uint64_t slot_end = expected_end;
  ASSERT_GT(slot_end, slot_start);
  ASSERT_OK(list.PopulateSlotsInInterval(slot_start * kPageSize, (slot_end + 1) * kPageSize));

  constexpr uint64_t expected_intervals[2] = {expected_start, slot_start - 1};
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t intervals[2];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  uint64_t slot = slot_start * kPageSize;
  // We should see interval slots in the range we populated.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsInterval()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() || p->IsIntervalEnd()) {
          if (p->IsIntervalStart() && interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          if (p->IsIntervalEnd() && interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
          return ZX_ERR_NEXT;
        }
        if (off != slot) {
          return ZX_ERR_BAD_STATE;
        }
        slot += kPageSize;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ((slot_end + 1) * kPageSize, slot);
  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_populate_slot_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Populate a single slot in the interval.
  constexpr uint64_t single_slot = expected_end - 3;
  ASSERT_OK(list.PopulateSlotsInInterval(single_slot * kPageSize, (single_slot + 1) * kPageSize));

  constexpr uint64_t expected_intervals[4] = {expected_start, single_slot - 1, single_slot + 1,
                                              expected_end};
  constexpr uint64_t expected_gaps[4] = {0, expected_start, expected_end + 1, size};
  uint64_t intervals[4];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  // We should see a single interval slot.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsInterval()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() || p->IsIntervalEnd()) {
          if (p->IsIntervalStart() && interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          if (p->IsIntervalEnd() && interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
          return ZX_ERR_NEXT;
        }
        if (off != single_slot * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Try to populate a slot over a single sentinel. This should be a no-op.
  ASSERT_OK(list.PopulateSlotsInInterval(single_slot * kPageSize, (single_slot + 1) * kPageSize));
  interval_index = 0;
  gap_index = 0;
  // We should see a single interval slot.
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsInterval()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() || p->IsIntervalEnd()) {
          if (p->IsIntervalStart() && interval_index % 2 == 1) {
            return ZX_ERR_BAD_STATE;
          }
          if (p->IsIntervalEnd() && interval_index % 2 == 0) {
            return ZX_ERR_BAD_STATE;
          }
          intervals[interval_index++] = off;
          return ZX_ERR_NEXT;
        }
        if (off != single_slot * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Try to return the single slot that we populated. This should return the interval to its
  // original state.
  list.ReturnIntervalSlot(single_slot * kPageSize);
  gap_index = 0;
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() && off != expected_start * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalEnd() && off != expected_end * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_full_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t expected_start = 1, expected_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, expected_end);
  ASSERT_OK(list.AddZeroInterval(expected_start * kPageSize, (expected_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Untracked interval overwrites old dirty interval.
  ASSERT_OK(list.OverwriteZeroInterval(expected_start * kPageSize, expected_end * kPageSize,
                                       expected_start * kPageSize, expected_end * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Untracked));

  // Start and end remain the same but the dirty state changes.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsIntervalStart() && off == expected_start * kPageSize) {
          if (!p->IsZeroIntervalUntracked()) {
            return ZX_ERR_BAD_STATE;
          }
          return ZX_ERR_NEXT;
        }
        if (p->IsIntervalEnd() && off == expected_end * kPageSize) {
          if (!p->IsZeroIntervalUntracked()) {
            return ZX_ERR_BAD_STATE;
          }
          return ZX_ERR_NEXT;
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, expected_start * kPageSize,
      (expected_end + 1) * kPageSize);
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_start_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t old_start = 1, old_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, old_end);
  ASSERT_OK(list.AddZeroInterval(old_start * kPageSize, (old_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Break off the start of the untracked interval into a new dirty interval.
  constexpr uint64_t new_end = old_end - 5;
  ASSERT_OK(list.OverwriteZeroInterval(old_start * kPageSize, UINT64_MAX, old_start * kPageSize,
                                       new_end * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Dirty));

  constexpr uint64_t expected_intervals[4] = {old_start, new_end, new_end + 1, old_end};
  constexpr VmPageOrMarker::IntervalDirtyState expected_state[4] = {
      VmPageOrMarker::IntervalDirtyState::Dirty, VmPageOrMarker::IntervalDirtyState::Dirty,
      VmPageOrMarker::IntervalDirtyState::Untracked, VmPageOrMarker::IntervalDirtyState::Untracked};
  uint64_t intervals[4];
  VmPageOrMarker::IntervalDirtyState interval_state[4];
  size_t interval_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsInterval()) {
          if ((p->IsIntervalStart() && interval_index % 2 == 0) ||
              (p->IsIntervalEnd() && interval_index % 2 == 1)) {
            intervals[interval_index] = off / kPageSize;
            interval_state[interval_index++] = p->GetZeroIntervalDirtyState();
            return ZX_ERR_NEXT;
          }
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, old_start * kPageSize,
      (old_end + 1) * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i], intervals[i]);
    EXPECT_EQ(expected_state[i], interval_state[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_end_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t old_start = 1, old_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, old_end);
  ASSERT_OK(list.AddZeroInterval(old_start * kPageSize, (old_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Break off the end of the untracked interval into a new dirty interval.
  constexpr uint64_t new_start = old_start + 5;
  ASSERT_OK(list.OverwriteZeroInterval(UINT64_MAX, old_end * kPageSize, new_start * kPageSize,
                                       old_end * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Dirty));

  constexpr uint64_t expected_intervals[4] = {old_start, new_start - 1, new_start, old_end};
  constexpr VmPageOrMarker::IntervalDirtyState expected_state[4] = {
      VmPageOrMarker::IntervalDirtyState::Untracked, VmPageOrMarker::IntervalDirtyState::Untracked,
      VmPageOrMarker::IntervalDirtyState::Dirty, VmPageOrMarker::IntervalDirtyState::Dirty};
  uint64_t intervals[4];
  VmPageOrMarker::IntervalDirtyState interval_state[4];
  size_t interval_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsInterval()) {
          if ((p->IsIntervalStart() && interval_index % 2 == 0) ||
              (p->IsIntervalEnd() && interval_index % 2 == 1)) {
            intervals[interval_index] = off / kPageSize;
            interval_state[interval_index++] = p->GetZeroIntervalDirtyState();
            return ZX_ERR_NEXT;
          }
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, old_start * kPageSize,
      (old_end + 1) * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i], intervals[i]);
    EXPECT_EQ(expected_state[i], interval_state[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_slot_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning a single slot.
  constexpr uint64_t expected_slot = 1;
  ASSERT_OK(list.AddZeroInterval(expected_slot * kPageSize, (expected_slot + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  EXPECT_TRUE(
      list.AnyPagesOrIntervalsInRange(expected_slot * kPageSize, (expected_slot + 1) * kPageSize));

  // Untracked interval overwrites old dirty interval.
  ASSERT_OK(list.OverwriteZeroInterval(expected_slot * kPageSize, expected_slot * kPageSize,
                                       expected_slot * kPageSize, expected_slot * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Untracked));

  // Start and end remain the same but the dirty state changes.
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsIntervalSlot() && off == expected_slot * kPageSize) {
          if (!p->IsZeroIntervalUntracked()) {
            return ZX_ERR_BAD_STATE;
          }
          return ZX_ERR_NEXT;
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, expected_slot * kPageSize,
      (expected_slot + 1) * kPageSize);
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_merge_left_test() {
  BEGIN_TEST;

  VmPageList list;

  // Two intervals next to each other with different dirty states.
  constexpr uint64_t left_start = 1, left_end = 4;
  constexpr uint64_t right_start = left_end + 1, right_end = 10;
  ASSERT_OK(list.AddZeroInterval(left_start * kPageSize, (left_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  ASSERT_OK(list.AddZeroInterval(right_start * kPageSize, (right_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(left_start * kPageSize, (right_end + 1) * kPageSize));

  // Break off the start of the right interval so that it merges with the left interval.
  constexpr uint64_t new_end = right_start + 2;
  ASSERT_OK(list.OverwriteZeroInterval(right_start * kPageSize, UINT64_MAX, right_start * kPageSize,
                                       new_end * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Dirty));

  constexpr uint64_t expected_intervals[4] = {left_start, new_end, new_end + 1, right_end};
  constexpr VmPageOrMarker::IntervalDirtyState expected_state[4] = {
      VmPageOrMarker::IntervalDirtyState::Dirty, VmPageOrMarker::IntervalDirtyState::Dirty,
      VmPageOrMarker::IntervalDirtyState::Untracked, VmPageOrMarker::IntervalDirtyState::Untracked};
  uint64_t intervals[4];
  VmPageOrMarker::IntervalDirtyState interval_state[4];
  size_t interval_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsInterval()) {
          if ((p->IsIntervalStart() && interval_index % 2 == 0) ||
              (p->IsIntervalEnd() && interval_index % 2 == 1)) {
            intervals[interval_index] = off / kPageSize;
            interval_state[interval_index++] = p->GetZeroIntervalDirtyState();
            return ZX_ERR_NEXT;
          }
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, left_start * kPageSize,
      (right_end + 1) * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i], intervals[i]);
    EXPECT_EQ(expected_state[i], interval_state[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_merge_right_test() {
  BEGIN_TEST;

  VmPageList list;

  // Two intervals next to each other with different dirty states.
  constexpr uint64_t left_start = 1, left_end = 6;
  constexpr uint64_t right_start = left_end + 1, right_end = 10;
  ASSERT_OK(list.AddZeroInterval(left_start * kPageSize, (left_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  ASSERT_OK(list.AddZeroInterval(right_start * kPageSize, (right_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(left_start * kPageSize, (right_end + 1) * kPageSize));

  // Break off the end of the left interval so that it merges with the right interval.
  constexpr uint64_t new_start = left_end - 2;
  ASSERT_OK(list.OverwriteZeroInterval(UINT64_MAX, left_end * kPageSize, new_start * kPageSize,
                                       left_end * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Untracked));

  constexpr uint64_t expected_intervals[4] = {left_start, new_start - 1, new_start, right_end};
  constexpr VmPageOrMarker::IntervalDirtyState expected_state[4] = {
      VmPageOrMarker::IntervalDirtyState::Dirty, VmPageOrMarker::IntervalDirtyState::Dirty,
      VmPageOrMarker::IntervalDirtyState::Untracked, VmPageOrMarker::IntervalDirtyState::Untracked};
  uint64_t intervals[4];
  VmPageOrMarker::IntervalDirtyState interval_state[4];
  size_t interval_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsInterval()) {
          if ((p->IsIntervalStart() && interval_index % 2 == 0) ||
              (p->IsIntervalEnd() && interval_index % 2 == 1)) {
            intervals[interval_index] = off / kPageSize;
            interval_state[interval_index++] = p->GetZeroIntervalDirtyState();
            return ZX_ERR_NEXT;
          }
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, left_start * kPageSize,
      (right_end + 1) * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i], intervals[i]);
    EXPECT_EQ(expected_state[i], interval_state[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_overwrite_merge_slots_test() {
  BEGIN_TEST;

  VmPageList list;

  // Three interval slots with alternating dirty states.
  constexpr uint64_t left = 3, mid = 4, right = 5;
  ASSERT_OK(list.AddZeroInterval(left * kPageSize, (left + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  ASSERT_OK(list.AddZeroInterval(mid * kPageSize, (mid + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));
  ASSERT_OK(list.AddZeroInterval(right * kPageSize, (right + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Untracked));
  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(left * kPageSize, (right + 1) * kPageSize));

  // Overwrite the center so that it merges on both sides.
  ASSERT_OK(list.OverwriteZeroInterval(mid * kPageSize, mid * kPageSize, mid * kPageSize,
                                       mid * kPageSize,
                                       VmPageOrMarker::IntervalDirtyState::Untracked));

  constexpr uint64_t expected_intervals[2] = {left, right};
  constexpr VmPageOrMarker::IntervalDirtyState expected_state[2] = {
      VmPageOrMarker::IntervalDirtyState::Untracked, VmPageOrMarker::IntervalDirtyState::Untracked};
  uint64_t intervals[2];
  VmPageOrMarker::IntervalDirtyState interval_state[2];
  size_t interval_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (p->IsInterval()) {
          if ((p->IsIntervalStart() && interval_index % 2 == 0) ||
              (p->IsIntervalEnd() && interval_index % 2 == 1)) {
            intervals[interval_index] = off / kPageSize;
            interval_state[interval_index++] = p->GetZeroIntervalDirtyState();
            return ZX_ERR_NEXT;
          }
        }
        return ZX_ERR_BAD_STATE;
      },
      [](uint64_t start, uint64_t end) { return ZX_ERR_BAD_STATE; }, left * kPageSize,
      (right + 1) * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i], intervals[i]);
    EXPECT_EQ(expected_state[i], interval_state[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_clip_start_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t old_start = 1, old_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, old_end);
  ASSERT_OK(list.AddZeroInterval(old_start * kPageSize, (old_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Clip the start such that the interval still spans multiple pages.
  constexpr uint64_t new_start = old_end - 3;
  ASSERT_OK(list.ClipIntervalStart(old_start * kPageSize, (new_start - old_start) * kPageSize));

  constexpr uint64_t expected_intervals[2] = {new_start, old_end};
  uint64_t expected_gaps[4] = {0, new_start, old_end + 1, size};
  uint64_t intervals[4];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() && interval_index % 2 == 1) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalEnd() && interval_index % 2 == 0) {
          return ZX_ERR_BAD_STATE;
        }
        intervals[interval_index++] = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Clip the start again, leaving behind just a single interval slot.
  ASSERT_OK(list.ClipIntervalStart(new_start * kPageSize, (old_end - new_start) * kPageSize));
  expected_gaps[1] = old_end;
  gap_index = 0;
  // We should see a single interval slot.
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalSlot()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (off != old_end * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_interval_clip_end_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t old_start = 1, old_end = 2 * VmPageListNode::kPageFanOut;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut;
  ASSERT_GT(size, old_end);
  ASSERT_OK(list.AddZeroInterval(old_start * kPageSize, (old_end + 1) * kPageSize,
                                 VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size * kPageSize));

  // Clip the end such that the interval still spans multiple pages.
  constexpr uint64_t new_end = old_start + 3;
  ASSERT_OK(list.ClipIntervalEnd(old_end * kPageSize, (old_end - new_end) * kPageSize));

  constexpr uint64_t expected_intervals[2] = {old_start, new_end};
  uint64_t expected_gaps[4] = {0, old_start, new_end + 1, size};
  uint64_t intervals[4];
  uint64_t gaps[4];
  size_t interval_index = 0, gap_index = 0;
  zx_status_t status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!(p->IsIntervalStart() || p->IsIntervalEnd())) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalStart() && interval_index % 2 == 1) {
          return ZX_ERR_BAD_STATE;
        }
        if (p->IsIntervalEnd() && interval_index % 2 == 0) {
          return ZX_ERR_BAD_STATE;
        }
        intervals[interval_index++] = off;
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(2u, interval_index);
  for (size_t i = 0; i < interval_index; i++) {
    EXPECT_EQ(expected_intervals[i] * kPageSize, intervals[i]);
  }
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  // Clip the end again, leaving behind just a single interval slot.
  ASSERT_OK(list.ClipIntervalEnd(new_end * kPageSize, (new_end - old_start) * kPageSize));
  expected_gaps[2] = old_start + 1;
  gap_index = 0;
  // We should see a single interval slot.
  status = list.ForEveryPageAndGapInRange(
      [&](const VmPageOrMarker* p, uint64_t off) {
        if (!p->IsIntervalSlot()) {
          return ZX_ERR_BAD_STATE;
        }
        if (!p->IsZeroIntervalDirty()) {
          return ZX_ERR_BAD_STATE;
        }
        if (off != old_start * kPageSize) {
          return ZX_ERR_BAD_STATE;
        }
        return ZX_ERR_NEXT;
      },
      [&](uint64_t begin, uint64_t end) {
        gaps[gap_index++] = begin;
        gaps[gap_index++] = end;
        return ZX_ERR_NEXT;
      },
      0, size * kPageSize);
  EXPECT_OK(status);
  EXPECT_EQ(4u, gap_index);
  for (size_t i = 0; i < gap_index; i++) {
    EXPECT_EQ(expected_gaps[i] * kPageSize, gaps[i]);
  }

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_split_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length.
  constexpr uint64_t expected_len = end - start + kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Split the interval in the middle.
  constexpr uint64_t mid = end - 2 * kPageSize;
  ASSERT_OK(list.PopulateSlotsInInterval(mid, mid + kPageSize));

  // Awaiting clean length remains unchanged.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid + kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Split the interval at the end.
  ASSERT_OK(list.PopulateSlotsInInterval(end, end + kPageSize));

  // Awaiting clean length remains unchanged.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(end)->GetZeroIntervalAwaitingCleanLength());

  // Split the interval at the start.
  ASSERT_OK(list.PopulateSlotsInInterval(start, start + kPageSize));

  // Awaiting clean length now moves to the new start.
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - kPageSize,
            list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(mid + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(end)->GetZeroIntervalAwaitingCleanLength());

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_clip_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length.
  constexpr uint64_t expected_len = end - start + kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Clip the interval at the end.
  ASSERT_OK(list.ClipIntervalEnd(end, 2 * kPageSize));

  // Awaiting clean length is unchanged.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Clip the interval at the start.
  ASSERT_OK(list.ClipIntervalStart(start, 2 * kPageSize));

  // Awaiting clean length is clipped too.
  EXPECT_EQ(expected_len - 2 * kPageSize,
            list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_return_slot_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length.
  constexpr uint64_t expected_len = end - start + kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Split the interval at the start.
  ASSERT_OK(list.PopulateSlotsInInterval(start, start + kPageSize));

  // Awaiting clean length now moves to the new start.
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - kPageSize,
            list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the populated slot.
  list.ReturnIntervalSlot(start);

  // Awaiting clean length is now restored.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_return_slots_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length.
  constexpr uint64_t expected_len = end - start + kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Split the start multiple times, so that all the resultant slots have non-zero awaiting clean
  // lengths.
  ASSERT_OK(list.PopulateSlotsInInterval(start, start + kPageSize));
  ASSERT_OK(list.PopulateSlotsInInterval(start + kPageSize, start + 2 * kPageSize));
  ASSERT_OK(list.PopulateSlotsInInterval(start + 2 * kPageSize, start + 3 * kPageSize));

  // Verify awaiting clean lengths.
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - 3 * kPageSize,
            list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the first slot. This will combine the first two slots into an interval.
  list.ReturnIntervalSlot(start);
  EXPECT_TRUE(list.Lookup(start)->IsIntervalStart());
  EXPECT_TRUE(list.Lookup(start + kPageSize)->IsIntervalEnd());

  // Verify awaiting clean lengths.
  EXPECT_EQ(static_cast<uint64_t>(2 * kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - 3 * kPageSize,
            list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the third slot. This will merge all the intervals and return everything to the original
  // state.
  list.ReturnIntervalSlot(start + 2 * kPageSize);
  // Awaiting clean length is restored.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  zx_status_t status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_populate_slots_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length.
  constexpr uint64_t expected_len = end - start + kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Populate some slots at the start.
  ASSERT_OK(list.PopulateSlotsInInterval(start, start + 3 * kPageSize));

  // Verify awaiting clean lengths.
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - 3 * kPageSize,
            list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the first slot. This will combine the first two slots into an interval.
  list.ReturnIntervalSlot(start);
  EXPECT_TRUE(list.Lookup(start)->IsIntervalStart());
  EXPECT_TRUE(list.Lookup(start + kPageSize)->IsIntervalEnd());

  // Verify awaiting clean lengths.
  EXPECT_EQ(static_cast<uint64_t>(2 * kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(expected_len - 3 * kPageSize,
            list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the third slot. This will merge all the intervals and return everything to the original
  // state.
  list.ReturnIntervalSlot(start + 2 * kPageSize);
  // Awaiting clean length is restored.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  zx_status_t status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_intersecting_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length to only a portion of the interval.
  constexpr uint64_t expected_len = 2 * kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Populate some slots at the start, some of them within the awating clean length, and some
  // outside.
  ASSERT_OK(list.PopulateSlotsInInterval(start, start + 3 * kPageSize));

  // Verify awaiting clean lengths.
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(static_cast<uint64_t>(kPageSize),
            list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the first slot. This will combine the first two slots into an interval.
  list.ReturnIntervalSlot(start);
  EXPECT_TRUE(list.Lookup(start)->IsIntervalStart());
  EXPECT_TRUE(list.Lookup(start + kPageSize)->IsIntervalEnd());

  // Verify awaiting clean lengths.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the third slot. This will merge all the intervals and return everything to the original
  // state.
  list.ReturnIntervalSlot(start + 2 * kPageSize);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  zx_status_t status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  // Populate a slot again, but starting partway into the interval.
  ASSERT_OK(list.PopulateSlotsInInterval(start + kPageSize, start + 2 * kPageSize));

  // The start's awaiting clean length should remain unchanged.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  // The awaiting clean length for the populated slot and the remaining interval is 0.
  EXPECT_EQ(0u, list.Lookup(start + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the slot. This should return to the original state.
  list.ReturnIntervalSlot(start + kPageSize);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

static bool vmpl_awaiting_clean_non_intersecting_test() {
  BEGIN_TEST;

  VmPageList list;

  // Interval spanning across 3 nodes, with the middle one unpopulated.
  constexpr uint64_t start = kPageSize, end = 2 * VmPageListNode::kPageFanOut * kPageSize;
  constexpr uint64_t size = 3 * VmPageListNode::kPageFanOut * kPageSize;
  ASSERT_GT(size, end);
  ASSERT_OK(
      list.AddZeroInterval(start, end + kPageSize, VmPageOrMarker::IntervalDirtyState::Dirty));

  EXPECT_TRUE(list.AnyPagesOrIntervalsInRange(0, size));

  // Set awaiting clean length to only a portion of the interval.
  constexpr uint64_t expected_len = 2 * kPageSize;
  list.LookupMutable(start).SetZeroIntervalAwaitingCleanLength(expected_len);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());

  // Populate some slots that do not insersect with the awaiting clean length.
  ASSERT_OK(
      list.PopulateSlotsInInterval(start + expected_len, start + expected_len + 3 * kPageSize));

  // Verify awaiting clean lengths.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u, list.Lookup(start + expected_len)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(0u,
            list.Lookup(start + expected_len + kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(
      0u, list.Lookup(start + expected_len + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(
      0u, list.Lookup(start + expected_len + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the first slot. This will merge the first two slots back into the interval.
  list.ReturnIntervalSlot(start + expected_len);
  EXPECT_TRUE(list.Lookup(start + expected_len + kPageSize)->IsIntervalEnd());

  // Verify awaiting clean lengths.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(
      0u, list.Lookup(start + expected_len + 2 * kPageSize)->GetZeroIntervalAwaitingCleanLength());
  EXPECT_EQ(
      0u, list.Lookup(start + expected_len + 3 * kPageSize)->GetZeroIntervalAwaitingCleanLength());

  // Return the third slot. This will merge all the intervals and return everything to the original
  // state.
  list.ReturnIntervalSlot(start + expected_len + 2 * kPageSize);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  zx_status_t status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  // Populate a slot again, this time at the end.
  ASSERT_OK(list.PopulateSlotsInInterval(end, end + kPageSize));

  // The start's awaiting clean length should remain unchanged.
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  // The awaiting clean length for the populated slot is 0.
  EXPECT_EQ(0u, list.Lookup(end)->GetZeroIntervalAwaitingCleanLength());

  // Return the slot. This should return to the original state.
  list.ReturnIntervalSlot(end);
  EXPECT_EQ(expected_len, list.Lookup(start)->GetZeroIntervalAwaitingCleanLength());
  status = list.ForEveryPage([](const VmPageOrMarker* p, uint64_t off) {
    if (p->IsIntervalStart()) {
      if (off != start) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    if (p->IsIntervalEnd()) {
      if (off != end) {
        return ZX_ERR_BAD_STATE;
      }
      return ZX_ERR_NEXT;
    }
    return ZX_ERR_BAD_STATE;
  });
  EXPECT_OK(status);

  list.RemoveAllContent([](VmPageOrMarker&&) {});

  END_TEST;
}

UNITTEST_START_TESTCASE(vm_page_list_tests)
VM_UNITTEST(vmpl_append_to_splice_list_test)
VM_UNITTEST(vmpl_add_remove_page_test)
VM_UNITTEST(vmpl_basic_marker_test)
VM_UNITTEST(vmpl_basic_reference_test)
VM_UNITTEST(vmpl_free_pages_test)
VM_UNITTEST(vmpl_free_pages_last_page_test)
VM_UNITTEST(vmpl_near_last_offset_free)
VM_UNITTEST(vmpl_take_single_page_even_test)
VM_UNITTEST(vmpl_take_single_page_odd_test)
VM_UNITTEST(vmpl_take_all_pages_test)
VM_UNITTEST(vmpl_take_middle_pages_test)
VM_UNITTEST(vmpl_take_gap_test)
VM_UNITTEST(vmpl_take_empty_test)
VM_UNITTEST(vmpl_take_cleanup_test)
VM_UNITTEST(vmpl_page_gap_iter_test)
VM_UNITTEST(vmpl_for_every_page_test)
VM_UNITTEST(vmpl_skip_last_gap_test)
VM_UNITTEST(vmpl_contiguous_run_test)
VM_UNITTEST(vmpl_contiguous_run_compare_test)
VM_UNITTEST(vmpl_contiguous_traversal_end_test)
VM_UNITTEST(vmpl_contiguous_traversal_error_test)
VM_UNITTEST(vmpl_cursor_test)
VM_UNITTEST(vmpl_interval_single_node_test)
VM_UNITTEST(vmpl_interval_multiple_nodes_test)
VM_UNITTEST(vmpl_interval_traversal_test)
VM_UNITTEST(vmpl_interval_merge_test)
VM_UNITTEST(vmpl_interval_add_page_test)
VM_UNITTEST(vmpl_interval_add_page_slots_test)
VM_UNITTEST(vmpl_interval_add_page_start_test)
VM_UNITTEST(vmpl_interval_add_page_end_test)
VM_UNITTEST(vmpl_interval_replace_slot_test)
VM_UNITTEST(vmpl_interval_contig_full_test)
VM_UNITTEST(vmpl_interval_contig_partial_test)
VM_UNITTEST(vmpl_interval_contig_compare_test)
VM_UNITTEST(vmpl_interval_populate_full_test)
VM_UNITTEST(vmpl_interval_populate_partial_test)
VM_UNITTEST(vmpl_interval_populate_start_test)
VM_UNITTEST(vmpl_interval_populate_end_test)
VM_UNITTEST(vmpl_interval_populate_slot_test)
VM_UNITTEST(vmpl_interval_overwrite_full_test)
VM_UNITTEST(vmpl_interval_overwrite_start_test)
VM_UNITTEST(vmpl_interval_overwrite_end_test)
VM_UNITTEST(vmpl_interval_overwrite_slot_test)
VM_UNITTEST(vmpl_interval_overwrite_merge_left_test)
VM_UNITTEST(vmpl_interval_overwrite_merge_right_test)
VM_UNITTEST(vmpl_interval_overwrite_merge_slots_test)
VM_UNITTEST(vmpl_interval_clip_start_test)
VM_UNITTEST(vmpl_interval_clip_end_test)
VM_UNITTEST(vmpl_awaiting_clean_split_test)
VM_UNITTEST(vmpl_awaiting_clean_clip_test)
VM_UNITTEST(vmpl_awaiting_clean_return_slot_test)
VM_UNITTEST(vmpl_awaiting_clean_return_slots_test)
VM_UNITTEST(vmpl_awaiting_clean_populate_slots_test)
VM_UNITTEST(vmpl_awaiting_clean_intersecting_test)
VM_UNITTEST(vmpl_awaiting_clean_non_intersecting_test)
UNITTEST_END_TESTCASE(vm_page_list_tests, "vmpl", "VmPageList tests")

}  // namespace vm_unittest
