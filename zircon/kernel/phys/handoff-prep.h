// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_HANDOFF_PREP_H_
#define ZIRCON_KERNEL_PHYS_HANDOFF_PREP_H_

#include <lib/fit/function.h>
#include <lib/trivial-allocator/basic-leaky-allocator.h>
#include <lib/trivial-allocator/new.h>
#include <lib/trivial-allocator/page-allocator.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_single_list.h>
#include <ktl/algorithm.h>
#include <ktl/byte.h>
#include <ktl/concepts.h>
#include <ktl/initializer_list.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/tuple.h>
#include <ktl/utility.h>
#include <phys/address-space.h>
#include <phys/elf-image.h>
#include <phys/handoff-ptr.h>
#include <phys/handoff.h>
#include <phys/kernel-package.h>
#include <phys/new.h>
#include <phys/uart.h>
#include <phys/zbitl-allocation.h>
#include <phys/zircon-abi-spec.h>

struct ArchPatchInfo;
struct BootOptions;
class PhysBootTimes;
class ElfImage;
class Log;

class HandoffPrep {
 public:
  explicit HandoffPrep(ElfImage kernel);

  // This is the main structure.
  PhysHandoff* handoff() { return handoff_; }

  // This returns new T(args...) using the temporary handoff allocator and
  // fills in the handoff_ptr to point to it.
  template <typename T, PhysHandoffPtrLifetime Lifetime, typename... Args>
    requires(Lifetime != PhysHandoffPtrLifetime::kKernelImage)
  T* New(PhysHandoffPtr<const T, Lifetime>& handoff_ptr, fbl::AllocChecker& ac, Args&&... args) {
    T* ptr;
    if constexpr (Lifetime == PhysHandoffPtrLifetime::kTemporary) {
      ptr = new (temporary_data_allocator_, ac) T(ktl::forward<Args>(args)...);
    } else {
      ptr = new (permanent_data_allocator_, ac) T(ktl::forward<Args>(args)...);
    }
    if (ptr) {
      handoff_ptr.ptr_ = ptr;
    }
    return ptr;
  }

  // Similar but for new T[n] using spans instead of pointers.
  template <typename T, PhysHandoffPtrLifetime Lifetime>
    requires(Lifetime != PhysHandoffPtrLifetime::kKernelImage)
  ktl::span<T> New(PhysHandoffSpan<const T, Lifetime>& handoff_span, fbl::AllocChecker& ac,
                   size_t n) {
    ZX_DEBUG_ASSERT(n > 0);
    T* ptr;
    if constexpr (Lifetime == PhysHandoffPtrLifetime::kTemporary) {
      ptr = new (temporary_data_allocator_, ac) T[n];
    } else {
      ptr = new (permanent_data_allocator_, ac) T[n];
    }
    if (ptr) {
      handoff_span.ptr_.ptr_ = ptr;
      handoff_span.size_ = n;
      return {ptr, n};
    }
    return {};
  }

  template <PhysHandoffPtrLifetime Lifetime>
    requires(Lifetime != PhysHandoffPtrLifetime::kKernelImage)
  ktl::string_view New(PhysHandoffString<Lifetime>& handoff_string, fbl::AllocChecker& ac,
                       ktl::string_view str) {
    ZX_DEBUG_ASSERT(!str.empty());
    ktl::span chars = New(handoff_string, ac, str.size());
    ZX_DEBUG_ASSERT(chars.size() == str.size());
    return {chars.data(), str.copy(chars.data(), chars.size())};
  }

  // Translate a pointer into the kernel's ElfImage::image() contents to its
  // kernel virtual address.
  template <typename T>
  PhysHandoffKernelImagePtr<T> KernelImagePtr(const T* kernel_data) {
    if (kernel_data) {
      return KernelImageSpan({&kernel_data, 1}).ptr_;
    }
    return {};
  }

  template <typename T>
  PhysHandoffKernelImageSpan<const T> KernelImageSpan(ktl::span<const T> kernel_data) {
    if (kernel_data.empty()) {
      return {};
    }
    const ktl::optional image_vaddr = kernel_.image().GetVaddr(kernel_data);
    ZX_DEBUG_ASSERT_MSG(image_vaddr, "[%p, %p) not in kernel_.image() [%p, %p)\n",
                        kernel_data.data(), kernel_data.data() + kernel_data.size(),
                        kernel_.image().image().data(),
                        kernel_.image().image().data() + kernel_.image().image().size());
    const uintptr_t kernel_vaddr = *image_vaddr + kernel_.load_bias();
    PhysHandoffKernelImageSpan<const T> result;
    result.ptr_.ptr_ = reinterpret_cast<const T*>(kernel_vaddr);
    result.size_ = kernel_data.size();
    return result;
  }

  PhysHandoffKernelImageString KernelImageString(ktl::string_view kernel_str) {
    PhysHandoffKernelImageString result;
    static_cast<PhysHandoffKernelImageString::Base&>(result) =
        KernelImageSpan(ktl::span(kernel_str));
    return result;
  }

  // This does all the main work of preparing for the kernel, and then calls
  // `boot` to transfer control to the kernel entry point with the handoff()
  // pointer as its argument. The `boot` function should do nothing but hand
  // off to the kernel; in particular, state has already been captured from
  // `uart` so no additional printing should be done at this stage.
  [[noreturn]] void DoHandoff(UartDriver& uart, ktl::span<ktl::byte> zbi,
                              const KernelStorage::Bootfs& kernel_package,
                              const ArchPatchInfo& patch_info);

  // Add an additonal, generic VMO to be simply published to userland.  The
  // kernel proper won't ever look at it.
  void PublishExtraVmo(PhysVmo&& vmo);

 private:
  struct ZirconAbi {
    uintptr_t machine_stack_top = 0;
    uintptr_t shadow_call_stack_base = 0;
    uintptr_t thread_abi_pointer = 0;
  };

  // Comprises a list in scratch memory of the pending VM objects so they can
  // be packed into a single array at the end (via NewFromList()).
  template <ktl::derived_from<PhysVmObject> VmObject>
  struct HandoffVmObject : public fbl::SinglyLinkedListable<HandoffVmObject<VmObject>*> {
    static HandoffVmObject* New(VmObject obj) {
      fbl::AllocChecker ac;
      HandoffVmObject* handoff_obj =
          new (gPhysNew<memalloc::Type::kPhysScratch>, ac) HandoffVmObject;
      ZX_ASSERT_MSG(ac.check(), "cannot allocate %zu scratch bytes for hand-off VM object",
                    sizeof(*handoff_obj));
      handoff_obj->object = ktl::move(obj);
      return handoff_obj;
    }

    VmObject object;
  };

  template <typename VmObject>
  using HandoffVmObjectList = fbl::SizedSinglyLinkedList<HandoffVmObject<VmObject>*>;

  using HandoffVmo = HandoffVmObject<PhysVmo>;
  using HandoffVmoList = HandoffVmObjectList<PhysVmo>;

  using HandoffVmar = HandoffVmObject<PhysVmar>;
  using HandoffVmarList = HandoffVmObjectList<PhysVmar>;

  using HandoffMapping = HandoffVmObject<PhysMapping>;
  using HandoffMappingList = HandoffVmObjectList<PhysMapping>;

  // Defined in handoff-prep-vm.cc.
  class VirtualAddressAllocator {
   public:
    enum class Strategy : bool { kDown, kUp };

    // The allocator for temporary hand-off data.
    static VirtualAddressAllocator TemporaryHandoffDataAllocator(const ElfImage& kernel,
                                                                 const ZirconAbiSpec& abi_spec);

    // The allocator for permanent hand-off data.
    static VirtualAddressAllocator PermanentHandoffDataAllocator(const ElfImage& kernel);

    // The allocator for first-class hand-off mappings (i.e., for important,
    // one-off things, likely to be packaged in their own VMARs).
    static VirtualAddressAllocator FirstClassMappingAllocator(const ElfImage& kernel,
                                                              const ZirconAbiSpec& abi_spec);

    constexpr VirtualAddressAllocator(uintptr_t start, Strategy strategy,
                                      ktl::optional<uintptr_t> boundary = ktl::nullopt)
        : start_{start}, strategy_{strategy}, boundary_{boundary} {
      if (boundary) {
        switch (strategy) {
          case Strategy::kDown:
            ZX_DEBUG_ASSERT(start >= *boundary);
            break;
          case Strategy::kUp:
            ZX_DEBUG_ASSERT(start <= *boundary);
            break;
        }
      }
    }

    // The default-constructed allocator is invalid and cannot be used for
    // allocation.
    constexpr VirtualAddressAllocator()
        :  // Paramteters are arbitrary but are chosen to ensure invaliditity.
          VirtualAddressAllocator(0, Strategy::kUp, 0) {}

    // Declares the allocator as done, ensuring no further allocations may be
    // made. Returns the end address of its allocations.
    constexpr uint64_t Finish() {
      boundary_ = start_;
      return start_;
    }

    // Allocates the given number virtual pages in bytes.
    uintptr_t AllocatePages(size_t size_bytes);

   private:
    uintptr_t start_;
    Strategy strategy_;
    ktl::optional<uintptr_t> boundary_;
  };

  template <memalloc::Type Type>
  class PhysPages {
   public:
    struct Capability {
      Allocation phys;
      void* virt;
    };

    template <typename... Args>
    explicit PhysPages(Args&&... args) : va_allocator_(ktl::forward<Args>(args)...) {}

    // This can only be called once and no more allocations can be made after
    // it's been called.
    HandoffMappingList TakeMappings() {
      ZX_DEBUG_ASSERT(mappings_);
      return *ktl::exchange(mappings_, ktl::nullopt);
    }

    // This reports the size the TakeMappings() return value will have if it's
    // called before any more allocations are made.
    size_t CountMappings() const {
      ZX_DEBUG_ASSERT(mappings_);
      return mappings_->size();
    }

    size_t page_size() const { return AddressSpace::kPageSize; }

    [[nodiscard]] ktl::pair<void*, Capability> Allocate(size_t size) {
      ZX_DEBUG_ASSERT(mappings_);

      fbl::AllocChecker ac;
      Allocation pages = Allocation::New(ac, Type, size, AddressSpace::kPageSize);
      if (!ac.check()) {
        return {};
      }

      const char* mapping_name;
      if constexpr (Type == memalloc::Type::kTemporaryPhysHandoff) {
        mapping_name = "temporary hand-off data";
      } else {
        static_assert(Type == memalloc::Type::kPermanentPhysHandoff);
        mapping_name = "permanent hand-off data";
      }

      uintptr_t vaddr = va_allocator_.AllocatePages(size);
      uintptr_t paddr = reinterpret_cast<uintptr_t>(pages.get());
      const PhysMapping mapping(mapping_name, PhysMapping::Type::kNormal, vaddr, size, paddr,
                                PhysMapping::Permissions::Rw());
      ApplyMapping(mapping);
      mappings_->push_front(HandoffMapping::New(mapping));

      void* ptr = reinterpret_cast<void*>(vaddr);
      return {ptr, Capability{ktl::move(pages), ptr}};
    }

    void Deallocate(Capability allocation, void* ptr, size_t size) {
      ZX_DEBUG_ASSERT(ptr == allocation.virt);
      ZX_DEBUG_ASSERT(size == allocation.phys.size_bytes());
      // Note: `allocation.phys` will free itself when it goes out of scope
      // on returning.
    }

    void Release(Capability allocation, void* ptr, size_t size) {
      ZX_DEBUG_ASSERT(ptr == allocation.virt);
      ZX_DEBUG_ASSERT(size == allocation.phys.size_bytes());
      ktl::ignore = allocation.phys.release();
    }

    void Seal(Capability, void*, size_t) { ZX_PANIC("Unexpected call to Seal::Capability"); }

   private:
    VirtualAddressAllocator va_allocator_;
    ktl::optional<HandoffMappingList> mappings_{ktl::in_place};
  };

  template <memalloc::Type Type>
  using PageAllocationFunction = trivial_allocator::PageAllocator<PhysPages<Type>>;

  template <memalloc::Type Type>
  using Allocator = trivial_allocator::BasicLeakyAllocator<PageAllocationFunction<Type>>;

  using TemporaryDataAllocator = Allocator<memalloc::Type::kTemporaryPhysHandoff>;
  using PermanentDataAllocator = Allocator<memalloc::Type::kPermanentPhysHandoff>;

  // A convenience class for building up a PhysVmar.
  class PhysVmarPrep {
   public:
    constexpr PhysVmarPrep() = default;
    PhysVmarPrep(const PhysVmarPrep&) = delete;
    PhysVmarPrep(PhysVmarPrep&&) = default;

    // Creates the provided mapping and publishes it within the associated VMAR
    // being built up.
    void PublishMapping(PhysMapping mapping) {
      ZX_DEBUG_ASSERT(vmar_.base <= mapping.vaddr);
      ZX_DEBUG_ASSERT(mapping.vaddr_end() <= vmar_.end());
      ApplyMapping(mapping);
      mappings_.push_front(HandoffMapping::New(ktl::move(mapping)));
    }

    // Publishes the PhysVmar in the hand-off.
    void Publish() && {
      ZX_DEBUG_ASSERT(!mappings_.is_empty());
      ZX_DEBUG_ASSERT(prep_->vmars_);
      prep_->NewFromList(vmar_.mappings, ktl::move(mappings_));
      prep_->vmars_->push_front(HandoffVmar::New(ktl::move(vmar_)));
      prep_ = nullptr;
    }

   private:
    friend class HandoffPrep;

    HandoffPrep* prep_ = nullptr;
    PhysVmar vmar_;
    HandoffMappingList mappings_;
  };

  struct Debugdata {
    ktl::string_view announce, sink_name, vmo_name;
    size_t size_bytes = 0;
  };

  // Constructs a PhysVmo from the provided information, enforcing that `data`
  // is page-aligned and that page-rounding `stream_size` up yields
  // `data.size_bytes()`.  If `known_zero` is true, then the tail of `data`
  // past `stream_size` is already known to be all zeros; otherwise zeroes it.
  static PhysVmo MakePhysVmo(ktl::span<const ktl::byte> data, ktl::string_view name,
                             size_t stream_size, bool known_zero = false);

  static void ApplyMapping(const PhysMapping& mapping);

  // Packs a list of pending VM objects into a single hand-off span in sorted
  // order.
  template <size_t Extra = 0, bool Sorted = true, typename VmObject>
  ktl::span<VmObject> NewFromList(PhysHandoffTemporarySpan<const VmObject>& span,
                                  HandoffVmObjectList<VmObject> list) {
    fbl::AllocChecker ac;
    ktl::span storage = New(span, ac, list.size() + Extra);
    ZX_ASSERT_MSG(ac.check(), "cannot allocate %zu * %zu-byte VM object span", list.size() + Extra,
                  sizeof(VmObject));
    ZX_DEBUG_ASSERT(storage.size() == list.size() + Extra);
    ktl::span objects = storage.subspan(0, list.size());

    for (VmObject& obj : objects) {
      obj = ktl::move(list.pop_front()->object);
    }
    ZX_DEBUG_ASSERT(list.is_empty());

    if constexpr (Sorted) {
      // It's useful to normalize VM object order (e.g., on base address for
      // PhysVmars) for more readable kernel start-up logging.
      ktl::ranges::sort(objects);
    }

    // Return the whole array, not just the filled prefix (if Extra > 0).
    return storage;
  }

  void SaveForMexec(const zbi_header_t& header, ktl::span<const ktl::byte> payload);

  // The arch-specific protocol for a given item.
  // Defined in //zircon/kernel/arch/$cpu/phys/arch-handoff-prep-zbi.cc.
  void ArchSummarizeMiscZbiItem(const zbi_header_t& header, ktl::span<const ktl::byte> payload);

  // Fills in handoff()->boot_options and returns the mutable reference to
  // update its fields later so that `.serial` can be transferred last.
  BootOptions& SetBootOptions(const BootOptions& boot_options);

  // Fetch things to be handed off from other files in the kernel package.
  void UsePackageFiles(KernelStorage::Bootfs kernel_package);
  void SetVersionString(ktl::string_view version);

  // Summarizes the provided data ZBI's miscellaneous simple items for the
  // kernel, filling in corresponding handoff()->item fields.  Certain fields,
  // may be cleaned after consumption for security considerations, such as
  // 'ZBI_TYPE_SECURE_ENTROPY'.
  void SummarizeMiscZbiItems(ktl::span<ktl::byte> zbi);

  // Add physboot's own instrumentation data to the handoff.  After this, the
  // live instrumented physboot code is updating the handoff data directly up
  // through the very last compiled basic block that jumps into the kernel.
  // This calls PublishExtraVmo, so it must come before FinishExtraVmos.
  void SetInstrumentation();

  // Do PublishExtraVmo with a Log buffer, which is consumed.
  void PublishLog(ktl::string_view vmo_name, Log&& log);

  // Constructs a prep object for publishing a PhysVmar.
  PhysVmarPrep PrepareVmarAt(ktl::string_view name, uintptr_t base, size_t size) {
    PhysVmarPrep prep;
    prep.prep_ = this;
    prep.vmar_ = PhysVmar{.base = base, .size = size};
    prep.vmar_.set_name(name);
    return prep;
  }

  // Publishes a PhysVmar with a single mapping covering its extent, returning
  // its mapped virtual address range.
  void PublishSingleMappingVmar(PhysMapping mapping);

  // A variation that assumes a first-class mapping and allocates and the
  // virtual addresses itself. The provided address range may be
  // non-page-aligned, in which the virtual mapping of [addr, addr + size) is
  // returned directly rather than with page alignment.
  ktl::span<ktl::byte> PublishSingleMappingVmar(ktl::string_view name, PhysMapping::Type type,
                                                uintptr_t addr, size_t size,
                                                PhysMapping::Permissions perms);

  // A specialization for an MMIO range.
  MappedMmioRange PublishSingleMmioMappingVmar(ktl::string_view name, uintptr_t addr, size_t size) {
    ktl::span vaddr_range = PublishSingleMappingVmar(name, PhysMapping::Type::kMmio, addr, size,
                                                     PhysMapping::Permissions::Rw());
    MappedMmioRange result;
    result.ptr_.ptr_ = vaddr_range.data();
    result.size_ = vaddr_range.size_bytes();
    result.paddr_ = addr;
    return result;
  }

  // A specialization for a non-MMIO physical range.
  PhysHandoffPhysicalSpan<ktl::byte> PublishSingleWritableDataMappingVmar(ktl::string_view name,
                                                                          uintptr_t addr,
                                                                          size_t size) {
    return FromPhysical(PublishSingleMappingVmar(name, PhysMapping::Type::kNormal, addr, size,
                                                 PhysMapping::Permissions::Rw()));
  }

  ktl::span<ktl::byte> PublishStackVmar(ZirconAbiSpec::Stack stack, memalloc::Type type);

  // This constructs a PhysElfImage from an ELF file in the KernelStorage.
  PhysElfImage MakePhysElfImage(KernelStorage::Bootfs::iterator file, ktl::string_view name);

  // Do final handoff of all VM and physical memory state.  This finalizes all
  // the the VM object lists; their contents are already in place so this does
  // not invalidate any pointers to the objects (e.g., from PublishExtraVmo).
  // This also normalizes and publishes RAM and the allocations of interest to
  // the kernel.  After this call, no more VMOs or mappings can be made and the
  // handoff memory allocators can no longer be used.  This must be the very
  // last set-up routine called within DoHandoff().
  void FinishVm();

  // Constructs and populates the kernel's address space, and returns the
  // mapped realizations of its ABI requirements per abi_spec_.
  ZirconAbi ConstructKernelAddressSpace(const UartDriver& uart);
  void ArchConstructKernelAddressSpace();  // The arch-specific subroutine

  // Sets handoff()->init_array after ConstructKernelAddressSpace() is done.
  void SetInitArray();

  // Finalizes handoff_.arch and performs the final, architecture-specific
  // subroutine of DoHandoff().
  //
  // This call intends to hand off - and thus either explicitly set or
  // explicitly clear to zero - all of the aspects of machine state that
  // constitute the C++ ABI, but to leave the rest of the machine state (like
  // exception handlers) in the ambient phys state until the kernel is on its
  // feet far enough to reset all that stuff for itself.
  //
  // Note that by the time this has been called the UART driver has been taken
  // and no more logging is permitted.
  [[noreturn]] void ArchDoHandoff(ZirconAbi abi, const ArchPatchInfo& patch_info);

  auto kernel_virtual_entry() const { return abi_spec_->entry; }

  template <typename T>
  T* InKernelImage(const PhysHandoffKernelImagePtr<T>& ptr) const;

  template <typename T, typename... Args>
    requires(ktl::constructible_from<T, Args...> && ktl::is_trivially_destructible_v<T>)
  T* NewInKernelImage(const PhysHandoffKernelImagePtr<const T>& ptr, Args&&... args) const;

  template <typename T>
  PhysHandoffPhysicalSpan<T> FromPhysical(ktl::span<T> span) {
    PhysHandoffPhysicalPtr<T> ptr;
    ptr.ptr_ = span.data();
    return {ktl::move(ptr), span.size()};
  }

  const ElfImage kernel_;
  PhysHandoff* handoff_ = nullptr;
  const ZirconAbiSpec* abi_spec_ = nullptr;
  BootConstants* boot_constants_ = nullptr;
  TemporaryDataAllocator temporary_data_allocator_;
  PermanentDataAllocator permanent_data_allocator_;
  VirtualAddressAllocator first_class_mapping_allocator_;
  zbitl::Image<Allocation> mexec_image_;
  ktl::optional<HandoffVmarList> vmars_{ktl::in_place};
  ktl::optional<HandoffVmoList> extra_vmos_{ktl::in_place};
};

#endif  // ZIRCON_KERNEL_PHYS_HANDOFF_PREP_H_
