// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_ABI_STUB_H_
#define LIB_LD_REMOTE_ABI_STUB_H_

#include <lib/elfldltl/dwarf/cfi-entry.h>
#include <lib/elfldltl/dwarf/eh-frame-hdr.h>
#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/symbol.h>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "abi.h"
#include "remote-decoded-module.h"
#include "tlsdesc.h"

namespace ld {

inline constexpr std::string_view kRemoteAbiStubPrefix = "ld-stub-";
inline constexpr std::string_view kRemoteAbiStubSuffix = ".so";

// In the remote dynamic linking case, the passive ABI is anchored by the "stub
// dynamic linker" (ld-stub.so in the build).  This shared library gets loaded
// as an implicit dependency to stand in for the in-process startup dynamic
// linker that's usually there at runtime.  Aside from the normal metadata and
// a tiny code segment containing TLSDESC runtime hook code, it consists of a
// final ZeroFillSegment or DataWithZeroFillSegment that holds space for the
// passive ABI symbols.
//
// The RemoteAbiStub object collects data about the layout and symbols in the
// stub dynamic linker as decoded into a RemoteDecodedModule.  The Init()
// method examines the module and records its layout, and stores the module
// RefPtr for later use.  The RemoteAbiStub object can be reused or copied as
// long as the same stub dynamic linker ELF file (or verbatim copy) is being
// used.
//
// This object is used by the RemoteAbiHeap to modify a RemoteLoadModule for
// the specific instantiation of the stub dynamic linker for a particular
// remote dynamic linking domain (or zygote thereof).  The decoded_module()
// pointer can be used to create that RemoteLoadModule.
//
// RemoteAbiStub also collects a set of TLSDESC runtime entry point addresses.
// These are not encoded as symbols, but obscured inside the .eh_frame table.
// They are self-identifying to this code by a private protocol with the
// assembly code that defines them (see <lib/ld/tlsdesc.h>), but in a way that
// will be harmlessly ignored by other consumers of the stub dynamic linker's
// .eh_frame CFI encoding such as runtime unwinders or debuggers.  Users of the
// RemoteAbiStub::tlsdesc_runtime methods should stay tightly coupled to the
// TLSDESC runtime assembly code that's described in <lib/ld/tlsdesc.h> so that
// the stub dynamic linker used at runtime came from the same ld library source
// tree as this class.

template <class Elf = elfldltl::Elf<>, elfldltl::ElfMachine Machine = elfldltl::ElfMachine::kNative>
class RemoteAbiStub : public fbl::RefCounted<RemoteAbiStub<Elf>> {
 public:
  // Once created, the RemoteAbiStub only needs to be used as const.
  using Ptr = fbl::RefPtr<const RemoteAbiStub>;

  using size_type = Elf::size_type;
  using Addr = Elf::Addr;
  using Phdr = Elf::Phdr;
  using Sym = Elf::Sym;
  using RemoteModule = RemoteDecodedModule<Elf>;
  using RemoteModulePtr = RemoteModule::Ptr;
  using LocalAbi = abi::Abi<Elf>;
  using TlsDescResolver = ld::StaticTlsDescResolver<Elf, Machine>;
  using TlsdescRuntimeHooks = TlsDescResolver::RuntimeHooks;

  RemoteAbiStub() = default;
  RemoteAbiStub(const RemoteAbiStub&) = default;
  RemoteAbiStub(RemoteAbiStub&&) = default;

  // This is the file name (without directory) where the stub dynamic linker
  // file is usually installed in the remote dynamic linker's own package,
  // usually under its lib/.
  static constexpr auto& kFilename =
      Elf::template kFilename<kRemoteAbiStubPrefix, Machine, kRemoteAbiStubSuffix>;

  // This is the ld::RemoteDecodedModule for the stub dynamic
  // linker, the same pointer that was given to Create().
  const RemoteModulePtr& decoded_module() const { return decoded_module_; }

  // Return the module-relative vaddr of the data segment.
  size_type data_vaddr() const {
    const auto& load_info = decoded_module_->load_info();
    size_type vaddr;
    load_info.VisitSegment(
        [&vaddr](const auto& segment) -> bool {
          vaddr = segment.vaddr();
          return true;
        },
        load_info.segments().back());
    return vaddr;
  }

  // Exact size of the data segment.  This includes whatever file data gives it
  // a page-aligned starting vaddr, but the total size is not page-aligned.
  // Space directly after this can be used to lengthen the segment.
  size_type data_size() const { return data_size_; }

  // Offset into the segment where _ld_abi sits.
  size_type abi_offset() const { return abi_offset_; }

  // Offset into the segment where _r_debug sits.
  size_type rdebug_offset() const { return rdebug_offset_; }

  // Return the module-relative vaddr of _ld_abi.
  size_type abi_vaddr() const { return data_vaddr() + abi_offset_; }

  // Return the module-relative vaddr of _r_debug.
  size_type rdebug_vaddr() const { return data_vaddr() + rdebug_offset_; }

  // Module-relative addresses of the TLSDESC runtime entry points.
  const TlsdescRuntimeHooks& tlsdesc_runtime() const { return tlsdesc_runtime_; }

  // Fetch a particular TLSDESC runtime entry point.
  Addr tlsdesc_runtime(TlsdescRuntime hook) const {
    return tlsdesc_runtime_[static_cast<size_t>(hook)];
  }

  // Return a TLSDESC resolver callable object that can be passed to
  // elfldltl::MakeSymbolResolver for static TLS layouts.  It will use entry
  // points in the stub dynamic linker acquired by Init, applying the given
  // load bias for where the stub dynamic linker is to be loaded.
  TlsDescResolver tls_desc_resolver(size_type stub_load_bias) const {
    return TlsDescResolver{tlsdesc_runtime_, stub_load_bias};
  }

  // Create() calculates and records all those values by examining the stub
  // dynamic linker previously decoded.  The module pointer is saved for later
  // use via decoded_module(), below.  Note that if the Diagnostics object says
  // to keep going, this may return with partial information both in the
  // RemoteAbiStub and in the decoded_module().
  template <class Diagnostics>
  static Ptr Create(Diagnostics& diag, RemoteModulePtr ld_stub) {
    auto abi_stub = fbl::MakeRefCounted<RemoteAbiStub>();
    abi_stub->decoded_module_ = std::move(ld_stub);
    if (!abi_stub->Init(diag)) {
      abi_stub.reset();
    }
    return abi_stub;
  }

  // This just rolls in RemoteModule::Create with the Create above.
  template <class Diagnostics>
  static Ptr Create(Diagnostics& diag, zx::vmo ld_stub_vmo, size_type page_size) {
    Ptr result;
    if (RemoteModulePtr ld_stub = RemoteModule::Create(diag, std::move(ld_stub_vmo), page_size)) {
      result = Create(diag, std::move(ld_stub));
    }
    return result;
  }

 private:
  using Abi = abi::Abi<Elf, elfldltl::RemoteAbiTraits>;
  using RDebug = Elf::template RDebug<elfldltl::RemoteAbiTraits>;

  using EhFrameHdr = elfldltl::dwarf::EhFrameHdr<Elf>;

  template <class Diagnostics>
  bool Init(Diagnostics& diag) {
    if (decoded_module_->load_info().segments().empty()) [[unlikely]] {
      return diag.FormatError("stub ", LocalAbi::kSoname.str(), " has no segments");
    }

    // Find the unrounded vaddr size.  The PT_LOADs are in ascending vaddr
    // order, so the last one will be the highest addressed, while the module's
    // vaddr_start will correspond to the first one's vaddr.
    const Phdr* eh_frame_hdr = nullptr;
    for (auto it = decoded_module_->module().phdrs.rbegin();
         (data_size_ == 0 || !eh_frame_hdr) &&  // Bail early when done.
         it != decoded_module_->module().phdrs.rend();
         ++it) {
      switch (it->type()) {
        case elfldltl::ElfPhdrType::kLoad:
          if (data_size_ == 0) {
            data_size_ = it->vaddr + it->memsz;
          }
          break;
        case elfldltl::ElfPhdrType::kEhFrameHdr:
          if (!eh_frame_hdr) {
            eh_frame_hdr = &*it;
          }
          break;
        default:
          break;
      }
    }
    assert(decoded_module_->load_info().VisitSegment(
        [this](const auto& segment) {
          return segment.vaddr() <= data_size_ && segment.vaddr() + segment.memsz() >= data_size_;
        },
        decoded_module_->load_info().segments().back()));
    size_type stub_data_vaddr;
    decoded_module_->load_info().VisitSegment(
        [&stub_data_vaddr](const auto& segment) -> std::true_type {
          stub_data_vaddr = segment.vaddr();
          return {};
        },
        decoded_module_->load_info().segments().back());
    data_size_ -= stub_data_vaddr;

    auto get_offset = [this, &diag, stub_data_vaddr](  //
                          size_type& offset, const elfldltl::SymbolName& name,
                          size_t size) -> bool {
      const Sym* symbol = name.Lookup(decoded_module_->module().symbols);
      if (!symbol) [[unlikely]] {
        return diag.FormatError("stub ", LocalAbi::kSoname.str(), " does not define ", name,
                                " symbol");
      }
      if (symbol->size != size) [[unlikely]] {
        return diag.FormatError("stub ", LocalAbi::kSoname.str(), " ", name, " symbol size ",
                                symbol->size, " != expected ", size);
      }
      if (symbol->value < stub_data_vaddr || symbol->value - stub_data_vaddr > data_size_ - size)
          [[unlikely]] {
        return diag.FormatError("stub ", LocalAbi::kSoname.str(), " ", name,
                                elfldltl::FileAddress{symbol->value}, " can't fit ", size,
                                " bytes in ", data_size_, "-byte data segment",
                                elfldltl::FileAddress{stub_data_vaddr});
      }
      offset = symbol->value - stub_data_vaddr;
      return true;
    };

    constexpr auto no_overlap = [](size_type start1, size_type len1, size_type start2,
                                   size_type len2) -> bool {
      return start1 >= start2 + len2 || start2 >= start1 + len1;
    };

    return get_offset(abi_offset_, abi::kAbiSymbol, sizeof(Abi)) &&
           get_offset(rdebug_offset_, abi::kRDebugSymbol, sizeof(RDebug)) &&
           (no_overlap(abi_offset_, sizeof(Abi), rdebug_offset_, sizeof(RDebug)) ||
            diag.FormatError("stub ", LocalAbi::kSoname.str(), " symbols overlap!")) &&
           FindTlsdescRuntime(diag, eh_frame_hdr, *decoded_module_);
  }

  // In lieu of symbols that would be directly visible to users, the stub
  // dynamic linker unofficially "exports" its TLSDESC entry point addresses
  // via breadcrumbs in the .eh_frame CFI.  There is one FDE for each entry
  // point function (each is small and contiguous), where the FDE starts at the
  // entry point.  Each FDE sets its LSDA pointer (via .cfi_lsda in the
  // assembly code via the .tlsdesc.lsda assembly macro in <lib/lfd/tlsdesc.h>)
  // even though there is no personality routine.  The LSDA has no real purpose
  // in the absence of a personality routine, so unwinders will just decode it
  // and ignore it since there is no personality routine to pass it to.
  //
  // This takes the PT_GNU_EH_FRAME phdr and decodes all the FDEs in the stub,
  // checking each for a magic number in its LSDA that corresponds to one of
  // the <lib/ld/tlsdesc.h> entry point functions.
  template <class Diagnostics>
  bool FindTlsdescRuntime(Diagnostics& diag, const Phdr* phdr, const RemoteModule& ld_stub) {
    using namespace std::string_view_literals;

    if (!phdr) [[unlikely]] {
      return diag.FormatError("stub "sv, LocalAbi::kSoname.str(),
                              " missing PT_GNU_EH_FRAME program header"sv);
    }

    auto memory = decoded_module_->metadata_memory();
    if (memory.base() != 0) [[unlikely]] {
      return diag.FormatError("stub "sv, LocalAbi::kSoname.str(), " has base address ",
                              memory.base());
    }

    EhFrameHdr eh_frame_hdr;
    if (!eh_frame_hdr.Init(diag, memory, *phdr, " in stub "sv, LocalAbi::kSoname.str()))
        [[unlikely]] {
      return false;
    }

    if (eh_frame_hdr.size() != kTlsdescRuntimeCount &&
        !diag.FormatWarning(  //
            "stub "sv, LocalAbi::kSoname.str(), " PT_GNU_EH_FRAME table"sv,
            elfldltl::FileAddress{phdr->vaddr}, " has "sv, eh_frame_hdr.size(),
            " FDEs != expected "sv, kTlsdescRuntimeCount)) [[unlikely]] {
      return false;
    }

    for (auto [pc, fde_vaddr] : eh_frame_hdr) {
      std::optional<elfldltl::dwarf::CfiEntry> fde =
          elfldltl::dwarf::CfiEntry::ReadEhFrameFromMemory<Elf>(
              diag, memory, fde_vaddr, " in stub "sv, LocalAbi::kSoname.str(),
              " PT_GNU_EH_FRAME table"sv);
      if (!fde) [[unlikely]] {
        return false;
      }
      std::optional<elfldltl::dwarf::CfiEntry> cie = fde->template ReadEhFrameCieFromMemory<Elf>(
          diag, memory, " for FDE"sv, elfldltl::FileAddress{fde_vaddr}, " in stub "sv,
          LocalAbi::kSoname.str(), " PT_GNU_EH_FRAME table"sv);
      if (!cie) [[unlikely]] {
        return false;
      }
      std::optional<elfldltl::dwarf::CfiCie> cie_info =
          cie->DecodeCie<Elf>(diag, fde->cie_pointer().value_or(0));
      if (!cie_info) [[unlikely]] {
        return false;
      }
      std::optional<elfldltl::dwarf::CfiFde> fde_info =
          fde->DecodeFde<Elf>(diag, fde_vaddr, *cie_info);
      if (!fde_info) [[unlikely]] {
        return false;
      }

      auto found_hook = TlsdescRuntimeFromMagic(fde_info->lsda);
      if (!found_hook) [[unlikely]] {
        if (!diag.FormatError("stub "sv, LocalAbi::kSoname.str(),
                              " .eh_frame FDE has unrecognized LSDA value "sv, fde_info->lsda,
                              elfldltl::FileAddress{pc})) {
          return false;
        }
        continue;
      }
      Addr& hook_slot = tlsdesc_runtime_[static_cast<size_t>(*found_hook)];
      if (hook_slot != 0 &&  // A valid PC was already stored for this hook.
          !diag.FormatError("stub "sv, LocalAbi::kSoname.str(), " .eh_frame FDE"sv,
                            elfldltl::FileAddress{fde_vaddr}, " with same magic LSDA value "sv,
                            fde_info->lsda, elfldltl::FileAddress{hook_slot}, " and"sv,
                            elfldltl::FileAddress{pc})) {
        return false;
      }
      hook_slot = pc;
      if (hook_slot == 0 &&  // Zero isn't plausible for a hook PC.
          !diag.FormatError("stub "sv, LocalAbi::kSoname.str(), " .eh_frame FDE"sv,
                            elfldltl::FileAddress{pc}, " with magic LSDA value "sv, fde_info->lsda,
                            " has zero PC!"sv)) {
        return false;
      }
    }

    return std::none_of(tlsdesc_runtime_.begin(), tlsdesc_runtime_.end(),
                        [](Addr hook) { return hook == 0; }) ||
           diag.FormatError("stub "sv, LocalAbi::kSoname.str(),
                            " .eh_frame is missing some FDEs with expected"
                            " magic LSDA values for TLSDESC entry points"sv);
  }

  RemoteModulePtr decoded_module_;
  size_type abi_offset_ = 0;
  size_type rdebug_offset_ = 0;
  size_type data_size_ = 0;
  TlsdescRuntimeHooks tlsdesc_runtime_ = {};
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_ABI_STUB_H_
