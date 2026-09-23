// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_RUNTIME_DYNAMIC_LINKER_H_
#define LIB_C_DLFCN_DL_RUNTIME_DYNAMIC_LINKER_H_

#include <dlfcn.h>  // for RTLD_* macros
#include <lib/elfldltl/soname.h>
#include <lib/fit/function.h>
#include <lib/fit/result.h>

#include <fbl/intrusive_double_list.h>

#include "diagnostics.h"
#include "error.h"
#include "linking-session.h"
#include "runtime-module.h"

// TODO(https://fxbug.dev/338239201): These flags should go into the new libc's
// dlfcn.h.
#ifndef RTLD_DI_TLS_MODID
#define RTLD_DI_TLS_MODID 9
#endif

#ifndef RTLD_DI_TLS_DATA
#define RTLD_DI_TLS_DATA 10
#endif

#ifndef RTLD_DI_PHDR
#define RTLD_DI_PHDR 11
#endif

namespace dl {

using size_type = Elf::size_type;
using DlIteratePhdrCallback = int(dl_phdr_info*, size_t, void*);

// Supported dlopen flags:
constexpr int kLocal = RTLD_LOCAL;
constexpr int kGlobal = RTLD_GLOBAL;
constexpr int kNow = RTLD_NOW;
// RTLD_LAZY functionality is not supported, but keep the flag definition
// because it's a legitimate flag that can be passed in.
constexpr int kLazy = RTLD_LAZY;
constexpr int kNoload = RTLD_NOLOAD;
constexpr int kNodelete = RTLD_NODELETE;
// TODO(https://fxbug.dev/323425900): support glibc's RTLD_DEEPBIND flag.
// kDEEPBIND = RTLD_DEEPBIND,

// Supported dlinfo request values:
constexpr int kLinkMap = RTLD_DI_LINKMAP;
constexpr int kTlsModid = RTLD_DI_TLS_MODID;
constexpr int kTlsData = RTLD_DI_TLS_DATA;
constexpr int kPhdrs = RTLD_DI_PHDR;

// Masks used to validate flag values.
inline constexpr int kOpenSymbolScopeMask = kLocal | kGlobal;
inline constexpr int kOpenBindingModeMask = kLazy | kNow;
inline constexpr int kOpenFlagsMask = kNoload | kNodelete;

class RuntimeDynamicLinker {
 public:
  using Soname = elfldltl::Soname<>;

  // Create a RuntimeDynamicLinker with the passed in passive `abi`. The caller
  // is required to pass an AllocChecker and check it to verify the
  // RuntimeDynamicLinker was created and initialized successfully.
  static std::unique_ptr<RuntimeDynamicLinker> Create(const ld::abi::Abi<>& abi,
                                                      fbl::AllocChecker& ac);

  constexpr const ModuleList& modules() const { return modules_; }
  constexpr ModuleList& modules() { return modules_; }

  size_t max_static_tls_modid() const { return abi_.static_tls_modules.size(); }

  // Lookup a symbol from the given module, returning a pointer to it in memory,
  // or an error if not found (ie undefined symbol).
  fit::result<Error, void*> LookupSymbol(const RuntimeModule& root, const char* ref);

  // - TODO(https://fxbug.dev/339037138): Add a test exercising the system error
  // case and include it as an example for the fit::error{Error} description.

  // Open `file` with the given `mode`, returning a pointer to the loaded module
  // for the file. The `retrieve_file` argument is to the LinkingSession and
  // is called as a `fit::result<std::optional<Error>, File>(Diagnostics&, std::string_view)`
  // with the following semantics:
  //   - fit::error{std::nullopt} is a not found error
  //   - fit::error{Error} is an error type that can be passed to
  //     Diagnostics::SystemError (see <lib/elfldltl/diagnostics.h>) to give
  //     more context to the error message.
  //   - fit::ok{File} is the found elfldltl File API type for the module
  //     (see <lib/elfldltl/memory.h>).
  // The Diagnostics reference passed to `retrieve_file` is not used by the
  // function itself to report its errors, but is plumbed into the created File
  // API object that will use it for reporting file read errors.
  template <class Loader, typename RetrieveFile>
  fit::result<Error, void*> Open(const char* file, int mode, RetrieveFile&& retrieve_file) {
    // `mode` must be a valid value.
    if (mode & ~(kOpenSymbolScopeMask | kOpenBindingModeMask | kOpenFlagsMask)) {
      return fit::error{Error{"invalid mode parameter"}};
    }

    // Use a non-scoped diagnostics object for the root module. Because errors
    // are generated on this module directly, its name does not need to be
    // prefixed to the error, as is the case using ld::ScopedModuleDiagnostics.
    dl::Diagnostics diag;

    // If NULL is passed in for `file`, return a handle to the first module in
    // the modules_ list (ie the executable).
    if (!file) {
      return diag.ok(&modules_.front());
    }

    Soname name{file};
    // If a module for this file is already loaded, return a reference to it.
    // Update its global visibility if dlopen(...RTLD_GLOBAL) was passed.
    if (RuntimeModule* found = FindModule(name)) {
      if (!found->ReifyModuleTree(diag)) {
        return diag.take_error();
      }
      if (mode & kGlobal) {
        MakeGlobal(found->module_tree());
      }
      if (mode & kNodelete) {
        found->set_no_delete();
      }
      return diag.ok(found);
    }

    if (mode & kNoload) {
      return diag.ok(nullptr);
    }

    // A Module for `file` does not yet exist; create a new LinkingSession
    // to perform the loading and linking of the file and all its dependencies.
    LinkingSession<Loader> linking_session{*this};

    if (!linking_session.Link(diag, name, std::forward<RetrieveFile>(retrieve_file))) {
      return diag.take_error();
    }

    // Commit the linking session and its mapped modules.
    LinkingResult linking_result = std::move(linking_session).Commit();

    // Obtain a reference to the root module for the dlopen-ed file to return
    // back to the caller.
    RuntimeModule& root_module = linking_result.loaded_modules.front();

    if (auto result = AddNewTlsModules(std::move(linking_result.tls_modules)); result.is_error()) {
      return result.take_error();
    }

    // After successful loading and relocation, append the new permanent modules
    // created by the linking session to the dynamic linker's module list.
    AddNewModules(std::move(linking_result.loaded_modules));

    // If RTLD_GLOBAL was passed, make the module and all of its dependencies
    // global. This is done after modules from the linking session have been
    // added to the modules_ list, because this operation may change the
    // ordering of all loaded modules.
    if (mode & kGlobal) {
      MakeGlobal(root_module.module_tree());
    }

    if (mode & kNodelete) {
      root_module.set_no_delete();
    }

    return diag.ok(&root_module);
  }

  // Create a `dl_phdr_info` for each module in `modules_` and pass it
  // to the caller-supplied `callback`. Iteration ceases when `callback` returns
  // a non-zero value. The result of the last callback function to run is
  // returned to the caller.
  int IteratePhdrInfo(DlIteratePhdrCallback* callback, void* data) const;

  // Information specified by `request` is stored in the location pointed to by
  // `info`. On success:
  //  - Returns the number of program headers if `request` is `RTLD_DI_PHDR`.
  //  - Returns 0 for all other valid `RTLD_DI_*` values.
  // An error message is returned on failure.
  fit::result<Error, int> DlInfo(void* handle, int request, void* info);

  const Vector<TlsModule>& dynamic_tls_modules() const { return dynamic_tls_modules_; }

 private:
  // A The RuntimeDynamicLinker can only be created with RuntimeDynamicLinker::Create...).
  explicit RuntimeDynamicLinker(const ld::abi::Abi<>& abi) : abi_(abi) {}

  // Append new modules to the end of the `modules_`.
  void AddNewModules(ModuleList modules);

  // Append new dynamic TLS modules to the dynamic TLS array.
  fit::result<Error> AddNewTlsModules(Vector<TlsModule> tls_modules);

  // Attempt to find the loaded module with the given name, returning a nullptr
  // if the module was not found.
  RuntimeModule* FindModule(Soname name);

  // Apply RTLD_GLOBAL to any module that is not already global in the provided
  // `module_tree`. When a module is promoted to global, its load order in the
  // dynamic linker's modules_ list changes: it is moved to the back of the
  // list, as if it was just loaded with RTLD_GLOBAL.
  void MakeGlobal(ModuleTree module_tree);

  // Create RuntimeModule data structures from the passive ABI and add them to
  // the dynamic linker's modules_ list. The caller is required to pass an
  // AllocChecker and check it to verify the success/failure of loading the
  // passive ABI into the RuntimeDynamicLinker.
  void PopulateStartupModules(fbl::AllocChecker& ac, const ld::abi::Abi<>& abi);

  ld::DlPhdrInfoCounts dl_phdr_info_counts() const {
    return {.adds = loaded_, .subs = loaded_ - modules_.size()};
  }

  // Return a pointer to the beginning of a module's static or dynamic TLS block.
  void* TlsBlock(const RuntimeModule& module) const;

  const ld::abi::Abi<>& abi_;

  // The RuntimeDynamicLinker owns the list of all 'live' modules that have
  // been loaded into the system image.
  ModuleList modules_;

  // The dynamic TLS modules that have been loaded by dlopen.  Elements are ordered by sequentially
  // increasing TLS module ID.
  Vector<TlsModule> dynamic_tls_modules_;

  // This is incremented every time a module is loaded into the system. This
  // number only ever increases and includes startup modules.
  size_t loaded_ = 0;
};

}  // namespace dl

#endif  // LIB_C_DLFCN_DL_RUNTIME_DYNAMIC_LINKER_H_
