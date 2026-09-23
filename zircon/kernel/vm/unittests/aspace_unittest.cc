// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/page/size.h>
#include <pow2.h>
#include <zircon/errors.h>

#include <arch/kernel_aspace.h>
#include <ktl/initializer_list.h>
#include <ktl/limits.h>
#include <vm/vm.h>
#include <vm/vm_address_region_enumerator.h>

#include "test_helper.h"

#include <ktl/enforce.h>

namespace vm_unittest {

struct KernelRegion {
  const char* name;
  vaddr_t base;
  size_t size;
  arch_mmu_flags_t arch_mmu_flags;
};

const ktl::array kernel_regions = {
    KernelRegion{
        .name = "kernel_code",
        .base = (vaddr_t)__code_start,
        .size = RoundUpPageSize((uintptr_t)__code_end - (uintptr_t)__code_start),
        .arch_mmu_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_EXECUTE,
    },
    KernelRegion{
        .name = "kernel_rodata",
        .base = (vaddr_t)__rodata_start,
        .size = RoundUpPageSize((uintptr_t)__rodata_end - (uintptr_t)__rodata_start),
        .arch_mmu_flags = ARCH_MMU_FLAG_PERM_READ,
    },
    KernelRegion{
        .name = "kernel_relro",
        .base = (vaddr_t)__relro_start,
        .size = RoundUpPageSize((uintptr_t)__relro_end - (uintptr_t)__relro_start),
        .arch_mmu_flags = ARCH_MMU_FLAG_PERM_READ,
    },
    KernelRegion{
        .name = "kernel_data_bss",
        .base = (vaddr_t)__data_start,
        .size = RoundUpPageSize((uintptr_t)_end - (uintptr_t)__data_start),
        .arch_mmu_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
    },
};

// Wrapper for harvesting access bits that informs the page queues
static void harvest_access_bits(VmAspace::NonTerminalAction non_terminal_action,
                                VmAspace::TerminalAction terminal_action) {
  AutoVmScannerDisable scanner_disable;
  VmAspace::HarvestAllUserAccessedBits(non_terminal_action, terminal_action);
}

// Consume the (scalar) value, ensuring that the operation to calculate the value can not be
// optimized out/ deemed as unused by the compiler. I.e. this function can be used as a wrapper to a
// calculation to ensure it will be in the binary.
template <typename T>
void ConsumeValue(T value) {
  // The compiler must materialize the value into a register, since it doesn't
  // know that the register's value isn't actually used.
  __asm__ volatile("" : : "r"(value));
}

// Allocates a contiguous region in kernel space, reads/writes it,
// then destroys it.
static bool vmm_alloc_contiguous_smoke_test() {
  BEGIN_TEST;
  static const size_t alloc_size = 256 * 1024;

  // allocate a region of memory
  void* ptr;
  auto kaspace = VmAspace::kernel_aspace();
  auto err = kaspace->AllocContiguous("test", alloc_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
                                      kArchRwFlags);
  ASSERT_EQ(ZX_OK, err, "VmAspace::AllocContiguous region of memory");
  ASSERT_NONNULL(ptr, "VmAspace::AllocContiguous region of memory");

  // fill with known pattern and test
  if (!fill_and_test(ptr, alloc_size)) {
    all_ok = false;
  }

  // test that it is indeed contiguous
  unittest_printf("testing that region is contiguous\n");
  paddr_t last_pa = 0;
  for (size_t i = 0; i < alloc_size / kPageSize; i++) {
    paddr_t pa = vaddr_to_paddr((uint8_t*)ptr + i * kPageSize);
    if (last_pa != 0) {
      EXPECT_EQ(pa, last_pa + kPageSize, "region is contiguous");
    }

    last_pa = pa;
  }

  // free the region
  err = kaspace->FreeRegion(reinterpret_cast<vaddr_t>(ptr));
  EXPECT_EQ(ZX_OK, err, "VmAspace::FreeRegion region of memory");
  END_TEST;
}

// Allocates a new address space and creates a few regions in it,
// then destroys it.
static bool multiple_regions_test() {
  BEGIN_TEST;

  user_inout_ptr<void> ptr{nullptr};
  static const size_t alloc_size = 16 * 1024;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");
  ASSERT_NONNULL(aspace, "VmAspace::Create pointer");

  VmAspace* old_aspace = Thread::Current::active_aspace();
  vmm_set_active_aspace(aspace.get());

  // allocate region 0
  zx_status_t err = AllocUser(aspace.get(), "test0", alloc_size, &ptr);
  ASSERT_EQ(ZX_OK, err, "VmAspace::Alloc region of memory");

  // fill with known pattern and test
  if (!fill_and_test_user(ptr, alloc_size)) {
    all_ok = false;
  }

  // allocate region 1
  err = AllocUser(aspace.get(), "test1", alloc_size, &ptr);
  ASSERT_EQ(ZX_OK, err, "VmAspace::Alloc region of memory");

  // fill with known pattern and test
  if (!fill_and_test_user(ptr, alloc_size)) {
    all_ok = false;
  }

  // allocate region 2
  err = AllocUser(aspace.get(), "test2", alloc_size, &ptr);
  ASSERT_EQ(ZX_OK, err, "VmAspace::Alloc region of memory");

  // fill with known pattern and test
  if (!fill_and_test_user(ptr, alloc_size)) {
    all_ok = false;
  }

  vmm_set_active_aspace(old_aspace);

  // free the address space all at once
  err = aspace->Destroy();
  EXPECT_EQ(ZX_OK, err, "VmAspace::Destroy");
  END_TEST;
}

static bool vmm_alloc_contiguous_missing_flag_commit_fails() {
  BEGIN_TEST;
  // should have VmAspace::VMM_FLAG_COMMIT
  const uint zero_vmm_flags = 0;
  void* ptr;
  zx_status_t err = VmAspace::kernel_aspace()->AllocContiguous("test", 4096, &ptr, 0,
                                                               zero_vmm_flags, kArchRwFlags);
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, err);
  END_TEST;
}

static bool vmm_alloc_contiguous_zero_size_fails() {
  BEGIN_TEST;
  const size_t zero_size = 0;
  void* ptr;
  zx_status_t err = VmAspace::kernel_aspace()->AllocContiguous(
      "test", zero_size, &ptr, 0, VmAspace::VMM_FLAG_COMMIT, kArchRwFlags);
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, err);
  END_TEST;
}

// Allocates a vm address space object directly, allows it to go out of scope.
static bool vmaspace_create_smoke_test() {
  BEGIN_TEST;
  auto aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");
  zx_status_t err = aspace->Destroy();
  EXPECT_EQ(ZX_OK, err, "VmAspace::Destroy");
  END_TEST;
}

static bool vmaspace_create_invalid_ranges() {
  BEGIN_TEST;

// These are defined in vm_aspace.cc.
#define GUEST_PHYSICAL_ASPACE_BASE 0UL
#define GUEST_PHYSICAL_ASPACE_SIZE (1UL << MMU_GUEST_SIZE_SHIFT)

  // Test when base < valid base.
  EXPECT_NULL(VmAspace::Create(USER_ASPACE_BASE - 1, 4096, VmAspace::Type::User, "test",
                               VmAspace::ShareOpt::None));
  EXPECT_NULL(VmAspace::Create(KERNEL_ASPACE_BASE - 1, 4096, VmAspace::Type::Kernel, "test",
                               VmAspace::ShareOpt::None));
  EXPECT_NULL(VmAspace::Create(GUEST_PHYSICAL_ASPACE_BASE - 1, 4096, VmAspace::Type::GuestPhysical,
                               "test", VmAspace::ShareOpt::None));

  // Test when base + size exceeds valid range.
  EXPECT_NULL(VmAspace::Create(USER_ASPACE_BASE, USER_ASPACE_SIZE + 1, VmAspace::Type::User, "test",
                               VmAspace::ShareOpt::None));
  EXPECT_NULL(VmAspace::Create(KERNEL_ASPACE_BASE, KERNEL_ASPACE_SIZE + 1, VmAspace::Type::Kernel,
                               "test", VmAspace::ShareOpt::None));
  EXPECT_NULL(VmAspace::Create(GUEST_PHYSICAL_ASPACE_BASE, GUEST_PHYSICAL_ASPACE_SIZE + 1,
                               VmAspace::Type::GuestPhysical, "test", VmAspace::ShareOpt::None));

  END_TEST;
}

// Allocates a vm address space object directly, maps something on it,
// allows it to go out of scope.
static bool vmaspace_alloc_smoke_test() {
  BEGIN_TEST;
  auto aspace = VmAspace::Create(VmAspace::Type::User, "test aspace2");

  user_inout_ptr<void> ptr{nullptr};
  auto err = AllocUser(aspace.get(), "test", kPageSize, &ptr);
  ASSERT_EQ(ZX_OK, err, "allocating region\n");

  // destroy the aspace, which should drop all the internal refs to it
  err = aspace->Destroy();
  EXPECT_EQ(ZX_OK, err, "VmAspace::Destroy");

  // drop the ref held by this pointer
  aspace.reset();
  END_TEST;
}

// Touch mappings in an aspace and ensure we can correctly harvest the accessed bits.
// This test takes an optional tag that is placed in the top byte of the address when performing a
// user_copy.
static bool vmaspace_accessed_test(uint8_t tag) {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create some memory we can map touch to test accessed tracking on. Needs to be created from
  // user pager backed memory as harvesting is allowed to be limited to just that.
  vm_page_t* page;
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &vmo);
  ASSERT_EQ(ZX_OK, status);
  auto mem = testing::UserMemory::Create(vmo, tag);

  ASSERT_EQ(ZX_OK, mem->CommitAndMap(kPageSize));

  // Initial accessed state is undefined, so harvest it away.
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);

  // Grab the current queue for the page and then rotate the page queues. This means any future,
  // correct, access harvesting should result in a new page queue.
  uint8_t current_queue = page->object.get_page_queue_ref().load();
  pmm_page_queues()->RotateReclaimQueues();

  // Read from the mapping to (hopefully) set the accessed bit.
  ConsumeValue(mem->get<int>(0));
  // Harvest it to move it in the page queue.
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);

  EXPECT_NE(current_queue, page->object.get_page_queue_ref().load());
  current_queue = page->object.get_page_queue_ref().load();

  // Rotating and harvesting again should not make the queue change since we have not accessed it.
  pmm_page_queues()->RotateReclaimQueues();
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(current_queue, page->object.get_page_queue_ref().load());

  // Set the accessed bit again, and make sure it does now harvest.
  pmm_page_queues()->RotateReclaimQueues();
  ConsumeValue(mem->get<int>(0));
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_NE(current_queue, page->object.get_page_queue_ref().load());

  // Set the accessed bit and update age without harvesting.
  ConsumeValue(mem->get<int>(0));
  harvest_access_bits(VmAspace::NonTerminalAction::Retain, VmAspace::TerminalAction::UpdateAge);
  current_queue = page->object.get_page_queue_ref().load();

  // Now if we rotate and update again, we should re-age the page.
  pmm_page_queues()->RotateReclaimQueues();
  harvest_access_bits(VmAspace::NonTerminalAction::Retain, VmAspace::TerminalAction::UpdateAge);
  EXPECT_NE(current_queue, page->object.get_page_queue_ref().load());
  current_queue = page->object.get_page_queue_ref().load();
  pmm_page_queues()->RotateReclaimQueues();
  harvest_access_bits(VmAspace::NonTerminalAction::Retain, VmAspace::TerminalAction::UpdateAge);
  EXPECT_NE(current_queue, page->object.get_page_queue_ref().load());

  END_TEST;
}

static bool vmaspace_accessed_test_untagged() { return vmaspace_accessed_test(0); }

#if defined(__aarch64__)
// Rerun the `vmaspace_accessed_test` tests with tags in the top byte of user pointers. This tests
// that the subsequent accessed faults are handled successfully, even if the FAR contains a tag.
static bool vmaspace_accessed_test_tagged() { return vmaspace_accessed_test(0xAB); }
#endif

// Ensure that if a user requested VMO read/write operation would hit a page that has had its
// accessed bits harvested that any resulting fault (on ARM) can be handled.
static bool vmaspace_usercopy_accessed_fault_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create some memory we can map touch to test accessed tracking on. Needs to be created from
  // user pager backed memory as harvesting is allowed to be limited to just that.
  vm_page_t* page;
  fbl::RefPtr<VmObjectPaged> mapping_vmo;
  zx_status_t status =
      make_committed_pager_vmo(1, /*trap_dirty=*/false, /*resizable=*/false, &page, &mapping_vmo);
  ASSERT_EQ(ZX_OK, status);
  auto mem = testing::UserMemory::Create(mapping_vmo);

  ASSERT_EQ(ZX_OK, mem->CommitAndMap(kPageSize));

  // Need a separate VMO to read/write from.
  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize, &vmo);
  ASSERT_EQ(status, ZX_OK);

  // Touch the mapping to make sure it is committed and mapped.
  mem->put<char>(42);

  // Harvest any accessed bits.
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);

  // Read from the VMO into the mapping that has been harvested.
  auto [read_status, read_actual] =
      vmo->ReadUser(mem->user_out<char>(), 0, sizeof(char), VmObjectReadWriteOptions::None);
  ASSERT_EQ(read_status, ZX_OK);
  ASSERT_EQ(read_actual, sizeof(char));

  END_TEST;
}

// Test that page tables that do not get accessed can be successfully unmapped and freed.
static bool vmaspace_free_unaccessed_page_tables_test() {
  BEGIN_TEST;

  // Disable for RISC-V for now, since the ArchMmmu code for this architecture currently
  // does not track accessed bits in intermediate page tables, and thus has no reasonable
  // way to honor NonTerminalAction::FreeUnaccessed on harvest calls.
#if defined(__riscv)
  printf("Skipping on RISC-V\n");
  return true;
#endif

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmObjectPaged> vmo;
  constexpr size_t kNumPages = 512 * 3;
  constexpr size_t kMiddlePage = kNumPages / 2;
  constexpr size_t kMiddleOffset = kMiddlePage * kPageSize;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize * kNumPages, &vmo));

  // Construct an additional aspace to use for mappings and touching pages. This allows us to
  // control whether the aspace is considered active, which can effect reclamation and scanning.
  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  auto cleanup_aspace = fit::defer([&aspace]() { aspace->Destroy(); });

  auto mem = testing::UserMemory::CreateInAspace(vmo, aspace);

  // Put the state we need to share in a struct so we can easily share it with the thread.
  struct State {
    testing::UserMemory* mem = nullptr;
    AutounsignalEvent touch_event;
    AutounsignalEvent complete_event;
    ktl::atomic<bool> running = true;
  } state;
  state.mem = &*mem;

  // Spin up a kernel thread in the aspace we made. This thread will just continuously wait on an
  // event, touching the mapping whenever it is signaled.
  auto thread_body = [](void* arg) -> int {
    State* state = static_cast<State*>(arg);

    while (state->running) {
      state->touch_event.Wait(Deadline::infinite());
      // Check running again so we do not try and touch mem if attempting to shutdown suddenly.
      if (state->running) {
        state->mem->put<char>(42, kMiddleOffset);
        // Signal the event back
        state->complete_event.Signal();
      }
    }
    return 0;
  };

  Thread* thread = Thread::Create("test-thread", thread_body, &state, DEFAULT_PRIORITY);
  ASSERT_NONNULL(thread);
  aspace->AttachToThread(thread);
  thread->Resume();

  auto cleanup_thread = fit::defer([&state, thread]() {
    state.running = false;
    state.touch_event.Signal();
    thread->Join(nullptr, ZX_TIME_INFINITE);
  });

  // Helper to synchronously wait for the thread to perform a touch.
  auto touch = [&state]() {
    state.touch_event.Signal();
    state.complete_event.Wait(Deadline::infinite());
  };

  EXPECT_OK(mem->CommitAndMap(kPageSize, kMiddleOffset));

  // Touch the mapping to ensure its accessed.
  touch();

  // Attempting to map should fail, as it's already mapped.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));

  touch();
  // Harvest the accessed information, this should not actually unmap it, even if we ask it to.
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));

  touch();
  // Harvest the accessed information, then attempt to do it again so that it gets unmapped.
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_OK(mem->CommitAndMap(kPageSize, kMiddleOffset));

  // Touch the mapping to ensure its accessed.
  touch();

  // Harvest the page accessed information, but retain the non-terminals.
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  // We can do this a few times.
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  // Now if we attempt to free unaccessed the non-terminal should still be accessed and so nothing
  // should get unmapped.
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));

  // If we are not requesting a free, then we should be able to harvest repeatedly.
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, mem->CommitAndMap(kPageSize, kMiddleOffset));
  harvest_access_bits(VmAspace::NonTerminalAction::Retain,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);

  END_TEST;
}

// Touch mappings in both the shared and restricted region of a unified aspace and ensure we can
// correctly harvest accessed bits.
static bool vmaspace_unified_accessed_test() {
  BEGIN_TEST;

  // Disable for RISC-V for now, since the ArchMmmu code for this architecture currently
  // does not track accessed bits in intermediate page tables, and thus has no reasonable
  // way to honor NonTerminalAction::FreeUnaccessed on harvest calls.
#if defined(__riscv)
  printf("Skipping on RISC-V\n");
  return true;
#endif

  AutoVmScannerDisable scanner_disable;

  // Create a unified aspace.
  constexpr vaddr_t kPrivateAspaceBase = USER_ASPACE_BASE;
  constexpr vaddr_t kPrivateAspaceSize = USER_RESTRICTED_ASPACE_SIZE;
  constexpr vaddr_t kSharedAspaceBase = kPrivateAspaceBase + kPrivateAspaceSize + kPageSize;
  constexpr vaddr_t kSharedAspaceSize = USER_ASPACE_BASE + USER_ASPACE_SIZE - kSharedAspaceBase;
  fbl::RefPtr<VmAspace> restricted_aspace =
      VmAspace::Create(kPrivateAspaceBase, kPrivateAspaceSize, VmAspace::Type::User,
                       "test restricted aspace", VmAspace::ShareOpt::Restricted);
  fbl::RefPtr<VmAspace> shared_aspace =
      VmAspace::Create(kSharedAspaceBase, kSharedAspaceSize, VmAspace::Type::User,
                       "test shared aspace", VmAspace::ShareOpt::Shared);
  fbl::RefPtr<VmAspace> unified_aspace =
      VmAspace::CreateUnified(shared_aspace.get(), restricted_aspace.get(), "test unified aspace");
  auto cleanup_aspace = fit::defer([&restricted_aspace, &shared_aspace, &unified_aspace]() {
    unified_aspace->Destroy();
    restricted_aspace->Destroy();
    shared_aspace->Destroy();
  });

  // Create regions of user memory that we can touch in both the shared and restricted regions.
  constexpr uint64_t kSize = 4 * kPageSize;
  fbl::RefPtr<VmObjectPaged> shared_vmo;
  fbl::RefPtr<VmObjectPaged> restricted_vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kSize, &shared_vmo));
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kSize, &restricted_vmo));
  ktl::unique_ptr<testing::UserMemory> shared_mem =
      testing::UserMemory::CreateInAspace(shared_vmo, shared_aspace);
  ktl::unique_ptr<testing::UserMemory> restricted_mem =
      testing::UserMemory::CreateInAspace(restricted_vmo, restricted_aspace);

  // Commit and map these regions to avoid page faults when we call `put` later on. We have to do
  // this because the `put` function invokes a `copy_to_user` that may trigger a page fault, which
  // the fault handler will try to resolve using the thread's current aspace. That aspace, in turn,
  // will be the unified aspace, which cannot resolve faults.
  constexpr uint64_t kMiddleOffset = kSize / 2;
  EXPECT_OK(shared_mem->CommitAndMap(kPageSize, kMiddleOffset));
  EXPECT_OK(restricted_mem->CommitAndMap(kPageSize, kMiddleOffset));

  // Switch to the unified aspace.
  // NOTE: This test takes care to not trigger any page faults or accessed faults from this point
  // on, because the unified aspace cannot resolve faults. For a user thread, we would rely on the
  // process dispatcher to look up if the faulting address lies in the shared aspace or the
  // restricted aspace, and use one of those to resolve the fault instead. We cannot exercise that
  // behavior from within a kernel unit test.
  VmAspace* old_aspace = Thread::Current::Get()->active_aspace();
  vmm_set_active_aspace(unified_aspace.get());
  auto reset_old_aspace = fit::defer([&old_aspace]() { vmm_set_active_aspace(old_aspace); });

#if defined __x86_64__
  // Touch the shared and restricted regions via the unified aspace. This will guarantee that
  // the accessed bits are set on x86, where the hardware sets the accessed bits.
  // On ARM, where we use software managed accessed bits, the CommitAndMap above will already have
  // set them.
  shared_mem->put<char>(42, kMiddleOffset);
  restricted_mem->put<char>(42, kMiddleOffset);
#endif

  // Harvest the accessed information. This should not actually unmap the pages.
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, shared_mem->CommitAndMap(kPageSize, kMiddleOffset));
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, restricted_mem->CommitAndMap(kPageSize, kMiddleOffset));

#if defined __x86_64__
  // Touch the memory again so that the accessed bits are guaranteed to be set. We must do this
  // because `CommitAndMap` does not set the accessed flag on x86.
  //
  // We specifically want to avoid doing this on ARM because the harvest will have cleared accessed
  // bits, and this `put` now will trigger an accessed fault on the unified aspace, which cannot
  // resolve any faults. Moreover, the CommitAndMap above will have already set the non-terminal
  // accessed bits on the walk down to the page.
  shared_mem->put<char>(43, kMiddleOffset);
  restricted_mem->put<char>(43, kMiddleOffset);
#endif

  // Harvest the accessed information, then attempt to do it again so that it gets unmapped.
  // The first `harvest_access_bits` call will clear the accessed bits, and the second will unmap
  // the memory.
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  harvest_access_bits(VmAspace::NonTerminalAction::FreeUnaccessed,
                      VmAspace::TerminalAction::UpdateAgeAndHarvest);
  EXPECT_OK(shared_mem->CommitAndMap(kPageSize, kMiddleOffset));
  EXPECT_OK(restricted_mem->CommitAndMap(kPageSize, kMiddleOffset));

  END_TEST;
}

// Tests that VmMappings that are marked mergeable behave correctly.
static bool vmaspace_merge_mapping_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");

  // Create a sub VMAR we'll use for all our testing.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  // Create two different vmos to make mappings into.
  fbl::RefPtr<VmObjectPaged> vmo1;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize * 4, &vmo1));
  fbl::RefPtr<VmObjectPaged> vmo2;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, kPageSize * 4, &vmo2));

  // Declare some enums to make writing test cases more readable instead of having lots of bools.
  enum MmuFlags { FLAG_TYPE_1, FLAG_TYPE_2 };
  enum MarkMerge { MERGE, NO_MERGE };
  enum MergeResult { MERGES_LEFT, DOES_NOT_MERGE };
  enum BeyondStreamSize { OK, FAULT };

  // To avoid boilerplate declare some tests in a data driven way.
  struct {
    struct {
      uint64_t vmar_offset;
      fbl::RefPtr<VmObjectPaged> vmo;
      uint64_t vmo_offset;
      MmuFlags flags;
      BeyondStreamSize beyond_stream_size;
      MergeResult merge_result;

    } mappings[3];
  } cases[] = {
      // Simple two mapping merge
      {{{0, vmo1, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, OK, MERGES_LEFT},
        {}}},
      // Simple three mapping merge
      {{{0, vmo1, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, OK, MERGES_LEFT},
        {kPageSize * 2, vmo1, kPageSize * 2, FLAG_TYPE_1, OK, MERGES_LEFT}}},
      // Different mapping flags should block merge
      {{{0, vmo1, 0, FLAG_TYPE_2, OK, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize * 2, vmo1, kPageSize * 2, FLAG_TYPE_1, OK, MERGES_LEFT}}},
      // Discontiguous aspace, but contiguous vmo should not work.
      {{{0, vmo1, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize * 2, vmo1, kPageSize, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {}}},
      // Similar discontiguous vmo, but contiguous aspace should not work.
      {{{0, vmo1, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize * 2, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {}}},
      // Leaving a contiguous hole also does not work, mapping needs to actually join.
      {{{0, vmo1, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize * 2, vmo1, kPageSize * 2, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {}}},
      // Different vmo should not work.
      {{{0, vmo2, 0, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {kPageSize * 2, vmo1, kPageSize * 2, FLAG_TYPE_1, OK, MERGES_LEFT}}},
      // Two fault-beyond-stream-size mapping merge
      {{{0, vmo1, 0, FLAG_TYPE_1, FAULT, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, FAULT, MERGES_LEFT},
        {}}},
      // Can't merge adjacent mappings if only one has fault-beyond-stream-size.
      {{{0, vmo1, 0, FLAG_TYPE_1, FAULT, DOES_NOT_MERGE},
        {kPageSize, vmo1, kPageSize, FLAG_TYPE_1, OK, DOES_NOT_MERGE},
        {}}},

  };

  for (auto& test : cases) {
    // Want to test all combinations of placing the mappings in subvmars, we just choose this by
    // iterating all the binary representations of 3 digits.
    for (int sub_vmar_comination = 0; sub_vmar_comination < 0b1000; sub_vmar_comination++) {
      const int use_subvmar[3] = {BIT_SET(sub_vmar_comination, 0), BIT_SET(sub_vmar_comination, 1),
                                  BIT_SET(sub_vmar_comination, 2)};
      // Iterate all orders of marking mergeable. For 3 mappings there are  6 possibilities.
      for (int merge_order_combination = 0; merge_order_combination < 6;
           merge_order_combination++) {
        const bool even_merge = (merge_order_combination % 2) == 0;
        const int first_merge = merge_order_combination / 2;
        const int merge_order[3] = {first_merge, (first_merge + (even_merge ? 1 : 2)) % 3,
                                    (first_merge + (even_merge ? 2 : 1)) % 3};

        // Instantiate the requested mappings.
        fbl::RefPtr<VmAddressRegion> vmars[3];
        fbl::RefPtr<VmMapping> mappings[3];
        MergeResult merge_result[3] = {DOES_NOT_MERGE, DOES_NOT_MERGE, DOES_NOT_MERGE};
        for (int i = 0; i < 3; i++) {
          if (test.mappings[i].vmo) {
            arch_mmu_flags_t mmu_flags =
                ARCH_MMU_FLAG_PERM_READ |
                (test.mappings[i].flags == FLAG_TYPE_1 ? ARCH_MMU_FLAG_PERM_WRITE : 0);
            uint vmar_flags = VMAR_FLAG_SPECIFIC | (test.mappings[i].beyond_stream_size == FAULT
                                                        ? VMAR_FLAG_FAULT_BEYOND_STREAM_SIZE
                                                        : 0);
            if (use_subvmar[i]) {
              ASSERT_OK(vmar->CreateSubVmar(test.mappings[i].vmar_offset, kPageSize, 0,
                                            VMAR_FLAG_SPECIFIC | VMAR_FLAG_CAN_MAP_SPECIFIC |
                                                VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
                                            "sub vmar", &vmars[i]));
              auto map_result =
                  vmars[i]->CreateVmMapping(0, kPageSize, 0, vmar_flags, test.mappings[i].vmo,
                                            test.mappings[i].vmo_offset, mmu_flags, "test mapping");
              ASSERT_OK(map_result.status_value());
              mappings[i] = ktl::move(map_result->mapping);
            } else {
              auto map_result = vmar->CreateVmMapping(
                  test.mappings[i].vmar_offset, kPageSize, 0, vmar_flags, test.mappings[i].vmo,
                  test.mappings[i].vmo_offset, mmu_flags, "test mapping");
              ASSERT_OK(map_result.status_value());
              mappings[i] = ktl::move(map_result->mapping);
            }
          }
          // By default we assume merging happens as declared in the test, unless either this our
          // immediate left is in a subvmar, in which case merging is blocked.
          if (use_subvmar[i] || (i > 0 && use_subvmar[i - 1])) {
            merge_result[i] = DOES_NOT_MERGE;
          } else {
            merge_result[i] = test.mappings[i].merge_result;
          }
        }

        // As we merge track expected mapping sizes and what we have merged
        bool merged[3] = {false, false, false};
        size_t expected_size[3] = {kPageSize, kPageSize, kPageSize};
        // Mark each mapping as mergeable based on merge_order
        for (const auto& mapping : merge_order) {
          if (test.mappings[mapping].vmo) {
            VmMapping::MarkMergeable(ktl::move(mappings[mapping]));
            merged[mapping] = true;
            // See if we have anything pending from the right
            if (mapping < 2 && merged[mapping + 1] && merge_result[mapping + 1] == MERGES_LEFT) {
              expected_size[mapping] += expected_size[mapping + 1];
              expected_size[mapping + 1] = 0;
            }
            // See if we should merge to the left.
            if (merge_result[mapping] == MERGES_LEFT && mapping > 0 && merged[mapping - 1]) {
              if (expected_size[mapping - 1] == 0) {
                expected_size[mapping - 2] += expected_size[mapping];
              } else {
                expected_size[mapping - 1] += expected_size[mapping];
              }
              expected_size[mapping] = 0;
            }
          }
          // Validate sizes to ensure any expected merging happened.
          for (int j = 0; j < 3; j++) {
            if (test.mappings[j].vmo) {
              Guard<CriticalMutex> guard{vmar->lock()};
              VmMapping* map = vmar->FindMappingLocked(test.mappings[j].vmar_offset + vmar->base());
              ASSERT_NONNULL(map);
              AssertHeld(map->lock_ref());
              if (expected_size[j] != 0) {
                EXPECT_EQ(map->size(), expected_size[j]);
                EXPECT_EQ(map->base(), vmar->base() + test.mappings[j].vmar_offset);
              }
            }
          }
        }

        // Destroy any mappings and VMARs.
        for (int i = 0; i < 3; i++) {
          if (test.mappings[i].vmo) {
            EXPECT_OK(vmar->Unmap(vmar->base() + test.mappings[i].vmar_offset, kPageSize,
                                  VmAddressRegionOpChildren::Yes));
          }
        }
      }
    }
  }

  // Cleanup the address space.
  EXPECT_OK(vmar->Destroy());
  EXPECT_OK(aspace->Destroy());
  END_TEST;
}

// Test that memory priority gets propagated through hierarchies and into newly created objects.
static bool vmaspace_priority_propagation_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create VMAR and a VMO and map it in.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 4, &vmo);
  ASSERT_OK(status);

  auto mapping_result =
      vmar->CreateVmMapping(0, kPageSize * 4, 0, 0, vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  // Set the priority in our vmar and validate it propagates to the VMO and the aspace.
  status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Create a new VMAR and VMO and map them into the high priority vmar. Memory priority should
  // propagate.
  fbl::RefPtr<VmAddressRegion> sub_vmar;
  ASSERT_OK(vmar->CreateSubVmar(
      0, kPageSize * 16, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
      "test sub-vmar", &sub_vmar));

  fbl::RefPtr<VmObjectPaged> vmo2;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 4, &vmo2);
  ASSERT_OK(status);

  auto mapping2_result =
      sub_vmar->CreateVmMapping(0, kPageSize * 4, 0, 0, vmo2, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping2_result.status_value());
  EXPECT_TRUE(vmo2->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Change the priority of the sub vmar. It should not effect the original vmar / vmo priority.
  status = sub_vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::DEFAULT);
  EXPECT_OK(status);
  EXPECT_FALSE(vmo2->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  EXPECT_OK(vmar->Destroy());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

// Test that unmapping parts of a mapping preserves priority.
static bool vmaspace_priority_unmap_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create VMAR and a VMO and map it in.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 8, &vmo);
  ASSERT_OK(status);

  auto mapping_result =
      vmar->CreateVmMapping(0, kPageSize * 8, 0, 0, vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  // Set the priority in our vmar and validate it propagates to the VMO and the aspace.
  status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  const vaddr_t base = mapping_result->base;

  // Unmap one page from either end of the mapping, ensuring memory priority did not change.
  EXPECT_OK(vmar->Unmap(base, kPageSize, VmAddressRegionOpChildren::No));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  EXPECT_OK(vmar->Unmap(base + kPageSize * 7, kPageSize, VmAddressRegionOpChildren::No));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Unmap a page from the middle. This will split this into two mappings.
  EXPECT_OK(vmar->Unmap(base + kPageSize * 4, kPageSize, VmAddressRegionOpChildren::No));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());
  // Now completely unmap one portion. This will destroy one of the mappings, but the VMO should
  // still have priority from the other mapping that was previously split.
  EXPECT_OK(vmar->Unmap(base + kPageSize, kPageSize * 3, VmAddressRegionOpChildren::No));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Unmapping the rest of the other portion should finally cause the priority to be removed.
  EXPECT_OK(vmar->Unmap(base + kPageSize * 5, kPageSize * 2, VmAddressRegionOpChildren::No));
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

// Test that overwriting a mapping maintains priority counts.
static bool vmaspace_priority_mapping_overwrite_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create VMAR and a VMO and map it in.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo);
  ASSERT_OK(status);

  auto mapping_result =
      vmar->CreateVmMapping(0, kPageSize, 0, 0, vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());
  fbl::RefPtr<VmMapping> mapping = ktl::move(mapping_result->mapping);

  status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Overwrite the mapping with a new one from a new VMO.
  fbl::RefPtr<VmObjectPaged> vmo2;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize, &vmo2);
  ASSERT_OK(status);

  mapping_result = vmar->CreateVmMapping(mapping->base() - vmar->base(), mapping->size(), 0,
                                         VMAR_FLAG_SPECIFIC_OVERWRITE, vmo2, 0, kArchRwUserFlags,
                                         "test-mapping2");
  ASSERT_OK(mapping_result.status_value());

  // Original VMO should have lost its priority, and the VMO for our new mapping should have gained.
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(vmo2->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

static bool vmaspace_priority_merged_mapping_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  zx_status_t status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 2, &vmo);
  ASSERT_OK(status);

  // Create a mapping for the first page of the VMO, and mark it mergeable
  auto mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                              vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  VmMapping::MarkMergeable(ktl::move(mapping_result->mapping));

  // Map in the second page.
  mapping_result = vmar->CreateVmMapping(kPageSize * 2, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                         vmo, kPageSize, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  VmMapping::MarkMergeable(ktl::move(mapping_result->mapping));

  // Query the vmar, should have a single mapping of the combined size.
  fbl::RefPtr<VmAddressRegionOrMapping> region = vmar->FindRegion(vmar->base() + kPageSize);
  ASSERT(region);
  fbl::RefPtr<VmMapping> map = region->as_vm_mapping();
  ASSERT(map);
  EXPECT_EQ(static_cast<size_t>(kPageSize * 2u), map->size());

  // Now destroy the mapping and check the VMO loses priority.
  EXPECT_OK(map->Destroy());

  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

static bool vmaspace_priority_bidir_clone_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  zx_status_t status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 2, &vmo);
  ASSERT_OK(status);

  auto mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                              vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Create a clone of the VMO.
  fbl::RefPtr<VmObject> vmo_child;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, true,
                            &vmo_child);
  ASSERT_OK(status);
  VmObjectPaged* childp = reinterpret_cast<VmObjectPaged*>(vmo_child.get());

  // Child should not have priority.
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_FALSE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Destroying the clone should leave memory priority unchanged of the original.
  vmo_child.reset();
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Remove the mapping.
  EXPECT_OK(mapping_result->mapping->Destroy());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Create a new clone of the VMO and map in the clone.
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kPageSize, true,
                            &vmo_child);
  ASSERT_OK(status);
  childp = reinterpret_cast<VmObjectPaged*>(vmo_child.get());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_FALSE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());
  mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                         vmo_child, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Now destroy the parent VMO and ensure child retains priority.
  vmo.reset();
  EXPECT_TRUE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());
  EXPECT_FALSE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  END_TEST;
}

static bool vmaspace_priority_slice_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  zx_status_t status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 2, &vmo);
  ASSERT_OK(status);

  auto mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                              vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Create a slice of the VMO.
  fbl::RefPtr<VmObject> vmo_slice;
  status = vmo->CreateChildSlice(0, kPageSize, true, &vmo_slice);
  ASSERT_OK(status);
  VmObjectPaged* slicep = reinterpret_cast<VmObjectPaged*>(vmo_slice.get());

  // Slice inherits priority.
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(slicep->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Change priority of the VMAR should remove from the VMO.
  EXPECT_OK(vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::DEFAULT));
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_FALSE(aspace->IsHighMemoryPriority());

  // Re-enable priority and verify.
  EXPECT_OK(vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH));
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(slicep->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Destroy slice and unmap.
  vmo_slice.reset();

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  END_TEST;
}

static bool vmaspace_priority_pager_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  zx_status_t status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  fbl::RefPtr<VmObjectPaged> vmo;
  status = make_committed_pager_vmo(1, false, false, nullptr, &vmo);
  ASSERT_OK(status);

  // Create a clone of the VMO.
  fbl::RefPtr<VmObject> vmo_child;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize, true,
                            &vmo_child);
  ASSERT_OK(status);
  VmObjectPaged* childp = reinterpret_cast<VmObjectPaged*>(vmo_child.get());

  // Map in the clone.
  auto mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                              vmo_child, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  // Validate the root and clone received the priority.
  EXPECT_TRUE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Create a second child of the root.
  fbl::RefPtr<VmObject> vmo_child2;
  status = vmo->CreateClone(Resizability::NonResizable, SnapshotType::OnWrite, 0, kPageSize, true,
                            &vmo_child);
  ASSERT_OK(status);
  VmObjectPaged* childp2 = reinterpret_cast<VmObjectPaged*>(vmo_child.get());

  // This child should not have any priority.
  EXPECT_FALSE(childp2->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Destroy it should leave the rest of the tree unchanged.
  vmo_child2.reset();
  EXPECT_TRUE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Remove priority and validate.
  EXPECT_OK(vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::DEFAULT));

  EXPECT_FALSE(childp->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

static bool vmaspace_priority_reference_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  zx_status_t status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  fbl::RefPtr<VmObjectPaged> vmo;
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 2, &vmo);
  ASSERT_OK(status);

  auto mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                              vmo, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(aspace->IsHighMemoryPriority());

  // Create a reference of the VMO.
  fbl::RefPtr<VmObject> vmo_reference;
  status =
      vmo->CreateChildReference(Resizability::NonResizable, 0, 0, true, nullptr, &vmo_reference);
  ASSERT_OK(status);
  VmObjectPaged* refp = reinterpret_cast<VmObjectPaged*>(vmo_reference.get());

  // Reference should have same priority.
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(refp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Remove the original mapping.
  mapping_result->mapping->Destroy();
  EXPECT_FALSE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_FALSE(refp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  // Now map in the reference.
  mapping_result = vmar->CreateVmMapping(kPageSize, kPageSize, 0, VMAR_FLAG_SPECIFIC_OVERWRITE,
                                         vmo_reference, 0, kArchRwUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  // Reference and vmo should have same priority.
  EXPECT_TRUE(vmo->DebugGetCowPages()->DebugIsHighMemoryPriority());
  EXPECT_TRUE(refp->DebugGetCowPages()->DebugIsHighMemoryPriority());

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

// Tests that memory attribution works as expected in a nested aspace hierarchy.
static bool vmaspace_nested_attribution_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // 8 page vmar.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 8, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  // Child vmar that covers the first 4 pages of the previous vmar.
  fbl::RefPtr<VmAddressRegion> subvmar1;
  ASSERT_OK(vmar->CreateSubVmar(
      0, kPageSize * 4, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &subvmar1));

  // Grandchild vmar that covers the first 2 pages of the child.
  fbl::RefPtr<VmAddressRegion> subvmar2;
  ASSERT_OK(subvmar1->CreateSubVmar(
      0, kPageSize * 2, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &subvmar2));

  // Make 2 page vmo.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 2 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  // Map VMO to grandchild.
  EXPECT_EQ(aspace->is_user(), true);
  auto mapping_result =
      subvmar2->CreateVmMapping(0, 2 * kPageSize, 0, 0, vmo, 0, kArchRwUserFlags, "test-mapping");
  EXPECT_EQ(ZX_OK, mapping_result.status_value());
  fbl::RefPtr<VmMapping> mapping = ktl::move(mapping_result->mapping);

  // Commit 2 pages into mapping.
  status = vmo->CommitRange(0, 2 * kPageSize);
  ASSERT_EQ(ZX_OK, status);

  // Verify that the two pages are counted for the parent vmar chain.
  ASSERT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) ==
              mapping->GetAttributedMemory());
  ASSERT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) ==
              subvmar2->GetAttributedMemory());
  ASSERT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) ==
              subvmar1->GetAttributedMemory());
  ASSERT_TRUE(make_private_attribution_counts(2ul * kPageSize, 0) == vmar->GetAttributedMemory());

  END_TEST;
}

// Tests that memory attribution at the VmMapping layer behaves as expected under commits and
// decommits on the vmo range.
static bool vm_mapping_attribution_commit_decommit_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  using AttributionCounts = VmObject::AttributionCounts;
  // Create a test VmAspace to temporarily switch to for creating test mappings.
  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create a VMO to map.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 16 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Map the left half of the VMO.
  EXPECT_EQ(aspace->is_user(), true);
  auto mapping_result = aspace->RootVmar()->CreateVmMapping(0, 8 * kPageSize, 0, 0, vmo, 0,
                                                            kArchRwUserFlags, "test-mapping");
  EXPECT_EQ(ZX_OK, mapping_result.status_value());
  fbl::RefPtr<VmMapping> mapping = ktl::move(mapping_result->mapping);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_TRUE(mapping->GetAttributedMemory() == AttributionCounts{});

  // Commit pages a little into the mapping, and past it.
  status = vmo->CommitRange(4 * kPageSize, 8 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(8ul * kPageSize, 0));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(4ul * kPageSize, 0));

  // Decommit the pages committed above, returning the VMO to zero committed pages.
  status = vmo->DecommitRange(4 * kPageSize, 8 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_TRUE(mapping->GetAttributedMemory() == AttributionCounts{});

  // Commit some pages in the VMO again.
  status = vmo->CommitRange(0, 10 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(10ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 10ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(8ul * kPageSize, 0));

  // Decommit pages in the vmo via the mapping.
  status = mapping->DecommitRange(0, mapping->size());
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() == AttributionCounts{});

  // Destroy the mapping.
  status = mapping->Destroy();
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(2ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 2ul * kPageSize));
  EXPECT_TRUE((vm::AttributionCounts{}) == mapping->GetAttributedMemory());

  // Free the test address space.
  status = aspace->Destroy();
  EXPECT_EQ(ZX_OK, status);

  END_TEST;
}

// Tests that memory attribution at the VmMapping layer behaves as expected under map and unmap
// operations on the mapping.
static bool vm_mapping_attribution_map_unmap_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  using AttributionCounts = VmObject::AttributionCounts;
  // Create a test VmAspace to temporarily switch to for creating test mappings.
  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create a VMO to map.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 16 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Map the left half of the VMO.
  EXPECT_EQ(aspace->is_user(), true);
  auto mapping_result = aspace->RootVmar()->CreateVmMapping(0, 8 * kPageSize, 0, 0, vmo, 0,
                                                            kArchRwUserFlags, "test-mapping");
  EXPECT_EQ(ZX_OK, mapping_result.status_value());
  fbl::RefPtr<VmMapping> mapping = ktl::move(mapping_result->mapping);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
  EXPECT_TRUE(mapping->GetAttributedMemory() == AttributionCounts{});

  // Commit pages in the vmo via the mapping.
  status = mapping->MapRange(0, mapping->size(), true);
  ASSERT_EQ(ZX_OK, status);
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(8ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 8ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(8ul * kPageSize, 0));

  // Unmap from the right end of the mapping.
  auto old_base = mapping->base();
  status = mapping->DebugUnmap(mapping->base() + mapping->size() - kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  mapping = aspace->FindRegion(old_base)->as_vm_mapping();
  ASSERT_TRUE(mapping);
  EXPECT_EQ(old_base, mapping->base());
  EXPECT_EQ(7ul * kPageSize, mapping->size());
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(8ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 8ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(7ul * kPageSize, 0));

  // Unmap from the center of the mapping.
  status = mapping->DebugUnmap(mapping->base() + 4 * kPageSize, kPageSize);
  ASSERT_EQ(ZX_OK, status);
  mapping = aspace->FindRegion(old_base)->as_vm_mapping();
  ASSERT_TRUE(mapping);
  EXPECT_EQ(old_base, mapping->base());
  EXPECT_EQ(4ul * kPageSize, mapping->size());
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(8ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 8ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(4ul * kPageSize, 0));

  // Unmap from the left end of the mapping.
  status = mapping->DebugUnmap(mapping->base(), kPageSize);
  ASSERT_EQ(ZX_OK, status);
  mapping = aspace->FindRegion(old_base + kPageSize)->as_vm_mapping();
  ASSERT_TRUE(mapping);
  EXPECT_NE(old_base, mapping->base());
  EXPECT_EQ(3ul * kPageSize, mapping->size());
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(8ul * kPageSize, 0));
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 8ul * kPageSize));
  EXPECT_TRUE(mapping->GetAttributedMemory() ==
              make_private_attribution_counts(3ul * kPageSize, 0));

  // Free the test address space.
  status = aspace->Destroy();
  EXPECT_EQ(ZX_OK, status);

  END_TEST;
}

// Tests that memory attribution at the VmMapping layer behaves as expected when adjacent mappings
// are merged.
static bool vm_mapping_attribution_merge_test() {
  BEGIN_TEST;
  AutoVmScannerDisable scanner_disable;

  using AttributionCounts = VmObject::AttributionCounts;
  // Create a test VmAspace to temporarily switch to for creating test mappings.
  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);
  EXPECT_EQ(aspace->is_user(), true);

  // Create a VMO to map.
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, 16 * kPageSize, &vmo);
  ASSERT_EQ(ZX_OK, status);

  EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
  EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));

  // Create some contiguous mappings, marked unmergeable (default behavior) to begin with.
  struct {
    fbl::RefPtr<VmMapping> ref = nullptr;
    AttributionCounts expected_attribution_counts;
  } mappings[4];

  uint64_t offset = 0;
  static constexpr uint64_t kSize = 4 * kPageSize;
  for (int i = 0; i < 4; i++) {
    auto mapping_result = aspace->RootVmar()->CreateVmMapping(
        offset, kSize, 0, VMAR_FLAG_SPECIFIC, vmo, offset, kArchRwUserFlags, "test-mapping");
    ASSERT_EQ(ZX_OK, mapping_result.status_value());
    mappings[i].ref = ktl::move(mapping_result->mapping);
    EXPECT_TRUE(vmo->GetAttributedMemory() == AttributionCounts{});
    EXPECT_TRUE(verify_continuous_attribution_bytes(*vmo, 0));
    EXPECT_TRUE(mappings[i].ref->GetAttributedMemory() == mappings[i].expected_attribution_counts);
    offset += kSize;
  }
  EXPECT_EQ(offset, 16ul * kPageSize);

  // Commit pages in the VMO.
  status = vmo->CommitRange(0, 16 * kPageSize);
  ASSERT_EQ(ZX_OK, status);
  for (int i = 0; i < 4; i++) {
    mappings[i].expected_attribution_counts = make_private_attribution_counts(4ul * kPageSize, 0);
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(16ul * kPageSize, 0));
    EXPECT_TRUE(mappings[i].ref->GetAttributedMemory() == mappings[i].expected_attribution_counts);
  }

  // Mark mappings 0 and 2 mergeable. This should not change anything since they're separated by an
  // unmergeable mapping.
  VmMapping::MarkMergeable(ktl::move(mappings[0].ref));
  VmMapping::MarkMergeable(ktl::move(mappings[2].ref));
  for (int i = 0; i < 4; i++) {
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(16ul * kPageSize, 0));
    fbl::RefPtr<VmMapping> map =
        aspace->FindRegion(aspace->RootVmar()->base() + kSize * i)->as_vm_mapping();
    ASSERT_TRUE(map);
    EXPECT_TRUE(map->GetAttributedMemory() == mappings[i].expected_attribution_counts);
  }

  // Mark mapping 3 mergeable. This will merge mappings 2 and 3, destroying mapping 3 and moving all
  // of its pages into mapping 2.
  VmMapping::MarkMergeable(ktl::move(mappings[3].ref));
  mappings[2].expected_attribution_counts += mappings[3].expected_attribution_counts;
  for (int i = 0; i < 3; i++) {
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(16ul * kPageSize, 0));
    fbl::RefPtr<VmMapping> map =
        aspace->FindRegion(aspace->RootVmar()->base() + kSize * i)->as_vm_mapping();
    ASSERT_TRUE(map);
    EXPECT_TRUE(map->GetAttributedMemory() == mappings[i].expected_attribution_counts);
  }

  // Mark mapping 1 mergeable. This will merge mappings 0, 1 and 2, with only mapping 0 surviving
  // the merge. All the VMO's pages will have been moved to mapping 0.
  VmMapping::MarkMergeable(ktl::move(mappings[1].ref));
  mappings[0].expected_attribution_counts += mappings[1].expected_attribution_counts;
  mappings[0].expected_attribution_counts += mappings[2].expected_attribution_counts;
  EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(16ul * kPageSize, 0));
  fbl::RefPtr<VmMapping> map = aspace->FindRegion(aspace->RootVmar()->base())->as_vm_mapping();
  ASSERT_TRUE(map);
  EXPECT_TRUE(map->GetAttributedMemory() == mappings[0].expected_attribution_counts);

  // Free the test address space.
  status = aspace->Destroy();
  EXPECT_EQ(ZX_OK, status);

  END_TEST;
}

static bool vm_mapping_sparse_mapping_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  // Create a large memory mapping with an empty backing VMO. Although this is a large virtual
  // address range, our later attempts to map it should be efficient.
  const size_t kMemorySize = 16 * GB;
  auto memory = testing::UserMemory::Create(kMemorySize);

  // Memory backing the user memory is currently empty, so attempting to map in it should succeed,
  // albeit with nothing populated.
  EXPECT_OK(memory->MapExisting(kMemorySize));

  // Commit a page in the middle, then re-map the whole thing and ensure the mapping is there.
  uint64_t val = 42;
  EXPECT_OK(memory->VmoWrite(&val, kMemorySize / 2, sizeof(val)));
  EXPECT_OK(memory->MapExisting(kMemorySize));
  EXPECT_EQ(val, memory->get<uint64_t>(kMemorySize / 2 / sizeof(uint64_t)));

  // Do the same test, but this time with the pages at the start and end of the range.
  EXPECT_OK(memory->VmoWrite(&val, 0, sizeof(val)));
  EXPECT_OK(memory->VmoWrite(&val, kMemorySize - kPageSize, sizeof(val)));
  EXPECT_OK(memory->MapExisting(kMemorySize));
  EXPECT_EQ(val, memory->get<uint64_t>(0));
  EXPECT_EQ(val, memory->get<uint64_t>((kMemorySize - kPageSize) / sizeof(uint64_t)));

  END_TEST;
}

static bool vm_mapping_page_fault_optimisation_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr uint64_t kMaxOptPages = VmMapping::kPageFaultMaxOptimisticPages;

  // Size the allocation of the VMO / mapping to be double the optimistic extension so we can
  // validate that it is limited by the optimistic cap, not the size of the VMO.
  constexpr size_t alloc_size = kMaxOptPages * 2 * kPageSize;
  static const uint8_t align_pow2 = log2_floor(alloc_size);

  // Mapped & fully committed VMO.
  fbl::RefPtr<VmObjectPaged> committed_vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &committed_vmo));

  ktl::unique_ptr<testing::UserMemory> mapping =
      testing::UserMemory::Create(committed_vmo, 0, align_pow2);
  ASSERT_NONNULL(mapping);

  committed_vmo->CommitRange(0, alloc_size);

  // Trigger a page fault on the first page in the VMO/Mapping.
  mapping->put(42);

  // Optimisation will fault the minimum of kMaxOptPages pages and the end of the VMO, protection
  // range, mapping or page table. We have ensured that all of these will be > kMaxOptPages in this
  // case.
  ASSERT_TRUE(verify_mapped_page_range(mapping->base(), alloc_size, kMaxOptPages));

  // Mapped but not committed VMO.
  fbl::RefPtr<VmObjectPaged> uncommitted_vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &uncommitted_vmo));

  ktl::unique_ptr<testing::UserMemory> mapping2 =
      testing::UserMemory::Create(uncommitted_vmo, 0, align_pow2);
  ASSERT_NONNULL(mapping2);

  // Trigger a page fault on the first page in the VMO/Mapping.
  mapping2->put(42);

  // As the VMO is uncommitted, only the requested page should have been faulted.
  ASSERT_TRUE(verify_mapped_page_range(mapping2->base(), alloc_size, 1));

  // Single committed page.
  fbl::RefPtr<VmObjectPaged> onepage_committed_vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &onepage_committed_vmo));

  ktl::unique_ptr<testing::UserMemory> mapping3 =
      testing::UserMemory::Create(onepage_committed_vmo, 0, align_pow2);
  ASSERT_NONNULL(mapping3);

  onepage_committed_vmo->CommitRange(0, kPageSize);

  // Trigger a page fault on the first page in the VMO/Mapping.
  mapping3->put(42);

  // Only the requested page should have been faulted.
  ASSERT_TRUE(verify_mapped_page_range(mapping3->base(), alloc_size, 1));

  // 4 committed pages.
  static_assert(4 <= kMaxOptPages);
  fbl::RefPtr<VmObjectPaged> partially_committed_vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, alloc_size, &partially_committed_vmo));

  ktl::unique_ptr<testing::UserMemory> mapping4 =
      testing::UserMemory::Create(partially_committed_vmo, 0, align_pow2);
  ASSERT_NONNULL(mapping4);

  partially_committed_vmo->CommitRange(0, 4 * kPageSize);

  // Trigger a page fault on the first page in the VMO/Mapping.
  mapping4->put(42);

  // Only the already committed pages should be committed.
  ASSERT_TRUE(verify_mapped_page_range(mapping4->base(), alloc_size, 4));

  END_TEST;
}

// Validate that the page fault optimisation correctly respects page table boundaries.
static bool vm_mapping_page_fault_optimization_pt_limit_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kMaxOptPages = VmMapping::kPageFaultMaxOptimisticPages;
  // Size our top level vmar allocation to be two page tables in size, ensuring that we will both
  // have a page table boundary crossing in the allocation, as well as some amount of allocation on
  // either side of it.
  constexpr size_t kPageTableSize = ArchVmAspace::NextUserPageTableOffset(0);
  constexpr size_t kVmarSize = kPageTableSize * 2;
  // Align our allocation on a page table boundary, ensuring we have 1 page table worth of space
  // before and after our PT crossing point.
  constexpr size_t kVmarAlign = log2_floor(kPageTableSize);
  // Size the allocation of the VMO / mapping to be double the optimistic extension so we can
  // validate that it is limited by the optimistic cap, not the size of the VMO.
  constexpr size_t kMapSize = kMaxOptPages * 2 * kPageSize;

  // Allocate our large top level vmar in root vmar of the current aspace.
  fbl::RefPtr<VmAddressRegion> root_vmar = Thread::Current::active_aspace()->RootVmar();
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(root_vmar->CreateSubVmar(0, kVmarSize, kVmarAlign,
                                     VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE |
                                         VMAR_FLAG_CAN_MAP_EXECUTE | VMAR_FLAG_CAN_MAP_SPECIFIC,
                                     "unittest", &vmar));
  auto cleanup_vmar = fit::defer([&] { vmar->Destroy(); });

  const vaddr_t next_pt_base = ArchVmAspace::NextUserPageTableOffset(vmar->base());
  // If our alignment was specified correctly the next pt should be exactly one pt from our base.
  ASSERT_EQ(vmar->base() + kPageTableSize, next_pt_base);

  // Try touching at different distances from the start of the next page table and validate that
  // mappings are not added beyond it.
  for (size_t page_offset = 0; page_offset <= kMaxOptPages + 1; page_offset++) {
    // Create a subvmar at the correct offset that will precisely hold our mapping.
    fbl::RefPtr<VmAddressRegion> sub_vmar;
    const size_t offset = kPageTableSize - kPageSize * page_offset;
    ASSERT_OK(vmar->CreateSubVmar(offset, kMapSize, 0,
                                  VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE |
                                      VMAR_FLAG_CAN_MAP_EXECUTE | VMAR_FLAG_SPECIFIC,
                                  "unittest", &sub_vmar));
    auto cleanup_sub_vmar = fit::defer([&] { sub_vmar->Destroy(); });

    fbl::RefPtr<VmObjectPaged> committed_vmo;
    ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kMapSize, &committed_vmo));

    ktl::unique_ptr<testing::UserMemory> mapping =
        testing::UserMemory::CreateInVmar(committed_vmo, sub_vmar);
    ASSERT_NONNULL(mapping);

    committed_vmo->CommitRange(0, kMapSize);

    // Trigger a page fault on the first page of the mapping.
    mapping->put(42);

    // We expect the number of pages that are mapped in to be clipped at the page table boundary,
    // which would be |page_offset|. The two exceptions to this are if page_offset is greater than
    // kMaxOptPages, in which case that becomes the cap, or if the page_offset is 0, in which case
    // we are actually at the *start* of the next page table, and so kMaxOptPages should get mapped.
    const size_t kExpectedPages =
        page_offset == 0 ? kMaxOptPages : ktl::min(kMaxOptPages, page_offset);

    ASSERT_TRUE(verify_mapped_page_range(mapping->base(), kMapSize, kExpectedPages));
  }

  END_TEST;
}

static bool vm_mapping_page_fault_range_test() {
  BEGIN_TEST;

  AutoVmScannerDisable scanner_disable;

  constexpr size_t kTestPages = VmMapping::kPageFaultMaxOptimisticPages * 2;
  constexpr size_t kAllocSize = kTestPages * kPageSize;
  constexpr uint kReadFlags = VMM_PF_FLAG_USER;
  constexpr uint kWriteFlags = VMM_PF_FLAG_USER | VMM_PF_FLAG_WRITE;
  // Aligning the mapping is for when testing the optimistic fault handler to ensure that there are
  // no spurious failures due to crossing a page table boundary.
  static const uint8_t align_pow2 = log2_floor(kAllocSize);

  fbl::RefPtr<VmObjectPaged> vmo;
  ASSERT_OK(VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kResizable, kAllocSize, &vmo));

  ktl::unique_ptr<testing::UserMemory> mapping = testing::UserMemory::Create(vmo, 0, align_pow2);
  ASSERT_NONNULL(mapping);

  // Faulting even 1 additional page should prevent optimistic faulting.
  {
    // Decommit and recommit the VMO to ensure no page table mappings.
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    EXPECT_OK(vmo->CommitRange(0, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));

    // Fault a two page range should only give two pages.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kReadFlags, kPageSize * 2));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 2));

    // Reset and fault a single page to validate optimistic faulting would otherwise have happened.
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    EXPECT_OK(vmo->CommitRange(0, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kReadFlags, kPageSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize,
                                         VmMapping::kPageFaultMaxOptimisticPages));
  }

  // Will map in pages that are not committed on read without allocating.
  {
    // Start with one page committed.
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    EXPECT_OK(vmo->CommitRange(0, kPageSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));

    // Read faulting the range should map without allocating.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kReadFlags, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, kTestPages));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));
  }

  // Write faulting should cause allocations
  {
    // Start with one page committed.
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    EXPECT_OK(vmo->CommitRange(0, kPageSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kPageSize, 0));

    // Write faulting the range should both map and allocate the pages.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kWriteFlags, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, kTestPages));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kAllocSize, 0));
  }

  // Faulting a partial range should not overrun
  {
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(0, 0));

    // Write faulting the range should both map and allocate the requested pages, but no more.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kWriteFlags, kAllocSize / 2));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, kTestPages / 2));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kAllocSize / 2, 0));
  }

  // Should not error if > VMO length.
  {
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    // Shrink the VMO so that it is smaller than the mapping.
    EXPECT_OK(vmo->Resize(kAllocSize / 2));
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));

    // Attempt to fault the entire mapping range, which is now larger than the VMO.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kWriteFlags, kAllocSize));
    // Only half should have been mapped and what is now the whole VMO should be committed.
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, kTestPages / 2));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kAllocSize / 2, 0));

    // Restore the VMO size.
    EXPECT_OK(vmo->Resize(kAllocSize));
  }

  // Will respect protection boundaries.
  {
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    // Remove write permissions from half the mapping.
    EXPECT_OK(mapping->Protect(ARCH_MMU_FLAG_PERM_READ, kAllocSize / 2));

    // Attempt to write fault the entire mapping.
    EXPECT_OK(Thread::Current::SoftFaultInRange(mapping->base(), kWriteFlags, kAllocSize));
    // Only the writable half should have been mapped and committed.
    EXPECT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, kTestPages / 2));
    EXPECT_TRUE(vmo->GetAttributedMemory() == make_private_attribution_counts(kAllocSize / 2, 0));

    // Reset protections.
    EXPECT_OK(mapping->Protect(ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE));
  }

  // Will mark modified if even one writable page is mapped even if mapping aborts early due to an
  // error.
  {
    // Create a pager backed VMO with one page committed and map it.
    fbl::RefPtr<VmObjectPaged> paged_vmo;
    ASSERT_OK(
        make_partially_committed_pager_vmo(kTestPages, 1, false, false, true, nullptr, &paged_vmo));
    ktl::unique_ptr<testing::UserMemory> paged_mapping = testing::UserMemory::Create(paged_vmo);

    // Consume any existing modified flag.
    zx_pager_vmo_stats_t stats;
    EXPECT_OK(paged_vmo->QueryPagerVmoStats(true, &stats));
    EXPECT_OK(paged_vmo->QueryPagerVmoStats(true, &stats));
    EXPECT_EQ(stats.modified, 0u);

    // Perform a fault that will have to generate a page request. To avoid blocking on the page
    // request we must directly call the PageFaultLocked method instead of the VmAspace fault.
    {
      const vaddr_t base = paged_mapping->base();
      ktl::pair<zx_status_t, uint32_t> result;
      size_t retry_count = 0;
      do {
        Guard<CriticalMutex> guard{paged_mapping->mapping()->lock()};
        MultiPageRequest page_request;
        // Although the first page is supplied to paged_vmo, attempting to map it could still fail
        // due to either it being deduped to a marker, or it being a loaned page and needing to be
        // swapped. Both of these cases require an allocation, which could need to wait. This wait
        // request should only be due to the pmm random delayed allocations, and so we can just
        // ignore it and try again.
        result = paged_mapping->mapping()->PageFaultLocked(base, kWriteFlags, kTestPages - 1,
                                                           &page_request);
        page_request.CancelRequests();
        retry_count++;
      } while (result.first == ZX_ERR_SHOULD_WAIT && result.second == 0 && retry_count < 100);
      EXPECT_EQ(result.first, ZX_ERR_SHOULD_WAIT);
      EXPECT_EQ(result.second, 1u);
    }

    // The one previously committed page should have been mapped in and the VMO marked modified.
    EXPECT_TRUE(verify_mapped_page_range(paged_mapping->base(), kAllocSize, 1));

    EXPECT_OK(paged_vmo->QueryPagerVmoStats(true, &stats));
    EXPECT_EQ(stats.modified, ZX_PAGER_VMO_STATS_MODIFIED);
  }

  // Read fault on copy-on-write hierarchy with some leaf pages will map both parent and child
  // pages without committing extra pages into the child.
  {
    EXPECT_OK(vmo->CommitRange(0, kAllocSize));
    // Create a snapshot with some committed pages and map it in.
    fbl::RefPtr<VmObject> child_vmo;
    ASSERT_OK(vmo->CreateClone(Resizability::NonResizable, SnapshotType::Full, 0, kAllocSize, true,
                               &child_vmo));
    EXPECT_OK(child_vmo->CommitRange(0, kPageSize));
    EXPECT_OK(child_vmo->CommitRange(kAllocSize / 2, kPageSize));
    ktl::unique_ptr<testing::UserMemory> child_mapping = testing::UserMemory::Create(child_vmo);

    // Read fault the entire range. Everything should get mapped with the child's memory attribution
    // being unchanged.
    VmObject::AttributionCounts original_counts = child_vmo->GetAttributedMemory();
    EXPECT_OK(Thread::Current::SoftFaultInRange(child_mapping->base(), kReadFlags, kAllocSize));
    EXPECT_TRUE(verify_mapped_page_range(child_mapping->base(), kAllocSize, kTestPages));
    EXPECT_TRUE(original_counts == child_vmo->GetAttributedMemory());
    EXPECT_TRUE(verify_continuous_attribution_bytes(
        *child_vmo, original_counts.uncompressed_bytes + original_counts.compressed_bytes));
  }

  // Calling ReadUser will fault the requested range.
  {
    EXPECT_OK(vmo->DecommitRange(0, kAllocSize));
    vmo->CommitRange(0, kAllocSize);

    ASSERT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 0));

    auto [status, read_actual] = vmo->ReadUser(
        mapping->user_out<char>(), 0, sizeof(char[kPageSize * 2]), VmObjectReadWriteOptions::None);
    ASSERT_EQ(status, ZX_OK);
    ASSERT_EQ(read_actual, sizeof(char[kPageSize * 2]));

    // The page fault optimisation should not have been triggered so the exact range is mapped.
    ASSERT_TRUE(verify_mapped_page_range(mapping->base(), kAllocSize, 2));
  }

  END_TEST;
}

// Attempt force a high priority region to be writeable.
static bool vm_mapping_force_writeable_high_priority() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test-aspace");
  ASSERT_NONNULL(aspace);

  // Create VMAR and a VMO and map it in.
  fbl::RefPtr<VmAddressRegion> vmar;
  ASSERT_OK(aspace->RootVmar()->CreateSubVmar(
      0, kPageSize * 64, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, "test vmar",
      &vmar));

  auto cleanup_sub_vmar = fit::defer([&] { vmar->Destroy(); });

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, kPageSize * 4, &vmo);
  ASSERT_OK(status);

  // Create a read-only user mapping. Since there is no ARCH_MMU_FLAG_PERM_WRITE, ForceWritable
  // won't trivially succeed.
  constexpr uint kArchReadUserFlags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_USER;
  auto mapping_result =
      vmar->CreateVmMapping(0, kPageSize * 4, 0, 0, vmo, 0, kArchReadUserFlags, "test-mapping");
  ASSERT_OK(mapping_result.status_value());

  status = vmar->SetMemoryPriority(VmAddressRegion::MemoryPriority::HIGH);
  EXPECT_OK(status);

  zx::result<fbl::RefPtr<VmMapping>> force_result = mapping_result->mapping->ForceWritable();
  EXPECT_OK(force_result.status_value());

  END_TEST;
}

using ArchUnmapOptions = ArchVmAspaceInterface::ArchUnmapOptions;

static bool arch_noncontiguous_map() {
  BEGIN_TEST;

  // Get some phys pages to test on
  paddr_t phys[3];
  VmPageDoublyLinkedList phys_list;
  zx_status_t status = pmm_alloc_pages(ktl::size(phys), 0, &phys_list);
  ASSERT_EQ(ZX_OK, status, "non contig map alloc");
  {
    size_t i = 0;
    for (auto& p : phys_list) {
      phys[i] = p.paddr();
      ++i;
    }
  }

  {
    constexpr vaddr_t base = USER_ASPACE_BASE + 10 * kPageSize;

    ArchVmAspace aspace(USER_ASPACE_BASE, USER_ASPACE_SIZE, 0);
    status = aspace.Init();
    ASSERT_EQ(ZX_OK, status, "failed to init aspace\n");

    // Attempt to map a set of vm_page_t
    status = aspace.Map(base, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Error);
    ASSERT_EQ(ZX_OK, status, "failed first map\n");

    // Expect that the map succeeded
    for (size_t i = 0; i < ktl::size(phys); ++i) {
      paddr_t paddr;
      arch_mmu_flags_t mmu_flags;
      status = aspace.Query(base + i * kPageSize, &paddr, &mmu_flags);
      EXPECT_EQ(ZX_OK, status, "bad first map\n");
      EXPECT_EQ(phys[i], paddr, "bad first map\n");
      EXPECT_EQ(ARCH_MMU_FLAG_PERM_READ, mmu_flags, "bad first map\n");
    }

    // Attempt to map again, should fail
    status = aspace.Map(base, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Error);
    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, status, "double map\n");

    // Attempt to map partially overlapping, should fail
    status = aspace.Map(base + 2 * kPageSize, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Error);
    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, status, "double map\n");
    status = aspace.Map(base - 2 * kPageSize, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Error);
    EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, status, "double map\n");

    // No entries should have been created by the partial failures
    status = aspace.Query(base - 2 * kPageSize, nullptr, nullptr);
    EXPECT_EQ(ZX_ERR_NOT_FOUND, status, "bad first map\n");
    status = aspace.Query(base - kPageSize, nullptr, nullptr);
    EXPECT_EQ(ZX_ERR_NOT_FOUND, status, "bad first map\n");
    status = aspace.Query(base + 3 * kPageSize, nullptr, nullptr);
    EXPECT_EQ(ZX_ERR_NOT_FOUND, status, "bad first map\n");
    status = aspace.Query(base + 4 * kPageSize, nullptr, nullptr);
    EXPECT_EQ(ZX_ERR_NOT_FOUND, status, "bad first map\n");

    // Unmap all remaining entries
    // The partial failures did not create any new entries, so only entries
    // created by the first map should be unmapped.
    status = aspace.Unmap(base, ktl::size(phys), ArchUnmapOptions::Enlarge);
    ASSERT_EQ(ZX_OK, status, "failed unmap\n");

    status = aspace.Destroy();
    EXPECT_EQ(ZX_OK, status, "failed to destroy aspace\n");
  }

  pmm_free(&phys_list);

  END_TEST;
}

static bool arch_noncontiguous_map_with_upgrade() {
  BEGIN_TEST;

  // Get some phys pages to test on
  paddr_t phys[3];
  VmPageDoublyLinkedList phys_list;
  zx_status_t status = pmm_alloc_pages(ktl::size(phys), 0, &phys_list);
  ASSERT_EQ(ZX_OK, status, "non contig map alloc");
  {
    size_t i = 0;
    for (auto& p : phys_list) {
      phys[i] = p.paddr();
      ++i;
    }
  }

  {
    constexpr vaddr_t base = USER_ASPACE_BASE + 10 * kPageSize;
    constexpr vaddr_t window_base = base - 2 * kPageSize;

    ArchVmAspace aspace(USER_ASPACE_BASE, USER_ASPACE_SIZE, 0);
    status = aspace.Init();
    ASSERT_EQ(ZX_OK, status, "failed to init aspace\n");

    // Attempt to map a set of vm_page_t
    status = aspace.Map(base, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Error);
    ASSERT_EQ(ZX_OK, status, "failed first map\n");

    // Attempt to map with upgrades allowed, should succeed
    status =
        aspace.Map(base, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
                   ArchVmAspace::ExistingEntryAction::Upgrade);
    EXPECT_EQ(ZX_OK, status, "map upgrade failed\n");

    // Attempt to map with upgrades allowed, should succeed but not remap anything
    // b/c downgrade to read not allowed
    status = aspace.Map(base, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Upgrade);
    EXPECT_EQ(ZX_OK, status, "map upgrade failed\n");

    // Expect that the upgrade maps succeeded
    for (size_t i = 0; i < ktl::size(phys); ++i) {
      const uint MAP_WINDOW_PADDR_INDEX[ktl::size(phys)] = {0, 1, 2};
      constexpr uint MAP_WINDOW_MMU_FLAGS[ktl::size(phys)] = {
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE};

      paddr_t paddr;
      arch_mmu_flags_t mmu_flags;
      status = aspace.Query(base + i * kPageSize, &paddr, &mmu_flags);
      EXPECT_EQ(ZX_OK, status, "bad map upgrade\n");
      EXPECT_EQ(phys[MAP_WINDOW_PADDR_INDEX[i]], paddr, "bad map upgrade\n");
      EXPECT_EQ(MAP_WINDOW_MMU_FLAGS[i], mmu_flags, "bad map upgrade\n");
    }

    // Attempt to map partially overlapping with upgrades allowed, should succeed
    status = aspace.Map(base + 2 * kPageSize, phys, ktl::size(phys),
                        ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
                        ArchVmAspace::ExistingEntryAction::Upgrade);
    EXPECT_EQ(ZX_OK, status, "map upgrade failed\n");
    status = aspace.Map(base - 2 * kPageSize, phys, ktl::size(phys), ARCH_MMU_FLAG_PERM_READ,
                        ArchVmAspace::ExistingEntryAction::Upgrade);
    EXPECT_EQ(ZX_OK, status, "map upgrade failed\n");

    // Expect that the `Upgrade` maps succeeded
    // We check the entire [base - 2, base + 4] "window" covered by the partial maps
    constexpr size_t MAP_WINDOW_SIZE = 7;
    for (size_t i = 0; i < MAP_WINDOW_SIZE; ++i) {
      const uint MAP_WINDOW_PADDR_INDEX[MAP_WINDOW_SIZE] = {0, 1, 2, 1, 0, 1, 2};
      constexpr uint MAP_WINDOW_MMU_FLAGS[MAP_WINDOW_SIZE] = {
          ARCH_MMU_FLAG_PERM_READ,
          ARCH_MMU_FLAG_PERM_READ,
          ARCH_MMU_FLAG_PERM_READ,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
          ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE};

      paddr_t paddr;
      arch_mmu_flags_t mmu_flags;
      status = aspace.Query(window_base + i * kPageSize, &paddr, &mmu_flags);
      EXPECT_EQ(ZX_OK, status, "bad map upgrade\n");
      EXPECT_EQ(phys[MAP_WINDOW_PADDR_INDEX[i]], paddr, "bad map upgrade\n");
      EXPECT_EQ(MAP_WINDOW_MMU_FLAGS[i], mmu_flags, "bad map upgrade\n");
    }

    // Unmap any remaining entries
    status = aspace.Unmap(window_base, MAP_WINDOW_SIZE, ArchUnmapOptions::Enlarge);
    ASSERT_EQ(ZX_OK, status, "failed unmap\n");

    status = aspace.Destroy();
    EXPECT_EQ(ZX_OK, status, "failed to destroy aspace\n");
  }

  pmm_free(&phys_list);

  END_TEST;
}

// Get the mmu_flags of the given vaddr of the given aspace.
//
// Return 0 if the page is unmapped or on error.
static arch_mmu_flags_t get_vaddr_flags(ArchVmAspace* aspace, vaddr_t vaddr) {
  paddr_t unused_paddr;
  arch_mmu_flags_t mmu_flags;
  if (aspace->Query(vaddr, &unused_paddr, &mmu_flags) != ZX_OK) {
    return 0;
  }
  return mmu_flags;
}

// Determine if the given page is mapped in.
static bool is_vaddr_mapped(ArchVmAspace* aspace, vaddr_t vaddr) {
  return get_vaddr_flags(aspace, vaddr) != 0;
}

static bool arch_vm_aspace_protect_split_pages() {
  BEGIN_TEST;

  constexpr uint kReadOnly = ARCH_MMU_FLAG_PERM_READ;
  constexpr uint kReadWrite = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

  // Create a basic address space, starting from vaddr 0.
  ArchVmAspace aspace(0, USER_ASPACE_SIZE, 0);
  ASSERT_OK(aspace.Init());
  auto cleanup = fit::defer([&]() {
    aspace.Unmap(0, USER_ASPACE_SIZE / kPageSize, ArchUnmapOptions::Enlarge);
    aspace.Destroy();
  });

  // Map in a large contiguous area, which should be mapped by two large pages.
  static_assert(ZX_MAX_PAGE_SIZE > kPageSize);
  constexpr size_t kRegionSize = 16ul * 1024 * 1024 * 1024;  // 16 GiB.
  ASSERT_OK(
      aspace.MapContiguous(/*vaddr=*/0, /*paddr=*/0, /*count=*/kRegionSize / kPageSize, kReadOnly));

  // Attempt to protect a subrange in the middle of the region, which will require splitting
  // pages.
  constexpr vaddr_t kProtectedRange = kRegionSize / 2 - kPageSize;
  constexpr size_t kProtectedPages = 2;
  ASSERT_OK(aspace.Protect(kProtectedRange, /*count=*/kProtectedPages, kReadWrite,
                           ArchUnmapOptions::Enlarge));

  // Ensure the pages inside the range changed.
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange), kReadWrite);
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange + kPageSize), kReadWrite);

  // Ensure the pages surrounding the range did not change.
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange - kPageSize), kReadOnly);
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange + kProtectedPages * kPageSize), kReadOnly);

  END_TEST;
}

static bool arch_vm_aspace_protect_split_pages_out_of_memory() {
  BEGIN_TEST;

  constexpr uint kReadOnly = ARCH_MMU_FLAG_PERM_READ;
  constexpr uint kReadWrite = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

  // Create a custom allocator that we can cause to stop returning allocations.
  //
  // ArchVmAspace doesn't allow us to send state to the allocator, so we use a
  // global static here to control the allocator.
  static bool allow_allocations;
  auto allocator = +[](uint alloc_flags, vm_page** p, paddr_t* pa) -> zx_status_t {
    if (!allow_allocations) {
      return ZX_ERR_NO_MEMORY;
    }
    return pmm_alloc_page(0, p, pa);
  };
  allow_allocations = true;

  // Create a basic address space, starting from vaddr 0.
  ArchVmAspace aspace(0, USER_ASPACE_SIZE, 0, allocator);
  ASSERT_OK(aspace.Init());
  auto cleanup = fit::defer([&]() {
    aspace.Unmap(0, USER_ASPACE_SIZE / kPageSize, ArchUnmapOptions::Enlarge);
    aspace.Destroy();
  });

  // Map in a large contiguous area, large enough to use large pages to fill.
  constexpr size_t kRegionSize = 16ul * 1024 * 1024 * 1024;  // 16 GiB.
  ASSERT_OK(
      aspace.MapContiguous(/*vaddr=*/0, /*paddr=*/0, /*count=*/kRegionSize / kPageSize, kReadOnly));

  // Prevent further allocations.
  allow_allocations = false;

  // Attempt to protect a subrange in the middle of the region, which will require splitting
  // pages. Expect this to fail.
  constexpr vaddr_t kProtectedRange = kRegionSize / 2 - kPageSize;
  constexpr size_t kProtectedSize = 2 * kPageSize;
  zx_status_t status =
      aspace.Protect(kProtectedRange, /*count=*/2, kReadWrite, ArchUnmapOptions::Enlarge);
  EXPECT_EQ(status, ZX_ERR_NO_MEMORY);

  // The pages surrounding our protect range should still be mapped.
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange - kPageSize), kReadOnly);
  EXPECT_EQ(get_vaddr_flags(&aspace, kProtectedRange + kProtectedSize), kReadOnly);

  // The pages we tried to protect should still be mapped, albeit permissions might
  // be changed.
  EXPECT_TRUE(is_vaddr_mapped(&aspace, kProtectedRange));
  EXPECT_TRUE(is_vaddr_mapped(&aspace, kProtectedRange + kPageSize));

  END_TEST;
}

// Test to make sure all the vm kernel regions (code, rodata, data, bss, etc.) are correctly mapped
// in vm and have the correct arch_mmu_flags. This test also check that all gaps are contained
// within a VMAR.
static bool vm_kernel_region_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAddressRegionOrMapping> kernel_vmar =
      VmAspace::kernel_aspace()->RootVmar()->FindRegion(
          reinterpret_cast<vaddr_t>(__executable_start));
  EXPECT_NE(kernel_vmar.get(), nullptr);
  EXPECT_FALSE(kernel_vmar->is_mapping());
  for (vaddr_t base = reinterpret_cast<vaddr_t>(__executable_start);
       base < reinterpret_cast<vaddr_t>(_end); base += kPageSize) {
    bool within_region = false;
    for (const auto& kernel_region : kernel_regions) {
      // This would not overflow because the region base and size are hard-coded.
      if (kernel_region.size != 0 && base >= kernel_region.base &&
          base + kPageSize <= kernel_region.base + kernel_region.size) {
        // If this page exists within a kernel region, then it should be within a VmMapping with
        // the correct arch MMU flags.
        within_region = true;
        fbl::RefPtr<VmAddressRegionOrMapping> region =
            kernel_vmar->as_vm_address_region()->FindRegion(base);
        // Every page from __code_start to _end should either be a VmMapping or a VMAR.
        EXPECT_NE(region.get(), nullptr);
        EXPECT_TRUE(region->is_mapping());
        Guard<CriticalMutex> guard{region->as_vm_mapping()->lock()};
        EXPECT_EQ(kernel_region.arch_mmu_flags,
                  region->as_vm_mapping()->arch_mmu_flags_locked(base));
        break;
      }
    }
    if (!within_region) {
      auto region = VmAspace::kernel_aspace()->RootVmar()->FindRegion(base);
      EXPECT_EQ(region.get(), kernel_vmar.get());
    }
  }

  END_TEST;
}

class TestRegionList;

class TestRegion : public fbl::RefCounted<TestRegion> {
 public:
  TestRegion(vaddr_t base, size_t size, TestRegionList const& list)
      : list_(list), base_(base), size_(size) {}
  ~TestRegion() = default;

  vaddr_t base() const { return base_; }
  size_t size() const { return size_; }

  vaddr_t base_locked() const { return base_; }
  size_t size_locked() const { return size_; }

  Lock<CriticalMutex>* lock() const;
  Lock<CriticalMutex>& lock_ref() const;

 private:
  friend class TestRegionList;
  // Simulates aspace for templated code
  TestRegionList const& list_;
  vaddr_t base_;
  size_t size_;
};

class TestRegionList : public fbl::RefCounted<TestRegionList> {
 public:
  TestRegionList() : guard_(&lock_) {}
  Lock<CriticalMutex>* lock() const TA_RET_CAP(lock_) { return &lock_; }
  Lock<CriticalMutex>& lock_ref() const TA_RET_CAP(lock_) { return lock_; }

  RegionList<TestRegion>* get_regions() { return &regions_; }

  void insert_region(vaddr_t base, size_t size) {
    fbl::AllocChecker ac;
    auto test_region = fbl::AdoptRef(new (&ac) TestRegion(base, size, *this));
    ASSERT(ac.check());
    zx_status_t status = regions_.InsertRegion(ktl::move(test_region));
    ASSERT(status == ZX_OK);
  }

  bool remove_region(vaddr_t base) {
    auto region = regions_.FindRegion(base);
    if (region == nullptr) {
      return false;
    }
    regions_.RemoveRegion(region);
    return true;
  }

 private:
  friend class TestRegion;
  mutable DECLARE_CRITICAL_MUTEX(TestRegionList) lock_;
  Guard<CriticalMutex> guard_;
  RegionList<TestRegion> regions_;
};

Lock<CriticalMutex>* TestRegion::lock() const TA_RET_CAP(list_.lock()) { return list_.lock(); }
Lock<CriticalMutex>& TestRegion::lock_ref() const TA_RET_CAP(list_.lock()) {
  return list_.lock_ref();
}

static bool region_list_get_alloc_spot_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;
  vaddr_t size = 0x0001000000000000;
  vaddr_t alloc_spot = 0;
  // Set the align to be 0x1000.
  uint8_t align_pow2 = 12;
  // Allocate 1 page, should be allocated at [+0, +0x1000].
  size_t alloc_size = 0x1000;
  zx_status_t status = regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size,
                                             base, size, /*prng=*/nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(base, alloc_spot);

  test_list.insert_region(alloc_spot, alloc_size);

  // Manually insert a sub region at [+0x2000, 0x3000].
  test_list.insert_region(base + 0x2000, alloc_size);

  // Try to allocate 2 page, since the gap is too small, we would allocate at [0x3000, 0x5000].
  alloc_size = 0x2000;
  status = regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size, base, size,
                                 /*prng=*/nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(base + 0x3000, alloc_spot);
  test_list.insert_region(alloc_spot, alloc_size);

  EXPECT_TRUE(test_list.remove_region(base + 0x2000));

  // After we remove the region, we now have a gap at [0x1000, 0x3000].
  alloc_size = 0x2000;
  status = regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size, base, size,
                                 /*prng=*/nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(base + 0x1000, alloc_spot);
  test_list.insert_region(alloc_spot, alloc_size);

  // Now we have fill all the gaps, next region should start at 0x5000.
  alloc_size = 0x1000;
  status = regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size, base, size,
                                 /*prng=*/nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(base + 0x5000, alloc_spot);
  test_list.insert_region(alloc_spot, alloc_size);

  // Test for possible overflow cases. We try to allocate all the rest of the spaces. The last
  // region should be from [0x6000, base + size - 1], we should be able to find this region and
  // allocate all the size from it.
  alloc_size = size - 0x6000;
  status = regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size, base, size,
                                 /*prng=*/nullptr);
  EXPECT_EQ(ZX_OK, status);
  EXPECT_EQ(base + 0x6000, alloc_spot);

  END_TEST;
}

static bool region_list_get_alloc_spot_no_memory_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;
  vaddr_t size = 0x0001000000000000;
  // Set the align to be 0x1000.
  uint8_t align_pow2 = 12;

  test_list.insert_region(base, size - 0x1000);

  size_t alloc_size = 0x2000;
  vaddr_t alloc_spot = 0;
  // There is only a 1 page gap, and we are asking for two pages, so ZX_ERR_NO_RESOURCES should be
  // returned.
  zx_status_t status =
      regions->GetAllocSpot(&alloc_spot, align_pow2, /*entropy=*/0, alloc_size, base, size,
                            /*prng=*/nullptr);
  EXPECT_EQ(ZX_ERR_NO_RESOURCES, status);

  END_TEST;
}

static bool region_list_find_region_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;

  auto region = regions->FindRegion(base);
  EXPECT_EQ(region, nullptr);

  test_list.insert_region(base + 0x1000, 0x1000);

  region = regions->FindRegion(base + 1);
  EXPECT_EQ(region, nullptr);

  region = regions->FindRegion(base + 0x1001);
  EXPECT_NE(region, nullptr);
  EXPECT_EQ(base + 0x1000, region->base());
  EXPECT_EQ((size_t)0x1000, region->size());

  END_TEST;
}

static bool region_list_include_or_higher_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;

  test_list.insert_region(base + 0x1000, 0x1000);

  auto itr = regions->IncludeOrHigher(base + 1);
  EXPECT_TRUE(itr.IsValid());
  EXPECT_EQ(base + 0x1000, (*itr).second->base());
  EXPECT_EQ((size_t)0x1000, (*itr).second->size());

  itr = regions->IncludeOrHigher(base + 0x1001);
  EXPECT_TRUE(itr.IsValid());
  EXPECT_EQ(base + 0x1000, (*itr).second->base());
  EXPECT_EQ((size_t)0x1000, (*itr).second->size());

  itr = regions->IncludeOrHigher(base + 0x2000);
  EXPECT_FALSE(itr.IsValid());

  END_TEST;
}

static bool region_list_upper_bound_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;

  test_list.insert_region(base + 0x1000, 0x1000);

  auto itr = regions->UpperBound(base + 0xFFF);
  EXPECT_TRUE(itr.IsValid());
  EXPECT_EQ(base + 0x1000, (*itr).second->base());
  EXPECT_EQ((size_t)0x1000, (*itr).second->size());

  itr = regions->UpperBound(base + 0x1000);
  EXPECT_FALSE(itr.IsValid());

  END_TEST;
}

static bool region_list_is_range_available_test() {
  BEGIN_TEST;

  TestRegionList test_list;
  auto regions = test_list.get_regions();
  vaddr_t base = 0xFFFF000000000000;

  test_list.insert_region(base + 0x1000, 0x1000);
  test_list.insert_region(base + 0x3000, 0x1000);

  EXPECT_TRUE(regions->IsRangeAvailable(base, 0x1000));
  EXPECT_FALSE(regions->IsRangeAvailable(base, 0x1001));
  EXPECT_FALSE(regions->IsRangeAvailable(base + 1, 0x1000));
  EXPECT_TRUE(regions->IsRangeAvailable(base + 0x2000, 1));
  EXPECT_FALSE(regions->IsRangeAvailable(base + 0x1FFF, 0x2000));

  EXPECT_TRUE(regions->IsRangeAvailable(0xFFFFFFFFFFFFFFFF, 1));
  EXPECT_FALSE(regions->IsRangeAvailable(base, 0x0001000000000000));

  END_TEST;
}

// Helper class for writing tests against the pausable VmAddressRegionEnumerator
template <VmAddressRegionEnumeratorType Type>
class EnumeratorTestHelper {
 public:
  EnumeratorTestHelper() = default;
  ~EnumeratorTestHelper() { Destroy(); }
  zx_status_t Init(fbl::RefPtr<VmAspace> aspace) TA_EXCL(lock()) {
    Destroy();
    zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0u, GB, &vmo_);
    if (status != ZX_OK) {
      return status;
    }

    status = aspace->RootVmar()->CreateSubVmar(
        0, GB, 0, VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ, "test vmar", &test_vmar_);
    if (status != ZX_OK) {
      return status;
    }
    return ZX_OK;
  }

  struct ChildRegion {
    bool mapping;
    size_t page_offset_begin;
    size_t page_offset_end;
  };
  zx_status_t AddRegions(ktl::initializer_list<ChildRegion>&& regions) TA_EXCL(lock()) {
    for (auto& region : regions) {
      ASSERT(region.page_offset_end > region.page_offset_begin);
      const size_t offset = region.page_offset_begin * kPageSize;
      const vaddr_t vaddr = test_vmar_->base() + offset;
      // See if there's a child VMAR that we should be making this in instead of our test root.
      fbl::RefPtr<VmAddressRegion> vmar = test_vmar_;
      auto next_region = [&]() {
        auto child_region = vmar->FindRegion(vaddr);
        if (child_region) {
          return child_region->as_vm_address_region();
        }
        return fbl::RefPtr<VmAddressRegion>();
      };
      while (auto next = next_region()) {
        vmar = next;
      }
      // Create either a mapping or vmar as requested.
      const size_t size = (region.page_offset_end - region.page_offset_begin) * kPageSize;
      const size_t relative_offset = vaddr - vmar->base();
      zx_status_t status;
      if (region.mapping) {
        auto new_mapping_result = vmar->CreateVmMapping(
            relative_offset, size, 0, VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_SPECIFIC, vmo_, 0,
            ARCH_MMU_FLAG_PERM_READ, "mapping");
        status = new_mapping_result.status_value();
      } else {
        fbl::RefPtr<VmAddressRegion> new_vmar;
        status = vmar->CreateSubVmar(
            relative_offset, size, 0,
            VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_SPECIFIC | VMAR_FLAG_CAN_MAP_SPECIFIC, "vmar",
            &new_vmar);
      }
      if (status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }
  using RegionEnumerator = VmAddressRegionEnumerator<Type, VmAddressRegion>;
  RegionEnumerator Enumerator(size_t page_offset_begin, size_t page_offset_end) TA_REQ(lock()) {
    const vaddr_t min_addr = test_vmar_->base() + page_offset_begin * kPageSize;
    const vaddr_t max_addr = test_vmar_->base() + page_offset_end * kPageSize;
    return VmAddressRegionEnumerator<Type, VmAddressRegion>(*test_vmar_, min_addr, max_addr);
  }

  void Resume(RegionEnumerator& enumerator) TA_REQ(lock()) {
    AssertHeld(enumerator.lock_ref());
    enumerator.resume();
  }

  bool ExpectRegions(RegionEnumerator& enumerator, ktl::initializer_list<ChildRegion>&& regions)
      TA_REQ(lock()) {
    AssertHeld(enumerator.lock_ref());
    for (auto& region : regions) {
      ASSERT(region.page_offset_end > region.page_offset_begin);
      auto next = enumerator.next();
      if (!next.has_value()) {
        return false;
      }
      AssertHeld(next->region_or_mapping->lock_ref());
      if (region.mapping != next->region_or_mapping->is_mapping()) {
        return false;
      }
      if (next->region_or_mapping->base() !=
          test_vmar_->base() + region.page_offset_begin * kPageSize) {
        return false;
      }
      if (next->region_or_mapping->size() !=
          (region.page_offset_end - region.page_offset_begin) * kPageSize) {
        return false;
      }
    }
    return true;
  }

  zx_status_t Unmap(size_t page_offset_begin, size_t page_offset_end) TA_EXCL(lock()) {
    ASSERT(page_offset_end > page_offset_begin);
    const vaddr_t vaddr = test_vmar_->base() + page_offset_begin * kPageSize;
    const size_t size = (page_offset_end - page_offset_begin) * kPageSize;
    // Attempt to unmap, walking down into child vmars if the unmap fails due to it causing a
    // subvmar to be partially unmapped.
    fbl::RefPtr<VmAddressRegion> vmar = test_vmar_;
    do {
      zx_status_t status = vmar->Unmap(vaddr, size, VmAddressRegionOpChildren::Yes);
      if (status != ZX_ERR_INVALID_ARGS) {
        return status;
      }
      fbl::RefPtr<VmAddressRegionOrMapping> next = vmar->FindRegion(vaddr);
      if (!next) {
        return status;
      }
      vmar = next->as_vm_address_region();
    } while (vmar);
    return ZX_ERR_NOT_FOUND;
  }

  Lock<CriticalMutex>* lock() const TA_RET_CAP(test_vmar_->lock()) { return test_vmar_->lock(); }

 private:
  void Destroy() {
    if (test_vmar_) {
      test_vmar_->Destroy();
      test_vmar_.reset();
    }
    vmo_.reset();
  }
  fbl::RefPtr<VmObjectPaged> vmo_;
  fbl::RefPtr<VmAddressRegion> test_vmar_;
};

static bool address_region_enumerator_test() {
  BEGIN_TEST;

  fbl::RefPtr<VmAspace> aspace = VmAspace::Create(VmAspace::Type::User, "test aspace");

  // Smoke test of a single region.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 0, 1}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 1);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 0, 1}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Unmap while iterating a subvmar and resume in the parent.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(
        test.AddRegions({{false, 0, 7}, {true, 1, 2}, {true, 3, 4}, {true, 5, 6}, {true, 7, 8}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 10);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{false, 0, 7}, {true, 1, 2}}));
    enumerator.pause();
    // Unmap the entire subvmar we created
    guard.CallUnlocked([&test] { test.Unmap(0, 7); });
    test.Resume(enumerator);
    // Last mapping should still be there.
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 7, 8}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Pause immediately without enumerating when the start is a subvmar.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{false, 0, 2}, {true, 1, 2}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 2);
    AssertHeld(enumerator.lock_ref());
    enumerator.pause();
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{false, 0, 2}, {true, 1, 2}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Add future mapping.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 0, 1}, {true, 1, 2}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 3);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 0, 1}}));
    enumerator.pause();
    guard.CallUnlocked([&test] { test.AddRegions({{true, 2, 3}}); });
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 1, 2}, {true, 2, 3}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Replace the next mapping.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 0, 1}, {true, 1, 2}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 3);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 0, 1}}));
    enumerator.pause();
    guard.CallUnlocked([&test] {
      test.Unmap(1, 2);
      test.AddRegions({{true, 1, 3}});
    });
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 1, 3}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Add earlier regions.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 2, 3}, {true, 3, 4}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 4);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 2, 3}}));
    enumerator.pause();
    guard.CallUnlocked([&test] { test.AddRegions({{true, 0, 1}, {true, 1, 32}}); });
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 3, 4}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Replace current.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 1, 2}, {true, 2, 3}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 3);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 1, 2}}));
    enumerator.pause();
    guard.CallUnlocked([&test] {
      test.Unmap(1, 2);
      test.AddRegions({{true, 0, 2}});
    });
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 2, 3}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Replace current and next with a single mapping.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{true, 1, 2}, {true, 2, 3}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 3);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 1, 2}}));
    enumerator.pause();
    guard.CallUnlocked([&test] {
      test.Unmap(1, 3);
      test.AddRegions({{true, 0, 3}});
    });
    test.Resume(enumerator);
    EXPECT_FALSE(test.ExpectRegions(enumerator, {{true, 0, 3}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Start enumerating part way into a mapping.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::MappingsOnly> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{false, 0, 6}, {true, 0, 2}, {true, 2, 4}, {true, 6, 7}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(3, 7);
    AssertHeld(enumerator.lock_ref());
    enumerator.pause();
    test.Resume(enumerator);
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 2, 4}, {true, 6, 7}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Delete depth that was just yielded
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{false, 0, 10}, {false, 0, 9}, {false, 0, 8}, {false, 0, 7}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 10);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{false, 0, 10}, {false, 0, 9}}));
    enumerator.pause();
    guard.CallUnlocked([&test] {
      ASSERT(test.Unmap(0, 9) == ZX_OK);
      ASSERT(test.AddRegions({{false, 0, 8}, {false, 0, 7}}) == ZX_OK);
    });
    test.Resume(enumerator);
    // Subtree was deleted and the new one will not be yielded.
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Delete next depth to be yielded
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::VmarsAndMappings> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{false, 0, 10}, {false, 0, 9}, {false, 0, 8}, {false, 0, 7}}));
    Guard<CriticalMutex> guard{test.lock()};
    auto enumerator = test.Enumerator(0, 10);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{false, 0, 10}, {false, 0, 9}}));
    enumerator.pause();
    guard.CallUnlocked([&test] {
      ASSERT(test.Unmap(0, 8) == ZX_OK);
      ASSERT(test.AddRegions({{false, 0, 7}}) == ZX_OK);
    });
    test.Resume(enumerator);
    // Subtree was deleted and the new one will not be yielded.
    EXPECT_FALSE(enumerator.next().has_value());
  }
  // Nested mapping enumeration.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::MappingsOnly> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(test.AddRegions({{false, 0, 1000},
                               {false, 0, 100},
                               {false, 100, 200},
                               {false, 200, 300},
                               {true, 10, 20},
                               {true, 110, 120},
                               {true, 210, 220}}));
    Guard<CriticalMutex> guard{test.lock()};
    // Attempt to query for each mapping using its exact location.
    for (const auto& probe : {10u, 110u, 210u}) {
      auto enumerator = test.Enumerator(probe, probe + 5);
      AssertHeld(enumerator.lock_ref());
      // The enumerator will accurately yield only this mapping.
      // Furthermore, because mappings and VMARs are sorted, the enumerator naturally
      // short-circuits as soon as `itr_->base() >= max_addr_` without iterating the rest of the
      // tree.
      EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, probe, probe + 10}}));
      EXPECT_FALSE(enumerator.next().has_value());
    }
  }
  // Sub-VMAR entirely below min_addr_ is skipped.
  {
    EnumeratorTestHelper<VmAddressRegionEnumeratorType::MappingsOnly> test;
    ASSERT_OK(test.Init(aspace));
    EXPECT_OK(
        test.AddRegions({{false, 0, 100}, {true, 10, 20}, {false, 100, 200}, {true, 110, 120}}));
    Guard<CriticalMutex> guard{test.lock()};
    // Querying starting at 105. This should skip descending into the first sub-VMAR (0..100)
    // because all its children are < min_addr_, and correctly fall through to traverse the next
    // sibling VMAR.
    auto enumerator = test.Enumerator(105, 120);
    AssertHeld(enumerator.lock_ref());
    EXPECT_TRUE(test.ExpectRegions(enumerator, {{true, 110, 120}}));
    EXPECT_FALSE(enumerator.next().has_value());
  }

  EXPECT_OK(aspace->Destroy());

  END_TEST;
}

// Doesn't do anything, just prints all aspaces.
// Should be run after all other tests so that people can manually comb
// through the output for leaked test aspaces.
static bool dump_all_aspaces() {
  BEGIN_TEST;

  // Remove for debugging.
  END_TEST;

  unittest_printf("verify there are no test aspaces left around\n");
  VmAspace::DumpAllAspaces(/*verbose*/ true);
  END_TEST;
}

// Check if a range of addresses is accessible to the user. If `spectre_validation` is true, this is
// done by checking if `validate_user_accessible_range` returns {0,0}. Otherwise, check using
// `is_user_accessible_range`.
static bool check_user_accessible_range(vaddr_t vaddr, size_t len, bool spectre_validation) {
  if (spectre_validation) {
    // If the address and length were not modified, then the pair is valid.
    vaddr_t old_vaddr = vaddr;
    size_t old_len = len;
    internal::validate_user_accessible_range(&vaddr, &len);
    return vaddr == old_vaddr && len == old_len;
  }

  return is_user_accessible_range(vaddr, len);
}

static bool check_user_accessible_range_test(bool spectre_validation) {
  BEGIN_TEST;
  vaddr_t va;
  size_t len;

  // Test address of zero.
  va = 0;
  len = kPageSize;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test address and length of zero (both are valid).
  va = 0;
  len = 0;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test very end of address space and zero length (this is invalid since the start has bit 55 set
  // despite zero length).
  va = ktl::numeric_limits<uint64_t>::max();
  len = 0;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test a regular user address.
  va = USER_ASPACE_BASE;
  len = kPageSize;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test zero-length on a regular user address.
  va = USER_ASPACE_BASE;
  len = 0;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test overflow past 64 bits.
  va = USER_ASPACE_BASE;
  len = ktl::numeric_limits<uint64_t>::max() - va + 1;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

#if defined(__aarch64__)

  // On aarch64, an address is accessible to the user if bit 55 is zero.

  // Test starting on a bad user address.
  constexpr vaddr_t kBadAddrMask = UINT64_C(1) << 55;
  va = kBadAddrMask | USER_ASPACE_BASE;
  len = kPageSize;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test zero-length on a bad user address.
  va = kBadAddrMask | USER_ASPACE_BASE;
  len = 0;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test 2^55 is in the range of [va, va+len), ending on a bad user address.
  va = USER_ASPACE_BASE;
  len = kBadAddrMask;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test this returns false if any address within the range of [va, va+len)
  // contains a value where bit 55 is set. This also implies there are many
  // gaps in ranges above 2^56.
  //
  // Here both the start and end values are valid, but this range contains an
  // address that is invalid.
  va = 0;
  len = 0x17f'ffff'ffff'ffff;  // Bits 0-56 (except 55) are set.
  ASSERT_TRUE(is_user_accessible(va));
  ASSERT_TRUE(is_user_accessible(va + len));
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test the range of the largest value less than 2^55 and the smallest value
  // greater than 2^55 where bit 55 == 0.
  va = (UINT64_C(1) << 55) - 1;
  len = 0x80'0000'0000'0001;  // End = va + len = 2^56.
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Be careful not to just check that 2^55 is in the range. We really want to
  // check whenever bit 55 is flipped in the range.
  va = 0x17f'ffff'ffff'ffff;  // Start above 2^56. Bit 55 is not set.
  len = 0x80'0000'0000'0001;  // End = va + len = 0x200'0000'0000'0000. This is above 2^56 and bit
                              // 55 also is not set.
  ASSERT_TRUE(is_user_accessible(va));
  ASSERT_TRUE(is_user_accessible(va + len));
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  va = USER_ASPACE_BASE;
  len = (UINT64_C(1) << 57) + 1;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test a range above 2^56 where bit 55 is never set.
  va = 0x170'0000'0000'0000;
  len = 0xf'ffff'ffff'ffff;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test a range right below 2^55 where bit 55 is never set.
  va = 0x70'0000'0000'0000;
  len = 0xf'ffff'ffff'ffff;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

  // Test the last valid user space address with a tag of 0.
  va = ktl::numeric_limits<uint64_t>::max();
  va &= ~(UINT64_C(0xFF) << 56);  // Set tag to zero.
  va &= ~kBadAddrMask;            // Ensure valid user address.
  len = 0;
  EXPECT_TRUE(check_user_accessible_range(va, len, spectre_validation));

#elif defined(__x86_64__)

  // On x86_64, an address is accessible to the user if bits 48-63 are zero.

  // Test a bad user address.
  constexpr vaddr_t kBadAddrMask = UINT64_C(1) << 48;
  va = kBadAddrMask | USER_ASPACE_BASE;
  len = kPageSize;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test zero-length on a bad user address.
  va = kBadAddrMask | USER_ASPACE_BASE;
  len = 0;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

  // Test ending on a bad user address.
  va = USER_ASPACE_BASE;
  len = kBadAddrMask;
  EXPECT_FALSE(check_user_accessible_range(va, len, spectre_validation));

#endif

  END_TEST;
}

static bool arch_is_user_accessible_range() { return check_user_accessible_range_test(false); }

static bool validate_user_address_range() { return check_user_accessible_range_test(true); }

UNITTEST_START_TESTCASE(aspace_tests)
VM_UNITTEST(vmm_alloc_contiguous_smoke_test)
VM_UNITTEST(multiple_regions_test)
VM_UNITTEST(vmm_alloc_contiguous_missing_flag_commit_fails)
VM_UNITTEST(vmm_alloc_contiguous_zero_size_fails)
VM_UNITTEST(vmaspace_create_smoke_test)
VM_UNITTEST(vmaspace_create_invalid_ranges)
VM_UNITTEST(vmaspace_alloc_smoke_test)
VM_UNITTEST(vmaspace_accessed_test_untagged)
#if defined(__aarch64__)
VM_UNITTEST(vmaspace_accessed_test_tagged)
#endif
VM_UNITTEST(vmaspace_unified_accessed_test)
VM_UNITTEST(vmaspace_usercopy_accessed_fault_test)
VM_UNITTEST(vmaspace_free_unaccessed_page_tables_test)
VM_UNITTEST(vmaspace_merge_mapping_test)
VM_UNITTEST(vmaspace_priority_propagation_test)
VM_UNITTEST(vmaspace_priority_unmap_test)
VM_UNITTEST(vmaspace_priority_mapping_overwrite_test)
VM_UNITTEST(vmaspace_priority_merged_mapping_test)
VM_UNITTEST(vmaspace_priority_bidir_clone_test)
VM_UNITTEST(vmaspace_priority_slice_test)
VM_UNITTEST(vmaspace_priority_pager_test)
VM_UNITTEST(vmaspace_priority_reference_test)
VM_UNITTEST(vmaspace_nested_attribution_test)
VM_UNITTEST(vm_mapping_attribution_commit_decommit_test)
VM_UNITTEST(vm_mapping_attribution_map_unmap_test)
VM_UNITTEST(vm_mapping_attribution_merge_test)
VM_UNITTEST(vm_mapping_sparse_mapping_test)
VM_UNITTEST(vm_mapping_page_fault_optimisation_test)
VM_UNITTEST(vm_mapping_page_fault_optimization_pt_limit_test)
VM_UNITTEST(vm_mapping_page_fault_range_test)
VM_UNITTEST(vm_mapping_force_writeable_high_priority)
VM_UNITTEST(arch_is_user_accessible_range)
VM_UNITTEST(validate_user_address_range)
VM_UNITTEST(arch_noncontiguous_map)
VM_UNITTEST(arch_noncontiguous_map_with_upgrade)
VM_UNITTEST(arch_vm_aspace_protect_split_pages)
VM_UNITTEST(arch_vm_aspace_protect_split_pages_out_of_memory)
VM_UNITTEST(vm_kernel_region_test)
VM_UNITTEST(region_list_get_alloc_spot_test)
VM_UNITTEST(region_list_get_alloc_spot_no_memory_test)
VM_UNITTEST(region_list_find_region_test)
VM_UNITTEST(region_list_include_or_higher_test)
VM_UNITTEST(region_list_upper_bound_test)
VM_UNITTEST(region_list_is_range_available_test)
VM_UNITTEST(address_region_enumerator_test)
VM_UNITTEST(dump_all_aspaces)  // Run last
UNITTEST_END_TESTCASE(aspace_tests, "aspace", "VmAspace / ArchVmAspace / VMAR tests")

}  // namespace vm_unittest
