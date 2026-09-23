// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/asm.h>
#include <lib/arch/ticks.h>
#include <lib/boot-options/boot-options.h>
#include <lib/code-patching/self-test.h>
#include <lib/counters.h>
#include <lib/elfldltl/machine.h>
#include <lib/page/size.h>
#include <lib/thread-stack/abi.h>
#include <lib/zbitl/view.h>
#include <platform.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/rights.h>

#include <arch/kernel_aspace.h>
#include <fbl/ref_ptr.h>
#include <kernel/thread.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/utility.h>
#include <lk/init.h>
#include <object/handle.h>
#include <object/vm_object_dispatcher.h>
#include <phys/boot-constants.h>
#include <phys/handoff.h>
#include <phys/zircon-abi-spec.h>
#include <platform/timer.h>
#include <vm/handoff-end.h>
#include <vm/kstack.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_object_paged.h>

#include <ktl/enforce.h>

PhysHandoff* gPhysHandoff;
vaddr_t gPhysmapBase;
size_t gPhysmapSize;

// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern arch::AsmLabel __text_start;  // Defined by the linker script.

// This is declared as `extern "C" constinit const` so the compiler knows that
// it needs to be defined for real to satisfy link-time references (from -e).
// But the compiler still knows it's constexpr (or const would be enough) and
// so it could substitute reads from its members for their known initializers.
// The members initialized here (and the declared-initialized const members)
// are only read by physboot and not by the kernel, so there are no references
// into this for the compiler to optimize anyway.
extern "C" constexpr ZirconAbiSpec kZirconAbiSpec = {
    .machine_stack = kMachineStack,
#if __has_feature(shadow_call_stack)
    .shadow_call_stack = kShadowCallStack,
#endif
#if __has_feature(safe_stack)
    .unsafe_stack = kUnsafeStack,
#endif

    .boot_constants{kBootConstants},

    .kernel_aspace_base = KERNEL_ASPACE_BASE,
    .kernel_aspace_size = KERNEL_ASPACE_SIZE,

#if LK_DEBUGLEVEL >= 2
    // Without debug support, this stays nullptr and physboot will panic if the
    // `kernel.debug.boot-spin` boot option is used.
    .debug_boot_spin_ready{gDebugBootSpinReady},
#endif

    // This is just handy for physboot to be able to print out.
    .text_start{__text_start[0]},
};

#if LK_DEBUGLEVEL >= 2
constinit volatile bool gDebugBootSpinReady = false;
#endif

namespace {

// When using physboot, other samples are available in the handoff data too.
//
// **NOTE** Each sample here is represented in the userland test code in
// //src/tests/benchmarks/kernel_boot_stats.cc that knows the order of the
// steps and gives names to the intervals between the steps (as well as
// tracking the first-to-last total elapsed time across the first to last
// boot.timeline.* samples, not all recorded right here).  Any time a new time
// sample is added to PhysBootTimes, a kcounter should be added here and
// kernel_boot_stats.cc should be updated to give the new intervals appropriate
// names for the performance tracking infrastructure (see the pages at
// https://chromeperf.appspot.com/report and look for "fuchsia.kernel.boot").
KCOUNTER(timeline_hw_startup, "boot.timeline.hw")
KCOUNTER(timeline_zbi_entry, "boot.timeline.zbi")
KCOUNTER(timeline_physboot_setup, "boot.timeline.physboot-setup")
KCOUNTER(timeline_decompress_start, "boot.timeline.decompress-start")
KCOUNTER(timeline_decompress_end, "boot.timeline.decompress-end")
KCOUNTER(timeline_zbi_done, "boot.timeline.zbi-done")

void Set(const Counter& counter, arch::EarlyTicks sample) {
  counter.Set(platform_convert_early_ticks(sample));
}

void Set(const Counter& counter, PhysBootTimes::Index i) {
  counter.Set(platform_convert_early_ticks(gPhysHandoff->times.Get(i)));
}

// Convert early boot timeline points into zx_ticks_t values in kcounters.
void TimelineCounters(unsigned int level) {
  // This isn't really a loop in any meaningful sense, but structuring it
  // this way gets the compiler to warn about any forgotten enum entry.
  for (size_t i = 0; i <= PhysBootTimes::kCount; ++i) {
    const PhysBootTimes::Index when = static_cast<PhysBootTimes::Index>(i);
    switch (when) {
      case PhysBootTimes::kZbiEntry:
        Set(timeline_zbi_entry, when);
        break;
      case PhysBootTimes::kPhysSetup:
        Set(timeline_physboot_setup, when);
        break;
      case PhysBootTimes::kDecompressStart:
        Set(timeline_decompress_start, when);
        break;
      case PhysBootTimes::kDecompressEnd:
        Set(timeline_decompress_end, when);
        break;
      case PhysBootTimes::kZbiDone:
        Set(timeline_zbi_done, when);
        break;
      case PhysBootTimes::kCount:
        // There is no PhysBootTimes entry corresponding to kCount.
        break;
    }
  }
  Set(timeline_hw_startup, arch::EarlyTicks::Zero());
}

// This can happen really any time after the platform clock is configured.
LK_INIT_HOOK(TimelineCounters, TimelineCounters, LK_INIT_LEVEL_PLATFORM)

fbl::RefPtr<VmObject> CreatePhysVmo(const PhysVmo& phys_vmo) {
  ktl::string_view name{phys_vmo.name.data(), phys_vmo.name.size()};
  name = name.substr(0, name.find_first_of('\0'));
  DEBUG_ASSERT(!name.empty());

  DEBUG_ASSERT(IsPageRounded(phys_vmo.addr));
  DEBUG_ASSERT(IsPageRounded(phys_vmo.SizeBytes<kPageSize>()));

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateFromWiredPages(
      paddr_to_physmap(phys_vmo.addr), phys_vmo.SizeBytes<kPageSize>(), true, &vmo);
  ASSERT(status == ZX_OK);

  status = vmo->set_name(name.data(), name.size());
  DEBUG_ASSERT(status == ZX_OK);

  phys_vmo.Log("VM");

  return vmo;
}

HandleOwner CreateHandle(fbl::RefPtr<VmObject> vmo, size_t stream_size, bool writable = false) {
  zx_rights_t rights;
  KernelHandle<VmObjectDispatcher> handle;
  zx_status_t status =
      VmObjectDispatcher::Create(ktl::move(vmo), stream_size,
                                 VmObjectDispatcher::InitialMutability::kMutable, &handle, &rights);
  ASSERT(status == ZX_OK);

  if (writable) {
    rights |= ZX_RIGHT_WRITE;
  } else {
    rights &= ~ZX_RIGHT_WRITE;
  }
  return Handle::Make(ktl::move(handle), rights);
}

HandleOwner CreatePhysVmoHandle(const PhysVmo& phys_vmo, bool writable = false) {
  return CreateHandle(CreatePhysVmo(phys_vmo), phys_vmo.stream_size, writable);
}

HandleOwner CreateStubVmoHandle() {
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, 0, &vmo);
  ASSERT(status == ZX_OK);
  return CreateHandle(ktl::move(vmo), 0);
}

HandoffEnd::Elf CreatePhysElf(const PhysElfImage& image) {
  ZX_DEBUG_ASSERT(image.vmar.base == 0);
  HandoffEnd::Elf elf = {
      .vmo = CreatePhysVmo(image.vmo),
      .stream_size = image.vmo.stream_size,
      .vmar_size = image.vmar.size,
      .info = image.info,
  };
  fbl::AllocChecker ac;
  elf.mappings.reserve(image.vmar.mappings.size(), &ac);
  ZX_ASSERT_MSG(ac.check(), "no kernel heap space for ELF %zu mappings in %s",
                image.vmar.mappings.size(), image.vmo.name.data());
  for (const PhysMapping& mapping : image.vmar.mappings.get()) {
    elf.mappings.push_back(mapping, &ac);
    ZX_ASSERT(ac.check());
  }
  return elf;
}

}  // namespace

const BootOptions* BootOptions::Get() { return kBootConstants.boot_options.get(); }

// This function is called first thing on kernel entry, so it should be
// careful on what it assumes is present.
void PostHandoffBootstrap(PhysHandoff* handoff) {
  // This is not really part of the function, it can go anywhere as long as the
  // compiler doesn't duplicate it.  But it must be inside a function since
  // Clang doesn't support asm with operands outside functions like GCC >= 15
  // does.  This is a convenient function that's definite only called once.
  // This is just defining kBootConstants as a const variable, but so the
  // compiler can't see how and thus cannot optimize away reading its contents.
  // The contents are filled with distinctive garbage in the link-time image.
  // Before the kernel runs, physboot write real contents here.
  //
  // TODO(https://fxbug.dev/379891035): arch/x86/start.S uses the
  // kKernelPhysicalLoadAddress symbol.  It can be removed when that is gone.
  constexpr ktl::byte kFill{0xdd};
  __asm__ volatile(
      R"""(
      .pushsection .rodata.kBootConstants, "a", %%progbits
      .balign %cc[alignment]
      .globl kBootConstants
      .hidden kBootConstants
      .type kBootConstants, %%object
      kBootConstants:
        .space %cc[size], %cc[fill]
      .size kBootConstants, %cc[size]
      .globl kKernelPhysicalLoadAddress
      .hidden kKernelPhysicalLoadAddress
      .type kKernelPhysicalLoadAddress, %%object
      kKernelPhysicalLoadAddress = kBootConstants + %cc[offsetof_loadaddr]
      .popsection
      )"""
      :
      :
      [alignment] "i"(alignof(BootConstants)), [size] "i"(sizeof(BootConstants)), [fill] "i"(kFill),
      [offsetof_loadaddr] "i"(offsetof(BootConstants, kernel_physical_load_address)));

  // Crucial set-up happens in ArchPostHandoffBootstrap() and it should happen
  // early. We take care to only sequence the simple setting of several,
  // fundamental global variables before then, along with entrypoint time
  // sampling.

  ZX_DEBUG_ASSERT(handoff);
  gPhysHandoff = handoff;

  ZX_DEBUG_ASSERT(KernelPhysicalAddressOf<__executable_start>() ==
                  kBootConstants.kernel_physical_load_address);

  gPhysmapBase = gPhysHandoff->physmap_base;
  gPhysmapSize = gPhysHandoff->physmap_size;

  ArchPostHandoffBootstrap(&handoff->arch_handoff);

  // This serves as a verification that code-patching was performed before
  // the kernel was booted; if unpatched, we would trap here and halt.
  CodePatchingNopTest();
}

paddr_t KernelPhysicalLoadAddress() { return kBootConstants.kernel_physical_load_address; }

paddr_t KernelPhysicalAddressOf(uintptr_t va) {
  // Only allow addresses within the bounds of the original file image, which
  // is physically contiguous.  The bss pages are not necessarily contiguous.
  const uintptr_t start = reinterpret_cast<uintptr_t>(__executable_start);
  [[maybe_unused]] const uintptr_t end = reinterpret_cast<uintptr_t>(__data_end);
  ZX_DEBUG_ASSERT_MSG(va >= start, "%#" PRIxPTR " < %p", va, __executable_start);
  ZX_DEBUG_ASSERT_MSG(va < end, "%#" PRIxPTR " >= %p", va, __data_end);
  return kBootConstants.kernel_physical_load_address + (va - start);
}

HandoffEnd EndHandoff() {
  HandoffEnd end{
      // Userboot expects the ZBI as writable.
      .zbi = CreatePhysVmoHandle(gPhysHandoff->zbi, /*writable=*/true),
      .vdso = CreatePhysElf(gPhysHandoff->vdso),
      .userboot = CreatePhysElf(gPhysHandoff->userboot),
  };

  // If the number of extra VMOs from physboot is less than the number of VMOs
  // the userboot protocol expects, fill the rest with empty VMOs.
  ktl::span<const PhysVmo> phys_vmos = gPhysHandoff->extra_vmos.get();
  for (size_t i = 0; i < phys_vmos.size(); ++i) {
    end.extra_phys_vmos[i] = CreatePhysVmoHandle(phys_vmos[i]);
  }
  for (size_t i = phys_vmos.size(); i < PhysVmo::kMaxExtraHandoffPhysVmos; ++i) {
    end.extra_phys_vmos[i] = CreateStubVmoHandle();
  }

  // Point of temporary hand-off memory expiration: first unmapped, then freed
  // in the PMM. Since gPhysHandoff is itself temporary hand-off memory, we
  // immediately unset the pointer afterward.
  vm_end_handoff();
  pmm_end_handoff();
  gPhysHandoff = nullptr;

  return end;
}
