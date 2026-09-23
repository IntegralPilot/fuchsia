// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ELF_IMAGE_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ELF_IMAGE_H_

#include <lib/arch/cache.h>
#include <lib/arch/paging.h>
#include <lib/code-patching/code-patching.h>
#include <lib/elfldltl/init-fini.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/note.h>
#include <lib/elfldltl/static-vector.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <lib/zbitl/items/bootfs.h>
#include <lib/zbitl/view.h>
#include <zircon/assert.h>

#include <ktl/array.h>
#include <ktl/byte.h>
#include <ktl/initializer_list.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <ktl/type_traits.h>
#include <ktl/utility.h>
#include <phys/address-space.h>

#include "allocation.h"

class ElfImage {
 public:
  static constexpr ktl::string_view kImageName = "image.elf";

  static constexpr size_t kMaxLoad = 5;  // RODATA, CODE, RELRO, DATA, BSS

  static constexpr size_t kMaxBuildIdLen = 32;

  using Elf = elfldltl::Elf<>;
  using size_type = Elf::size_type;
  using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StaticVector<kMaxLoad>::Container,
                                      elfldltl::PhdrLoadPolicy::kContiguous>;

  using BootfsDir = zbitl::BootfsView<ktl::span<ktl::byte>>;
  using Error = BootfsDir::Error;

  using PublishDebugdataFunction = fit::inline_function<ktl::span<ktl::byte>(
      ktl::string_view sink_name, ktl::string_view vmo_name, ktl::string_view suffix,
      size_t stream_size)>;

  using PrintPatchFunction = fit::inline_function<void(ktl::initializer_list<ktl::string_view>)>;

  using MapSegmentFunction = fit::inline_function<fit::result<arch::MapError>(
      uintptr_t vaddr, size_type offset, size_type filesz, size_type memsz,
      arch::AccessPermissions)>;

  // An ELF image is found at "dir/name". That can be an ELF file or a subtree.
  // The subtree should contain "image.elf", "code-patches.bin", etc.  A
  // singleton file will be treated as the image with no patches to apply.
  fit::result<Error> Init(BootfsDir dir, ktl::string_view name, bool relocated);

  // This does the same with a singleton file already located in the BootfsDir.
  fit::result<Error> InitFromFile(BootfsDir::iterator file, bool relocated);

  // This does the same with an ELF image subdirectory already located.
  fit::result<Error> InitFromDir(BootfsDir subdir, ktl::string_view name, bool relocated);

  ktl::string_view name() const { return name_; }

  LoadInfo& load_info() { return load_info_; }
  const LoadInfo& load_info() const { return load_info_; }

  elfldltl::DirectMemory& image() { return image_; }
  const elfldltl::DirectMemory& image() const { return image_; }

  uintptr_t load_bias() const {
    ZX_DEBUG_ASSERT(load_bias_);
    return *load_bias_;
  }

  bool contains_vaddr_range(Elf::size_type vaddr, Elf::size_type len) const {
    vaddr = static_cast<Elf::size_type>(vaddr - load_bias());
    return load_info_.contains_vaddr_range(vaddr, len);
  }

  // Return the memory image within the current address space. Must be called
  // after Init().
  ktl::span<const ktl::byte> memory_image() const { return image_.image(); }

  // This aligns the size up to include the page-alignment padding always
  // present in the filesystem image.
  ktl::span<const ktl::byte> aligned_memory_image() const {
    return {
        image_.image().data(),
        ZBI_BOOTFS_PAGE_ALIGN(image_.image().size_bytes()),
    };
  }

  uint64_t entry() const { return entry_; }

  // The entry point is some sort of pointer into the image.  It could be a
  // function pointer or a data pointer.  Note that when it's a function
  // pointer, the type parameter should be a _function_ (not pointer) type and
  // should include the [[clang::cfi_unchecked_callee]] attribute _in the type_
  // if it's intended!
  template <typename T>
  T* ImageEntry() const {
    return image_.GetPointer<T>(entry_);
  }

  elfldltl::ElfMachine machine() const { return machine_; }

  const elfldltl::InitFiniInfo<>& init_info() const { return init_info_; }

  ktl::optional<uint64_t> cfi_check_function() const { return cfi_check_; }

  void set_cfi_check_function(uint64_t cfi_check) { cfi_check_ = cfi_check; }

  ktl::optional<size_t> stack_size() const { return stack_size_; }

  ktl::optional<ktl::string_view> interp() const { return interp_; }

  const ktl::optional<elfldltl::ElfNote>& build_id() const { return build_id_; }

  bool has_patches() const { return !patches().empty(); }

  size_t patch_count() const { return patches().size(); }

  // The template parameter must be an `enum class Id : uint32_t` type.  Calls
  // the callback as fit::result<Error>(code_patching::Patcher&, Id,
  // ktl::span<ktl::byte>, PrintPatchFunction) for each patch in the file
  // (Print is as for ArchCodePatch in <lib/code-patching/code-patches.h>).
  // Before Load() this patches the BOOTFS file image in place.  After Load()
  // this patches the load image (which could sometimes still be using the file
  // image in place).
  template <typename Id, typename Callback>
  fit::result<Error> ForEachPatch(Callback&& callback) {
    static_assert(ktl::is_enum_v<Id>);
    static_assert(ktl::is_same_v<uint32_t, ktl::underlying_type_t<Id>>);
    static_assert(ktl::is_invocable_r_v<fit::result<Error>, Callback, code_patching::Patcher&, Id,
                                        ktl::span<ktl::byte>, PrintPatchFunction>);
    fit::result<Error> result = fit::ok();
    if (patches().empty()) {
      return result;
    }
    arch::GlobalCacheConsistencyContext cache;
    for (const code_patching::Directive& patch : patches()) {
      ktl::span<ktl::byte> bytes = GetBytesToPatch(patch);
      auto print = [this, &patch](ktl::initializer_list<ktl::string_view> strings) {
        PrintPatch(patch, strings);
      };
      result = callback(patcher_, static_cast<Id>(patch.id), bytes, print);
      if (result.is_error()) {
        break;
      }
      cache.SyncRange(reinterpret_cast<uintptr_t>(bytes.data()), bytes.size());
    }
    return result;
  }

  // Return true if the memory within the BOOTFS image for this file is
  // sufficient to be used in place as the load image.
  bool CanLoadInPlace() const {
    return load_info_.vaddr_size() <= ZBI_BOOTFS_PAGE_ALIGN(image_.image().size_bytes());
  }

  // Rewrite the load_info().segments() list after Init() so that each
  // DataWithZeroFillSegment is replaced with a separate DataSegment and
  // ZeroFillSegment.  Any partial page after the filesz is zero-filled in
  // place in the file image.
  fit::result<Error> SeparateZeroFill();

  // Load in place if possible, or else copy into a new Allocation
  // A virtual load address at which relocation is expected to occur may be
  // provided; if not, the image will be loaded within the current address
  // space.
  // The Allocation returned is null if LoadInPlace was used; otherwise, it
  // owns the memory backing the new load image and should be kept alive for
  // the lifetime of the ElfImage. In general, the returned allocation should
  // not be consulted for addresses within the load image; that is what
  // memory_image() is for.
  Allocation Load(memalloc::Type type, ktl::optional<uint64_t> relocation_address = {},
                  bool in_place_ok = true);

  size_t vaddr_size() const { return load_info_.vaddr_size(); }

  // Returns the virtual address where the image will be loaded. Must be called
  // after Load().
  uintptr_t load_address() const {
    ZX_DEBUG_ASSERT(load_bias_);
    return static_cast<uintptr_t>(load_info_.vaddr_start() + *load_bias_);
  }

  // Set the virtual address where the image will be loaded.
  // This is the address Relocate() adjusts things for.
  void set_load_address(uint64_t address) {
    ZX_ASSERT(address % AddressSpace::kPageSize == 0);
    load_bias_ = address - load_info_.vaddr_start();
  }

  // Returns the physical address where the image will be loaded. Must be
  // called after Load().
  uintptr_t physical_load_address() const {
    return reinterpret_cast<uintptr_t>(memory_image().data());
  }

  // Apply relocations to the image in place after setting the load address.
  void Relocate();

  // Maps the image at its loaded address, mapping each of its load segments
  // with appropriate access permissions (modulo the execute-only exception of
  // AddressSpace::Map()). Must be called after Load().
  fit::result<arch::MapError> MapInto(AddressSpace& aspace) const;

  // Generalized form of MapInto uses a MapSegmentFunction callback in place of
  // aspace.Map; the translation from image() offset to paddr is handled by the
  // callback.  The callback need not require Load() beforehand, if can handle
  // memsz > filesz (.bss) without adding physically contiguous space past the
  // end of the original image() of ELF file contents left by Init().
  fit::result<arch::MapError> MapInto(MapSegmentFunction map_segment) const;

  // Panic if the loaded file doesn't have a PT_INTERP matching the hex string
  // corresponding to this build ID note; the prefix is used in panic messages.
  void AssertInterpMatchesBuildId(ktl::string_view prefix, const elfldltl::ElfNote& build_id);

  // Set up state to describe the running phys executable.
  void InitSelf(ktl::string_view name, elfldltl::DirectMemory& memory, uintptr_t load_bias,
                const Elf::Phdr& load_segment, ktl::span<const ktl::byte> build_id_note);

  // This uses the symbolizer_markup::Writer API to emit the contextual
  // elements describing this ELF module.  The ID number should be unique among
  // modules in the same address space, i.e. since the last Reset() in the same
  // markup output stream.
  template <class Writer>
  Writer& SymbolizerContext(Writer& writer, unsigned int id, ktl::string_view prefix = {},
                            bool physical = false) const {
    uintptr_t address = physical ? physical_load_address() : load_address();
    return load_info_.SymbolizerContext(writer, id, name(), build_id_->desc, address, prefix);
  }

  // Publish instrumentation VMOs for this module.  The argument is similar
  // called like HandoffPrep::PublishExtraVmo, which see; PublishDebugdataFunction
  // takes different arguments to name the data, so it can be used to compose
  // either a ZBI_TYPE_DEBUGDATA payload or a named VMO.
  void PublishDebugdata(PublishDebugdataFunction publish_debugdata) const;

  // Call the image's entry point as a function type F.
  template <typename F, typename... Args>
  decltype(auto) Call(Args&&... args) const {
    static_assert(ktl::is_function_v<F>);
    return ImageEntry<F>()(ktl::forward<Args>(args)...);
  }

  // Call the image's entry point as a [[noreturn]] function type F.
  template <typename F, typename... Args>
  [[noreturn]] void Handoff(Args&&... args) const {
    static_assert(ktl::is_function_v<F>);
    Call<F>(ktl::forward<Args>(args)...);
    ZX_PANIC("ELF image entry point returned!");
  }

  // A function called by Handoff calls this on its own module to provide code
  // that must be instantiated separately in each module to publish per-module
  // instrumentation data.
  void OnHandoff() { publish_self_ = PublishSelf; }

  // Describes the file before ": " and then does printf.
  [[gnu::format(printf, 2, 3)]] void Printf(const char* fmt, ...) const;
  void Printf() const;  // Same as Printf("").
  void Printf(Error error) const;

 private:
  // PublishSelf takes one of these for each particular kind of per-module
  // instrumentation VMO supported.  The callback knows what kind of data it's
  // for and deals with calling a PublishDebugdataFunction with the right
  // arguments.  That code exists in the module where PublishDebugdata is
  // called.  PublishSelf exists separately in each module and deals with
  // filling the buffer with the right per-module data.
  using PublishSelfCallback = fit::inline_function<ktl::span<ktl::byte>(size_t stream_size)>;

  // publish_self_ is set to point to this by the module itself, so it points
  // to the instantiation of this function inside that one module.  The first
  // argument is always with the module that contains the code being called.
  static void PublishSelf(const ElfImage& module, PublishSelfCallback llvmprofdata);

  // Subroutine of PublishDebugdata only used inside elf-image-vmos.cc.
  template <const ktl::string_view& kSinkName, const ktl::string_view& kSuffix,
            const ktl::string_view& kAnnounce>
  PublishSelfCallback MakePublishSelfCallback(PublishDebugdataFunction& publish_debugdata) const;

  // This is only called from PublishSelf so it will always be the
  // instantiation inside this module.
  void PublishSelfLlvmProfdata(PublishSelfCallback publish) const;

  ktl::span<const code_patching::Directive> patches() const { return patcher_.patches(); }

  ktl::span<ktl::byte> GetBytesToPatch(const code_patching::Directive& patch);

  void PrintPatch(const code_patching::Directive& patch,
                  ktl::initializer_list<ktl::string_view> strings) const;

  ktl::string_view name_;
  ktl::string_view package_;
  elfldltl::DirectMemory image_{{}};
  LoadInfo load_info_;
  uint64_t entry_ = 0;
  elfldltl::ElfMachine machine_{};
  ktl::span<const Elf::Dyn> dynamic_;
  elfldltl::InitFiniInfo<> init_info_;
  ktl::optional<elfldltl::ElfNote> build_id_;
  ktl::optional<ktl::string_view> interp_;
  code_patching::Patcher patcher_;
  ktl::optional<uintptr_t> load_bias_;
  ktl::optional<size_type> stack_size_;
  ktl::optional<uint64_t> cfi_check_;
  decltype(PublishSelf)* publish_self_ = nullptr;
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_ELF_IMAGE_H_
