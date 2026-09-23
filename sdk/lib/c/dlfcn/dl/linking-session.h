// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_LINKING_SESSION_H_
#define LIB_C_DLFCN_DL_LINKING_SESSION_H_

#include <lib/elfldltl/init-fini.h>
#include <lib/elfldltl/resolve.h>
#include <lib/fit/result.h>
#include <lib/ld/decoded-module-in-memory.h>
#include <lib/ld/load-module.h>

#include <ranges>

#include "concat-view.h"
#include "diagnostics.h"
#include "runtime-module.h"
#include "tls-desc-resolver.h"

namespace dl {

using size_type = Elf::size_type;

class RuntimeDynamicLinker;  // runtime-dynamic-linker.h

// A LinkingSession encapsulates the decoding, loading, relocation and creation
// of RuntimeModules from a single dlopen call.  A LinkingSession instance only
// lives as long as the dlopen call, and a successful LinkingSession will
// provide the results via the Commit() method, which returns this object.
struct LinkingResult {
  // The list of new modules loaded by the LinkingSession, to be appended to
  // the RuntimeDynamicLinker::modules_ list.
  ModuleList loaded_modules;

  // ABI information for the modules in the above list with TLS.  Ordered parallel to
  // loaded_modules, which is also in order of sequentially increasing TLS module ID.
  Vector<TlsModule> tls_modules;
};

// The base class holds the state that's independent of the Loader class used.
class LinkingSessionBase {
 public:
  LinkingSessionBase() = delete;
  LinkingSessionBase(LinkingSessionBase&&) = delete;

  // The caller calls Commit() to finalize the LinkingSession after it has
  // loaded and linked all the modules needed for a single dlopen call. This
  // will transfer ownership of the RuntimeModules and TlsModules created during
  // this session in the LinkingResult returned back to the caller.
  LinkingResult Commit() && { return std::move(result_); }

 protected:
  // The RuntimeDynamicLinker and its modules() list are always treated as
  // const by LinkingSession methods.  But it needs to be a non-const reference
  // so that the RuntimeModule pointers drawn from it can be non-const, as the
  // RuntimeModule lists of pointers into other RuntimeModules need to be
  // non-const (mostly for MakeGlobal).
  explicit LinkingSessionBase(RuntimeDynamicLinker& linker);

  ModuleList& loaded_modules();

  size_type max_static_tls_modid() const;
  size_type max_tls_modid() const;

  ModuleList& result_modules() { return result_.loaded_modules; }

  RuntimeDynamicLinker& linker_;

  // New (prospective) "permanent" modules are appended here to parallel new session_modules_
  // elements, and TLS modules are appended likewise for each new PT_TLS segment.  Commit() moves
  // this out of a successful session.
  LinkingResult result_;
};

template <class Loader>
class LinkingSession : public LinkingSessionBase {
 public:
  explicit LinkingSession(RuntimeDynamicLinker& linker) : LinkingSessionBase{linker} {}

  template <typename RetrieveFile>
  bool Link(Diagnostics& diag, Soname soname, RetrieveFile&& retrieve_file) {
    if (!Load(diag, soname, std::forward<RetrieveFile>(retrieve_file))) {
      return false;
    }
    // The root module for the dlopen-ed file is always the first module
    // enqueued in this list.
    RuntimeModule& root_module = result_modules().front();
    return root_module.ReifyModuleTree(diag) && Relocate(diag, root_module.module_tree());
  }

 private:
  // Forward declaration; see definition below.
  class SessionModule;

  using SessionModuleList = fbl::DoublyLinkedList<std::unique_ptr<SessionModule>>;

  // Load the root module and all its dependencies. If a module for a dependency
  // is already loaded (e.g. by a previous dlopen call), its reference is
  // reused. The `retrieve_file` argument is a callable passed down from `Open`
  // and is invoked to retrieve a new module's file from the file system for
  // processing.
  template <typename RetrieveFile>
  bool Load(Diagnostics& diag, Soname soname, RetrieveFile&& retrieve_file) {
    static_assert(std::is_invocable_v<RetrieveFile, Diagnostics&, std::string_view>);

    // The root module will always be the first module in the LinkingSession's
    // bookkeeping lists.
    if (!EnqueueModule(diag, soname)) {
      return false;
    }

    // This lambda will retrieve the module's file, load the module into the
    // system image, and then create new modules for each of its dependencies
    // to enqueue onto session_modules_ for future processing. A
    // fit::result<bool> is returned to the caller where the boolean indicates
    // if the file was found, so that the caller can handle the "not-found"
    // error case.
    auto load_and_enqueue_deps = [&](auto& module) -> fit::result<bool> {
      auto file = retrieve_file(diag, module.name().str());
      if (file.is_error()) [[unlikely]] {
        // Check if the error is a not-found error or a system error.
        if (auto error = file.error_value()) {
          // If a general system error occurred, emit the error for the module.
          diag.SystemError("cannot open ", module.name().str(), ": ", *error);
          return fit::error(false);
        }
        // A "not-found" error occurred, and the caller is responsible for
        // emitting the error message for the module.
        return fit::error(true);
      }

      if (auto dep_names =
              module.Load(diag, *std::move(file), max_tls_modid() + result_.tls_modules.size())) {
        if (module.tls_module()) {
          if (!result_.tls_modules.push_back(diag, "TLS module vector", *module.tls_module())) {
            return fit::error(false);
          }
        }
        // Create and enqueue a module for each dependency, skipping
        // dependencies that have already been enqueued. The (parent) module
        // that was just loaded will also store a reference to its dependencies'
        // RuntimeModules in its direct_deps list.
        auto enqueue_dep = [this, &diag, &parent_list = module.runtime_module().direct_deps()](
                               const Soname& name) {
          if (std::ranges::any_of(session_modules_, name.equal_to())) {
            return true;
          }
          if (RuntimeModule* dep = EnqueueModule(diag, name)) {
            return parent_list.push_back(diag, "direct dependency container", dep);
          }
          return false;
        };
        if (std::ranges::all_of(*dep_names, enqueue_dep)) {
          return fit::ok();
        }
      }

      return fit::error(false);
    };

    // Proceed to load and enqueue the root module's dependencies and their
    // dependencies in a breadth-first order.
    for (auto it = session_modules_.begin(); it != session_modules_.end(); ++it) {
      if (auto result = load_and_enqueue_deps(*it); result.is_error()) {
        // If fit::error{true} is returned, this is a not-found error.
        if (result.error_value()) {
          if (it == session_modules_.begin()) {
            diag.SystemError(it->name().str(), " not found");
          } else {
            // TODO(https://fxbug.dev/336633049): harmonize this error message
            // with musl, which appends a "(needed by <depending module>)" to the
            // message.
            diag.MissingDependency(it->name().str());
          }
        }
        return false;
      }
    }

    return true;
  }

  // Create module data structures for `soname` and enqueue the modules onto
  // this LinkingSession's bookkeeping lists. If a module with `soname` has
  // already been loaded, then a reference to the loaded module is returned
  // instead.
  RuntimeModule* EnqueueModule(Diagnostics& diag, Soname soname) {
    auto& known_modules = loaded_modules();
    if (auto it = std::ranges::find_if(known_modules, soname.equal_to());
        it != known_modules.end()) {
      // Return a reference to the module if it was already loaded at startup
      // or by a LinkingSession from a previous dlopen() call.
      return &*it;
    }

    fbl::AllocChecker module_ac;
    auto module = RuntimeModule::Create(module_ac, soname);
    if (!module_ac.check()) [[unlikely]] {
      diag.OutOfMemory("permanent module data structure", sizeof(RuntimeModule));
      return nullptr;
    }
    fbl::AllocChecker session_module_ac;
    auto session_module = SessionModule::Create(session_module_ac, *module);
    if (!session_module_ac.check()) [[unlikely]] {
      diag.OutOfMemory("temporary module data structure", sizeof(SessionModule));
      return nullptr;
    }

    result_modules().push_back(std::move(module));
    session_modules_.push_back(std::move(session_module));

    // Return the RuntimeModule pointer that was just created and enqueued.
    return &result_modules().back();
  }

  // Perform relocations on all pending modules to be loaded. Return a boolean
  // if relocations succeeded on all modules.
  bool Relocate(Diagnostics& diag, auto&& session_modules) {
    if (session_modules.empty()) {
      return false;
    }

    // Construct a view of modules that will be used for symbol resolution.
    // This is an ordered list of global modules that have already been loaded,
    // followed by the non-global modules being loaded by this session.
    auto loaded_global = std::views::filter(loaded_modules(), &RuntimeModule::is_global);
    auto session_local = std::views::filter(session_modules, &RuntimeModule::is_local);
    static_assert(
        std::same_as<RuntimeModule&, std::ranges::range_reference_t<decltype(loaded_global)>>);
    static_assert(
        std::same_as<RuntimeModule&, std::ranges::range_reference_t<decltype(session_local)>>);
    auto relocate_and_relro =
        // The concat_view created here will be used as const since the lambda
        // is not mutable--anyway RuntimeModule::Relocate et al take the module
        // list object as const& rather than assuming it's a view.  filter_view
        // doesn't have const overloads so it can't be used as const and thus
        // can't be directly in a const concat_view.  However, ref_view has
        // const overloads that don't need the referenced view to have them.
        [resolution_modules =
             ConcatView{
                 std::ranges::ref_view(loaded_global),
                 std::ranges::ref_view(session_local),
             },
         &diag, this](SessionModule& session_module) -> bool {
      // TODO(https://fxbug.dev/339662473): this doesn't use the root module's
      // name in the scoped diagnostics. Add test for missing transitive symbol
      // and make sure the correct name is used in the error message.
      ld::ScopedModuleDiagnostics root_module_diag{diag, session_module.name().str()};
      return session_module.Relocate(diag, resolution_modules, max_static_tls_modid()) &&
             session_module.ProtectRelro(diag);
    };
    return std::all_of(std::begin(session_modules_), std::end(session_modules_),
                       relocate_and_relro);
  }

  // The list of "temporary" SessionModules needed to perform loading,
  // decoding, relocations, etc during this LinkingSession.  There is a 1:1
  // mapping between elements in session_modules_ and result_.loaded_modules:
  // each element in this list is responsible for filling out the runtime and
  // ABI data for the corresponding RuntimeModule located at the same index in
  // result_.loaded_modules.  IOW, session_modules_[idx].runtime_module() is a
  // reference to the runtime module at result_.loaded_modules[idx].  Unlike
  // the result_ list, this list will live only as long as the LinkingSession.
  SessionModuleList session_modules_;
};

// SessionModule is the temporary data structure created to load a file and
// perform relocations for a new module. A SessionModule is managed by
// session_modules_ and will get destroyed with the LinkingSession instance.
template <class Loader>
class LinkingSession<Loader>::SessionModule
    : public ld::LoadModule<ld::DecodedModuleInMemory<>>,
      public fbl::DoublyLinkedListable<std::unique_ptr<SessionModule>> {
 public:
  using Relro = typename Loader::Relro;
  using Phdr = Elf::Phdr;
  using Dyn = Elf::Dyn;
  using LoadInfo = elfldltl::LoadInfo<Elf, elfldltl::StaticVector<ld::kMaxSegments>::Container>;

  // The SessionModule::Create(...) takes a reference to the Module for the
  // file, setting information on it during the loading, decoding, and
  // relocation process.
  [[nodiscard]] static std::unique_ptr<SessionModule> Create(fbl::AllocChecker& ac,
                                                             RuntimeModule& runtime_module) {
    std::unique_ptr<SessionModule> session_module{new (ac) SessionModule(runtime_module)};
    if (session_module) [[likely]] {
      // Have the underlying DecodedModule (see <lib/ld/decoded-module.h>) point to
      // the ABIModule embedded in the Module, so that its information will
      // be filled out during decoding operations.
      session_module->decoded().set_module(runtime_module.module());
      session_module->set_name(runtime_module.name());
    }
    return session_module;
  }

  // Load `file` into the system image, decode phdrs and save the metadata in
  // the the ABI module. A vector of Soname objects of the module's DT_NEEDEDs
  // are returned to the caller.
  template <class File>
  std::optional<Vector<Soname>> Load(Diagnostics& diag, File&& file, size_type max_tls_modid) {
    // Read the file header and program headers into stack buffers and map in
    // the image.  This fills in load_info() as well as the module vaddr bounds
    // and phdrs fields.
    Loader loader;
    auto headers = decoded().LoadFromFile(diag, loader, std::forward<File>(file));
    if (!headers) [[unlikely]] {
      return {};
    }

    Vector<size_type> needed_offsets;
    if (!decoded().DecodeFromMemory(  //
            diag, loader.memory(), loader.page_size(), *headers, max_tls_modid,
            elfldltl::DynamicRelocationInfoObserver(decoded().reloc_info()),
            elfldltl::DynamicInitObserver(decoded().module().init),
            elfldltl::DynamicFiniObserver(decoded().module().fini),
            decoded().MakeNeededObserver(needed_offsets))) [[unlikely]] {
      return {};
    }

    if (decoded().tls_module_id() > 0) {
      tls_module_ = decoded().tls_module();
    }

    // After successfully loading the file, finalize the module's mapping by
    // calling `Commit` on the loader. Save the returned relro capability that
    // will be used to apply relro protections later.
    relro_ = decoded().CommitLoader(std::move(loader));

    // Return the parsed Sonames from the DT_NEEDED offsets.
    return decoded().template ReifyNeeded<Vector>(diag, needed_offsets);
  }

  // Perform relative and symbolic relocations, resolving symbols from the
  // ordered list of modules as needed.
  bool Relocate(Diagnostics& diag, std::ranges::forward_range auto&& ordered_modules,
                size_type max_static_tls_modid) {
    TlsDescResolver tls_desc_resolver =
        TlsDescResolver(max_static_tls_modid, runtime_module_.tls_desc_indirect_list());
    ld::ModuleMemory memory = ld::ModuleMemory{module()};
    auto resolver = elfldltl::MakeSymbolResolver(
        runtime_module_, std::forward<decltype(ordered_modules)>(ordered_modules), diag,
        tls_desc_resolver, ld::kResolverPolicy);
    return elfldltl::RelocateRelative(diag, memory, reloc_info(), load_bias()) &&
           elfldltl::RelocateSymbolic(memory, diag, reloc_info(), symbol_info(), load_bias(),
                                      resolver);
  }

  // Apply relro protections. `relro_` cannot be used after this call.
  bool ProtectRelro(Diagnostics& diag) { return std::move(relro_).Commit(diag); }

  RuntimeModule& runtime_module() { return runtime_module_; }
  const std::optional<TlsModule>& tls_module() const { return tls_module_; }

 private:
  // A SessionModule can only be created with SessionModule::Create...).
  explicit SessionModule(RuntimeModule& runtime_module) : runtime_module_(runtime_module) {}

  // This is a reference to the "permanent" module data structure that this
  // SessionModule is responsible for: runtime information is set on the
  // `runtime_module_` during the course of the loading process. Whereas this
  // SessionModule instance will get destroyed at the end of `dlopen`,
  // its `runtime_module_` will live as long as the file is loaded, in the
  // RuntimeDynamicLinker's `modules_` list.
  RuntimeModule& runtime_module_;

  // The TLS header info for the module.
  std::optional<TlsModule> tls_module_;

  // The relro capability that is provided when the module is decoded and is
  // used to apply relro protections after the module is relocated.
  Relro relro_;
};

}  // namespace dl

#endif  // LIB_C_DLFCN_DL_LINKING_SESSION_H_
