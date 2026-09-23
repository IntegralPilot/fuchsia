// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_ABI_H_
#define LIB_LD_REMOTE_ABI_H_

#include <set>
#include <tuple>  // std::ignore

#include "abi.h"
#include "module.h"
#include "remote-abi-heap.h"
#include "remote-abi-stub.h"
#include "remote-abi-transcriber.h"
#include "remote-load-module.h"

namespace ld {

// The RemoteAbi object handles all the details of populating the passive ABI
// for a given dynamic linking domain in a remote process.  It takes the
// RemoteAbiStub collected from the stub dynamic linker ELF file and modifies
// the RemoteLoadModule representing the instance of the stub module, based on
// a list of RemoteLoadModule objects representing the load order of modules
// going into the remote dynamic linking domain.
//
// There are only two methods: Init is called after decoding all modules but
// before laying out the remote address space; Finish is called after address
// space layout is known, to finalize the data.

template <class RemoteModule, elfldltl::ElfMachine Machine>
class RemoteAbi {
 public:
  using Elf = RemoteModule::Elf;

  using size_type = Elf::size_type;
  using Addr = Elf::Addr;
  using TlsLayout = elfldltl::TlsLayout<Elf>;

  using AbiStub = RemoteAbiStub<Elf, Machine>;
  using AbiStubPtr = AbiStub::Ptr;
  using ModuleList = RemoteModule::List;

  using LocalAbi = abi::Abi<Elf>;
  using LocalAbiModule = LocalAbi::Module;
  using LocalRDebug = Elf::template RDebug<elfldltl::LocalAbiTraits>;

  RemoteAbi() = default;

  RemoteAbi(RemoteAbi&&) = default;

  RemoteAbi& operator=(RemoteAbi&&) = default;

  // Lay out the final stub data segment given the full decoded module list.
  // Then modify the decoded stub_module (which could be a copy of the one that
  // was used to initialize abi_stub, or the same one) so its mutable segment
  // is replaced by a longer ConstantSegment.  After this, the stub_module has
  // its final vaddr_size; load addresses can be selected for all modules.
  template <class Diagnostics>
  zx::result<> Init(Diagnostics& diag, AbiStubPtr stub, RemoteModule& stub_module,
                    ModuleList& modules, size_type max_tls_modid) {
    stub_ = std::move(stub);

    RemoteAbiHeapLayout layout{stub_->data_size()};

    // The _ld_abi.loaded_modules linked-list will be populated by elements of
    // a flat array.  To simplify transcription, this array is indexed by the
    // module's symbolizer_modid, which is also its index in the ModuleList.
    abi_modules_ = layout.Allocate<AbiModule>(modules.size());

    // The TLS arrays are indexed by TLS module ID - 1.  Not every module has a
    // slot in these arrays, only those with a PT_TLS segment.
    if (max_tls_modid > 0) {
      abi_tls_modules_ = layout.Allocate<AbiTlsModule>(max_tls_modid);
      abi_tls_offsets_ = layout.Allocate<Addr>(max_tls_modid);
    }

    // The link_map.name pointer is one of the most complicated aspects of
    // transcription.  It's the only pointer in the whole (flattened) AbiModule
    // data structure that isn't just a pointer into the loaded image of that
    // module (somewhere between its vaddr_start and its vaddr_end).  It can
    // come from three different places:
    //
    //  * A dependent module's DT_NEEDED string in *that* module's DT_STRTAB.
    //    The name string is actually a pointer into the load image (DT_STRTAB)
    //    of that dependent module (the "loaded-by" module).  This is the
    //    common case: DT_NEEDED is why most modules get loaded at all.
    //
    //  * The module's own DT_SONAME string in its own DT_STRTAB.  This is like
    //    all the other pointers under AbiModule: it points into that module's
    //    own load image.  This comes up primarily for the case of a predecoded
    //    module that nothing had a DT_NEEDED for, where the module name is
    //    just set to match its DT_SONAME (the predecoded modules always have
    //    one in practice).  It can also be used just to avoid the final case.
    //
    //  * An otherwise unknown string that has to be stored somewhere.  This
    //    includes the special case of the empty string, which still needs to
    //    be a pointer to a NUL terminator in some read-only memory.  Since
    //    there's no pre-existing module load image address that's known to
    //    hold the right string, it needs space allocated in the ABI heap.
    //    (This is the use case for RemoteAbiHeapLayout::AddString and the
    //    whole RemoteAbiHeap string table implementation.)  This can only
    //    arise for a root module of a dynamic linking domain: a main
    //    executable with the implicit empty name string; or a loadable module
    //    discovered by some name that need not match its DT_SONAME (if any).
    //
    // Build a vector indexed by module, for names that need to go into the ABI
    // heap as strings.  The logic for setting the link_map.name pointer in
    // FinishAbi (below) will only look here for a module with no loaded-by and
    // whose name doesn't match its own DT_SONAME.
    auto get_heap_name = [&layout](const RemoteModule& module) -> RemoteAbiString {
      if (module.loaded_by_modid() || module.name() == module.soname()) {
        // No heap name is needed for a DT_NEEDED or DT_SONAME string.
        return {};
      }
      return layout.AddString(module.name().str());
    };
    module_heap_names_.reserve(modules.size());
    for (const RemoteModule& module : modules) {
      module_heap_names_.emplace_back(get_heap_name(module));
    }
    assert(module_heap_names_.size() == modules.size());

    auto result = AbiHeap::Create(diag, stub_->data_size(), stub_module, std::move(layout));
    if (result.is_error()) {
      return result.take_error();
    }

    heap_.emplace(*std::move(result));

    // Clear the remote _ld_abi struct before Finish is called to fill it in.
    Abi& abi = heap_->template Local<Abi>(stub_->abi_offset());
    abi = {};

    if (max_tls_modid > 0) {
      for (RemoteModule& module : modules) {
        // Do TLS layout, directly updating the remote
        // _ld_abi.static_tls_layout while setting module.static_tls_bias().
        module.AssignStaticTls(abi.static_tls_layout);

        if (module.tls_module_id() == max_tls_modid) {
          // Don't keep scanning the list if there aren't any more.
          break;
        }
      }
    }

    return zx::ok();
  }

  // After every module's load_bias() is finalized, this writes the data out.
  // This must be the last method called.  After this, the stub_module is ready
  // to be loaded into the target address space along with the other modules.
  // Note that this modifies the RemoteLoadModule objects in the list to
  // install the linked-list pointers in each one's module().link_map.
  template <class Diagnostics>
  zx::result<> Finish(Diagnostics& diag, const RemoteModule& stub_module, ModuleList& modules) && {
    Abi& abi = heap_->template Local<Abi>(stub_->abi_offset());
    if (!FinishAbi(diag, abi, modules, stub_module)) {
      // The only way this can fail is by some FromLocal call failing.  The
      // failing call is responsible for doing its own error logging, via a
      // Diagnostics object reachable from the context object.
      return zx::error{ZX_ERR_IO};
    }

    RDebug& r_debug = heap_->template Local<RDebug>(stub_->rdebug_offset());
    FillRDebug(r_debug, abi, stub_module.load_bias());

    // Write the data into the stub data segment's VMO.
    return std::move(*heap_).Commit();
  }

 private:
  using AbiHeap = RemoteAbiHeap<RemoteModule, elfldltl::RemoteAbiTraits>;
  using AbiStringPtr = AbiHeap::template AbiPtr<const char>;

  using Abi = abi::Abi<Elf, elfldltl::RemoteAbiTraits>;
  using AbiModule = Abi::Module;
  using AbiModuleSpan = AbiHeap::template AbiSpan<AbiModule>;
  using AbiTlsModule = Abi::TlsModule;
  using RDebug = Elf::template RDebug<elfldltl::RemoteAbiTraits>;
  using LinkMap = Elf::template LinkMap<elfldltl::RemoteAbiTraits>;
  using LinkMapPtr = AbiHeap::template AbiPtr<LinkMap>;
  using LocalLinkMap = Elf::template LinkMap<>;

  using AbiTranscriber = RemoteAbiTranscriber<Abi>;
  using ModuleTranscriber = RemoteAbiTranscriber<AbiModule>;

  static constexpr LocalLinkMap& LocalMap(RemoteModule& module) { return module.module().link_map; }

  // The link_map linked-list pointers are to the LocalLinkMap type.  But
  // actually that's always the LocalAbiModule::link_map member, which is
  // guaranteed to be the first member.  So it can be converted into the
  // reference to the LocalAbiModule itself, which is more useful.
  static const LocalAbiModule& LocalMapToModule(const LocalLinkMap& in) {
    return reinterpret_cast<const LocalAbiModule&>(in);
  }

  // This maps an offset into the RemoteDecodedModule::mapped_vmo() image to
  // the corresponding absolute vaddr in the remote address space where that
  // part of the module's file is mapped.
  class ModuleVaddrMap {
   public:
    // Make it move-only so the set doesn't get copied.
    ModuleVaddrMap(const ModuleVaddrMap&) = delete;
    ModuleVaddrMap(ModuleVaddrMap&&) = default;

    // Build up a set ordered by offset that gives corresponding vaddr, memsz.
    explicit ModuleVaddrMap(const RemoteModule& module) : module_(module) {
      const size_type module_bias = module_.load_bias();
      auto make_segment = [module_bias](const auto& segment) -> Segment {
        return {
            .offset = segment.offset(),
            .vaddr = segment.vaddr() + module_bias,
            .memsz = segment.memsz(),
        };
      };
      for (const auto& segment : module_.load_info().segments()) {
        segments_.insert(std::visit(make_segment, segment));
      }
    }

    const RemoteModule& module() const { return module_; }

    // Given a pointer into this module's mapped_vmo() image, yield the
    // absolute vaddr if it lies in any of this module's segments.
    template <class Diagnostics, typename T>
    std::optional<size_type> GetVaddr(Diagnostics& diag, const T& ptr) const {
      std::optional<size_type> offset = module_.decoded().mapped_vmo().GetVaddr(&ptr);
      if (!offset) [[unlikely]] {
        diag.FormatError("remote ABI transcription bug:",
                         " pointer not within mapped file for module ", DiagnosticsModuleName());
        return std::nullopt;
      }
      std::optional<size_type> vaddr = GetVaddr(*offset, sizeof(T));
      if (!vaddr) [[unlikely]] {
        diag.FormatError(
            "remote ABI transcription bug:"
            " no corresponding vaddr in module ",
            DiagnosticsModuleName(), elfldltl::FileOffset{*offset});
        return std::nullopt;
      }
      return *vaddr;
    }

   private:
    struct Segment {
      constexpr bool operator<(const Segment& other) const { return offset < other.offset; }

      constexpr bool Contains(size_type start, size_t size) const {
        return start >= offset &&                 // Not before it.
               start - offset < memsz &&          // Not after it.
               memsz - (start - offset) >= size;  // Big enough.
      }

      // Given an offset in the file image, yield the absolute vaddr if it lies
      // inside this segment.
      std::optional<size_type> GetVaddr(size_type start, size_t size) const {
        if (!Contains(start, size)) [[unlikely]] {
          return std::nullopt;
        }
        return start - offset + vaddr;
      }

      size_type offset, vaddr, memsz;
    };

    static constexpr bool Below(const Segment& segment, size_type offset) {
      return segment.offset + segment.memsz - 1 < offset;
    }

    // Given an offset in the file image, yield the absolute vaddr if it lies
    // in any of this module's segments.
    std::optional<size_type> GetVaddr(size_type offset, size_t size) const {
      auto it = std::lower_bound(segments_.begin(), segments_.end(), offset, Below);
      if (it == segments_.end()) [[unlikely]] {
        return std::nullopt;
      }
      return it->GetVaddr(offset, size);
    }

    std::string_view DiagnosticsModuleName() const {
      std::string_view name = module_.name().str();
      if (name.empty()) {
        name = "<main executable>";
      }
      return name;
    }

    const RemoteModule& module_;
    std::set<Segment> segments_;
  };

  // All of the RemoteAbiTranscriber machinery boils down to calling the
  // FromLocalPtr method for the non-null pointers in the local data
  // structures, and the MemberFromLocal method for class / struct members, on
  // one of the *TranscriberContext objects.
  //
  // Failures in the FromLocalPtr methods should be impossible, but they are
  // diagnosed gracefully just to avoid crashing the whole service process with
  // assertion failures if there is some input-dependent bug somewhere in the
  // ABI transcription logic.

  // In the ABI heap context we are dealing with a few kinds of pointers.
  template <class Diagnostics>
  struct AbiTranscriberContext {
    // The LocalAbiModule sits inside a RemoteModule.  The symbolizer module ID
    // matches the index in the RemoteModule::List, which matches the index in
    // the remote AbiModule array in the remote ABI heap.
    std::optional<Addr> FromLocalPtr(const LocalAbiModule& in) const {
      return abi_modules.subspan(in.symbolizer_modid, 1).ptr().address();
    }

    // The link_map linked-list pointers can be turned it into LocalAbiModule
    // pointers to operate on those instead.
    std::optional<Addr> FromLocalPtr(const LocalLinkMap& in) const {
      return FromLocalPtr(LocalMapToModule(in));
    }

    // All members in the heap context are treated the same.
    template <auto Member, typename MemberType, auto LocalMember, typename Local>
    bool MemberFromLocal(MemberType& out, const Local& in) const {
      return RemoteAbiTranscriber<MemberType>::FromLocal(*this, out, in.*LocalMember);
    }

    Diagnostics& diag;
    const AbiModuleSpan abi_modules;
  };

  // In this context, pointers point into the module's loaded image.
  template <class Diagnostics>
  struct ModuleTranscriberContext {
    template <typename T>
    std::optional<Addr> FromLocalPtr(const T& ptr) const {
      return vaddr_map.GetVaddr(abi.diag, ptr);
    }

    // Most members stay in the module context where the FromLocalPtr above is
    // used for any AbiPtr-typed members: they point into this module's image.
    template <auto Member, typename MemberType, auto LocalMember, typename Local>
    bool MemberFromLocal(MemberType& out, const Local& in) const {
      return RemoteAbiTranscriber<MemberType>::FromLocal(*this, out, in.*LocalMember);
    }

    // The link_map linked-list pointers are inside the AbiModule object but
    // really they are heap pointers, so they redirect back to that context.

    template <>
    bool MemberFromLocal<&LinkMap::next, LinkMapPtr, &LocalLinkMap::next, LocalLinkMap>(
        LinkMapPtr& out, const LocalLinkMap& in) const {
      return RemoteAbiTranscriber<LinkMapPtr>::FromLocal(abi, out, in.next);
    }

    template <>
    bool MemberFromLocal<&LinkMap::prev, LinkMapPtr, &LocalLinkMap::prev, LocalLinkMap>(
        LinkMapPtr& out, const LocalLinkMap& in) const {
      return RemoteAbiTranscriber<LinkMapPtr>::FromLocal(abi, out, in.prev);
    }

    // The name pointer is a special case.  It doesn't get transcribed from the
    // local pointer.  Instead it gets set directly after transcription, below.
    template <>
    bool MemberFromLocal<&LinkMap::name, AbiStringPtr, &LocalLinkMap::name, LocalLinkMap>(
        AbiStringPtr& out, const LocalLinkMap& in) const {
      return true;
    }

    const AbiTranscriberContext<Diagnostics>& abi;
    const ModuleVaddrMap& vaddr_map;
  };

  // Fill out the remote _ld_abi and the things it points to in the ABI heap.
  // The heap layout was already set up by Init by knowing the module list and
  // TLS details.  Now the stub module's load bias has been chosen so it's
  // possible to materialize pointers into the ABI heap by collecting all the
  // information from the modules.
  template <class Diagnostics>
  bool FinishAbi(Diagnostics& diag, Abi& abi, ModuleList& modules,
                 const RemoteModule& stub_module) {
    using HeapContext = AbiTranscriberContext<Diagnostics>;
    using ModuleContext = ModuleTranscriberContext<Diagnostics>;

    // Init allocated the flat abi_modules array.  Each element will correspond
    // to the element in the modules list with the same index.  This is just a
    // convenience for the layout and pointer mapping done here.  The ABI does
    // not involve an array nor any other such assumptions about individual
    // AbiModule addresses, only a doubly-linked list.  Fill in each module's
    // module().link_map linked-list pointers before transcribing its module();
    // the local pointers get translated by the ModuleTranscriber.

    assert(!modules.empty());
    assert(modules.size() == abi_modules_.size());

    std::span abi_modules = heap_->Local(abi_modules_);
    assert(modules.size() == abi_modules.size());

    std::vector<ModuleVaddrMap> vaddr_maps;
    vaddr_maps.reserve(modules.size());
    for (const RemoteModule& module : modules) {
      vaddr_maps.emplace_back(module);
    }
    assert(vaddr_maps.size() == modules.size());

    // next_module() advances these two iterators in parallel.
    auto abi_module = abi_modules.begin();
    auto vaddr_map = vaddr_maps.begin();
    auto next_module = [&abi_module, &abi_modules, &vaddr_map, &vaddr_maps]() {
      std::ignore = &vaddr_maps;  // Optimized away if NDEBUG.
      ++abi_module;
      ++vaddr_map;
      if (abi_module == abi_modules.end()) {
        assert(vaddr_map == vaddr_maps.end());
        return false;
      }
      assert(vaddr_map != vaddr_maps.end());
      return true;
    };

    const size_type heap_vaddr = AbiHeap::HeapVaddr(stub_module);
    const HeapContext abi_context = {
        .diag = diag,
        .abi_modules = heap_->Remote(heap_vaddr, abi_modules_),
    };
    assert(abi_context.abi_modules.size() == abi_modules.size());

    auto make_module_context =
        [&abi_context](const ModuleVaddrMap& map) -> ModuleTranscriberContext<Diagnostics> {
      return {.abi{abi_context}, .vaddr_map{map}};
    };

    // The link_map.name string points different places in different cases.
    // It's skipped by the ModuleTranscriber.
    auto set_link_map_name = [this, &abi_module, &vaddr_map, &vaddr_maps,
                              heap_vaddr](auto& module_context) {
      AbiStringPtr& remote_name = abi_module->link_map.name;
      const RemoteModule& module = vaddr_map->module();

      auto transcribe_name = [&remote_name](auto& ctx, const auto& soname) {
        const elfldltl::AbiPtr<const char, Elf> local_name{soname.c_str()};
        return RemoteAbiTranscriber<AbiStringPtr>::FromLocal(  //
            ctx, remote_name, local_name);
      };

      auto loaded_by_context = [&abi_context = module_context.abi, &module,
                                &vaddr_maps]() -> ModuleContext {
        const size_t idx = *module.loaded_by_modid();
        const ModuleVaddrMap& loaded_by = vaddr_maps[idx];
        return {.abi = abi_context, .vaddr_map = loaded_by};
      };

      auto no_heap_name = [this, &module]() {
        std::ignore = this;  // Optimized out if NDEBUG.
        std::ignore = &module;
        assert(module_heap_names_[module.module().symbolizer_modid].empty());
        assert(!module.name().empty());
      };

      if (module.loaded_by_modid()) {
        // The name() string points into the loaded_by module's DT_STRTAB.
        const ModuleContext name_context = loaded_by_context();
        no_heap_name();
        if (!transcribe_name(name_context, module.name())) [[unlikely]] {
          return false;
        }
      } else if (!module.name().empty() && module.name() == module.soname()) {
        // The name string can point into the module's own DT_SONAME string,
        // even if that's not where the module.name() pointer came from.
        no_heap_name();
        if (!transcribe_name(module_context, module.soname())) [[unlikely]] {
          return false;
        }
      } else {
        // This has a (possibly empty) string allocated in the ABI heap.
        const size_t idx = module.module().symbolizer_modid;
        const RemoteAbiString& heap_name = module_heap_names_[idx];
        assert(heap_name.size() == module.name().str().size());
        remote_name = heap_->Remote(heap_vaddr, heap_name).ptr();
      }

      return true;
    };

    // Set the module->module().link_map.{next,prev} pointers so they
    // correspond to the next and previous elements in the modules list.
    auto chain_module = [&modules, &vaddr_map, &vaddr_maps]() {
      const size_t idx = std::distance(vaddr_maps.begin(), vaddr_map);
      if (idx > 0) {
        LocalMap(modules[idx]).prev = &LocalMap(modules[idx - 1]);
      }
      if (idx + 1 < modules.size()) {
        LocalMap(modules[idx]).next = &LocalMap(modules[idx + 1]);
      }
    };

    // The TLS arrays get filled in only for modules with a PT_TLS.
    assert(abi_tls_modules_.size() == abi_tls_offsets_.size());
    auto do_tls = [tls_modules = heap_->Local(abi_tls_modules_),
                   tls_offsets = heap_->Local(abi_tls_offsets_)](  //
                      const ModuleContext& context) -> bool {
      const RemoteModule& module = context.vaddr_map.module();
      if (module.tls_module_id() == 0) {
        return true;
      }
      const size_t idx = module.tls_module_id() - 1;
      tls_offsets[idx] = module.static_tls_bias();
      return RemoteAbiTranscriber<AbiTlsModule>::FromLocal(  //
          context, tls_modules[idx], module.tls_module());
    };

    do {
      // Set up the linked-list pointers in the LocalAbiModule.
      chain_module();

      // Now transcribe the LocalAbiModule into the remote AbiModule.
      // link_map.name is handled specially after the main transcription.
      const auto context = make_module_context(*vaddr_map);
      const RemoteModule& module = vaddr_map->module();
      const LocalAbiModule& local_module = module.module();
      if (!ModuleTranscriber::FromLocal(context, *abi_module, local_module) ||
          !set_link_map_name(context) || !do_tls(context)) [[unlikely]] {
        return false;
      }
    } while (next_module());

    // Now that all the individual modules have been transcribed, finally
    // finish setting the remote _ld_abi members.  This could fill a LocalAbi
    // and then transcribe it, but that would just require extra machinery for
    // the few pointers it needs.  Instead, just populate the remote Abi struct
    // directly.  It was already cleared by Init, which also did TLS layout
    // by directly updating abi.static_tls_layout.

    // This is the head of the list, which is also the front of the array.
    abi.loaded_modules = abi_context.abi_modules.ptr();
    abi.loaded_modules_count = abi_context.abi_modules.size();

    // These are used directly as flat arrays.
    abi.static_tls_modules = heap_->Remote(heap_vaddr, abi_tls_modules_);
    abi.static_tls_offsets = heap_->Remote(heap_vaddr, abi_tls_offsets_);

    // When there is a main executable, it can have a DT_PREINIT_ARRAY and its
    // PT_GNU_STACK can have a p_memsz.  These aren't in every Abi::Module
    // since they're not used for most.  So they have their own Abi::stack_size
    // and Abi::preinit_array members, pointing into the executable's image.
    if (modules.front().name() == LocalAbi::kExecutableName) {
      const auto& exec_info = modules.front().decoded().exec_info();
      abi.stack_size = exec_info.stack_size.value_or(0);
      using Remote = decltype(abi.preinit_array);
      Remote& remote = abi.preinit_array;
      const auto& local = exec_info.preinit_array;
      const auto context = make_module_context(vaddr_maps.front());
      if (!RemoteAbiTranscriber<Remote>::FromLocal(context, remote, local)) {
        return false;
      }
    }

    // Since we don't use the AbiTranscriber, this lists out each member we set
    // directly in this method to ensure there will be a static_assert failure
    // if a new one is omitted.
    AbiTranscriber::template AssertAbiMembers<
        &Abi::loaded_modules, &Abi::loaded_modules_count, &Abi::stack_size, &Abi::preinit_array,
        &Abi::static_tls_modules, &Abi::static_tls_offsets, &Abi::static_tls_layout>();

    return true;
  }

  void FillRDebug(RDebug& r_debug, const Abi& abi, size_type stub_load_bias) {
    // There's isn't much to _r_debug, so just fill it out directly.
    r_debug.version = elfldltl::kRDebugVersion;
    r_debug.map = abi.loaded_modules.template Reinterpret<LinkMap>();
    r_debug.state = elfldltl::RDebugState::kConsistent;
    r_debug.ldbase = stub_load_bias;
  }

  AbiStubPtr stub_;
  std::optional<AbiHeap> heap_;
  RemoteAbiSpan<AbiModule> abi_modules_;
  RemoteAbiSpan<AbiTlsModule> abi_tls_modules_;
  RemoteAbiSpan<Addr> abi_tls_offsets_;
  std::vector<RemoteAbiString> module_heap_names_;
};

}  // namespace ld

#endif  // LIB_LD_REMOTE_ABI_H_
