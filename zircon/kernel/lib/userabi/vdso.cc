// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/affine/ratio.h>
#include <lib/boot-options/boot-options.h>
#include <lib/fasttime/internal/abi.h>
#include <lib/page/size.h>
#include <lib/userabi/vdso-constants.h>
#include <lib/userabi/vdso.h>
#include <platform.h>
#include <trace.h>
#include <zircon/types.h>

#include <arch/quirks.h>
#include <fbl/alloc_checker.h>
#include <kernel/mp.h>
#include <ktl/array.h>
#include <object/handle.h>
#include <phys/boot-constants.h>
#include <vm/pmm.h>
#include <vm/vm.h>
#include <vm/vm_aspace.h>
#include <vm/vm_object.h>

#if defined(__aarch64__)
#include <dev/timer/arm_generic.h>
#endif

#include "sysret-offsets.h"
#include "vdso-code.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

namespace {

constexpr uint64_t kVdsoCodeStart = VDSO_CODE_START;
constexpr size_t kVdsoCodeSize = VDSO_CODE_END - VDSO_CODE_START;

static_assert(VDSO_MAX_PAGE_SIZE >= kPageSize, "Need to adjust VDSO_MAX_PAGE_SIZE");
static_assert(VDSO_MAX_PAGE_SIZE > sizeof(vdso_constants), "Need to adjust VDSO_MAX_PAGE_SIZE");
static_assert(VDSO_MAX_PAGE_SIZE >= alignof(vdso_constants), "Need to adjust VDSO_MAX_PAGE_SIZE");

// This always leaves space for the NUL terminator.
constexpr size_t kMaxVersionString = VDSO_MAX_PAGE_SIZE - sizeof(vdso_constants) - 1;

class VDsoMutator {
 public:
  explicit VDsoMutator(const fbl::RefPtr<VmObject>& vmo) : vmo_(vmo) {}

  void RedirectSymbol(const char* from, const char* to, size_t idx1, size_t idx2, uintptr_t value) {
    auto [sym1, sym2] = ReadSymbol(idx1, idx2);

    // Just change the st_value of the symbol.
    sym1.value = sym2.value = value;
    WriteSymbol(idx1, sym1);
    WriteSymbol(idx2, sym2);

    LTRACEF("%s -> %s @ %#" PRIxPTR "\n", from, to, value);
  }

  void BlockSymbol(const char* name, uintptr_t value, size_t size, size_t idx1, size_t idx2) {
    auto [sym1, sym2] = ReadSymbol(idx1, idx2);

    // First change the symbol to have local binding so it can't be resolved.
    // The high nybble is the STB_* bits; STB_LOCAL is 0.
    sym1.info &= 0xf;
    sym2.info &= 0xf;
    WriteSymbol(idx1, sym1);
    WriteSymbol(idx2, sym2);

    // Now fill the code region (a whole function) with safely invalid code.
    // This code should never be run, and any attempt to use it should crash.
    // This uses the compile-time st_value and st_size passed in by the
    // BLOCK_SYSCALL macro, in case the symbol table entry's previous value was
    // already from a previous RedirectSymbol.
    ASSERT(value >= VDSO_CODE_START);
    ASSERT(value + size < VDSO_CODE_END);
    zx_status_t status = vmo_->Write(GetTrapFill(size), value, size);
    ASSERT_MSG(status == ZX_OK, "vDSO VMO Write failed: %d", status);

    LTRACEF("%s @ [%#" PRIxPTR ", %#" PRIxPTR ")\n", name, value, value + size);
  }

 private:
  struct ElfSym {
    uintptr_t info, value, size;
  };

#if defined(__x86_64__)
  // Fill with the single-byte HLT instruction, so any place
  // user-mode jumps into this code, it gets a trap.
  using Insn = uint8_t;
  static constexpr Insn kTrapFill = 0xf4;  // hlt
#elif defined(__aarch64__)
  // Fixed-size instructions.  Use 'brk #1' (what __builtin_trap() emits).
  using Insn = uint32_t;
  static constexpr Insn kTrapFill = 0xd4200020;
#elif defined(__riscv)
  // Instructions are 16 or 32 bits.  All zeros is the 16-bit `unimp` instruction.
  using Insn = uint16_t;
  static constexpr Insn kTrapFill = 0;
#else
#error what architecture?
#endif

  void* GetTrapFill(size_t fill_size) {
    ASSERT(fill_size % sizeof(Insn) == 0);
    fill_size /= sizeof(Insn);
    if (fill_size > trap_fill_size_) {
      fbl::AllocChecker ac;
      trap_fill_.reset(new (&ac) Insn[fill_size]);
      ASSERT(ac.check());
      trap_fill_size_ = fill_size;
      for (size_t i = 0; i < fill_size; ++i) {
        trap_fill_[i] = kTrapFill;
      }
    }
    return trap_fill_.get();
  }

  static uintptr_t SymtabAddress(size_t idx) {
    ASSERT(idx < VDSO_DYNSYM_COUNT);
    return VDSO_DATA_START_dynsym + (idx * sizeof(ElfSym));
  }

  ElfSym ReadSymbol(size_t idx) {
    ElfSym sym;
    zx_status_t status = vmo_->Read(&sym, SymtabAddress(idx), sizeof(sym));
    ASSERT_MSG(status == ZX_OK, "vDSO VMO Read failed: %d", status);
    return sym;
  }

  ktl::array<ElfSym, 2> ReadSymbol(size_t idx1, size_t idx2) {
    ElfSym sym1 = ReadSymbol(idx1);
    ElfSym sym2 = ReadSymbol(idx2);
    ASSERT_MSG(sym1.value == sym2.value, "dynsym %zu vs %zu value %#lx vs %#lx", idx2, idx2,
               sym1.value, sym2.value);
    ASSERT_MSG(sym1.size == sym2.size, "dynsym %zu vs %zu size %#lx vs %#lx", idx2, idx2, sym1.size,
               sym2.size);
    return {sym1, sym2};
  }

  void WriteSymbol(size_t idx, const ElfSym& sym) {
    zx_status_t status = vmo_->Write(&sym, SymtabAddress(idx), sizeof(sym));
    ASSERT_MSG(status == ZX_OK, "vDSO VMO Write failed: %d", status);
  }

  const fbl::RefPtr<VmObject>& vmo_;
  ktl::unique_ptr<Insn[]> trap_fill_;
  size_t trap_fill_size_ = 0;
};

#define PASTE(a, b, c) PASTE_1(a, b, c)
#define PASTE_1(a, b, c) a##b##c

#define REDIRECT_SYSCALL(mutator, symbol, target)                         \
  mutator.RedirectSymbol(#symbol, #target, PASTE(VDSO_DYNSYM_, symbol, ), \
                         PASTE(VDSO_DYNSYM__, symbol, ), PASTE(VDSO_CODE_, target, ))

// Block the named zx_* function.  The symbol table entry will
// become invisible to runtime symbol resolution, and the code of
// the function will be clobbered with trapping instructions.
#define BLOCK_SYSCALL(mutator, symbol)                                              \
  mutator.BlockSymbol(#symbol, PASTE(VDSO_, symbol, ), PASTE(VDSO_, symbol, _SIZE), \
                      PASTE(VDSO_DYNSYM_, symbol, ), PASTE(VDSO_DYNSYM__, symbol, ))

// Random attributes in zx fidl files become "categories" of syscalls.
// For each category, define a function block_<category> to block all the
// syscalls in that category.  These functions can be used in
// VDso::CreateVariant (below) to block a category of syscalls for a particular
// variant vDSO.
#define SYSCALL_CATEGORY_BEGIN(category) \
  [[maybe_unused]] void block_##category##_syscalls(VDsoMutator& mutator) {
#define SYSCALL_IN_CATEGORY(syscall) BLOCK_SYSCALL(mutator, zx_##syscall);
#define SYSCALL_CATEGORY_END(category) }
#include <lib/syscalls/category.inc>
#undef SYSCALL_CATEGORY_BEGIN
#undef SYSCALL_IN_CATEGORY_END
#undef SYSCALL_CATEGORY_END

#ifndef HAVE_SYSCALL_CATEGORY_test_category1
[[maybe_unused]] void block_test_category1_syscalls(VDsoMutator& mutator) {}
#endif

#ifndef HAVE_SYSCALL_CATEGORY_test_category2
[[maybe_unused]] void block_test_category2_syscalls(VDsoMutator& mutator) {}
#endif

// This is extracted from the vDSO image at build time.
using VdsoBuildIdNote = ktl::array<uint8_t, VDSO_BUILD_ID_NOTE_SIZE>;
constexpr VdsoBuildIdNote kVdsoBuildIdNote = VDSO_BUILD_ID_NOTE_BYTES;

// That should exactly match the note read from the vDSO image at runtime.
void CheckBuildId(const fbl::RefPtr<VmObject>& vmo) {
  VdsoBuildIdNote note;
  zx_status_t status = vmo->Read(&note, VDSO_BUILD_ID_NOTE_ADDRESS, sizeof(note));
  ASSERT_MSG(status == ZX_OK, "vDSO VMO Read failed: %d", status);
  ASSERT(note == kVdsoBuildIdNote);
}

// Fill out the contents of the time_values struct.
void SetTimeValues(const fbl::RefPtr<VmObject>& vmo) {
  zx_ticks_t per_second = ticks_per_second();

  // Grab a copy of the ticks to time ratio; we need this to initialize the
  // constants window.
  affine::Ratio ticks_to_time_ratio = timer_get_ticks_to_time_ratio();

  // At this point in time, we absolutely must know the rate that our tick
  // counter is ticking at.  If we don't, then something has gone horribly
  // wrong.
  ASSERT(per_second != 0);
  ASSERT(ticks_to_time_ratio.numerator() != 0);
  ASSERT(ticks_to_time_ratio.denominator() != 0);

  // Check if usermode can access ticks.
  const bool usermode_can_access_ticks = platform_usermode_can_access_tick_registers() &&
                                         !BootOptions::Get()->vdso_ticks_get_force_syscall;
  bool needs_a73_mitigation = false;
  bool use_pct_instead_of_vct = false;
#if ARCH_ARM64
  // We only need to install the A73 quirks for zx_ticks_get if we can access ticks from usermode.
  if (usermode_can_access_ticks) {
    // Before we got here (during an INIT_HOOK run at LK_INIT_LEVEL_USER - 1),
    // we should have already waited for all of the CPUs in the system to have
    // started up and checked in.
    //
    // Now that all CPUs have started, it should be safe to check whether or not
    // we need to deploy the ARM A73 timer read mitigation. In the case that the
    // CPUs did not all manage to start, go ahead and install the mitigation
    // anyway. This would be a bad situation, and the mitigation is slower then
    // the alternative if it is not needed, but at least it will read correctly
    // on all cores.
    //
    // see arch/quirks.h for details about the quirk itself.
    const zx_status_t wait_status = mp_wait_for_all_cpus_ready(Deadline::no_slack(0));
    if ((wait_status != ZX_OK) || arch_quirks_needs_arm_erratum_858921_mitigation()) {
      if (wait_status != ZX_OK) {
        dprintf(ALWAYS,
                "WARNING: Timed out waiting for all CPUs to start.  "
                "Using A73 quirks for zx_ticks_get in VDSO as a defensive measure.\n");
      } else {
        dprintf(INFO, "Using A73 quirks for zx_ticks_get in VDSO\n");
      }
      needs_a73_mitigation = true;
    }

    if (ArmUsePhysTimerInVdso()) {
      dprintf(INFO,
              "Using PCT instead of VCT as the system counter reference for zx_ticks_get in "
              "the VDSO\n");
      use_pct_instead_of_vct = true;
    }
  }
#endif

  // Initialize the time values that should be visible to the vDSO.
  fasttime::internal::TimeValues values = {
      .version = 1,
      .ticks_per_second = per_second,
      .boot_ticks_offset = timer_get_boot_ticks_offset(),
      .mono_ticks_offset = timer_get_mono_ticks_offset(),
      .ticks_to_time_numerator = ticks_to_time_ratio.numerator(),
      .ticks_to_time_denominator = ticks_to_time_ratio.denominator(),
      .usermode_can_access_ticks = usermode_can_access_ticks,
      .use_a73_errata_mitigation = needs_a73_mitigation,
      .use_pct_instead_of_vct = use_pct_instead_of_vct,
  };

  // Write the time values to the appropriate section in the vDSO.
  ktl::span bytes = ktl::as_bytes(ktl::span{&values, 1});
  zx_status_t status = vmo->Write(bytes.data(), VDSO_DATA_TIME_VALUES, bytes.size());
  ASSERT_MSG(status == ZX_OK, "vDSO Time Values VMO Write of %zu bytes at %#" PRIx64 " failed: %d",
             bytes.size(), uint64_t{VDSO_DATA_TIME_VALUES}, status);
}

// Fill out the contents of the vdso_constants struct.
void SetConstants(const fbl::RefPtr<VmObject>& vmo) {
  ktl::string_view version_string = kBootConstants.system_version_string.get();
  ASSERT_MSG(version_string.size() <= kMaxVersionString,
             "version string size %zu > max %zu: \"%.*s\"",
             kBootConstants.system_version_string.size(), kMaxVersionString,
             static_cast<int>(version_string.size()), version_string.data());

  // Initialize the constants that should be visible to the vDSO.
  // Rather than assigning each member individually, do this with
  // struct assignment and a compound literal so that the compiler
  // can warn if the initializer list omits any member.
  vdso_constants constants = {
      arch_max_num_cpus(),
      {
          arch_cpu_features(),
          arch_get_hw_breakpoint_count(),
          arch_get_hw_watchpoint_count(),
          arch_address_tagging_features(),
          arch_vm_features(),
      },
      arch_dcache_line_size(),
      arch_icache_line_size(),
      kPageSize,
      0,  // Padding.
      pmm_count_total_bytes(),
      version_string.size(),
  };

  auto write_vmo = [offset = uint64_t{VDSO_DATA_CONSTANTS}, &vmo](auto data) mutable {
    ktl::span bytes = ktl::as_bytes(ktl::span(data));
    zx_status_t status = vmo->Write(bytes.data(), offset, bytes.size());
    ASSERT_MSG(status == ZX_OK, "vDSO VMO Write of %zu bytes at %#" PRIx64 " failed: %d",
               bytes.size(), offset, status);
    offset += bytes.size();
  };

  // Write the constants initialized above, without the flexible array member.
  write_vmo(ktl::span{&constants, 1});

  // Store the version string and NUL terminator in the flexible array member.
  // The kMaxVersionString check ensures there is enough space for all that.
  write_vmo(version_string);
  write_vmo(ktl::span{"", 1});
}

// Conditionally patch some of the entry points related to time based on
// platform details which get determined at runtime.
void PatchTimeSyscalls(VDsoMutator mutator) {
  // If user mode cannot access the tick counter registers, or kernel command
  // line arguments demand that we access the tick counter via a syscall
  // instead of direct observation, then we need to make sure to redirect
  // symbol in the vDSO such that we always syscall in order to query ticks.
  //
  // Since this can effect how clock monotonic is calculated as well, we may
  // need to redirect zx_clock_get_monotonic as well.
  const bool need_syscall_for_ticks = !platform_usermode_can_access_tick_registers() ||
                                      BootOptions::Get()->vdso_ticks_get_force_syscall;

  if (need_syscall_for_ticks) {
    REDIRECT_SYSCALL(mutator, zx_ticks_get, SYSCALL_zx_ticks_get_via_kernel);
    REDIRECT_SYSCALL(mutator, zx_ticks_get_boot, SYSCALL_zx_ticks_get_boot_via_kernel);
    REDIRECT_SYSCALL(mutator, zx_clock_read_mapped, clock_read_mapped_via_kernel);
    REDIRECT_SYSCALL(mutator, zx_clock_get_details_mapped, clock_get_details_mapped_via_kernel);
  }

  if (BootOptions::Get()->vdso_clock_get_force_syscall) {
    // Force a syscall for zx_clock_get_monotonic and zx_clock_get_boot if instructed to do so by
    // the kernel command line arguments.  Make sure to swap out the implementation
    // of zx_deadline_after as well.
    REDIRECT_SYSCALL(mutator, zx_clock_get_boot, SYSCALL_zx_clock_get_boot_via_kernel);
    REDIRECT_SYSCALL(mutator, zx_clock_get_monotonic, SYSCALL_zx_clock_get_monotonic_via_kernel);
    REDIRECT_SYSCALL(mutator, zx_deadline_after, deadline_after_via_kernel_mono);
  } else if (need_syscall_for_ticks) {
    // If ticks must be accessed via syscall, then choose the alternate form
    // for clock_get_monotonic and clock_get_boot which performs the scaling in user mode, but
    // thunks into the kernel to read the ticks register.
    REDIRECT_SYSCALL(mutator, zx_clock_get_monotonic, clock_get_monotonic_via_kernel_ticks);
    REDIRECT_SYSCALL(mutator, zx_clock_get_boot, clock_get_boot_via_kernel_ticks);
    REDIRECT_SYSCALL(mutator, zx_deadline_after, deadline_after_via_kernel_ticks);
  }
}

}  // anonymous namespace

const VDso* VDso::instance_ = nullptr;

// This is called exactly once, at boot time.
const VDso* VDso::Create(
    const HandoffEnd::Elf& elf_image,
    ktl::span<KernelHandle<VmObjectDispatcher>, VDso::kNumVdsoVariants> vmo_kernel_handles,
    KernelHandle<VmObjectDispatcher>* time_values_handle) {
  ASSERT(!instance_);

  // Check the ELF segments are valid for the vDSO.
  for (const PhysMapping& segment : elf_image.mappings) {
    ZX_ASSERT_MSG(!segment.perms.writable() && segment.paddr != PhysElfImage::kZeroFill,
                  "vDSO cannot have writable segment [%#" PRIxPTR ", %#" PRIxPTR ")", segment.vaddr,
                  segment.vaddr + segment.size);
    if (segment.perms.executable()) {
      ZX_ASSERT_MSG(segment.vaddr == kVdsoCodeStart && segment.size == kVdsoCodeSize,
                    "vDSO code segment [%#" PRIxPTR ", %#" PRIxPTR
                    ") doesn't match expected [%#" PRIxPTR ", %#" PRIxPTR ")",
                    segment.vaddr, segment.vaddr + segment.size, kVdsoCodeStart, kVdsoCodeSize);
    }
  }

  fbl::AllocChecker ac;
  VDso* vdso = new (&ac) VDso;
  ASSERT(ac.check());

  vdso->vmo_ = elf_image.vmo;

  // build and point a dispatcher at it
  zx_status_t status = VmObjectDispatcher::Create(
      vdso->vmo(), elf_image.stream_size, VmObjectDispatcher::InitialMutability::kMutable,
      &vmo_kernel_handles[variant_index(Variant::NEXT)], &vdso->vmo_rights_);
  ASSERT(status == ZX_OK);
  vdso->vmo_rights_ &= ~ZX_RIGHT_WRITE;
  vdso->vmo_rights_ |= ZX_RIGHT_EXECUTE;

  vdso->variant_vmo_[variant_index(Variant::NEXT)] =
      vmo_kernel_handles[variant_index(Variant::NEXT)].dispatcher();

  // Sanity-check that it's the exact vDSO image the kernel was compiled for.
  CheckBuildId(vdso->vmo());

  // Fill out the contents of the vdso_constants struct.
  SetConstants(vdso->vmo());

  PatchTimeSyscalls(VDsoMutator{vdso->vmo()});

  // Fill out the contents of the time_values struct.
  SetTimeValues(vdso->vmo());

  // Create the standalone time values VMO for use by fasttime.
  vdso->CreateTimeValuesVmo(time_values_handle);

  // Create the vDSO variants.
  for (size_t v = static_cast<size_t>(Variant::STABLE); v < static_cast<size_t>(Variant::COUNT);
       ++v)
    vdso->CreateVariant(static_cast<Variant>(v), &vmo_kernel_handles[v]);

  // Map and pin the time values VMO for each variant. We do this after having created all of the
  // variants to avoid any issues with pinning pages in a VMO prior to snapshotting it.
  for (size_t v = static_cast<size_t>(Variant::STABLE); v < static_cast<size_t>(Variant::COUNT);
       ++v) {
    Variant var = static_cast<Variant>(v);
    status = vdso->MapTimeValuesVmo(var, vdso->variant_vmo_[variant_index(var)]->vmo());
    ASSERT(status == ZX_OK);
  }

  instance_ = vdso;
  return instance_;
}

uintptr_t VDso::base_address(const fbl::RefPtr<VmMapping>& code_mapping) {
  return code_mapping->base() - VDSO_CODE_START;
}

// The time_values_vmo is a child slice of the read-only section of the vDSO that contains just the
// time_values structure.
void VDso::CreateTimeValuesVmo(KernelHandle<VmObjectDispatcher>* time_values_handle) {
  fbl::RefPtr<VmObject> new_vmo;
  zx_status_t status = dispatcher()->CreateChild(ZX_VMO_CHILD_SLICE, VDSO_DATA_TIME_VALUES,
                                                 VDSO_DATA_TIME_VALUES_SIZE, false, &new_vmo);
  ASSERT(status == ZX_OK);

  zx_rights_t rights;
  status = VmObjectDispatcher::Create(ktl::move(new_vmo), VDSO_DATA_TIME_VALUES_SIZE,
                                      VmObjectDispatcher::InitialMutability::kMutable,
                                      time_values_handle, &rights);
  ASSERT(status == ZX_OK);

  status =
      time_values_handle->dispatcher()->set_name(kTimeValuesVmoName, strlen(kTimeValuesVmoName));
  ASSERT(status == ZX_OK);
}

void VDso::SetMonotonicTicksOffset(zx_ticks_t new_offset) {
  if (!instance_) {
    return;
  }
  for (auto time_values : instance_->time_values_) {
    // TODO(https://fxbug.dev/341785588): This code should be made resilient to a changing
    // mono_ticks_offset once we start pausing the clock during system suspension.
    time_values->mono_ticks_offset.store(new_offset, ktl::memory_order_relaxed);
  }
}

zx_status_t VDso::MapTimeValuesVmo(Variant variant, const fbl::RefPtr<VmObject>& vdso_vmo) {
  size_t variant_idx = variant_index(variant);
  zx_status_t status = variant_time_mappings_[variant_idx].Init(
      vdso_vmo, VDSO_DATA_TIME_VALUES, VDSO_DATA_TIME_VALUES_SIZE, "vdso time values");
  if (status != ZX_OK) {
    return status;
  }

  // Store the pointer to the actual TimeValues structure to avoid having to call base_locking every
  // time. We can do this because the mappings are not changed once created.
  time_values_[variant_index(variant)] = reinterpret_cast<fasttime::internal::TimeValues*>(
      variant_time_mappings_[variant_idx].base_locking());

  return ZX_OK;
}

// Each vDSO variant VMO is made via a COW clone of the next vDSO
// VMO.  A variant can block some system calls, by syscall category.
// This works by modifying the symbol table entries to make the symbols
// invisible to dynamic linking (STB_LOCAL) and then clobbering the code
// with trapping instructions.  In this way, all the code locations are the
// same across variants and the syscall entry enforcement doesn't have to
// care which variant is in use.  The places where the blocked
// syscalls' syscall entry instructions would be no longer have the syscall
// instructions, so a process using the variant can never get into syscall
// entry with that PC value and hence can never pass the vDSO enforcement
// test.
void VDso::CreateVariant(Variant variant, KernelHandle<VmObjectDispatcher>* vmo_kernel_handle) {
  DEBUG_ASSERT(variant >= Variant::STABLE);
  DEBUG_ASSERT(variant < Variant::COUNT);

  if (variant == Variant::NEXT) {
    // The next variant already has a VMO.
    DEBUG_ASSERT(variant_vmo_[variant_index(variant)] == vmo_kernel_handle->dispatcher());
    return;
  }

  DEBUG_ASSERT(!variant_vmo_[variant_index(variant)]);

  fbl::RefPtr<VmObject> new_vmo;
  zx_status_t status = dispatcher()->CreateChild(ZX_VMO_CHILD_SNAPSHOT, 0,
                                                 dispatcher()->vmo()->size(), false, &new_vmo);
  ASSERT(status == ZX_OK);

  LTRACEF("variant %u\n", static_cast<unsigned int>(variant));

  VDsoMutator mutator{new_vmo};

  const char* name = nullptr;
  switch (variant) {
    case Variant::STABLE:
      name = "vdso/stable";
      LTRACEF("variant %s\n", name);
      block_next_syscalls(mutator);
      break;

    case Variant::TEST1:
      name = "vdso/test1";
      LTRACEF("variant %s\n", name);
      block_test_category1_syscalls(mutator);
      break;

    case Variant::TEST2:
      name = "vdso/test2";
      LTRACEF("variant %s\n", name);
      block_test_category2_syscalls(mutator);
      break;

    // No default case so the compiler will warn about new enum entries.
    case Variant::NEXT:
    case Variant::COUNT:
      PANIC("VDso::CreateVariant called with bad variant");
  }

  zx_rights_t rights;
  status = VmObjectDispatcher::Create(ktl::move(new_vmo), dispatcher()->GetStreamSize(),
                                      VmObjectDispatcher::InitialMutability::kMutable,
                                      vmo_kernel_handle, &rights);
  ASSERT(status == ZX_OK);

  status = vmo_kernel_handle->dispatcher()->set_name(name, strlen(name));
  ASSERT(status == ZX_OK);

  variant_vmo_[variant_index(variant)] = vmo_kernel_handle->dispatcher();
}

bool VDso::valid_code_mapping(uint64_t vmo_offset, size_t size) {
  return vmo_offset == kVdsoCodeStart && size == kVdsoCodeSize;
}

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_vmo_is_vdso(const VmObject* vmo) { return VDso::vmo_is_vdso(vmo); }

}  // extern "C"
