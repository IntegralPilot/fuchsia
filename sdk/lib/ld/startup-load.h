// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_STARTUP_LOAD_H_
#define LIB_LD_STARTUP_LOAD_H_

#include <lib/elfldltl/dynamic.h>
#include <lib/elfldltl/fd.h>
#include <lib/elfldltl/link.h>
#include <lib/elfldltl/load.h>
#include <lib/elfldltl/relocation.h>
#include <lib/elfldltl/relro.h>
#include <lib/elfldltl/resolve.h>
#include <lib/elfldltl/self.h>
#include <lib/elfldltl/soname.h>
#include <lib/elfldltl/static-vector.h>
#include <lib/ld/decoded-module-in-memory.h>
#include <lib/ld/load-module.h>
#include <lib/ld/load.h>
#include <lib/ld/tls.h>
#include <lib/ld/tlsdesc.h>

#include <algorithm>

#include <fbl/intrusive_double_list.h>

#include "allocator.h"
#include "mutable-abi.h"
#include "startup-bootstrap.h"
#include "startup-diagnostics.h"

namespace ld {

// The startup dynamic linker always uses the default ELF layout.
using Elf = elfldltl::Elf<>;
using size_type = Elf::size_type;
using Addr = Elf::Addr;
using Addend = Elf::Addend;
using Ehdr = Elf::Ehdr;
using Phdr = Elf::Phdr;
using Sym = Elf::Sym;
using Dyn = Elf::Dyn;
using TlsDescGot = Elf::TlsDescGot<>;

// StartupLoadModule::Load returns this.
struct StartupLoadResult {
  // This is the number of DT_NEEDED entries seen.  Their strings can't be
  // decoded without a second elfldltl::DecodeDynamic scan since the first one
  // has to find DT_STRTAB and it might not be first.  But the first scan
  // counts how many entries there are, so the second scan can be
  // short-circuited rather than always doing a full O(n) scan of all entries.
  size_t needed_count = 0;

  // These are only of interest for the main executable.
  uintptr_t entry = 0;                  // Runtime entry point address.
  std::span<const Addr> preinit_array;  // DT_PREINIT_ARRAY
  std::optional<size_t> stack_size;     // Requested initial stack size.
};

inline constexpr LocalRuntimeTlsDescResolver kTlsDescResolver{};

// StartupLoadModule is the LoadModule type used in the startup dynamic linker.
// Its LoadInfo uses fixed storage bounded by kMaxSegments.  The Module is
// allocated separately using the initial-exec allocator.

using StartupLoadModuleBase = LoadModule<DecodedModuleInMemory<>>;

template <class Loader>
struct StartupLoadModule : public StartupLoadModuleBase,
                           fbl::DoublyLinkedListable<StartupLoadModule<Loader>*> {
 public:
  using List =
      fbl::DoublyLinkedList<StartupLoadModule*, fbl::DefaultObjectTag, fbl::SizeOrder::Constant>;
  using PreloadedModulesList = std::pair<List, std::span<const Dyn>>;

  using NeededCountObserver = elfldltl::DynamicTagCountObserver<Elf, elfldltl::ElfDynTag::kNeeded>;

  using PreinitObserver = elfldltl::DynamicPreinitObserver<Elf>;

  StartupLoadModule() = delete;

  StartupLoadModule(StartupLoadModule&&) = default;

  template <typename... LoaderArgs>
  explicit StartupLoadModule(const elfldltl::Soname<>& name, LoaderArgs&&... loader_args)
      : StartupLoadModuleBase{name}, loader_{std::forward<LoaderArgs>(loader_args)...} {}

  // This uses the given scratch allocator to create a new module object.
  template <class Allocator, typename... LoaderArgs>
  static StartupLoadModule* New(Diagnostics& diag, Allocator& allocator,
                                const elfldltl::Soname<>& name, LoaderArgs&&... loader_args) {
    fbl::AllocChecker ac;
    StartupLoadModule* module =
        new (allocator, ac) StartupLoadModule{name, std::forward<LoaderArgs>(loader_args)...};
    CheckAlloc(diag, ac, "temporary module data structure");
    return module;
  }

  // Read the file and use Loader::Load on it.  If at least partially
  // successful, this uses the given initial-exec allocator to set up the
  // passive ABI module in this->module().  The allocator only guarantees two
  // mutable allocations at a time, so the caller must then promptly splice it
  // into the link_map list before the next Load call allocates the next one.
  template <class Allocator, class File, class... PhdrObservers>
  [[nodiscard]] StartupLoadResult Load(Diagnostics& diag, Allocator& allocator, File&& file,
                                       uint32_t symbolizer_modid, size_type& max_tls_modid,
                                       PhdrObservers&&... phdr_observers) {
    // Diagnostics sent to diag during loading will be prefixed with the module
    // name, unless the name is empty as it is for the main executable.
    ScopedModuleDiagnostics module_diag(diag, this->name().str());

    // Allocate the Module object first.
    fbl::AllocChecker ac;
    this->NewModule(symbolizer_modid, allocator, ac);
    CheckAlloc(diag, ac, "passive ABI module");

    // All modules allocated by StartupModule are part of the initial exec set
    // and their symbols are inherently visible.
    decoded().module().symbols_visible = true;

    // Read the file header and program headers into stack buffers and map in
    // the image.  This fills in load_info() as well as the module vaddr bounds
    // and phdrs fields.  Note that module().phdrs might remain empty if the
    // phdrs aren't in the load image, so DecodeFromMemory will keep using the
    // stack copy read from the file instead.
    constexpr ld::DecodedModuleInMemory<>::FixedPhdrAllocator phdr_allocator;
    auto headers = decoded().LoadFromFile(diag, loader_, std::forward<File>(file), phdr_allocator,
                                          std::forward<PhdrObservers>(phdr_observers)...);
    if (!headers) [[unlikely]] {
      return {};
    }

    // Now that there is a Memory object to use, decode everything else.
    StartupLoadResult result;
    if (auto decode_result = decoded().DecodeFromMemory(  //
            diag, memory(), loader_.page_size(), *headers, max_tls_modid,
            NeededCountObserver(result.needed_count), PreinitObserver(result.preinit_array)))
        [[likely]] {
      // Save the span of Dyn entries for LoadDeps to scan later.  With that,
      // everything is now prepared to proceed with loading dependencies and
      // performing relocation.
      dynamic_ = decode_result->dynamic;

      // The caller may also want these fields for a main executable.
      result.entry = decode_result->entry + loader_.load_bias();
      result.stack_size = decode_result->stack_size;
    }
    return result;
  }

  // If a module is constructed manually rather than by Load, this points it at
  // its PT_DYNAMIC segment in memory.
  void set_dynamic(std::span<const Dyn> dynamic) { dynamic_ = dynamic; }

  void Relocate(Diagnostics& diag, const List& modules) {
    elfldltl::RelocateRelative(diag, memory(), reloc_info(), load_bias());
    auto resolver =
        elfldltl::MakeSymbolResolver(*this, modules, diag, kTlsDescResolver, ld::kResolverPolicy);
    elfldltl::RelocateSymbolic(memory(), diag, reloc_info(), symbol_info(), load_bias(), resolver);
  }

  // Since later failures will be fatal anyway, we can go ahead and commit the
  // mappings so the Loader destructor won't unmap the module.  Transferring
  // ownership of the mappings and ending the lifetime of the Loader object is
  // part of preparing to apply RELRO protections.  But we have no need to hold
  // onto the Loader::Relro capability any longer.
  void CommitAndProtectRelro(Diagnostics& diag) {
    std::ignore = decoded().CommitLoader(std::move(loader_)).Commit(diag);
  }

  List MakeList() {
    List list;
    list.push_back(this);
    return list;
  }

  // This is only valid until CommitAndProtectRelro() is called.
  auto& memory() { return loader_.memory(); }

  template <typename ScratchAllocator, typename InitialExecAllocator, typename GetDepFile,
            typename... LoaderArgs>
  static void LinkModules(Diagnostics& diag, ScratchAllocator& scratch,
                          InitialExecAllocator& initial_exec, StartupLoadModule* main_executable,
                          GetDepFile&& get_dep_file, StartupBootstrap& bootstrap,
                          size_t executable_needed_count, LoaderArgs&&... loader_args) {
    main_executable->decoded().module().symbols_visible = true;

    // The main executable implicitly can use static TLS and doesn't have to
    // have DF_STATIC_TLS set at link time.
    main_executable->decoded().module().symbols.set_flags(
        main_executable->module().symbols.flags() | elfldltl::ElfDynFlags::kStaticTls);

    List modules = main_executable->MakeList();
    List preloaded_modules =
        MakePreloadedList(diag, scratch, bootstrap.preloaded(), loader_args...);

    // This will be incremented by each Load() of a module that has a PT_TLS.
    size_type max_tls_modid = main_executable->tls_module_id();

    LoadDeps(diag, scratch, initial_exec, modules, preloaded_modules, executable_needed_count,
             std::forward<GetDepFile>(get_dep_file), max_tls_modid, loader_args...);
    CheckErrors(diag);

    // This assigns static TLS offsets, so it must happen before relocation.
    PopulateAbiTls(diag, initial_exec, modules, max_tls_modid);

    RelocateModules(diag, modules);
    CheckErrors(diag);

    PopulateAbiLoadedModules(modules, std::move(preloaded_modules));
    PopulateAbiRdebug(modules);

    CommitModules(diag, std::move(modules));
  }

 private:
  void Preload(Diagnostics& diag, Module& module, std::span<const Dyn> dynamic) {
    decoded().set_module(module);
    dynamic_ = dynamic;

    // Scan the phdrs to populate the LoadInfo just so it can be used for
    // things like symbolizer markup.
    elfldltl::DecodePhdrs(diag, module.phdrs.get(),
                          decoded().load_info().GetPhdrObserver(loader_.page_size()));
  }

  bool IsLoaded() const { return decoded().HasModule(); }

  template <typename Allocator, typename... LoaderArgs>
  static List MakePreloadedList(Diagnostics& diag, Allocator& allocator,
                                std::span<Bootstrap::Preloaded> modules,
                                LoaderArgs&&... loader_args) {
    List preloaded_modules;
    for (const auto& [module, dyn] : modules) {
      StartupLoadModule* m = New(diag, allocator, module.soname, loader_args...);
      m->Preload(diag, module, dyn);
      preloaded_modules.push_back(m);
    }
    return preloaded_modules;
  }

  void AddToPassiveAbi(typename List::iterator it, bool symbols_visible) {
    decoded().module().symbols_visible = symbols_visible;
    auto& ins_link_map = it->decoded().module().link_map;
    auto& this_link_map = decoded().module().link_map;
    ins_link_map.next = &this_link_map;
    this_link_map.prev = &ins_link_map;
  }

  // If `soname` is found in `preloaded_modules` it will be removed from that
  // list and pushed into `modules`, making the symbols from those modules
  // visible to the program.
  static bool FindModule(List& modules, List& preloaded_modules, const elfldltl::Soname<>& soname) {
    if (std::find(modules.begin(), modules.end(), soname) != modules.end()) {
      return true;
    }
    if (auto found = std::find(preloaded_modules.begin(), preloaded_modules.end(), soname);
        found != preloaded_modules.end()) {
      // TODO(https://fxbug.dev/42080760): Mark this preloaded_module as having it's symbols visible
      // to the program.
      modules.push_back(preloaded_modules.erase(found));
      return true;
    }
    return false;
  }

  template <typename Allocator, typename... LoaderArgs>
  void EnqueueDeps(Diagnostics& diag, Allocator& allocator, List& modules, List& preloaded_modules,
                   size_t needed_count, LoaderArgs&&... loader_args) {
    auto handle_needed = [&](std::string_view soname_str) {
      assert(needed_count > 0);
      elfldltl::Soname<> soname{soname_str};
      if (!FindModule(modules, preloaded_modules, soname)) {
        modules.push_back(New(diag, allocator, soname, loader_args...));
      }
      return --needed_count > 0;
    };

    auto observer = elfldltl::DynamicNeededObserver(symbol_info(), handle_needed);
    elfldltl::DecodeDynamic(diag, memory(), dynamic_, observer);
  }

  // `get_dep_file` is called as `std::optional<File>(std::string_view)`.
  // File must meet the requirements of a File type described in
  // lib/elfldltl/memory.h.
  template <typename ScratchAllocator, typename InitialExecAllocator, typename GetDepFile,
            typename... LoaderArgs>
  static void LoadDeps(Diagnostics& diag, ScratchAllocator& scratch,
                       InitialExecAllocator& initial_exec, List& modules, List& preloaded_modules,
                       size_t needed_count, GetDepFile&& get_dep_file, size_type& max_tls_modid,
                       LoaderArgs&&... loader_args) {
    // Note, this assumes that ModuleList iterators are not invalidated after
    // push_back(), done by `EnqueueDeps`. This is true of lists and
    // StaticVector. No assumptions are made on the validity of the end()
    // iterator, so it is checked at every iteration.
    uint32_t symbolizer_modid = 0;
    for (auto it = modules.begin(); it != modules.end(); it++) {
      const bool was_already_loaded = it->IsLoaded();
      if (was_already_loaded) {
        it->decoded().module().symbolizer_modid = symbolizer_modid++;
      } else if (auto file = get_dep_file(it->name())) {
        needed_count =
            it->Load(diag, initial_exec, *file, symbolizer_modid++, max_tls_modid).needed_count;
        assert(it->IsLoaded());
      } else {
        diag.MissingDependency(it->name().str());
        return;
      }
      // The main executable is always first in the list, so its prev is
      // already correct and adding the second module will set its next.
      if (it != modules.begin()) {
        it->AddToPassiveAbi(std::prev(it), true);
        // Referenced preloaded modules can't have DT_NEEDED, do don't bother
        // enqueuing their deps.
        if (was_already_loaded) {
          continue;
        }
      }
      it->EnqueueDeps(diag, scratch, modules, preloaded_modules, needed_count, loader_args...);
    }
  }

  static void RelocateModules(Diagnostics& diag, List& modules) {
    for (auto& module : modules) {
      ScopedModuleDiagnostics module_diag{diag, module.name().str()};
      module.Relocate(diag, modules);
      module.CommitAndProtectRelro(diag);
    }
  }

  static void CommitModules(Diagnostics& diag, List modules) {
    while (!modules.is_empty()) {
      auto* module = modules.pop_front();
      diag.report().ReportModuleLoaded(*module);
      // The `operator delete` this calls does nothing since the scratch
      // allocator doesn't support deallocation per se since the scratch
      // memory will be deallocated en masse, but this calls destructors.
      delete module;
    }
  }

  static void PopulateAbiLoadedModules(List& modules, List preloaded_modules) {
    // We want to add the remaining modules to the list. Their symbols aren't
    // visible for symbolic resolution, but the program can still use their
    // functions even with no relocations resolving to their symbols.
    // Therefore, we need to add these modules to the global module list so
    // they can still be seen by dl_iterate_phdr for unwinding purposes.  For
    // example, TLSDESC implementation code lives in the dynamic linker and
    // will be called as part of the TLS implementation without ever having a
    // DT_NEEDED on ld.so. On systems other than Fuchsia it may also be
    // possible to get code from the vDSO without an explicit DT_NEEDED, which
    // is common on Linux.
    auto last = std::prev(modules.end());
    modules.splice(modules.end(), preloaded_modules);
    for (auto next = std::next(last), end = modules.end(); next != end; last = next, next++) {
      // Assign increasing symbolizer module IDs to the preloaded module now,
      // so the ID order matches the list order.  Its module() is still mutable
      // since it's in .bss rather than coming from the InitialExecAllocator.
      next->decoded().module().symbolizer_modid = last->module().symbolizer_modid + 1;
      next->AddToPassiveAbi(last, false);
    }

    ld::mutable_abi.loaded_modules = &modules.begin()->module();
    ld::mutable_abi.loaded_modules_count = modules.size();
  }

  static void PopulateAbiRdebug(const List& modules) {
    ld::mutable_r_debug.version = elfldltl::kRDebugVersion;
    ld::mutable_r_debug.map = &modules.begin()->module().link_map;
    assert(ld::mutable_r_debug.state == elfldltl::RDebugState::kConsistent);
    ld::mutable_r_debug.ldbase = elfldltl::Self<>::LoadBias();
  }

  // The passive ABI's TlsModule structs are allocated in a contiguous array
  // indexed by TLS module ID, so they cannot be built up piecemeal in their
  // final locations.  Instead, they're stored directly in the LoadModule when
  // a module has one.  This collects all those and copies them into the
  // passive ABI's array.
  template <typename InitialExecAllocator>
  static void PopulateAbiTls(Diagnostics& diag, InitialExecAllocator& initial_exec_allocator,
                             List& modules, size_type max_tls_modid) {
    if (max_tls_modid > 0) {
      auto new_array = [&diag, &initial_exec_allocator, max_tls_modid](auto& result) {
        using T = typename std::decay_t<decltype(result.front())>;
        fbl::AllocChecker ac;
        T* array = new (initial_exec_allocator, ac) T[max_tls_modid];
        CheckAlloc(diag, ac, "passive ABI for TLS modules");
        result = {array, max_tls_modid};
      };

      std::span<TlsModule> tls_modules;
      std::span<Addr> tls_offsets;
      new_array(tls_modules);
      new_array(tls_offsets);

      for (StartupLoadModule& module : modules) {
        if (module.AssignStaticTls(mutable_abi.static_tls_layout)) {
          const size_t idx = module.tls_module_id() - 1;
          tls_modules[idx] = module.tls_module();
          tls_offsets[idx] = module.static_tls_bias();
        }

        if (module.tls_module_id() == max_tls_modid) {
          // Don't keep scanning the list if there aren't any more.
          break;
        }
      }

      mutable_abi.static_tls_modules = tls_modules;
      mutable_abi.static_tls_offsets = tls_offsets;
    }
  }

  Loader loader_;  // Must be initialized by constructor.
  std::span<const Dyn> dynamic_;
};

}  // namespace ld

#endif  // LIB_LD_STARTUP_LOAD_H_
