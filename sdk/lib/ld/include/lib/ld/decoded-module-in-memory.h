// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_DECODED_MODULE_IN_MEMORY_H_
#define LIB_LD_DECODED_MODULE_IN_MEMORY_H_

#include <lib/elfldltl/load.h>
#include <lib/elfldltl/memory.h>
#include <lib/elfldltl/relocation.h>
#include <lib/elfldltl/soname.h>
#include <lib/elfldltl/static-vector.h>

#include "decoded-module.h"
#include "load.h"

namespace ld {

// This is a subclass of ld::DecodedModule specifically for decoding a module
// by loading it with a Loader API object (see <lib/elfldltl/mmap-loader.h> and
// <lib/elfldltl/vmar-loader.h>), and then decoding its metadata segments in
// place in the current process's own memory.
//
// The LoadFromFile method uses the Loader and File template APIs to populate
// load_info() and the vaddr / phdrs parts of module().

template <class ElfLayout = elfldltl::Elf<>,
          template <typename> class SegmentContainer =
              elfldltl::StaticVector<kMaxSegments>::Container,
          AbiModuleInline InlineModule = AbiModuleInline::kNo,
          DecodedModuleRelocInfo WithRelocInfo = DecodedModuleRelocInfo::kYes,
          template <class SegmentType> class SegmentWrapper = elfldltl::NoSegmentWrapper>
class DecodedModuleInMemory : public DecodedModule<ElfLayout, SegmentContainer, InlineModule,
                                                   WithRelocInfo, SegmentWrapper> {
 public:
  using Base =
      DecodedModule<ElfLayout, SegmentContainer, InlineModule, WithRelocInfo, SegmentWrapper>;

  using typename Base::Dyn;
  using typename Base::Ehdr;
  using typename Base::Elf;
  using typename Base::LoadInfo;
  using typename Base::Module;
  using typename Base::Phdr;
  using typename Base::size_type;
  using Region = LoadInfo::Region;

  // This is the successful return value of DecodeFromMemory, below.  Some
  // or all of these things may be useless to some callers, but the startup
  // dynamic linker uses each in some cases.
  struct DecodeResult {
    std::span<const Dyn> dynamic;  // PT_DYNAMIC contents in memory.
    size_type entry = 0;           // e_entry entry point, needs load bias.
    std::optional<size_type> stack_size;
  };

  using FixedPhdrAllocator = elfldltl::FixedArrayFromFile<Phdr, kMaxPhdrs>;

  // Set when the PT_GNU_RELRO phdr is seen by set_relro or DecodeFromMemory.
  // If no such phdr is seen, this will be empty.  It can be passed directly to
  // the Loader::Commit method.  Note these are the unadjusted module-relative
  // vaddr bounds without the load bias applied.
  constexpr const Region& relro_bounds() const { return relro_bounds_; }

  constexpr void set_relro(std::optional<Phdr> relro_phdr, size_t page_size) {
    if (relro_phdr) {
      relro_bounds_ = this->load_info().RelroBounds(*relro_phdr, page_size);
    }
  }

  // This uses Loader API and File API objects to get the file's memory image
  // set up, which populates load_info().  This must be called after
  // HasModule() is true, so it can initialize the module() fields for phdrs
  // and vaddr bounds.  It returns the elfldltl::LoadHeadersFromFile return
  // value, so the ehdr and phdrs can be examined further.  (Note that
  // module().phdrs might validly be empty, so the returned phdrs buffer should
  // be used for further decoding.)  The File object should no longer be needed
  // after this, so it can be passed as an rvalue and consumed if convenient.
  // As long as the PhdrAllocator object does not own the allocations it
  // returns, then it can be a consumed rvalue too.
  template <class Diagnostics, class Loader, class File,
            typename PhdrAllocator = FixedPhdrAllocator, class... PhdrObservers>
  constexpr auto LoadFromFile(Diagnostics& diag, Loader& loader, File&& file,
                              PhdrAllocator&& phdr_allocator = PhdrAllocator{},
                              PhdrObservers&&... phdr_observers)
      -> decltype(elfldltl::LoadHeadersFromFile<Elf>(  //
          diag, file, std::forward<PhdrAllocator>(phdr_allocator))) {
    assert(this->HasModule());

    // Read the file header and program headers into stack buffers.
    auto headers =
        elfldltl::LoadHeadersFromFile<Elf>(diag, file, std::forward<PhdrAllocator>(phdr_allocator));
    if (headers) [[likely]] {
      auto& [ehdr_owner, phdrs_owner] = *headers;
      const Ehdr& ehdr = ehdr_owner;
      const std::span<const Phdr> phdrs = phdrs_owner;

      // Decode phdrs just to fill load_info().  There will need to be another
      // pass over the phdrs after the image is mapped in, because metadata
      // segments like notes refer to data in memory and it's not there yet.
      // So everything else can wait until then.  With that, load_info() has
      // enough information to actually load the file.  Once the segments are
      // in all memory, then a metadata phdr can be decoded via its vaddr.
      if (elfldltl::DecodePhdrs(diag, phdrs, this->load_info().GetPhdrObserver(loader.page_size()),
                                std::forward<PhdrObservers>(phdr_observers)...) &&
          loader.Load(diag, this->load_info(), file.borrow())) [[likely]] {
        // Update the module to reflect the runtime vaddr bounds.
        SetModuleVaddrBounds(this->module(), this->load_info(), loader.load_bias());

        // Update the module to point to the phdrs in memory.  Note that the
        // rest of the decoding still uses the original phdrs buffer in the
        // return value just in case the phdrs aren't in the memory image.
        SetModulePhdrs(this->module(), ehdr, this->load_info(), loader.memory());

        return headers;
      }
    }

    return std::nullopt;
  }

  // After the module's image is in memory via LoadFromFile, this decodes the
  // ehdr and phdrs for everything else but the PT_LOADs for load_info(),
  // already filled by LoadFromFile.  Now module() is getting filled in with
  // direct pointers into the loaded image.  If it has a PT_TLS, max_tls_modid
  // will be incremented to set module().tls_modid.  The reloc_info() and
  // module() data is filled from the PT_DYNAMIC, but additional observer
  // objects for elfldltl::DecodeDynamic can be passed for e.g. DT_NEEDED.
  //
  // The return value's `dynamic` member points to the PT_DYNAMIC as read using
  // the Memory object; it also has the stack size request (if any) and the
  // e_entry header field without load bias adjustment applied.
  template <class Diagnostics, class Memory, class... DynamicObservers>
  [[nodiscard]] constexpr std::optional<DecodeResult> DecodeFromMemory(  //
      Diagnostics& diag, Memory&& memory, size_t page_size, const Ehdr& ehdr,
      std::span<const Phdr> phdrs, size_type& max_tls_modid,
      DynamicObservers&&... dynamic_observers) {
    auto result = DecodeModulePhdrs(  //
        diag, phdrs, PhdrMemoryBuildIdObserver(memory, this->module()));
    if (!result) [[unlikely]] {
      return std::nullopt;
    }

    auto [dyn_phdr, tls_phdr, relro_phdr, stack_size] = *result;

    // Store the PT_GNU_RELRO details so relro_bounds() can be used later.
    set_relro(relro_phdr, page_size);

    // If there was a PT_TLS, fill in tls_module() to be published later.
    if (tls_phdr) {
      this->SetTls(diag, memory, page_size, *tls_phdr, ++max_tls_modid);
    }

    auto dynamic = this->DecodeDynamic(diag, memory, dyn_phdr,
                                       std::forward<DynamicObservers>(dynamic_observers)...);

    if (dynamic.is_error()) [[unlikely]] {
      return std::nullopt;
    }

    return DecodeResult{
        .dynamic = *dynamic,
        .entry = ehdr.entry,
        .stack_size = stack_size,
    };
  }

  // This is a shorthand taking the successful return value from LoadFromFile
  // (that is, the std::optional<...>::value_type of that std::optional<...>).
  template <class Diagnostics, class Memory, class Headers, class... DynamicObservers>
  [[nodiscard]] constexpr std::optional<DecodeResult> DecodeFromMemory(  //
      Diagnostics& diag, Memory&& memory, size_t page_size, Headers&& headers,
      size_type& max_tls_modid, DynamicObservers&&... dynamic_observers) {
    auto& [ehdr, phdrs] = headers;
    return DecodeFromMemory(  //
        diag, memory, page_size, ehdr, phdrs, max_tls_modid,
        std::forward<DynamicObservers>(dynamic_observers)...);
  }

  // This consumes some Loader API object by calling its Commit method with the
  // RELRO bounds.  The caller always takes ownership of the module load image
  // mappings that the Loader previously owned.  The returned Loader::Relro
  // object can be used to protect the RELRO region or can be explicitly
  // discarded to (on Fuchsia) prevent further protection changes.
  template <class Loader>
  [[nodiscard]] constexpr auto CommitLoader(Loader loader) const {
    return std::move(loader).Commit(relro_bounds());
  }

 private:
  Region relro_bounds_;
};

}  // namespace ld

#endif  // LIB_LD_DECODED_MODULE_IN_MEMORY_H_
