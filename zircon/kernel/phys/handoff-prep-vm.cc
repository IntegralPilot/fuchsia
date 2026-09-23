// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/arch/paging.h>
#include <lib/boot-options/boot-options.h>
#include <lib/elfldltl/machine.h>
#include <lib/memalloc/range.h>
#include <zircon/tls.h>

#include <fbl/algorithm.h>
#include <ktl/algorithm.h>
#include <ktl/bit.h>
#include <ktl/inplace_vector.h>
#include <ktl/iterator.h>
#include <ktl/string_view.h>
#include <phys/address-space.h>
#include <phys/allocation.h>
#include <phys/arch/heap.h>
#include <phys/elf-image.h>
#include <region-alloc/region.h>

#include "handoff-prep.h"

#include <ktl/enforce.h>

//
// At a high-level, the kernel virtual address space is constructed as
// follows (taking the kernel's load address as an input):
//
// * The physmap is a fixed mapping below the rest of the others
// * Virtual addresses for temporary and permanent hand-off data are
//   bump-allocated downward (in 1GiB-separated ranges) below the kernel's
//   memory image (wherever it was loaded)
// * The remaining, various first-class mappings are made just above the
//   kernel's memory image (wherever it was loaded), with virtual address
//   ranges bump-allocated upward.
//
// ------------------------- KERNEL_ASPACE_BASE + KERNEL_ASPACE_SIZE
//            ...
//     other first-class     (↑)
//          mappings
// -------------------------
//       hole (1 page)
// -------------------------
//    kernel memory image
// -------------------------
//       hole (1 page)
// -------------------------
//  permanent hand-off data  (↓)
// ------------------------- kernel load address - 1GiB
//  temporary hand-off data  (↓)
//            ...
// ------------------------- KERNEL_ASPACE_BASE + PhysmapSize()
//          physmap
// ------------------------- KERNEL_ASPACE_BASE
//

namespace {

constexpr size_t k1MiB = 0x0010'0000;
constexpr size_t k1GiB = 0x4000'0000;

constexpr bool IsPageAligned(uintptr_t p) { return p % AddressSpace::kPageSize == 0; }
constexpr uintptr_t PageAlignDown(uintptr_t p) {
  return fbl::round_down(p, AddressSpace::kPageSize);
}
constexpr uintptr_t PageAlignUp(uintptr_t p) { return fbl::round_up(p, AddressSpace::kPageSize); }

constexpr arch::AccessPermissions ToAccessPermissions(PhysMapping::Permissions perms) {
  return {
      .readable = perms.readable(),
      .writable = perms.writable(),
      .executable = perms.executable(),
  };
}

uint64_t PhysmapSize() {
  // Find the highest RAM address, which gives the size of the physmap given
  // the nature of the mapping.
  auto last_ram = ktl::prev(Allocation::GetPool().end());
  while (!memalloc::IsRamType(last_ram->type)) {  // There can't not be any RAM
    --last_ram;
  }
  return PageAlignUp(last_ram->end());
}

}  // namespace

HandoffPrep::VirtualAddressAllocator
HandoffPrep::VirtualAddressAllocator::TemporaryHandoffDataAllocator(const ElfImage& kernel,
                                                                    const ZirconAbiSpec& abi_spec) {
  return {
      /*start=*/kernel.load_address() - k1GiB,
      /*strategy=*/HandoffPrep::VirtualAddressAllocator::Strategy::kDown,
      /*boundary=*/abi_spec.kernel_aspace_base + PhysmapSize(),
  };
}

HandoffPrep::VirtualAddressAllocator
HandoffPrep::VirtualAddressAllocator::PermanentHandoffDataAllocator(const ElfImage& kernel) {
  return {
      /*start=*/kernel.load_address() - AddressSpace::kPageSize,
      /*strategy=*/HandoffPrep::VirtualAddressAllocator::Strategy::kDown,
      /*boundary=*/kernel.load_address() - k1GiB,
  };
}

HandoffPrep::VirtualAddressAllocator
HandoffPrep::VirtualAddressAllocator::FirstClassMappingAllocator(const ElfImage& kernel,
                                                                 const ZirconAbiSpec& abi_spec) {
  // A boundary of ktl::nullopt in this case is equivalent to base + size
  // exceeding UINT64_MAX.
  ktl::optional<uintptr_t> boundary;
  if (abi_spec.kernel_aspace_size - 1 <
      ktl::numeric_limits<uint64_t>::max() - abi_spec.kernel_aspace_base) {
    boundary = abi_spec.kernel_aspace_base + abi_spec.kernel_aspace_size;
  }
  return {
      /*start=*/kernel.load_address() + kernel.aligned_memory_image().size_bytes() +
          AddressSpace::kPageSize,
      /*strategy=*/HandoffPrep::VirtualAddressAllocator::Strategy::kUp,
      /*boundary=*/boundary,
  };
}

uintptr_t HandoffPrep::VirtualAddressAllocator::AllocatePages(size_t size_bytes) {
  ZX_DEBUG_ASSERT(IsPageAligned(size_bytes));
  switch (strategy_) {
    case Strategy::kDown:
      ZX_DEBUG_ASSERT(start_ >= size_bytes);
      if (boundary_) {
        ZX_DEBUG_ASSERT(start_ - size_bytes >= *boundary_);
      }
      start_ -= size_bytes;
      return start_;

    case Strategy::kUp:
      if (boundary_) {
        ZX_DEBUG_ASSERT(size_bytes <= *boundary_);
        ZX_DEBUG_ASSERT(start_ <= *boundary_ - size_bytes);
      }
      return ktl::exchange(start_, start_ + size_bytes);
  }
  __UNREACHABLE;
}

void HandoffPrep::ApplyMapping(const PhysMapping& mapping) {
  ZX_DEBUG_ASSERT(mapping.vaddr >= AddressSpace::kUpperVirtualAddressRangeStart);
  ZX_DEBUG_ASSERT(mapping.size > 0);
  ZX_DEBUG_ASSERT(mapping.size - 1 <= ktl::numeric_limits<uint64_t>::max() - mapping.vaddr);

  AddressSpace::MapSettings settings;
  switch (mapping.type) {
    case PhysMapping::Type::kNormal: {
      arch::AccessPermissions access = ToAccessPermissions(mapping.perms);
      settings = AddressSpace::NormalMapSettings(access);
      break;
    }
    case PhysMapping::Type::kMmio:
      ZX_DEBUG_ASSERT(mapping.perms.readable());
      ZX_DEBUG_ASSERT(mapping.perms.writable());
      ZX_DEBUG_ASSERT(!mapping.perms.executable());
      settings = AddressSpace::MmioMapSettings();
      break;
  }
  AddressSpace::PanicIfError(
      gAddressSpace->Map(mapping.vaddr, mapping.size, mapping.paddr, settings));
}

void HandoffPrep::PublishSingleMappingVmar(PhysMapping mapping) {
  PhysVmarPrep prep =
      PrepareVmarAt(ktl::string_view{mapping.name.data()}, mapping.vaddr, mapping.size);
  prep.PublishMapping(mapping);
  ktl::move(prep).Publish();
}

ktl::span<ktl::byte> HandoffPrep::PublishSingleMappingVmar(  //
    ktl::string_view name, PhysMapping::Type type, uintptr_t addr, size_t size,
    PhysMapping::Permissions perms) {
  uint64_t aligned_paddr = PageAlignDown(addr);
  uint64_t aligned_size = PageAlignUp(size + (addr - aligned_paddr));

  // TODO(https://fxbug.dev/379891035): Revisit if kasan_shadow = true is the
  // right default for the mappings created with this utility.
  PhysMapping mapping{
      name,                                                        //
      type,                                                        //
      first_class_mapping_allocator_.AllocatePages(aligned_size),  //
      aligned_size,                                                //
      aligned_paddr,                                               //
      perms,                                                       //
  };
  PublishSingleMappingVmar(mapping);
  ktl::span aligned{reinterpret_cast<ktl::byte*>(mapping.vaddr), mapping.size};
  return aligned.subspan(addr - aligned_paddr, size);
}

ktl::span<ktl::byte> HandoffPrep::PublishStackVmar(ZirconAbiSpec::Stack stack,
                                                   memalloc::Type type) {
  ktl::string_view name = memalloc::ToString(type);

  ZX_DEBUG_ASSERT_MSG(IsPageAligned(stack.size_bytes), "%.*s size (%#x) is not page-aligned",
                      static_cast<int>(name.size()), name.data(), stack.size_bytes);
  ZX_DEBUG_ASSERT_MSG(IsPageAligned(stack.lower_guard_size_bytes),
                      "%.*s lower guard size (%#x) is not page-aligned",
                      static_cast<int>(name.size()), name.data(), stack.size_bytes);
  ZX_DEBUG_ASSERT_MSG(IsPageAligned(stack.upper_guard_size_bytes),
                      "%.*s upper guard size (%#x) is not page-aligned",
                      static_cast<int>(name.size()), name.data(), stack.size_bytes);

  size_t vmar_size = stack.lower_guard_size_bytes + stack.size_bytes + stack.upper_guard_size_bytes;
  uintptr_t base = first_class_mapping_allocator_.AllocatePages(vmar_size);
  auto prep = PrepareVmarAt(name, base, vmar_size);
  uint64_t paddr =
      Allocation::GetPool().Allocate(type, stack.size_bytes, AddressSpace::kPageSize).value();
  PhysMapping mapping{
      name,                                 //
      PhysMapping::Type::kNormal,           //
      base + stack.lower_guard_size_bytes,  //
      stack.size_bytes,                     //
      paddr,                                //
      PhysMapping::Permissions::Rw(),       //
  };
  prep.PublishMapping(mapping);
  ktl::span<ktl::byte> mapped{
      reinterpret_cast<ktl::byte*>(mapping.vaddr),
      mapping.size,
  };
  memset(mapped.data(), 0, mapped.size_bytes());
  ktl::move(prep).Publish();
  return mapped;
}

HandoffPrep::ZirconAbi HandoffPrep::ConstructKernelAddressSpace(const UartDriver& uart) {
  const memalloc::Pool& pool = Allocation::GetPool();
  ZirconAbi abi{};

  ktl::inplace_vector<ralloc_region_t, 1> mmio_deny;

  // Physmap.
  {
    const uint64_t base = abi_spec_->kernel_aspace_base;
    const size_t size = PhysmapSize();
    PhysVmarPrep prep = PrepareVmarAt("physmap"sv, base, size);
    auto map = [base, &prep](const memalloc::Range& range) {
      uint64_t aligned_paddr = PageAlignDown(range.addr);
      uint64_t aligned_size = PageAlignUp(range.size + (range.addr - aligned_paddr));

      // Shadowing the physmap would be redundantly wasteful.
      PhysMapping mapping("RAM"sv, PhysMapping::Type::kNormal, base + aligned_paddr, aligned_size,
                          aligned_paddr, PhysMapping::Permissions::Rw(),
                          /*kasan_shadow=*/false);
      prep.PublishMapping(mapping);
      return true;
    };
    memalloc::NormalizeRam(pool, map);
    ktl::move(prep).Publish();

    handoff_->physmap_base = base;
    handoff_->physmap_size = size;
  }

  // The kernel's mapping.
  {
    using size_type = ElfImage::size_type;

    PhysVmarPrep prep = PrepareVmarAt("kernel"sv, kernel_.load_address(), kernel_.vaddr_size());

    // Publish one contiguous vaddr->paddr mapping.
    auto publish_mapping = [&prep](  //
                               uint64_t vaddr, uint64_t memsz, uint64_t paddr,
                               PhysMapping::Permissions perms) {
      PhysMapping mapping({}, PhysMapping::Type::kNormal, vaddr, memsz, paddr, perms);
      // Use a compact name as the vaddr will take most of the available space.
      snprintf(mapping.name.data(), mapping.name.size(), "%c%c%c@%#" PRIxPTR,
               perms.readable() ? 'r' : '-', perms.writable() ? 'w' : '-',
               perms.executable() ? 'x' : '-', vaddr);
      prep.PublishMapping(mapping);
    };

    // The whole contiguous kernel image has been set up by Load(), including
    // all the zero-fill (.bss), even past the original size of the ELF file.
    ZX_DEBUG_ASSERT_MSG(kernel_.aligned_memory_image().size_bytes() >= kernel_.vaddr_size(),
                        ": %#zx vs %#zx", kernel_.aligned_memory_image().size_bytes(),
                        kernel_.vaddr_size());
    auto result = kernel_.MapInto([this, publish_mapping](                   //
                                      uintptr_t vaddr, size_type offset,     //
                                      size_type filesz, size_type memsz,     //
                                      arch::AccessPermissions access_perms)  //
                                  -> fit::result<AddressSpace::MapError> {
      // If the segment is executable and the hardware doesn't support
      // executable-only mappings, we fix up the permissions as also readable.
      constexpr bool kReadIfExecute = !AddressSpace::kExecuteOnlyAllowed;
      publish_mapping(  //
          vaddr, memsz, kernel_.physical_load_address() + offset,
          PhysMapping::Permissions{}
              .set_readable(access_perms.readable ||  //
                            (access_perms.executable && kReadIfExecute))
              .set_writable(access_perms.writable)
              .set_executable(access_perms.executable));
      return fit::ok();
    });
    ZX_ASSERT(result.is_ok());
    ktl::move(prep).Publish();
  }

  // Kernel ABI
  {
    // Machine stack.
    ktl::span machine_stack =
        PublishStackVmar(abi_spec_->machine_stack, memalloc::Type::kBootMachineStack);
    abi.machine_stack_top = elfldltl::AbiTraits<>::InitialStackPointer(
        reinterpret_cast<uintptr_t>(machine_stack.data()), machine_stack.size_bytes());

    // Shadow call stack.
    if (abi_spec_->shadow_call_stack.size_bytes > 0) {
      ktl::span shadow_call_stack =
          PublishStackVmar(abi_spec_->shadow_call_stack, memalloc::Type::kBootShadowCallStack);
      abi.shadow_call_stack_base = reinterpret_cast<uintptr_t>(shadow_call_stack.data());
    }

    // Thread ABI pointer.
    //
    // Static assertions of ArchTempThreadAbi fields respecting expected
    // ZX_TLS_*_OFFSETs done in <phys/arch/arch-handoff.h>.
    PhysHandoffTemporaryPtr<const ArchTempThreadAbi> handoff_thread_abi;
    fbl::AllocChecker ac;
    ArchTempThreadAbi* thread_abi = New(handoff_thread_abi, ac);
    ZX_ASSERT(ac.check());
    abi.thread_abi_pointer = reinterpret_cast<uintptr_t>(thread_abi->tp());

    // We expect the kernel properly set the stack guard, the value we set here
    // only being a debugging convenience until that time.
    //
    // TODO(https://fxbug.dev/42098994): But this ought to be set properly here
    // once physboot has access to a source of randomness.
    thread_abi->stack_guard = 0xdeadbeeffeedface;

    // Unsafe stack.
    if (abi_spec_->unsafe_stack.size_bytes > 0) {
      ktl::span unsafe_stack =
          PublishStackVmar(abi_spec_->unsafe_stack, memalloc::Type::kBootUnsafeStack);
      uintptr_t unsafe_stack_top =
          reinterpret_cast<uintptr_t>(unsafe_stack.data()) + unsafe_stack.size_bytes();
      thread_abi->unsafe_stack_pointer = unsafe_stack_top;
    }
  }

  // Periphmap
  {
    auto periph_filter = [](memalloc::Type type) -> ktl::optional<memalloc::Type> {
      return type == memalloc::Type::kPeripheral ? ktl::make_optional(type) : ktl::nullopt;
    };

    // Count the number of peripheral ranges...
    size_t count = 0;
    {
      auto count_ranges = [&count](const memalloc::Range& range) {
        ZX_DEBUG_ASSERT(range.type == memalloc::Type::kPeripheral);
        ++count;
        return true;
      };
      memalloc::NormalizeRanges(pool, count_ranges, periph_filter);
    }

    // ...so that we can allocate the number of such mappings in the hand-off.
    if (count > 0) {
      fbl::AllocChecker ac;
      ktl::span periph_ranges = New(handoff_->periph_ranges, ac, count);
      ZX_ASSERT(ac.check());

      auto map = [this, &periph_ranges](const memalloc::Range& range) {
        ZX_DEBUG_ASSERT(range.type == memalloc::Type::kPeripheral);
        periph_ranges.front() = PublishSingleMmioMappingVmar("periphmap"sv, range.addr, range.size);
        periph_ranges = periph_ranges.last(periph_ranges.size() - 1);
        return true;
      };
      memalloc::NormalizeRanges(pool, map, periph_filter);
    }
  }

  // UART.
  uart.Visit([&]<typename KernelDriver>(const KernelDriver& driver) {
    if constexpr (uart::MmioDriver<typename KernelDriver::uart_type>) {
      uart::MmioRange mmio = driver.mmio_range();
      handoff_->uart_mmio = PublishSingleMmioMappingVmar("UART"sv, mmio.address, mmio.size);
    }
  });

  // NVRAM
  {
    auto nvram = ktl::find_if(pool.begin(), pool.end(), [](const memalloc::Range& range) {
      return range.type == memalloc::Type::kNvram;
    });
    if (nvram != pool.end()) {
      handoff_->nvram = PublishSingleWritableDataMappingVmar("NVRAM"sv, nvram->addr, nvram->size);
      mmio_deny.push_back({.base = nvram->addr, .size = nvram->size});
    }
  }

  // Construct the arch-specific bits at the end (to give the non-arch-specific
  // placements in the address space a small amount of relative familiarity).
  ArchConstructKernelAddressSpace();

  // All first-class mappings in the kernel address space have been made. We
  // mark the allocator as done and base the starting address of the kernel's
  // virtual heap on where the first-class virtual range allocations ended.
  [[maybe_unused]] uintptr_t allocated_end = first_class_mapping_allocator_.Finish();

  // The kernel's virtual heap (if enabled).
  if (BootOptions::Get()->enable_virtual_heap) {
    constexpr size_t kHeapAlignment = 1u << kArchHeapAlignmentBits;
    handoff_->heap_vmar = PhysVmar{
        .base = fbl::round_up(allocated_end, kHeapAlignment),
        .size = fbl::round_up(BootOptions::Get()->heap_max_size_mb * k1MiB, kHeapAlignment)};
    handoff_->heap_vmar->set_name("heap"sv);
  }

  // If any mmio_deny regions were collected, transfer them over now.
  if (!mmio_deny.empty()) {
    fbl::AllocChecker ac;
    ktl::span handoff_deny = New(handoff_->mmio_deny, ac, mmio_deny.size());
    ZX_ASSERT(ac.check());
    ZX_DEBUG_ASSERT(handoff_deny.size() == mmio_deny.size());
    ktl::ranges::copy(mmio_deny, handoff_deny.begin());
  }

  return abi;
}
