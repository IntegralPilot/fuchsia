// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <debug.h>
#include <lib/arch/asm.h>
#include <lib/boot-options/boot-options.h>
#include <lib/debuglog.h>
#include <lib/fit/defer.h>
#include <lib/instrumentation/asan.h>
#include <lib/page/size.h>
#include <lib/power-management/energy-model.h>
#include <lib/power-management/pdev-power-level-controller.h>
#include <lib/relaxed_atomic.h>
#include <lib/syscalls/forward.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/checking.h>
#include <lib/zbitl/view.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <string.h>
#include <sys/types.h>
#include <trace.h>
#include <zircon/boot/crash-reason.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/resource.h>
#include <zircon/syscalls/system.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>

#include <arch/arch_ops.h>
#include <arch/mp.h>
#include <arch/ops.h>
#include <dev/hw_watchdog.h>
#include <dev/interrupt.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/cpu.h>
#include <kernel/idle_power_thread.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/percpu.h>
#include <kernel/range_check.h>
#include <kernel/scheduler.h>
#include <kernel/scheduler_inline.h>
#include <kernel/thread.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/unique_ptr.h>
#include <object/event_dispatcher.h>
#include <object/job_dispatcher.h>
#include <object/port_dispatcher.h>
#include <object/process_dispatcher.h>
#include <object/user_handles.h>
#include <object/vm_object_dispatcher.h>
#include <phys/handoff.h>
#include <platform/halt_helper.h>
#include <platform/halt_token.h>
#include <platform/mexec.h>
#include <platform/timer.h>
#include <vm/handoff-end.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>

#include "system_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

// Allocate this many extra bytes at the end of the bootdata for the platform
// to fill in with platform specific boot structures.
const size_t kBootdataPlatformExtraBytes = kPageSize * 4;

extern "C" arch::AsmLabel mexec_asm, mexec_asm_end;

class IdentityPageAllocator {
 public:
  explicit IdentityPageAllocator(uintptr_t alloc_start) : alloc_start_(alloc_start) {}
  ~IdentityPageAllocator() { pmm_free(&allocated_); }

  /* Allocates a page of memory that has the same physical and virtual
  addresses. */
  zx_status_t Allocate(void** result);

  // Activate the 1:1 address space. P
  void Activate();

 private:
  zx_status_t InitializeAspace();
  fbl::RefPtr<VmAspace> aspace_ = nullptr;
  size_t mapping_id_ = 0;
  // Minimum physical/virtual address for all allocations.
  uintptr_t alloc_start_;
  VmPageDoublyLinkedList allocated_;
};

zx_status_t IdentityPageAllocator::InitializeAspace() {
  // The Aspace has already been initialized, nothing to do.
  if (aspace_) {
    return ZX_OK;
  }

  aspace_ = VmAspace::Create(VmAspace::Type::LowKernel, "identity");
  if (!aspace_) {
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t alloc_pages_greater_than(paddr_t lower_bound, size_t count, size_t limit,
                                     paddr_t* paddrs) {
  VmPageDoublyLinkedList list;

  // We don't support partially completed requests. This function will either
  // allocate |count| pages or 0 pages. If we complete a partial allocation
  // but are unable to fulfil the complete request, we'll clean up any pages
  // that we may have allocated in the process.
  auto pmm_cleanup = fit::defer([&list]() { pmm_free(&list); });

  while (count) {
    // TODO: replace with pmm routine that can allocate while excluding a range.
    size_t actual = 0;
    VmPageDoublyLinkedList alloc_list;
    zx_status_t status = pmm_alloc_range(lower_bound, count, &alloc_list);
    if (status == ZX_OK) {
      actual = count;
      list.splice(list.end(), alloc_list);
    }

    for (size_t i = 0; i < actual; i++) {
      paddrs[count - (i + 1)] = lower_bound + kPageSize * i;
    }

    count -= actual;
    lower_bound += kPageSize * (actual + 1);

    // If we're past the limit and still trying to allocate, just give up.
    if (lower_bound >= limit) {
      return ZX_ERR_NO_RESOURCES;
    }
  }

  // mark all of the pages we allocated as WIRED.
  for (auto& p : list) {
    p.set_state(vm_page_state::WIRED);
  }

  // Make sure we don't free the pages we just allocated.
  pmm_cleanup.cancel();
  list.clear();

  return ZX_OK;
}

zx_status_t IdentityPageAllocator::Allocate(void** result) {
  zx_status_t st;

  // Start by obtaining an unused physical page. This address will eventually
  // be the physical/virtual address of our identity mapped page.
  // TODO: when https://fxbug.dev/42105842 is completed, we should allocate low memory directly
  //       from the pmm rather than using "alloc_pages_greater_than" which is
  //       somewhat of a hack.
  paddr_t pa;
  DEBUG_ASSERT(alloc_start_ < 4 * GB);
  st = alloc_pages_greater_than(alloc_start_, 1, 4 * GB - alloc_start_, &pa);
  if (st != ZX_OK) {
    LTRACEF("mexec: failed to allocate page in low memory\n");
    return st;
  }

  // Add this page to the list of allocated pages such that it gets freed when
  // the object is destroyed.
  vm_page_t* page = paddr_to_vm_page(pa);
  DEBUG_ASSERT(page);
  allocated_.push_back(page);

  // The kernel address space may be in high memory which cannot be identity
  // mapped since all Kernel Virtual Addresses might be out of range of the
  // physical address space. For this reason, we need to make a new address
  // space.
  st = InitializeAspace();
  if (st != ZX_OK) {
    return st;
  }

  // Create a new allocation in the new address space that identity maps the
  // target page.
  constexpr uint kPermissionFlagsRWX =
      (ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_PERM_EXECUTE);

  void* addr = reinterpret_cast<void*>(pa);

  // 2 ** 64 = 18446744073709551616
  // len("identity 18446744073709551616\n") == 30, round to sizeof(word) = 32
  char mapping_name[32];
  snprintf(mapping_name, sizeof(mapping_name), "identity %lu", mapping_id_++);

  st = aspace_->AllocPhysical(mapping_name, kPageSize, &addr, 0, pa,
                              VmAspace::VMM_FLAG_VALLOC_SPECIFIC, kPermissionFlagsRWX);
  if (st != ZX_OK) {
    return st;
  }

  *result = addr;
  return st;
}

void IdentityPageAllocator::Activate() {
  if (!aspace_) {
    panic("Cannot Activate 1:1 Aspace with no 1:1 mappings!");
  }
  vmm_set_active_aspace(aspace_.get());
}

/* Takes all the pages in a VMO and creates a copy of them where all the pages
 * occupy a physically contiguous region of physical memory.
 * TODO(gkalsi): Don't coalesce pages into a physically contiguous region and
 *               just pass a vectored I/O list to the mexec assembly.
 */
static zx_status_t vmo_coalesce_pages(zx_handle_t vmo_hdl, const size_t extra_bytes, paddr_t* addr,
                                      uint8_t** vaddr, size_t* size,
                                      VmPageDoublyLinkedList* allocated_pages) {
  DEBUG_ASSERT(addr);
  if (!addr) {
    return ZX_ERR_INVALID_ARGS;
  }

  DEBUG_ASSERT(size);
  if (!size) {
    return ZX_ERR_INVALID_ARGS;
  }

  DEBUG_ASSERT(allocated_pages);
  if (!allocated_pages) {
    return ZX_ERR_INVALID_ARGS;
  }

  ProcessDispatcher* up = ProcessDispatcher::GetCurrent();
  fbl::RefPtr<VmObjectDispatcher> vmo_dispatcher;
  zx_status_t st =
      up->handle_table().GetDispatcherWithRights(*up, vmo_hdl, ZX_RIGHT_READ, &vmo_dispatcher);
  if (st != ZX_OK)
    return st;

  fbl::RefPtr<VmObject> vmo = vmo_dispatcher->vmo();

  const size_t vmo_size = vmo->size();

  if (vmo_size == 0) {
    // Zero sized data, for either kernel or bootimage does not make sense and for simplicity of the
    // rest of the logic here we consider this case an error.
    return ZX_ERR_BAD_STATE;
  }

  const size_t num_pages = RoundUpPageSize(vmo_size + extra_bytes) / kPageSize;

  paddr_t base_addr;
  st = Pmm::Node().AllocContiguous(num_pages, PMM_ALLOC_FLAG_ANY, 0, &base_addr, allocated_pages);
  if (st != ZX_OK) {
    return st;
  }

  uint8_t* dst_addr = static_cast<uint8_t*>(paddr_to_physmap(base_addr));

  st = vmo->Read(dst_addr, 0, vmo_size);
  if (st != ZX_OK) {
    Pmm::Node().FreeList(allocated_pages);
    return st;
  }

  arch_clean_invalidate_cache_range((vaddr_t)dst_addr, vmo_size);

  *size = num_pages * kPageSize;
  *addr = base_addr;
  if (vaddr)
    *vaddr = dst_addr;

  return ZX_OK;
}

NO_ASAN zx_status_t system_mexec_core(zx_handle_t resource, zx_handle_t kernel_vmo,
                                      zx_handle_t data_zbi_vmo) {
  paddr_t new_kernel_addr;
  size_t new_kernel_len;
  VmPageDoublyLinkedList kernel_pages;
  zx_status_t result =
      vmo_coalesce_pages(kernel_vmo, 0, &new_kernel_addr, nullptr, &new_kernel_len, &kernel_pages);
  if (result != ZX_OK) {
    return result;
  }
  auto cleanup_kernel = fit::defer([&kernel_pages]() { Pmm::Node().FreeList(&kernel_pages); });

  paddr_t new_kernel_entry;
  {
    ktl::span zbi_span{reinterpret_cast<const ktl::byte*>(paddr_to_physmap(new_kernel_addr)),
                       new_kernel_len};
    zbitl::View zbi{zbi_span};

    if (zbitl::CheckBootable(zbi).is_error()) {
      return ZX_ERR_IO_DATA_INTEGRITY;
    }
    const zbi_kernel_t* kernel = reinterpret_cast<const zbi_kernel_t*>(zbi.begin()->payload.data());
    new_kernel_entry = KernelPhysicalLoadAddress() + kernel->entry;
    ZX_ASSERT(zbi.take_error().is_ok());
  }

  paddr_t new_data_zbi_addr;
  uint8_t* data_zbi_buffer;
  size_t data_zbi_len;
  VmPageDoublyLinkedList data_zbi_pages;
  result = vmo_coalesce_pages(data_zbi_vmo, kBootdataPlatformExtraBytes, &new_data_zbi_addr,
                              &data_zbi_buffer, &data_zbi_len, &data_zbi_pages);
  if (result != ZX_OK) {
    return result;
  }
  auto cleanup_data_zbi =
      fit::defer([&data_zbi_pages]() { Pmm::Node().FreeList(&data_zbi_pages); });

  uintptr_t kernel_image_end = KernelPhysicalLoadAddress() + new_kernel_len;

  paddr_t final_data_zbi_addr = new_data_zbi_addr;
  // For testing purposes, we may want the bootdata at a high address. Alternatively if our
  // coalesced VMO should overlap into the target kernel range then we also need to move it, and
  // placing it high is as good as anywhere else.
  if (BootOptions::Get()->mexec_force_high_ramdisk ||
      Intersects(final_data_zbi_addr, data_zbi_len, KernelPhysicalLoadAddress(),
                 kernel_image_end)) {
    const size_t page_count = (data_zbi_len / kPageSize) + 1;
    fbl::AllocChecker ac;
    ktl::unique_ptr<paddr_t[]> paddrs(new (&ac) paddr_t[page_count]);
    ASSERT(ac.check());

    // Allocate pages greater than 4GiB to test that we're tolerant of booting
    // with a ramdisk in high memory. This operation can be very expensive and
    // should be replaced with a PMM API that supports allocating from a
    // specific range of memory.
    result = alloc_pages_greater_than(4 * GB, page_count, 8 * GB, paddrs.get());
    ASSERT(result == ZX_OK);

    final_data_zbi_addr = paddrs.get()[0];
  }

  IdentityPageAllocator id_alloc(kernel_image_end);
  void* id_page_addr = 0x0;
  result = id_alloc.Allocate(&id_page_addr);
  if (result != ZX_OK) {
    return result;
  }

  LTRACEF("zx_system_mexec allocated identity mapped page at %p\n", id_page_addr);

  Thread::Current::MigrateToCpu(BOOT_CPU_ID);

  // We assume that when the system starts, only one CPU is running. We denote
  // this as the boot CPU.
  // We want to make sure that this is the CPU that eventually branches into
  // the new kernel so we attempt to migrate this thread to that cpu.
  result = platform_halt_secondary_cpus(ZX_TIME_INFINITE);
  DEBUG_ASSERT(result == ZX_OK);

  platform_mexec_prep(final_data_zbi_addr, data_zbi_len);

  const zx_instant_mono_t dlog_deadline = current_mono_time() + ZX_SEC(5);
  dlog_shutdown(dlog_deadline);

  // Give the watchdog one last pet to hold it off until the new image has booted far enough to pet
  // the dog itself (or disable it).
  hw_watchdog_pet();

  // Beyond this point mexec handoff is irreversible. Disarm cleanup guards.
  cleanup_kernel.cancel();
  cleanup_data_zbi.cancel();

  arch_disable_ints();

  // WARNING
  // It is unsafe to return from this function beyond this point.
  // This is because we have swapped out the user address space and halted the
  // secondary cores and there is no trivial way to bring both of these back.
  id_alloc.Activate();

  // We're going to copy this into our identity page, make sure it's not
  // longer than a single page.
  size_t mexec_asm_length = arch::kAsmLabelSize<mexec_asm, mexec_asm_end>;
  DEBUG_ASSERT(mexec_asm_length <= kPageSize);

  __unsanitized_memcpy(id_page_addr, (const void*)mexec_asm, mexec_asm_length);
  arch_sync_cache_range((vaddr_t)id_page_addr, mexec_asm_length);

  // We must pass in an arg that represents a list of memory regions to
  // shuffle around. We put this args list immediately after the mexec
  // assembly.
  // Put the args list in a separate page.
  void* ops_ptr;
  result = id_alloc.Allocate(&ops_ptr);
  DEBUG_ASSERT(result == ZX_OK);
  memmov_ops_t* ops = (memmov_ops_t*)(ops_ptr);

  uint32_t ops_idx = 0;

  // Op to move the new kernel into place.
  ops[ops_idx].src = (void*)new_kernel_addr;
  ops[ops_idx].dst = (void*)KernelPhysicalLoadAddress();
  ops[ops_idx].len = new_kernel_len;
  ops_idx++;

  // We can leave the data ZBI in place unless we've been asked to move it to
  // high memory.
  if (new_data_zbi_addr != final_data_zbi_addr) {
    ops[ops_idx].src = (void*)new_data_zbi_addr;
    ops[ops_idx].dst = (void*)final_data_zbi_addr;
    ops[ops_idx].len = data_zbi_len;
    ops_idx++;
  }

  // Null terminated list.
  ops[ops_idx++] = {0, 0, 0};

  // Make sure that the kernel, when copied, will not overwrite the bootdata, our mexec code or
  // copy ops.
  DEBUG_ASSERT(!Intersects(reinterpret_cast<uintptr_t>(ops[0].dst), ops[0].len,
                           reinterpret_cast<uintptr_t>(final_data_zbi_addr), data_zbi_len));
  DEBUG_ASSERT(!Intersects(reinterpret_cast<uintptr_t>(ops[0].dst), ops[0].len,
                           reinterpret_cast<uintptr_t>(id_page_addr),
                           static_cast<size_t>(kPageSize)));
  DEBUG_ASSERT(!Intersects(reinterpret_cast<uintptr_t>(ops[0].dst), ops[0].len,
                           reinterpret_cast<uintptr_t>(ops_ptr), static_cast<size_t>(kPageSize)));

  // Sync because there is code in here that we intend to run.
  arch_sync_cache_range((vaddr_t)id_page_addr, kPageSize);

  // Clean because we're going to turn the MMU/caches off and we want to make
  // sure that things are still available afterwards.
  arch_clean_cache_range((vaddr_t)id_page_addr, kPageSize);
  arch_clean_cache_range((vaddr_t)ops_ptr, kPageSize);

  // Shutdown the timer and interrupts.  Performing shutdown of these components
  // is critical as we might be using a PV clock or PV EOI signaling so we must
  // tell our hypervisor to stop updating them to avoid corrupting aribtrary
  // memory post-mexec.
  platform_stop_timer();
  platform_shutdown_timer();
  shutdown_interrupts_curr_cpu();
  shutdown_interrupts();

  // Ask the platform to mexec into the next kernel.
  mexec_asm_func mexec_assembly = (mexec_asm_func)id_page_addr;
  platform_mexec(mexec_assembly, ktl::span{ops, ops_idx}, reinterpret_cast<uintptr_t>(ops[0].dst),
                 ops[0].len, new_kernel_entry, final_data_zbi_addr, data_zbi_len);

  panic("Execution should never reach here\n");
}
