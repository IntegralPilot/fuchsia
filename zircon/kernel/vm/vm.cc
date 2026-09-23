// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/vm.h"

#include <align.h>
#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/boot-options/boot-options.h>
#include <lib/cmpctmalloc.h>
#include <lib/console.h>
#include <lib/crypto/global_prng.h>
#include <lib/instrumentation/asan.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/zircon-internal/macros.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/vector.h>
#include <kernel/thread.h>
#include <ktl/array.h>
#include <phys/handoff.h>
#include <vm/init.h>
#include <vm/physmap.h>
#include <vm/pmm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object_paged.h>

#include "vm_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

// boot time allocated page full of zeros
vm_page_t* zero_page;
paddr_t zero_page_paddr;

// This exact symbol name is referenced by scripts/vmzircon-gdb.py.  It will be
// relocated by physboot before the kernel starts.
[[gnu::used]] extern auto* const kernel_relocated_base = __executable_start;

namespace {

// The initialized VMARs described in the phys hand-off.
//
// TODO(mcgrathr): Consider moving these to a stack- or heap-allocated object.
fbl::Vector<fbl::RefPtr<VmAddressRegion>> handoff_vmars;
fbl::RefPtr<VmAddressRegion> temporary_handoff_vmar;

// Declare storage for the kernel's heap VMAR. Maybe unused if virtual heap is disabled.
lazy_init::LazyInit<VmAddressRegion> kernel_heap_vmar;
// Whether the virtual heap is enabled. Set during vm_init_preheap() and never changed.
bool using_virtual_heap = false;

constexpr uint32_t ToVmarFlags(PhysMapping::Permissions perms) {
  uint32_t flags = VMAR_FLAG_SPECIFIC | VMAR_FLAG_CAN_MAP_SPECIFIC;
  if (perms.readable()) {
    flags |= VMAR_FLAG_CAN_MAP_READ;
  }
  if (perms.writable()) {
    flags |= VMAR_FLAG_CAN_MAP_WRITE;
  }
  if (perms.executable()) {
    flags |= VMAR_FLAG_CAN_MAP_EXECUTE;
  }
  return flags;
}

constexpr arch_mmu_flags_t ToArchMmuFlags(PhysMapping::Permissions perms, PhysMapping::Type type) {
  arch_mmu_flags_t flags = 0;
  switch (type) {
    case PhysMapping::Type::kNormal:
      flags |= ARCH_MMU_FLAG_CACHED;
      break;
    case PhysMapping::Type::kMmio:
      flags |= ARCH_MMU_FLAG_UNCACHED_DEVICE;
  }
  if (perms.readable()) {
    flags |= ARCH_MMU_FLAG_PERM_READ;
  }
  if (perms.writable()) {
    flags |= ARCH_MMU_FLAG_PERM_WRITE;
  }
  if (perms.executable()) {
    flags |= ARCH_MMU_FLAG_PERM_EXECUTE;
  }
  return flags;
}

void RegisterMappings(ktl::span<const PhysMapping> mappings, fbl::RefPtr<VmAddressRegion> vmar) {
  for (const PhysMapping& mapping : mappings) {
    zx_status_t status = vmar->ReserveSpace(
        mapping.name.data(), mapping.vaddr, mapping.size,
        static_cast<arch_mmu_flags_t>(ToArchMmuFlags(mapping.perms, mapping.type)));
    ASSERT(status == ZX_OK);

#if __has_feature(address_sanitizer)
    if (mapping.kasan_shadow) {
      asan_map_shadow_for(mapping.vaddr, mapping.size);
    }
#endif  // __has_feature(address_sanitizer)
  }
}

fbl::RefPtr<VmAddressRegion> RegisterVmar(const PhysVmar& phys_vmar) {
  fbl::RefPtr<VmAddressRegion> root_vmar = VmAspace::kernel_aspace()->RootVmar();

  phys_vmar.Log("VM");
  fbl::RefPtr<VmAddressRegion> vmar;
  zx_status_t status =
      root_vmar->CreateSubVmar(phys_vmar.base - root_vmar->base(), phys_vmar.size, 0,
                               ToVmarFlags(phys_vmar.permissions()), phys_vmar.name.data(), &vmar);
  ASSERT(status == ZX_OK);
  RegisterMappings(phys_vmar.mappings.get(), vmar);

  return vmar;
}

}  // namespace

bool vm_using_virtual_heap() { return using_virtual_heap; }

// Request the heap dimensions.
vaddr_t vm_get_kernel_heap_base() {
  ASSERT(vm_using_virtual_heap());
  return kernel_heap_vmar->base();
}

size_t vm_get_kernel_heap_size() {
  ASSERT(vm_using_virtual_heap());
  return kernel_heap_vmar->size();
}

void vm_init_preheap() {
  // Initialize VMM data structures.
  VmAspace::KernelAspaceInitPreHeap();

  fbl::RefPtr<VmAddressRegion> root_vmar = VmAspace::kernel_aspace()->RootVmar();

  // Hold the vmar in a temporary refptr until we can activate it. Activating it will cause the
  // address space to acquire a refptr allowing us to then safely drop our ref without triggering
  // the object to get destroyed.
  fbl::RefPtr<VmAddressRegion> vmar;

  if (gPhysHandoff->heap_vmar.has_value()) {
    using_virtual_heap = true;
    gPhysHandoff->heap_vmar->Log("VM");
    // Reserve the range for the heap.
    const vaddr_t kernel_heap_base = gPhysHandoff->heap_vmar->base;
    const size_t heap_bytes = gPhysHandoff->heap_vmar->size;
    // The heap has nothing to initialize later and we can create this from the beginning with only
    // read and write and no execute.
    vmar = fbl::AdoptRef<VmAddressRegion>(&kernel_heap_vmar.Initialize(
        *root_vmar, kernel_heap_base, heap_bytes,
        VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
        "kernel heap"));
    {
      Guard<CriticalMutex> region_guard(kernel_heap_vmar->region_lock());
      Guard<CriticalMutex> guard(kernel_heap_vmar->lock());
      zx_status_t status = kernel_heap_vmar->Activate();
      ASSERT(status == ZX_OK);
    }
  }
}

// Global so that it can be friended by VmAddressRegion.
void vm_init() {
  LTRACE_ENTRY;

  // grab a page and mark it as the zero page
  zx_status_t status = pmm_alloc_page(0, &zero_page, &zero_page_paddr);
  DEBUG_ASSERT(status == ZX_OK);

  // consider the zero page a wired page part of the kernel.
  zero_page->set_state(vm_page_state::WIRED);

  void* ptr = paddr_to_physmap(zero_page_paddr);
  DEBUG_ASSERT(ptr);

  arch_zero_page(ptr);

  // Register the permanent and temporary hand-off VMARs.
  fbl::AllocChecker ac;
  handoff_vmars.reserve(gPhysHandoff->vmars.size(), &ac);
  ASSERT(ac.check());
  for (const PhysVmar& phys_vmar : gPhysHandoff->vmars.get()) {
    fbl::RefPtr<VmAddressRegion> vmar = RegisterVmar(phys_vmar);
    handoff_vmars.push_back(ktl::move(vmar), &ac);
    ASSERT(ac.check());
  }
  temporary_handoff_vmar = RegisterVmar(*gPhysHandoff->temporary_vmar.get());

  cmpct_set_fill_on_alloc_threshold(BootOptions::Get()->heap_alloc_fill_threshold);
}

void vm_end_handoff() {
  DEBUG_ASSERT(temporary_handoff_vmar);
  temporary_handoff_vmar = nullptr;
}

paddr_t vaddr_to_paddr(const void* va) {
  if (is_physmap_addr(va)) {
    return physmap_to_paddr(va);
  }

  // It doesn't make sense to be calling this on a non-kernel address, since we would otherwise be
  // querying some 'random' active user address space, which is unlikely to be what the caller
  // wants.
  if (!is_kernel_address(reinterpret_cast<vaddr_t>(va))) {
    return 0;
  }

  paddr_t pa;
  zx_status_t rc =
      VmAspace::kernel_aspace()->arch_aspace().Query(reinterpret_cast<vaddr_t>(va), &pa, nullptr);
  if (rc != ZX_OK) {
    return 0;
  }

  return pa;
}

static int cmd_vm(int argc, const cmd_args* argv, uint32_t) {
  if (argc < 2) {
  notenoughargs:
    printf("not enough arguments\n");
  usage:
    printf("usage:\n");
    printf("%s phys2virt <address>\n", argv[0].str);
    printf("%s virt2phys <address>\n", argv[0].str);
    printf("%s map <phys> <virt> <count> <flags>\n", argv[0].str);
    printf("%s unmap <virt> <count>\n", argv[0].str);
    return ZX_ERR_INTERNAL;
  }

  if (!strcmp(argv[1].str, "phys2virt")) {
    if (argc < 3) {
      goto notenoughargs;
    }

    if (!is_physmap_phys_addr(argv[2].u)) {
      printf("address isn't in physmap\n");
      return -1;
    }

    void* ptr = paddr_to_physmap((paddr_t)argv[2].u);
    printf("paddr_to_physmap returns %p\n", ptr);
  } else if (!strcmp(argv[1].str, "virt2phys")) {
    if (argc < 3) {
      goto notenoughargs;
    }

    if (!is_kernel_address(reinterpret_cast<vaddr_t>(argv[2].u))) {
      printf("ERROR: outside of kernel address space\n");
      return -1;
    }

    paddr_t pa;
    arch_mmu_flags_t mmu_flags;
    zx_status_t err = VmAspace::kernel_aspace()->arch_aspace().Query(argv[2].u, &pa, &mmu_flags);
    printf("arch_mmu_query returns %d\n", err);
    if (err >= 0) {
      printf("\tpa %#" PRIxPTR ", flags %#x\n", pa, mmu_flags);
    }
  } else if (!strcmp(argv[1].str, "map")) {
    if (argc < 6) {
      goto notenoughargs;
    }

    if (!is_kernel_address(reinterpret_cast<vaddr_t>(argv[3].u))) {
      printf("ERROR: outside of kernel address space\n");
      return -1;
    }

    auto err = VmAspace::kernel_aspace()->arch_aspace().MapContiguous(
        argv[3].u, argv[2].u, (uint)argv[4].u, static_cast<arch_mmu_flags_t>(argv[5].u));
    printf("arch_mmu_map returns %d\n", err);
  } else if (!strcmp(argv[1].str, "unmap")) {
    if (argc < 4) {
      goto notenoughargs;
    }

    if (!is_kernel_address(reinterpret_cast<vaddr_t>(argv[2].u))) {
      printf("ERROR: outside of kernel address space\n");
      return -1;
    }

    // Strictly only attempt to unmap exactly what the user requested, they can deal with any
    // failure that might result.
    auto err = VmAspace::kernel_aspace()->arch_aspace().Unmap(
        argv[2].u, (uint)argv[3].u, ArchVmAspaceInterface::ArchUnmapOptions::None);
    printf("arch_mmu_unmap returns %d\n", err);
  } else {
    printf("unknown command\n");
    goto usage;
  }

  return ZX_OK;
}

STATIC_COMMAND_START
STATIC_COMMAND("vm", "vm commands", &cmd_vm)
STATIC_COMMAND_END(vm)

extern "C" {
paddr_t cpp_vaddr_to_paddr(const void* va);
paddr_t cpp_vaddr_to_paddr(const void* va) { return vaddr_to_paddr(va); }
}
