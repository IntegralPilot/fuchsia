// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_REMOTE_LOAD_MODULE_H_
#define LIB_LD_REMOTE_LOAD_MODULE_H_

#include <lib/elfldltl/loadinfo-mutable-memory.h>
#include <lib/elfldltl/resolve.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>

#include <algorithm>
#include <type_traits>
#include <vector>

#include "remote-decoded-module.h"

namespace ld {

// RemoteLoadModule is the LoadModule type used in the remote dynamic linker.
// It points to an immutable ld::RemoteDecodedModule describing the ELF file
// itself (see <lib/ld/remote-decoded-module.h>), and then has the other common
// state about the particular instance of the module, such as a name, load
// bias, and relocated data segment contents.

// This the type of the second optional template parameter to RemoteLoadModule.
// It's a flag saying whether the module is going to be used as a "zygote".  A
// fully-relocated module ready to be loaded as VMOs of relocate data.  In the
// default case, those VMOs are mutable and get directly mapped into a process
// by the Load method, where they may be mutated further via writing mappings.
// In a zygote module, those VMOs are immutable after relocation and instead
// get copy-on-write clones mapped in by Load.
enum class RemoteLoadZygote : bool { kNo = false, kYes = true };

// This is an implementation detail of RemoteLoadModule, below.
template <class Elf>
using RemoteLoadModuleBase = LoadModule<typename RemoteDecodedModule<Elf>::Ptr>;

// Also known as RemoteDynamicLinker::Module.
template <class Elf = elfldltl::Elf<>, RemoteLoadZygote Zygote = RemoteLoadZygote::kNo>
class RemoteLoadModule : public RemoteLoadModuleBase<Elf> {
 public:
  using Base = RemoteLoadModuleBase<Elf>;
  static_assert(std::is_move_constructible_v<Base>);

  // Alias useful types from Decoded and LoadModule.
  using typename Base::Addr;
  using typename Base::Decoded;
  using typename Base::LookupResult;
  using typename Base::Module;
  using typename Base::size_type;
  using typename Base::Soname;
  using ExecInfo = Decoded::ExecInfo;
  using DecodedPtr = Decoded::Ptr;
  using Sym = Elf::Sym;

  // This is the type of the module list.  The ABI remoting scheme relies on
  // this being indexable; see <lib/ld/remote-abi.h> for details.  Being able
  // to use the convenient and efficient indexable containers like std::vector
  // is the main reason RemoteLoadModule needs to be kept movable.
  using List = std::vector<RemoteLoadModule>;

  // This is the type of the callback that can optionally be given to
  // set_symbol_filter().  Usually this should be set shortly after calling
  // RemoteDynamicLinker::Init(), e.g. on one of the initial modules as
  // returned via List::iterator.
  //
  // This can optionally be set to a callable for the function signature
  // fit::result<bool, const Sym*>(const Module&, elfldltl::SymbolName&).
  // When a symbol name is looked up for relocation, each module is consulted
  // in turn until one has a definition for that symbol.  When it's this
  // module's turn, this function will be called (if set), always with the
  // Module corresponding to this InitModule (provided so the callable need
  // not capture the module reference itself).
  //
  // If it returns success, the value can be nullptr to simply say this
  // module doesn't define the symbol (not an error), or else a const Sym*
  // that must point into this decoded_module.symbol_info().symtab().
  //
  // If it returns failure, that means that relocation of the referring
  // module fails.  If the error value is false, then the whole Relocate()
  // call fails immediately.  If it's true, then relocating the referring
  // module is abandoned, but can continue to attempt relocation of other
  // modules to diagnose (or ignore) more errors before Relocate() returns.
  // Note that no Diagnostics object is passed to this function, so if it can
  // return errors then it must capture its own means of reporting details.
  //
  // A null filter (the default) means to just use the module's symbol table.
  // **NOTE:** The filter function cannot call module.Lookup()--that will just
  // recurse back into the same filter!  The way to fall back to the default
  // behavior is `return fit::ok(name.Lookup(module.symbol_info()));` (this is
  // exactly what Base::Lookup does).
  using SymbolFilter = fit::function<LookupResult(const RemoteLoadModule&, elfldltl::SymbolName&)>;

  // RemoteLoadModule has its own LoadInfo that's initially copied from the
  // RemoteDecodedModule, but then gets its own mutable segment VMOs as needed
  // for relocation (or other special-case mutation, as in the ABI remoting).
  //
  // RemoteDecodedModule::LoadInfo always uses elfldltl::SegmentWithVmo::Copy
  // to express that its per-segment VMOs (from partial-page zeroing) should
  // not be mapped writable, only cloned.  However, RemoteLoadModule::LoadInfo
  // can consume its own relocated segments when it's a RemoteLoadZygote::kNo
  // instantiation.  Only the zygote case has reason to keep the segments
  // mutated by relocation immutable thereafter by cloning them for mapping
  // into a process.  (In both cases, the RemoteDecodedModule's segments are
  // left immutable.)
  //
  // Note that the elfldltl::VmarLoader::SegmentVmo partial specializations
  // defined for elfldltl::SegmentWithVmo must exactly match the SegmentWrapper
  // template parameter of the LoadInfo instantiation.  So it's important that
  // the LoadInfo instantiation here uses one of those exactly.  Therefore,
  // this uses a template alias parameterized by the wrapper to do the
  // instantiation inside std::conditional_t directly with the SegmwntWithVmo
  // SegmentWrapper template, rather than a single instantiation with a
  // template alias that uses std::conitional_t inside Segment instantiation.
  // Both ways produce the same Segment types in the LoadInfo, but one makes
  // the partial specializations on elfldltl::VmarLoader::SegmentVmo match.

  template <template <class> class SegmentWrapper>
  using LoadInfoWithWrapper =
      elfldltl::LoadInfo<Elf, RemoteContainer, elfldltl::PhdrLoadPolicy::kBasic, SegmentWrapper>;

  using LoadInfo = std::conditional_t<                      //
      Zygote == RemoteLoadZygote::kYes,                     //
      LoadInfoWithWrapper<elfldltl::SegmentWithVmo::Copy>,  //
      LoadInfoWithWrapper<elfldltl::SegmentWithVmo::NoCopy>>;

  // RemoteDecodedModule uses elfldltl::SegmentWithVmo::AlignSegments, so the
  // loader can rely on just cloning mutable VMOs without partial-page zeroing.
  using Loader = elfldltl::AlignedRemoteVmarLoader;

  // This is the SegmentVmo type that should be used as the basis for the
  // partial specialization of elfldltl::VmarLoader::SegmentVmo, just to make
  // sure it matched the right one.
  using SegmentVmo = std::conditional_t<         //
      Zygote == RemoteLoadZygote::kYes,          //
      elfldltl::SegmentWithVmo::CopySegmentVmo,  //
      elfldltl::SegmentWithVmo::NoCopySegmentVmo>;
  static_assert(std::is_base_of_v<SegmentVmo, Loader::SegmentVmo<LoadInfo>>);

  RemoteLoadModule() = default;

  RemoteLoadModule(const RemoteLoadModule&) = delete;

  RemoteLoadModule(RemoteLoadModule&&) noexcept = default;

  RemoteLoadModule(const Soname& name, std::optional<uint32_t> loaded_by_modid)
      : Base{name}, loaded_by_modid_{loaded_by_modid} {
    static_assert(std::is_move_constructible_v<RemoteLoadModule>);
    static_assert(std::is_move_assignable_v<RemoteLoadModule>);
  }

  RemoteLoadModule& operator=(RemoteLoadModule&& other) noexcept = default;

  const DecodedPtr& decoded_module() const { return this->decoded_storage(); }

  // Set the callback used to lookup symbols in this module for relocation (of
  // itself if done before Relocate(), and of other modules relocated later).
  // The API contract for SymbolFilter is described above.
  void set_symbol_filter(SymbolFilter filter) {
    symbol_filter_ = filter ? std::move(filter) : NoFilter;
  }

  const SymbolFilter& symbol_filter() const { return symbol_filter_; }

  // Note this shadows LoadModule::module(), so module() calls in the methods
  // of class and its subclasses return module_ but module() calls in the
  // LoadModule base class return the immutable decoded().module() instead.
  // The only uses LoadModule's own methods make of module() are for the data
  // that is not specific to a particular dynamic linking session: data
  // independent of module name, load bias, and TLS and symbolizer ID numbers.
  const Module& module() const {
    assert(this->HasModule());
    return module_;
  }
  Module& module() {
    assert(this->HasModule());
    return module_;
  }

  // This is set by the set_decoded method, below.
  size_type tls_module_id() const { return module_.tls_modid; }

  // This is set by the Allocate method, below.
  size_type load_bias() const { return module_.link_map.addr; }

  // This is only set by the Relocate method, below.  Before relocation is
  // complete, consult decoded().load_info() for layout information.  The
  // difference between load_info() and decoded().load_info() is that mutable
  // segment VMOs contain relocated data specific to this RemoteLoadModule
  // where as RemoteDecodedModule only has per-segment VMOs for partial-page
  // zeroing, and those must stay immutable.
  const LoadInfo& load_info() const { return load_info_; }
  LoadInfo& load_info() { return load_info_; }

  constexpr void set_name(const Soname& name) {
    Base::set_name(name);
    SetAbiName();
  }
  constexpr void set_name(std::string_view name) {
    Base::set_name(name);
    SetAbiName();
  }

  // Return the index of other module in the list (if any) that requested this
  // one be loaded.  This means that the name() string points into that other
  // module's DT_STRTAB image.
  std::optional<uint32_t> loaded_by_modid() const { return loaded_by_modid_; }

  // Change the module ID (i.e. List index) recording which other module (if
  // any) first requested this module be loaded via DT_NEEDED.  This is
  // normally set in construction at the time of that first request, but for
  // predecoded modules it needs to be updated in place.
  void set_loaded_by_modid(std::optional<uint32_t> loaded_by_modid) {
    loaded_by_modid_ = loaded_by_modid;
  }

  // Initialize the loader and allocate the address region for the module,
  // updating the module's runtime address fields on success.  The optional
  // vmar_offset argument can pick the load address used, in terms of the
  // offset within the containing VMAR.  The kernel chooses by default.
  template <class Diagnostics>
  bool Allocate(Diagnostics& diag, const zx::vmar& vmar,
                std::optional<size_t> vmar_offset = std::nullopt) {
    assert(loader_);
    if (this->HasModule()) [[likely]] {
      loader_.emplace(vmar);
      if (!loader_->Allocate(diag, this->decoded().load_info(), vmar_offset)) {
        return false;
      }

      // The bias can actually be negative and wrap around, which is fine.
      const zx_vaddr_t bias = loader_->load_bias();
      zx_vaddr_t start = this->decoded().load_info().vaddr_start() + bias;
      zx_vaddr_t end = start + this->decoded().load_info().vaddr_size();
      module_.vaddr_start = static_cast<size_type>(start);
      module_.vaddr_end = static_cast<size_type>(end);
      if (module_.vaddr_start != start || module_.vaddr_end != end) [[unlikely]] {
        // However, for Elf32 the result must fit into 32 bits.
        return diag.SystemError("load address [", start, ", ", end,
                                ") does not fit into address space");
      }
      // Recompute the load bias with the correct bit-width for wraparound.
      module_.link_map.addr = module_.vaddr_start - this->decoded().load_info().vaddr_start();
    }
    return true;
  }

  // Before Allocate() is called, this can be used to store a chosen vaddr that
  // RemoteDynamicLinker can fetch back to compute the vmar_offset to pass to
  // Allocate().
  void Preplaced(size_type load_bias) {
    SetModuleVaddrBounds<Elf>(module_, this->decoded().load_info(), load_bias);
    assert(preplaced());
  }

  // As an alternative to calling Allocate(), instead mark this module as
  // already loaded with a known load bias.
  void Preloaded(size_type load_bias) {
    // Before Allocate(), loader_ is a default-constructed Loader, which won't
    // work.  Allocate() would reset it to one that will work.  Instead, reset
    // it std::nullopt to tell Load() to just skip this module.
    loader_ = std::nullopt;
    Preplaced(load_bias);
    assert(preloaded());
  }

  // Returns the absolute vaddr_start if Preplaced() or Preloaded() was called.
  std::optional<size_type> preplaced() const {
    if (module_.vaddr_end == 0) {
      return std::nullopt;
    }
    return module_.vaddr_start;
  }

  // Returns true if Preloaded() was called rather than Allocate().
  bool preloaded() const { return !loader_; }

  // Before relocation can mutate any segments, load_info() needs to be set up
  // with its own copies of the segments, including copy-on-write cloning any
  // per-segment VMOs that decoded() owns.  This can be done earlier if the
  // segments need to be adjusted before relocation.
  template <class Diagnostics>
  bool PrepareLoadInfo(Diagnostics& diag) {
    if (preloaded()) {
      // Later work like the ABI remoting needs load_info_ to be set up with
      // all the vaddr details, though it will never be passed to Loader::Load
      // or LoadInfoMutableMemory.  The basic LoadInfo instantiation doesn't
      // have segment VMOs, so copying into it from the RemoteDecodedModule
      // won't copy them.  Then copying back into load_info_ won't install any
      // VMO handles, but preserves all the vaddr details.
      LoadInfoWithWrapper<elfldltl::NoSegmentWrapper> basic_info;
      return basic_info.CopyFrom(diag, this->decoded().load_info()) &&
             load_info_.CopyFrom(diag, basic_info);
    }

    return !load_info_.segments().empty() ||  // Shouldn't be done twice!
           load_info_.CopyFrom(diag, this->decoded().load_info());
  }

  template <elfldltl::ElfMachine Machine, class Diagnostics, class ModuleList,
            typename TlsDescResolver>
  bool Relocate(Diagnostics& diag, ModuleList& modules, const TlsDescResolver& tls_desc_resolver) {
    if (!PrepareLoadInfo(diag)) [[unlikely]] {
      return false;
    }

    if (preloaded()) {
      // Skip relocation for a preloaded module.
      return true;
    }

    auto mutable_memory = elfldltl::LoadInfoMutableMemory{
        diag, load_info_,
        elfldltl::SegmentWithVmo::GetMutableMemory<LoadInfo>{this->decoded().vmo().borrow()}};
    if (!mutable_memory.Init()) {
      return false;
    }
    if (!elfldltl::RelocateRelative(diag, mutable_memory, this->reloc_info(), this->load_bias())) {
      return false;
    }
    auto resolver =
        elfldltl::MakeSymbolResolver(*this, modules, diag, tls_desc_resolver, ld::kResolverPolicy);
    return elfldltl::RelocateSymbolic<Machine>(mutable_memory, diag, this->reloc_info(),
                                               this->symbol_info(), this->load_bias(), resolver);
  }

  // Load the module into its allocated vaddr region.
  // This is a no-op if Prelaoded() was called instead of Allocate().
  template <class Diagnostics>
  bool Load(Diagnostics& diag) {
    return preloaded() ||  // Nothing to do if the module was preloaded.
           loader_->Load(diag, load_info_, this->decoded().vmo().borrow());
  }

  // This must be the last method called with the loader. Direct the loader to
  // preserve the load image before it is garbage collected.
  void Commit() {
    assert(this->HasModule());

    if (loader_) {
      // This returns the Loader::Relro object that holds the VMAR handle.  But
      // it's not needed because the RELRO segment was always mapped read-only.
      std::ignore = std::move(*loader_).Commit(typename LoadInfo::Region{});
    }
  }

  void set_decoded(DecodedPtr decoded, uint32_t modid, bool symbols_visible,
                   size_type& max_tls_modid) {
    Base::set_decoded(std::move(decoded));

    // Copy the passive ABI Module from the DecodedModule.  That one is the
    // source of truth for all the actual data pointers, but its members
    // related to the vaddr are using the unbiased link-time vaddr range and
    // its module ID indices are not meaningful.  We could store just the or
    // compute the members that vary in each particular dynamic linking session
    // and get the others via indirection through the const decoded() object.
    // But it's simpler just to copy, especially for the ABI remoting logic.
    // It's only a handful of pointers and integers, so it's not a lot to copy.
    module_ = this->decoded().module();

    // The RemoteDecodedModule didn't set link_map.name; it used the generic
    // modid of 0, and the generic TLS module ID of 1 if there was a PT_TLS
    // segment at all.  Set those for this particular use of the module now.
    // The rest will be set later by Allocate via ld::SetModuleVaddrBounds.
    SetAbiName();
    module_.symbolizer_modid = modid;
    if (module_.tls_modid != 0) {
      module_.tls_modid = ++max_tls_modid;
    }

    module_.symbols_visible = symbols_visible;
  }

  // This meets the Module API for elfldltl::MakeSymbolResolver, overriding the
  // LoadModule definition.
  LookupResult Lookup(auto& diag, elfldltl::SymbolName& name) const {
    return symbol_filter_(*this, name);
  }

 private:
  // This has the same default semantics as LoadModule::Lookup.
  static fit::result<bool, const Sym*> NoFilter(const RemoteLoadModule& module,
                                                elfldltl::SymbolName& name) {
    return fit::ok(name.Lookup(module.symbol_info()));
  }

  void SetAbiName() { module_.link_map.name = this->name().c_str(); }

  Module module_;
  LoadInfo load_info_;
  SymbolFilter symbol_filter_ = NoFilter;
  std::optional<Loader> loader_{std::in_place};
  std::optional<uint32_t> loaded_by_modid_;
};
static_assert(std::is_move_constructible_v<RemoteLoadModule<>>);

}  // namespace ld

#endif  // LIB_LD_REMOTE_LOAD_MODULE_H_
