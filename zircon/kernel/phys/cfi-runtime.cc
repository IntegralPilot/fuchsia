// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <phys/symbolize.h>

// This is the internal signature used by the ubsan runtime.  If that's linked
// in, it will provide the detailed diagnostics.
// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" [[gnu::weak, noreturn]] void __ubsan_handle_cfi_check_fail(  //
    const void*, uintptr_t, uintptr_t);

// Code compiled with -fsanitize-cfi-cross-dso calls one of these two.
// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" Symbolize::CfiCheckFunction __cfi_slowpath_diag;
// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" void __cfi_slowpath(uint64_t key, void* entry);

// These functions all get copies linked into each phys module compiled with
// -fsanitize=cfi.  They just proxy to the single CfiSlowpath in physload.

[[clang::cfi_unchecked_callee]]
void __cfi_slowpath_diag(uint64_t key, void* entry, const void* diag_data) {
  Symbolize::CallCfiSlowpath(key, entry, diag_data);
}

void __cfi_slowpath(uint64_t key, void* entry) { Symbolize::CallCfiSlowpath(key, entry); }

void Symbolize::CallCfiSlowpath(  //
    uint64_t key, void* entry, const void* diag_data, void* caller) {
  if (!gSymbolize || !gSymbolize->cfi_slowpath_) [[unlikely]] {
    ZX_PANIC(
        "CFI-checked cross-DSO indirect call before handoff complete;"
        " call site {{{pc:%p}}}"
        " typeid %#" PRIx64 " entry-point {{{pc:%p}}} diag data %p",
        caller, key, entry, diag_data);
  }
  gSymbolize->cfi_slowpath_(gSymbolize, key, entry, diag_data, caller);
}
