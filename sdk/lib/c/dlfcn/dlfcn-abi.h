// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DLFCN_ABI_H_
#define LIB_C_DLFCN_DLFCN_ABI_H_

#include <lib/ld/abi.h>
#include <lib/ld/dl-phdr-info.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <type_traits>

#include "../weak.h"
#include "src/__support/macros/config.h"
#include "src/link/dl_iterate_phdr.h"

// This defines the ABI contract between libc and libdl.  It's implied that
// they also share the <lib/ld/abi.h> contract with the startup dynamic linker
// (or stub dynamic linker under remote dynamic linking).
//
// The symbols here are declared in LIBC_NAMESPACE but also `extern "C"`.  They
// always have the public linkage names as used in the DSO, even in test code.

namespace LIBC_NAMESPACE_DECL {
extern "C" {

// **Calls from libc into libdl**

// The libdl entrypoints do their own locking for the most part.  But in some
// places libc needs to exclude libdl (dlopen) changes while doing other things
// before or after it calls into a _dlfcn_* hook to have libdl do something.
[[gnu::weak]] void _dlfcn_lock();
[[gnu::weak]] void _dlfcn_unlock();

// This is used as the only way to call those, so thread-safety annotations
// always describe it for static checking.
inline constexpr WeakLock<_dlfcn_lock, _dlfcn_unlock> kDlfcnLock{};

// The compiler calls ld::DlPhdrInfoCounts an "incompatible with C" type just
// because it has explicit member initializers even though they're trivial.
// It's actually perfectly compatible with C in its ABI and semantics (just not
// its source declaration, both because it's namespaced and because it has
// member initializers), but it doesn't matter anyway since this is in fact
// only using C linkage between entirely C++ callers and callees.
static_assert(std::is_trivially_copyable_v<ld::DlPhdrInfoCounts>);
static_assert(std::is_trivially_destructible_v<ld::DlPhdrInfoCounts>);
#pragma GCC diagnostic push
#ifdef __clang__
#pragma GCC diagnostic ignored "-Wreturn-type-c-linkage"
#endif

// This returns the .dlpi_adds, .dlpi_subs values that dl_iterate_phdr will use
// in the callbacks it makes for startup modules.  If it's undefined, values
// ld::DlPhdrInfoInitialExecCounts(_ld_abi) are used.  Note that there is no
// synchronization intended between collecting these values and the eventual
// call to _dlfcn_iterate_phdr (see below).  If there are intervening dlopen or
// dlclose calls that change the values that _dlfcn_iterate_phdr will later
// give to its callbacks from the ones returned here, so be it.  Note that
// after this returns, _dlfcn_iterate_phdr will not necessarily be called at
// all (if the user's callback bailed out early).
[[gnu::weak]] ld::DlPhdrInfoCounts _dlfcn_phdr_info_counts() __TA_EXCLUDES(kDlfcnLock);

#pragma GCC diagnostic pop

// This has the exact signature and semantics of dl_iterate_phdr, except that
// it's only called after all the startup modules have been reported to the
// user's callback.  If the user's callback bailed out with a nonzero return
// value, this will never be called.  If this is called at all, then it's
// paired with the preceding _dlfcn_phdr_info_counts() call on the same thread.
// This is tail-called by libc's dl_iterate_phdr after reporting the startup
// modules.  If it's undefined, dl_iterate_phdr just returns zero instead.
[[gnu::weak]] decltype(dl_iterate_phdr) _dlfcn_iterate_phdr __TA_EXCLUDES(kDlfcnLock);

// This finds the module containing the given address, if any.  When present,
// it's used in lieu of ld::FindModuleByVaddr() on ld::AbiLoadedModules() for
// dladdr et al.  So it must search all startup, as well as dlopen'd, modules.
[[gnu::weak]] const ld::abi::Abi<>::Module* _dlfcn_module_by_vaddr(uintptr_t vaddr);

}  // extern "C"
}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_DLFCN_DLFCN_ABI_H_
