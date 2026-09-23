// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/kstack.h"

#include <assert.h>
#include <inttypes.h>
#include <lib/counters.h>
#include <lib/fit/defer.h>
#include <lib/thread-stack/abi.h>
#include <stdio.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <ktl/utility.h>
#include <phys/zircon-abi-spec.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object_paged.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

namespace {

struct Stack : public ZirconAbiSpec::Stack {
  enum class Growth : bool {
    kUp,
    kDown,
  };

  constexpr Stack(const char* name, ZirconAbiSpec::Stack stack, Growth growth)
      : ZirconAbiSpec::Stack(stack), name(name), growth(growth) {}

  const char* name;
  Growth growth;
};

KCOUNTER(vm_kernel_stack_bytes, "vm.kstack.allocated_bytes")

constexpr Stack kSafe = {"kernel-safe-stack", kMachineStack, Stack::Growth::kDown};
#if __has_feature(safe_stack)
constexpr Stack kUnsafe = {"kernel-unsafe-stack", kUnsafeStack, Stack::Growth::kDown};
#endif
#if __has_feature(shadow_call_stack)
constexpr Stack kShadowCall = {"kernel-shadow-call-stack", kShadowCallStack, Stack::Growth::kUp};
#endif

// Choose a pattern for the stack canary that hopefully doesn't collide with other patterns that
// might get used on the stack. Explicitly avoid 0xAA, for example, as it is used for filling
// uninitialized stack variables.
constexpr unsigned char kStackCanary = 0xBB;

size_t stack_canary_offset(const Stack& stack) {
  const size_t off =
      stack.size_bytes / 100 * ktl::min(100ul, BootOptions::Get()->stack_canary_percent_free);
  return stack.growth == Stack::Growth::kDown ? off : stack.size_bytes - off - 1;
}

void stack_canary_write(const Stack& type, void* base) {
  static_cast<unsigned char*>(base)[stack_canary_offset(type)] = kStackCanary;
}

void stack_canary_check(const Stack& stack, void* base) {
  const size_t off = stack_canary_offset(stack);
  if (static_cast<unsigned char*>(base)[off] != kStackCanary) {
    // TODO(https://fxbug.dev/431001857): Upgrade this to an OOPS once we have either fixed root
    // cause or have more diagnostic data to log.
    printf("Canary at offset %zu in stack %s as base %p of size %#x was corrupted.\n", off,
           stack.name, base, stack.size_bytes);
  }
}

// Takes a portion of the VMO and maps a kernel stack with one page of padding before and after the
// mapping.
zx_status_t map(const Stack& stack, fbl::RefPtr<VmObjectPaged>& vmo, uint64_t* offset,
                KernelStack::Mapping* map) {
  LTRACEF("allocating %s\n", stack.name);

  // assert that this mapping hasn't already be created
  DEBUG_ASSERT(!map->vmar_);

  // get a handle to the root vmar
  auto vmar = VmAspace::kernel_aspace()->RootVmar()->as_vm_address_region();
  DEBUG_ASSERT(!!vmar);

  // create a vmar with enough padding for a page before and after the stack
  fbl::RefPtr<VmAddressRegion> kstack_vmar;
  zx_status_t status = vmar->CreateSubVmar(
      0, stack.lower_guard_size_bytes + stack.size_bytes + stack.upper_guard_size_bytes, 0,
      VMAR_FLAG_CAN_MAP_SPECIFIC | VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE, stack.name,
      &kstack_vmar);
  if (status != ZX_OK) {
    return status;
  }

  // destroy the vmar if we early abort
  // this will also clean up any mappings that may get placed on the vmar
  auto vmar_cleanup = fit::defer([&kstack_vmar]() { kstack_vmar->Destroy(); });

  LTRACEF("%s vmar at %#" PRIxPTR "\n", stack.name, kstack_vmar->base());

  // create a mapping offset kStackGuardRegionSize into the vmar we created
  zx::result<VmAddressRegion::MapResult> mapping_result = kstack_vmar->CreateVmMapping(
      stack.lower_guard_size_bytes, stack.size_bytes, 0, VMAR_FLAG_SPECIFIC, vmo, *offset,
      ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE, stack.name);
  if (mapping_result.is_error()) {
    return mapping_result.status_value();
  }

  LTRACEF("%s mapping at %#" PRIxPTR "\n", stack.name, mapping_result->base);

  // fault in all the pages so we dont demand fault in the stack
  status = mapping_result->mapping->MapRange(0, stack.size_bytes, true);
  if (status != ZX_OK) {
    return status;
  }

  stack_canary_write(stack, reinterpret_cast<void*>(mapping_result->base));

  vm_kernel_stack_bytes.Add(stack.size_bytes);

  // Cancel the cleanup handler on the vmar since we're about to save a
  // reference to it.
  vmar_cleanup.cancel();

  // save the relevant bits
  map->vmar_ = ktl::move(kstack_vmar);

  // Increase the offset to claim this portion of the VMO.
  *offset += stack.size_bytes;

  return ZX_OK;
}

zx_status_t unmap(const Stack& type, KernelStack::Mapping& map) {
  if (!map.vmar_) {
    return ZX_OK;
  }
  LTRACEF("removing vmar at at %#" PRIxPTR "\n", map.vmar_->base());

  stack_canary_check(type, reinterpret_cast<void*>(map.base()));

  zx_status_t status = map.vmar_->Destroy();
  if (status != ZX_OK) {
    return status;
  }
  map.vmar_.reset();
  vm_kernel_stack_bytes.Add(-static_cast<int64_t>(type.size_bytes));
  return ZX_OK;
}

}  // namespace

vaddr_t KernelStack::Mapping::base() const {
  // The actual stack mapping starts after the padding.
  return vmar_ ? vmar_->base() + internal::kStackGuardRegionSize : 0;
}

size_t KernelStack::Mapping::size() const {
  // Remove the padding from the vmar to get the actual stack size.
  return vmar_ ? vmar_->size() - internal::kStackGuardRegionSize * 2 : 0;
}

zx_status_t KernelStack::Init() {
  // Determine the total VMO size we needed for all stacks.
  size_t vmo_size = kSafe.size_bytes;
#if __has_feature(safe_stack)
  vmo_size += kUnsafe.size_bytes;
#endif

#if __has_feature(shadow_call_stack)
  vmo_size += kShadowCall.size_bytes;
#endif

  // Create a VMO for our stacks. Although multiple stacks will be allocated from adjacent blocks of
  // the VMO, they are only referenced by their virtual mapping addresses, and so there is no
  // possibility of over or under run of any stack trampling an adjacent one, they will just fault
  // on the guard regions around the mappings. Similarly the mapping location of each stack is
  // randomized independently, so allocating out of the same VMO provides no correlation to the
  // mapped addresses.
  // Using a single VMO reduces bookkeeping memory overhead with no downside, since all stacks have
  // exactly the same lifetime.
  // TODO(https://fxbug.dev/42079345): VMOs containing kernel stacks for user threads should be
  // linked to the attribution objects of the corresponding processes.
  fbl::RefPtr<VmObjectPaged> stack_vmo;
  zx_status_t status =
      VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, VmObjectPaged::kAlwaysPinned, vmo_size, &stack_vmo);
  if (status != ZX_OK) {
    LTRACEF("error allocating kernel stacks for thread\n");
    return status;
  }
  constexpr const char kKernelStackName[] = "kernel-stack";
  stack_vmo->set_name(kKernelStackName, sizeof(kKernelStackName) - 1);

  uint64_t vmo_offset = 0;
  status = map(kSafe, stack_vmo, &vmo_offset, &main_map_);
  if (status != ZX_OK) {
    return status;
  }

#if __has_feature(safe_stack)
  DEBUG_ASSERT(!unsafe_map_.vmar_);
  status = map(kUnsafe, stack_vmo, &vmo_offset, &unsafe_map_);
  if (status != ZX_OK) {
    return status;
  }
#endif

#if __has_feature(shadow_call_stack)
  DEBUG_ASSERT(!shadow_call_map_.vmar_);
  status = map(kShadowCall, stack_vmo, &vmo_offset, &shadow_call_map_);
  if (status != ZX_OK) {
    return status;
  }
#endif
  return ZX_OK;
}

void KernelStack::DumpInfo(int debug_level) const {
  auto map_dump = [debug_level](const KernelStack::Mapping& map, const char* tag) {
    dprintf(debug_level, "\t%s base %#" PRIxPTR ", size %#zx, vmar %p\n", tag, map.base(),
            map.size(), map.vmar_.get());
  };

  map_dump(main_map_, "stack");
#if __has_feature(safe_stack)
  map_dump(unsafe_map_, "unsafe_stack");
#endif
#if __has_feature(shadow_call_stack)
  map_dump(shadow_call_map_, "shadow_call_stack");
#endif
}

KernelStack::~KernelStack() {
  [[maybe_unused]] zx_status_t status = Teardown();
  DEBUG_ASSERT_MSG(status == ZX_OK, "KernelStack::Teardown returned %d\n", status);
}

zx_status_t KernelStack::Teardown() {
  zx_status_t status = unmap(kSafe, main_map_);
  if (status != ZX_OK) {
    return status;
  }
#if __has_feature(safe_stack)
  status = unmap(kUnsafe, unsafe_map_);
  if (status != ZX_OK) {
    return status;
  }
#endif
#if __has_feature(shadow_call_stack)
  status = unmap(kShadowCall, shadow_call_map_);
  if (status != ZX_OK) {
    return status;
  }
#endif
  return ZX_OK;
}
