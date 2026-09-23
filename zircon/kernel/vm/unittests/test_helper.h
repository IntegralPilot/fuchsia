// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_H_
#define ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_H_

#include <align.h>
#include <assert.h>
#include <bits.h>
#include <lib/instrumentation/asan.h>
#include <lib/unittest/unittest.h>
#include <lib/unittest/user_memory.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/kernel_aspace.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/vector.h>
#include <kernel/semaphore.h>
#include <ktl/algorithm.h>
#include <ktl/iterator.h>
#include <ktl/utility.h>
#include <vm/compression.h>
#include <vm/fault.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/pmm_checker.h>
#include <vm/pmm_node.h>
#include <vm/scanner.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>
#include <vm/vm_object_paged.h>
#include <vm/vm_object_physical.h>

namespace vm_unittest {

constexpr uint kArchRwFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;
constexpr uint kArchRwUserFlags = kArchRwFlags | ARCH_MMU_FLAG_PERM_USER;

// Stubbed page provider that is intended to be allowed to create a vmo that believes it is backed
// by a user pager, but is incapable of actually providing pages.
class StubPageProvider : public PageProvider {
 public:
  explicit StubPageProvider(bool trap_dirty = false) : trap_dirty_(trap_dirty) {}
  StubPageProvider(bool trap_dirty, bool ignore_requests)
      : trap_dirty_(trap_dirty), ignore_requests_(ignore_requests) {}
  ~StubPageProvider() override = default;

  PageSourceProperties properties() const override {
    return PageSourceProperties{
        .is_user_pager = true,
        .supports_request_type = {true, trap_dirty_, false},
    };
  }

 private:
  void SendAsyncRequest(PageRequest* request) override { ASSERT(ignore_requests_); }
  void ClearAsyncRequest(PageRequest* request) override { ASSERT(ignore_requests_); }
  void SwapAsyncRequest(PageRequest* old, PageRequest* new_req) override {
    ASSERT(ignore_requests_);
  }
  bool DebugIsPageOk(vm_page_t* page, uint64_t offset) override { return true; }
  void OnDetach() override {}
  void OnClose() override {}
  zx_status_t WaitOnEvent(Event* event, bool suspendable) override { panic("Not implemented\n"); }
  void Dump(uint depth, uint32_t max_items) override {}

  const bool trap_dirty_ = false;
  const bool ignore_requests_ = false;
};

// Helper function to allocate memory in a user address space.
zx_status_t AllocUser(VmAspace* aspace, const char* name, size_t size, user_inout_ptr<void>* ptr);

// Create a pager-backed VMO |out_vmo| with size equals |num_pages| pages, and commit
// |commited_pages| of its pages. |trap_dirty| controls whether modifications to pages must be
// trapped in order to generate DIRTY page requests. |resizable| controls whether the created VMO is
// resizable. Returns pointers to the pages committed in |out_pages|, so that tests can examine
// their state. Allows tests to work with pager-backed VMOs without blocking on page faults. The
// |ignore_requests| flag can be set if attempts at sending page faults should be ignored, or result
// in a panic.
zx_status_t make_partially_committed_pager_vmo(size_t num_pages, size_t committed_pages,
                                               bool trap_dirty, bool resizable,
                                               bool ignore_requests, vm_page_t** out_pages,
                                               fbl::RefPtr<VmObjectPaged>* out_vmo);

// Convenience wrapper for |make_partially_committed_pager_vmo| that commits all pages.
zx_status_t make_committed_pager_vmo(size_t num_pages, bool trap_dirty, bool resizable,
                                     vm_page_t** out_pages, fbl::RefPtr<VmObjectPaged>* out_vmo);

// Same as make_committed_pager_vmo but does not commit any pages in the VMO.
zx_status_t make_uncommitted_pager_vmo(size_t num_pages, bool trap_dirty, bool resizable,
                                       fbl::RefPtr<VmObjectPaged>* out_vmo);

// Re-supplies |num_pages| to |vmo| starting at |page_offset|. |out_pages| is updated with the new
// pages.
zx_status_t supply_pager_vmo_pages(VmObjectPaged* vmo, uint64_t page_offset, uint64_t num_pages,
                                   vm_page_t** out_pages);

uint32_t test_rand(uint32_t seed);

// fill a region of memory with a pattern based on the address of the region
void fill_region(uintptr_t seed, void* _ptr, size_t len);

// just like |fill_region|, but for user memory
void fill_region_user(uintptr_t seed, user_inout_ptr<void> _ptr, size_t len);

// test a region of memory against a known pattern
bool test_region(uintptr_t seed, void* _ptr, size_t len);

// just like |test_region|, but for user memory
bool test_region_user(uintptr_t seed, user_inout_ptr<void> _ptr, size_t len);

bool fill_and_test(void* ptr, size_t len);

// just like |fill_and_test|, but for user memory
bool fill_and_test_user(user_inout_ptr<void> ptr, size_t len);

// Helper function used by vmo_mapping_page_fault_optimisation_test.
// Given a mapping, check that a run of consecutive pages are mapped (indicated by
// expected_mapped_page_count) and that remaining pages are unmapped.
bool verify_mapped_page_range(vaddr_t base, size_t mapping_size, size_t expected_mapped_page_count);

// Verify that |vmo| has |expected_bytes| tracked by the continuous attribution system.
//
// This test helper must only be called on paged, non-slice VMOs.
bool verify_continuous_attribution_bytes(VmObject& vmo, uint64_t expected_bytes);

// Helper function that produces a filled out AttributionCounts for testing simple VMOs that just
// have private and no shared content.
VmObject::AttributionCounts make_private_attribution_counts(uint64_t uncompressed,
                                                            uint64_t compressed);

// Changes |vmo|'s high priority count by |delta|, taking care of the prepare and locking steps
// required by PriorityChanger.
//
// |delta| must be non-zero. The count may never go negative, so callers must only subtract what
// they have already added, and must remove any additions before |vmo| is destroyed.
void change_vmo_high_priority_count(VmObjectPaged& vmo, int64_t delta);

// Use the function name as the test name
#define VM_UNITTEST(fname) UNITTEST(#fname, fname)

}  // namespace vm_unittest

#endif  // ZIRCON_KERNEL_VM_UNITTESTS_TEST_HELPER_H_
