// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "handoff-prep.h"

#include <lib/boot-options/boot-options.h>
#include <lib/instrumentation/debugdata.h>
#include <lib/llvm-profdata/llvm-profdata.h>
#include <lib/memalloc/pool-mem-config.h>
#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>
#include <lib/trivial-allocator/new.h>
#include <lib/zbitl/error-stdio.h>
#include <stdio.h>
#include <string-file.h>
#include <zircon/assert.h>

#include <ktl/tuple.h>
#include <ktl/utility.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/arch/arch-handoff.h>
#include <phys/boot-constants.h>
#include <phys/boot-options.h>
#include <phys/elf-image.h>
#include <phys/handoff.h>
#include <phys/kernel-package.h>
#include <phys/main.h>
#include <phys/new.h>
#include <phys/stdio.h>
#include <phys/symbolize.h>
#include <phys/uart.h>

#include "log.h"
#include "physboot.h"

#include <ktl/enforce.h>

namespace {

constexpr ktl::string_view kMachineFileName =
    elfldltl::ElfMachineFileName(elfldltl::ElfMachine::kNative);

// Carve out some physical pages requested for testing before handing off.
void FindTestRamReservation(RamReservation& ram) {
  ZX_ASSERT_MSG(!ram.paddr, "Must use kernel.test.ram.reserve=SIZE without ,ADDRESS!");

  memalloc::Pool& pool = Allocation::GetPool();

  // Don't just use Pool::Allocate because that will use the first (lowest)
  // address with space.  The kernel's PMM initialization doesn't like the
  // earliest memory being split up too small, and anyway that's not very
  // representative of just a normal machine with some device memory elsewhere,
  // which is what the test RAM reservation is really meant to simulate.
  // Instead, find the highest-addressed, most likely large chunk that is big
  // enough and just make it a little smaller, which is probably more like what
  // an actual machine with a little less RAM would look like.

  auto it = pool.end();
  while (true) {
    if (it == pool.begin()) {
      break;
    }
    --it;
    if (it->type == memalloc::Type::kFreeRam && it->size >= ram.size) {
      uint64_t aligned_start =
          (it->addr + it->size - ram.size) & -uint64_t{AddressSpace::kPageSize};
      uint64_t aligned_end = aligned_start + ram.size;
      if (aligned_start >= it->addr && aligned_end <= aligned_start + ram.size) {
        if (pool.UpdateRamSubranges(memalloc::Type::kTestRamReserve, aligned_start, ram.size)
                .is_ok()) {
          ram.paddr = aligned_start;
          debugf("%s: kernel.test.ram.reserve carve-out: [%#" PRIx64 ", %#" PRIx64 ")\n",
                 ProgramName(), aligned_start, aligned_end);
          return;
        }
        // Don't try another spot if something went wrong.
        break;
      }
    }
  }

  printf("%s: ERROR: Cannot reserve %#" PRIx64
         " bytes of RAM for kernel.test.ram.reserve request!\n",
         ProgramName(), ram.size);
}

// Returns a pointer into the array that was passed by reference.
constexpr ktl::string_view VmoNameString(const PhysVmo::Name& name) {
  ktl::string_view str(name.data(), name.size());
  return str.substr(0, str.find_first_of('\0'));
}

// Normalizes types so that only those that are of interest to the kernel
// remain.
ktl::optional<memalloc::Type> HandoffMemoryType(memalloc::Type type) {
  switch (type) {
    // The allocations that should survive into the hand-off.
    case memalloc::Type::kDataZbi:
    case memalloc::Type::kKernel:
    case memalloc::Type::kKernelPageTables:
    case memalloc::Type::kBootMachineStack:
    case memalloc::Type::kBootShadowCallStack:
    case memalloc::Type::kBootUnsafeStack:
    case memalloc::Type::kPhysDebugdata:
    case memalloc::Type::kPermanentPhysHandoff:
    case memalloc::Type::kPhysLog:
    case memalloc::Type::kReservedLow:
    case memalloc::Type::kTemporaryPhysHandoff:
    case memalloc::Type::kTestRamReserve:
    case memalloc::Type::kUserboot:
    case memalloc::Type::kVdso:
      return type;

    // The identity map needs to be installed at the time of hand-off, but
    // shouldn't actually be used by the kernel after that; mark it for
    // clean-up.
    case memalloc::Type::kTemporaryIdentityPageTables:
      // TODO(https://fxbug.dev/398950948): Ideally these ranges would be
      // passed on as temporary handoff data, but the kernel currently
      // expects this memory to persist past boot (e.g, for later
      // hotplugging). Pending revisiting that in the kernel, we hand off all
      // "temporary" identity tables as permanent for now.
      return memalloc::Type::kKernelPageTables;

    // An NVRAM range should no longer be treated like normal RAM. The kernel
    // will access it through the mapping provided with PhysHandoff::nvram,
    // and will further key off that to restrict userspace access to this
    // range of memory.
    case memalloc::Type::kNvram:
    // Truncations should now go into effect.
    case memalloc::Type::kTruncatedRam:
    // kPeripheral range content has been distilled in
    // PhysHandoff::periph_ranges and does not need to be present in this
    // accounting.
    case memalloc::Type::kPeripheral:
      return ktl::nullopt;

    default:
      ZX_DEBUG_ASSERT(type != memalloc::Type::kReserved);
      break;
  }

  if (memalloc::IsRamType(type)) {
    return memalloc::Type::kFreeRam;
  }

  // Anything unknown should be ignored.
  return ktl::nullopt;
}

}  // namespace

template <typename T>
T* HandoffPrep::InKernelImage(const PhysHandoffKernelImagePtr<T>& ptr) const {
  auto& memory = kernel_.image();
  // Translate the virtual pointer back to its link-time address.
  uintptr_t addr = reinterpret_cast<uintptr_t>(ptr.ptr_) - kernel_.load_bias();
  // Turn that into a pointer into the physical image.
  return memory.GetPointer<T>(addr);
}

template <typename T, typename... Args>
  requires(ktl::constructible_from<T, Args...> && ktl::is_trivially_destructible_v<T>)
T* HandoffPrep::NewInKernelImage(const PhysHandoffKernelImagePtr<const T>& ptr,
                                 Args&&... args) const {
  return new (InKernelImage<T>(ptr.template ConstCast<T>())) T{ktl::forward<Args>(args)...};
}

HandoffPrep::HandoffPrep(ElfImage kernel)
    : kernel_(ktl::move(kernel)),
      permanent_data_allocator_(VirtualAddressAllocator::PermanentHandoffDataAllocator(kernel_)) {
  // The kernel's Ehdr::e_entry actually points to its kZirconAbiSpec.  The
  // physical image is where it will stay even when mapped virtually (where it
  // will be read-only).  Stash the direct pointer into the image to use later.
  abi_spec_ = kernel.ImageEntry<const ZirconAbiSpec>();
  if (!abi_spec_) [[unlikely]] {
    ZX_PANIC("ZirconAbiSpec (e_entry) address %#" PRIx64 " invalid for image", kernel.entry());
  }

  // Check that this isn't clearly garbled data somehow.
  abi_spec_->AssertValid<AddressSpace::kPageSize, AddressSpace::kUpperVirtualAddressRangeStart>();

  // With a validated ABI spec we can now properly initialize the temporary
  // data and first-class mapping allocators.
  temporary_data_allocator_ = TemporaryDataAllocator{
      VirtualAddressAllocator::TemporaryHandoffDataAllocator(kernel_, *abi_spec_)};
  first_class_mapping_allocator_ =
      VirtualAddressAllocator::FirstClassMappingAllocator(kernel_, *abi_spec_);

  // Translate the relocated virtual address from the spec back into the image
  // to initialize the kernel's kBootContents.
  BootConstants constants = {
      .kernel_physical_load_address = kernel_.physical_load_address(),

      // The flag compiled into the kernel proper can override the boot option.
      .bypass_debuglog = abi_spec_->always_bypass_debuglog || BootOptions::Get()->bypass_debuglog,
  };

  // Other methods will fill in more values via boot_constants_, which points
  // to the writable physical address, not the kernel's RODATA virtual address.
  boot_constants_ = NewInKernelImage(abi_spec_->boot_constants, ktl::move(constants));

  // Note that this temporary hand-off data allocation must occur after we
  // properly initialize the temporary hand-off data allocator above.
  PhysHandoffTemporaryPtr<const PhysHandoff> handoff;
  fbl::AllocChecker ac;
  handoff_ = New(handoff, ac);
  ZX_ASSERT_MSG(ac.check(), "Failed to allocate PhysHandoff!");
}

PhysVmo HandoffPrep::MakePhysVmo(ktl::span<const ktl::byte> data, ktl::string_view name,
                                 size_t stream_size, bool known_zero) {
  uintptr_t addr = reinterpret_cast<uintptr_t>(data.data());
  ZX_ASSERT((addr % AddressSpace::kPageSize) == 0);
  ZX_ASSERT((data.size_bytes() % AddressSpace::kPageSize) == 0);
  ZX_ASSERT(((stream_size + AddressSpace::kPageSize - 1) & -AddressSpace::kPageSize) ==
            data.size_bytes());

  // Any space past the stream_size could be uninitialized.
  // Don't let garbage values leak into the VMO's last page.
  if (ktl::span partial_page = data.subspan(stream_size); !partial_page.empty()) {
    if (known_zero) {
      ZX_DEBUG_ASSERT(
          ktl::ranges::all_of(partial_page, [](ktl::byte byte) { return byte == ktl::byte{}; }));
    } else {
      memset(const_cast<ktl::byte*>(partial_page.data()), 0, partial_page.size_bytes());
    }
  }

  PhysVmo vmo{.addr = addr, .stream_size = stream_size};
  // The name is sometimes an arbitrary file name, which might be too long.
  // The set_name() method is strict on size to avoid literal names so long
  // they get truncated, but this path gets whatever names were packed into the
  // ZBI_TYPE_STORAGE_KERNEL package.
  vmo.set_name(name.substr(0, ZX_MAX_NAME_LEN - 1));
  return vmo;
}

void HandoffPrep::SetInstrumentation() {
  auto publish_debugdata = [this](ktl::string_view sink_name, ktl::string_view vmo_name,
                                  ktl::string_view vmo_name_suffix, size_t stream_size) {
    PhysVmo::Name phys_vmo_name =
        instrumentation::DebugdataVmoName(sink_name, vmo_name, vmo_name_suffix, /*is_static=*/true);

    size_t aligned_size = (stream_size + AddressSpace::kPageSize - 1) & -AddressSpace::kPageSize;
    fbl::AllocChecker ac;
    ktl::span contents =
        Allocation::New(ac, memalloc::Type::kPhysDebugdata, aligned_size, AddressSpace::kPageSize)
            .release();
    ZX_ASSERT_MSG(ac.check(), "cannot allocate %zu bytes for instrumentation phys VMO",
                  aligned_size);
    PublishExtraVmo(MakePhysVmo(contents, VmoNameString(phys_vmo_name), stream_size));
    return contents;
  };
  for (const ElfImage* module : gSymbolize->modules()) {
    module->PublishDebugdata(publish_debugdata);
  }
}

void HandoffPrep::PublishExtraVmo(PhysVmo&& vmo) {
  ZX_DEBUG_ASSERT(extra_vmos_);
  extra_vmos_->push_front(HandoffVmo::New(vmo));
}

void HandoffPrep::FinishVm() {
  ZX_ASSERT_MSG(extra_vmos_->size() <= PhysVmo::kMaxExtraHandoffPhysVmos,
                "Too many phys VMOs in hand-off! %zu > max %zu", extra_vmos_->size(),
                PhysVmo::kMaxExtraHandoffPhysVmos);
  NewFromList(handoff()->extra_vmos, *ktl::exchange(extra_vmos_, {}));
  // From here, any PublishExtraVmo() call would crash.

  auto populate_vmar = [this](PhysVmar* vmar, ktl::string_view name,
                              HandoffMappingList mapping_list) {
    vmar->set_name(name);
    ktl::span mappings = NewFromList(vmar->mappings, ktl::move(mapping_list));
    ZX_DEBUG_ASSERT(!mappings.empty());
    vmar->base = mappings.front().vaddr;
    uintptr_t vmar_end = mappings.back().vaddr_end();
    vmar->size = vmar_end - vmar->base;
  };

  // First reify the permanent handoff mappings.  There will be no more of
  // _these_ now, but doing this entails new _temporary_ handoff allocations
  // and thus perhaps new mappings.
  PhysVmar permanent_data_vmar;
  populate_vmar(&permanent_data_vmar, "permanent hand-off data",
                permanent_data_allocator_.allocate_function().memory().TakeMappings());

  // From here any more use of the permanent handoff allocator would crash _if_
  // it needed a new page, after TakeMappings().  Make sure that _any_ use will
  // crash by clearing out any remaining partial page it has left.
  constexpr auto clear_allocator = [](auto& allocator) {
    ktl::ignore = allocator.allocate(allocator.unallocated().size_bytes(), 1);
    ZX_DEBUG_ASSERT(allocator.unallocated().empty());
  };
  clear_allocator(permanent_data_allocator_);

  // The temporary handoff mappings need to be reified too.  But this needs
  // temporary handoff space of its own, and allocating that could require more
  // mappings!  And finally, the physical memory ranges must also be reified
  // into yet more temporary handoff space.  New handoff pages and perhaps new
  // page tables to map them could cause new range splits that increase the
  // exact size needed for that handoff array.
  //
  // The VMAR count cannot change, so that can do a normal allocation before
  // the sticky situation arises.  Move the permanent handoff data VMAR into
  // place without pushing it onto the vmars_ list.  The final VMAR list gets
  // sorted at the very end.
  const size_t vmars_count = vmars_->size();
  ktl::span final_vmars = NewFromList<1, false>(handoff()->vmars, *ktl::exchange(vmars_, {}));
  ZX_DEBUG_ASSERT(final_vmars.size() == vmars_count + 1);
  final_vmars[vmars_count] = ktl::move(permanent_data_vmar);

  // This can't quite be filled in yet, but it can be populated in place later.
  fbl::AllocChecker ac;
  PhysVmar* temporary_vmar = New(handoff()->temporary_vmar, ac);
  ZX_ASSERT(ac.check());

  // The sticky situation is handled by precomputing the total size of new
  // temporary handoff space and then reserving that in the allocator before
  // actually filling the remaining handoff arrays:
  //
  //  * PhysMapping list in temporary_vmar
  //  * Normalized memalloc::Range list
  //
  // This will reserve space for all those within one contiguous mapping of
  // contiguous physical pages (see PhysPages::Allocate).  After counting those
  // current lists, allocating the space to hold them all perturbs the counts:
  //
  //  * At most one additional PhysMapping in temporary_vmar
  //  * At most one additional memalloc::Range entry for the pages mapped there
  //  * At most one more for each level of page table (less one).  The
  //    top-level page table is already present to be sure, but worst case a
  //    new page table page is needed at each level down and there's a separate
  //    range split needed for each page table page allocation.  So with the
  //    handoff space plus the page tables (number of levels less one), there
  //    may be as many allocations as levels an entry per allocation.
  //
  // So the reservation of contiguous space in the temporary handoff allocator
  // overestimates accordingly.

  constexpr size_t kMaxNewPageTablePages =  // The root is never new.
      AddressSpace::UpperPaging::kLevels.size() - 1;
  size_t max_memory_ranges =
      // Start with the upper bound estimate of what might be added below.
      1 +                     // Map new page-range to cover max_final_space.
      kMaxNewPageTablePages;  // Page tables for that mapping.
  auto count_ranges = [&max_memory_ranges](const memalloc::Range& range) {
    ++max_memory_ranges;
    return true;
  };
  auto& pool = Allocation::GetPool();
  memalloc::NormalizeRanges(pool, count_ranges, HandoffMemoryType);

  auto& phys_pages = temporary_data_allocator_.allocate_function().memory();
  static_assert(alignof(PhysMapping) <= __STDCPP_DEFAULT_NEW_ALIGNMENT__);
  static_assert(alignof(memalloc::Range) <= __STDCPP_DEFAULT_NEW_ALIGNMENT__);
  const size_t max_new_space =  //
      ((phys_pages.CountMappings() + 1) * sizeof(PhysMapping)) +
      (max_memory_ranges * sizeof(memalloc::Range));
  ZX_ASSERT_MSG(temporary_data_allocator_.reserve(max_new_space),
                "cannot allocate %zu bytes for memory handoff", max_new_space);

  // The last page allocations and the last mappings have all been made now.
  // Populate the PhysVmar with all those mappings.
  populate_vmar(temporary_vmar, "temporary hand-off data", phys_pages.TakeMappings());

  // Sort the final VMARs list now that all the addresses are known.
  ktl::ranges::sort(final_vmars);

  // This final allocation cannot fail since the reserve() already worked.
  // Note the buffer size may be an overestimate, so it will be capped below.
  ktl::span handoff_ranges = New(handoff()->memory, ac, max_memory_ranges);
  ZX_ASSERT_MSG(ac.check(), "cannot allocate %zu bytes for memory handoff",
                max_memory_ranges * sizeof(memalloc::Range));

  // From here any more use of the temporary handoff allocator would crash _if_
  // it needed a new page, after TakeMappings().  Make sure that _any_ use will
  // crash by clearing out any remaining partial page it has left.
  clear_allocator(permanent_data_allocator_);

  // Now simply record the normalized ranges.
  auto it = handoff_ranges.begin();
  auto record_ranges = [&it](const memalloc::Range& range) {
    *it++ = range;
    return true;
  };
  memalloc::NormalizeRanges(pool, record_ranges, HandoffMemoryType);

  // Trim any excess buffer space that wasn't actually needed.
  handoff_ranges = ktl::span(handoff_ranges.begin(), it);
  handoff()->memory = handoff()->memory.subspan(0, handoff_ranges.size());

  if (BootOptions::Get()->phys_verbose) {
    printf("%s: Kernel VM handoff:\n", ProgramName());
    handoff()->LogVm(ProgramName());

    printf("%s: Physical memory handed off to the kernel:\n", ProgramName());
    memalloc::PrintRanges(handoff_ranges, ProgramName());
  }
}

BootOptions& HandoffPrep::SetBootOptions(const BootOptions& boot_options) {
  fbl::AllocChecker ac;
  BootOptions* handoff_options = New(boot_constants_->boot_options, ac, boot_options);
  ZX_ASSERT_MSG(ac.check(), "cannot allocate handoff BootOptions!");

  if (handoff_options->test_ram_reserve) {
    RamReservation reservation = *handoff_options->test_ram_reserve.to_std();
    FindTestRamReservation(reservation);
    handoff_options->test_ram_reserve = reservation;
  }

  return *handoff_options;
}

void HandoffPrep::PublishLog(ktl::string_view name, Log&& log) {
  if (log.empty()) {
    return;
  }

  const size_t stream_size = log.size_bytes();
  Allocation buffer = ktl::move(log).TakeBuffer();
  ZX_ASSERT(stream_size <= buffer.size_bytes());

  PublishExtraVmo(MakePhysVmo(buffer.data(), name, stream_size));

  // Intentionally leak as the PhysVmo now tracks this memory.
  ktl::ignore = buffer.release();
}

void HandoffPrep::UsePackageFiles(KernelStorage::Bootfs kernel_package) {
  auto& pool = Allocation::GetPool();
  const ktl::string_view userboot = BootOptions::Get()->userboot.data();
  for (auto it = kernel_package.begin(); it != kernel_package.end(); ++it) {
    ktl::span data = it->data;
    uintptr_t start = reinterpret_cast<uintptr_t>(data.data());
    // These are decompressed BOOTFS payloads, so there is only padding up to
    // the next page boundary.
    ktl::span aligned_data{
        data.data(), (data.size_bytes() + AddressSpace::kPageSize - 1) & -AddressSpace::kPageSize};
    if (it->name == userboot) {
      ZX_ASSERT(
          pool.UpdateRamSubranges(memalloc::Type::kUserboot, start, aligned_data.size()).is_ok());
      handoff_->userboot = MakePhysElfImage(it, it->name);
    }
    if (it->name == "version-string.txt"sv) {
      ktl::string_view version{reinterpret_cast<const char*>(data.data()), data.size()};
      SetVersionString(version);
    } else if (it->name == "vdso"sv) {
      ZX_ASSERT(pool.UpdateRamSubranges(memalloc::Type::kVdso, start, aligned_data.size()).is_ok());
      handoff_->vdso = MakePhysElfImage(it, "vdso/next"sv);
    }
  }
  if (auto result = kernel_package.take_error(); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
  }
  ZX_ASSERT_MSG(handoff_->vdso.vmar != PhysVmar{},
                "\n*** No vdso ELF file found "
                " in kernel package %.*s (VMO size %#zx) ***",
                static_cast<int>(kernel_package.directory().size()),
                kernel_package.directory().data(), handoff_->userboot.vmo.stream_size);
  ZX_ASSERT_MSG(handoff_->userboot.vmar != PhysVmar{},
                "\n*** kernel.select.userboot=%.*s but no such ELF file"
                " in kernel package %.*s (VMO size %#zx) ***",
                static_cast<int>(userboot.size()), userboot.data(),
                static_cast<int>(kernel_package.directory().size()),
                kernel_package.directory().data(), handoff_->userboot.vmo.stream_size);
  ZX_ASSERT_MSG(!boot_constants_->system_version_string.empty(),
                "no version.txt file in kernel package");
}

PhysElfImage HandoffPrep::MakePhysElfImage(KernelStorage::Bootfs::iterator file,
                                           ktl::string_view name) {
  ElfImage elf;
  if (auto result = elf.InitFromFile(file, false); result.is_error()) {
    elf.Printf(result.error_value());
    abort();
  }
  elf.set_load_address(0);

  if (auto result = elf.SeparateZeroFill(); result.is_error()) {
    elf.Printf(result.error_value());
    abort();
  }

  PhysElfImage handoff_elf = {
      .vmo = MakePhysVmo(elf.aligned_memory_image(), name, file->data.size(), true),
      .vmar = {.size = elf.vaddr_size()},
      .info = {
          .relative_entry_point = elf.entry(),
          .stack_size = elf.stack_size(),
      }};

  fbl::AllocChecker ac;
  ktl::span<PhysMapping> mappings =
      New(handoff_elf.vmar.mappings, ac, elf.load_info().segments().size());
  if (!ac.check()) {
    ZX_PANIC("cannot allocate %zu bytes of handoff space for ELF image details",
             sizeof(PhysMapping) * elf.load_info().segments().size());
  }
  elf.load_info().VisitSegments(
      [load_bias = elf.load_bias(), &mappings](const auto& segment) -> bool {
        PhysMapping& mapping = mappings.front();
        mappings = mappings.subspan(1);
        mapping = PhysMapping{
            "",
            PhysMapping::Type::kNormal,
            segment.vaddr() + load_bias,
            segment.memsz(),
            segment.filesz() == 0 ? PhysElfImage::kZeroFill : segment.offset(),
            PhysMapping::Permissions::FromSegment(segment),
        };
        return true;
      });
  ZX_DEBUG_ASSERT(mappings.empty());

  return handoff_elf;
}

[[noreturn]] void HandoffPrep::DoHandoff(UartDriver& uart, ktl::span<ktl::byte> zbi,
                                         const KernelStorage::Bootfs& kernel_package,
                                         const ArchPatchInfo& patch_info) {
  // Hand off the boot options first, which don't really change.  But keep a
  // mutable reference to update boot_options.serial later to include live
  // driver state and not just configuration like other BootOptions members do.
  BootOptions& handoff_options = SetBootOptions(*BootOptions::Get());

  // Use the updated copy from now on.
  InstallBootOptions(&handoff_options);

  UsePackageFiles(kernel_package);

  SummarizeMiscZbiItems(zbi);
  gBootTimes.SampleNow(PhysBootTimes::kZbiDone);

  SetInstrumentation();

  // This transfers the log, so logging after this is not preserved.
  // Extracting the log buffer will automatically detach it from stdout.
  // TODO(mcgrathr): Rename to physboot.log with some prefix.
  PublishLog("i/logs/physboot", ktl::move(*ktl::exchange(gLog, nullptr)));

  ZirconAbi abi = ConstructKernelAddressSpace(uart);

  // This must happen after the kernel image is mapped at its virtual address.
  SetInitArray();

  // Finalize the published VMOs (e.g., the log published just above), VMARs,
  // mappings, and physical memory ranges.  This must be called last, as this
  // finalizes the state of memory to hand off to the kernel, which is affected
  // by other set-up routines.
  FinishVm();

  // One last log before the next line where we effectively disable logging
  // altogether.
  debugf("%s: Handing off at physical load address %#" PRIxPTR ", entry %p...\n",
         gSymbolize->name(), kernel_.physical_load_address(), abi_spec_->entry);
  debugf("%s: (gdb) add-symbol-file kernel_%.*s/vmzircon %#" PRIxPTR "\n", gSymbolize->name(),
         static_cast<int>(kMachineFileName.size()), kMachineFileName.data(),
         abi_spec_->text_start.address());

  if (BootOptions::Get()->debug_boot_spin) {
    if (!abi_spec_->debug_boot_spin_ready) {
      ZX_PANIC("kernel.debug.boot-spin set with a kernel compiled without it");
    }

    uintptr_t vaddr = abi_spec_->debug_boot_spin_ready.address();
    auto query = AddressSpace::UpperPaging::Query(gAddressSpace->upper_root_paddr(),
                                                  AddressSpace::GetPageTableDirectIo, vaddr);
    ZX_ASSERT(query.is_ok());
    printf(
        "%s: Kernel will spin until `true` stored in %.*s @"
        " vaddr {{{data:%#" PRIxPTR "}}} / paddr %#" PRIx64 "\n",
        gSymbolize->name(), static_cast<int>(ZirconAbiSpec::kDebugBootSpinVariable.size()),
        ZirconAbiSpec::kDebugBootSpinVariable.data(), vaddr, query->paddr);
    printf("%s: (gdb) set *(bool*)%#" PRIxPTR " = true\n", gSymbolize->name(), vaddr);
  }

  // Hand-off the serial driver. There may be no more logging beyond this point.
  handoff()->uart = ktl::move(uart).TakeUart();

  // Now that all time samples have been collected, copy gBootTimes into the
  // hand-off.
  handoff()->times = gBootTimes;

  // Now for the remaining arch-specific settings and the actual hand-off...
  ArchDoHandoff(abi, patch_info);
}

void HandoffPrep::SetInitArray() {
  // DT_INIT should not be used, only DT_INIT_ARRAY.
  ZX_DEBUG_ASSERT(!kernel_.init_info().legacy());

  // The array collected by ElfImage points into the kernel's physical load
  // image.  Turn that into the virtual-address KernelImageSpan to hand off.
  handoff()->init_array = KernelImageSpan(kernel_.init_info().array());
}
