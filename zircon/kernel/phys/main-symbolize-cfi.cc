// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/zircon-internal/unique-backtrace.h>

#include <phys/elf-image.h>
#include <phys/stdio.h>
#include <phys/symbolize.h>

// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" Symbolize::CfiCheckFunction __cfi_check;

// This is what gets all of this linked in.  It's linked into physload (only)
// even if physload itself is not instrumented.  If the modules it loads are
// instrumented, they may require the indirection back to each other.
void MainSymbolize::HandleCfiSlowpath() {
  Symbolize::CfiCheckFunction* self_check = nullptr;
#if __has_feature(cfi_sanitizer)
  self_check = &__cfi_check;
#endif
  set_cfi_slowpath(CallCfiSlowpath, self_check);
}

void Symbolize::set_cfi_slowpath(CallCfiSlowpathFunction* cfi_slowpath,
                                 CfiCheckFunction* main_cfi_check) {
  ZX_DEBUG_ASSERT(cfi_slowpath);
  ZX_DEBUG_ASSERT(!cfi_slowpath_);
  cfi_slowpath_ = cfi_slowpath;
  const_cast<ElfImage*>(main_module_)
      ->set_cfi_check_function(reinterpret_cast<uintptr_t>(main_cfi_check));
}

// This is the internal signature used by the ubsan runtime.  If that's linked
// in, it will provide the detailed diagnostics by overriding this function.
// NOLINTNEXTLINE(bugprone-reserved-identifier)
extern "C" [[gnu::weak, noreturn]] void __ubsan_handle_cfi_check_fail(const void* diag_data,
                                                                      uintptr_t entry,
                                                                      uintptr_t valid_vtable);

void __ubsan_handle_cfi_check_fail(const void* diag_data, uintptr_t entry, uintptr_t valid_vtable) {
  ZX_PANIC(
      "*** CFI check failure without diagnostic runtime ***"
      " data %p target %#" PRIxPTR " valid_vtable=%" PRIuPTR,
      diag_data, entry, valid_vtable);
}

void MainSymbolize::CallCfiSlowpath(Symbolize* main_symbolize, uint64_t key, void* entry,
                                    const void* diag_data, void* caller) {
  static_cast<MainSymbolize*>(main_symbolize)->CfiSlowpath(key, entry, diag_data, caller);
}

void MainSymbolize::CfiSlowpath(uint64_t key, void* entry, const void* diag_data, void* caller) {
  // The FILE::Write that printf will do is an indirect call and that will get
  // checked too.  Make sure there isn't a cascade of failing checks.
  static bool in_progress = false;
  if (in_progress) [[unlikely]] {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }
  in_progress = true;
  auto clear_in_progress = fit::defer([] { in_progress = false; });

  auto fail = [key, entry, diag_data, caller](const char* why, const ElfImage* module) {
    // The ubsan runtime will print more details about the call site, but
    // does not print the "why" aspect diagnosed here.
    printf("*** Control Flow Integrity ERROR at call site {{{pc:%p}}}\n", caller);
    printf("*** typeid=%#" PRIx64 " for call to {{{pc:%p}}}\n", key, entry);
    printf("*** %s ***\n", why);
    if (module) {
      printf("*** Call attempted to enter %.*s ***\n", static_cast<int>(module->name().size()),
             module->name().data());
    }
    __ubsan_handle_cfi_check_fail(diag_data, reinterpret_cast<uintptr_t>(entry), false);
  };

  const uintptr_t target_vaddr = reinterpret_cast<uintptr_t>(entry);
  const ElfImage* module = module_for_vaddr_range(target_vaddr, 1);
  if (!module) [[unlikely]] {
    fail("CFI-checked cross-DSO indirect call target in no known module", nullptr);
    return;
  }

  const uintptr_t caller_vaddr = reinterpret_cast<uintptr_t>(caller);
  if (module->contains_vaddr_range(caller_vaddr, 1)) [[unlikely]] {
    fail("cross-DSO call resolved to caller module", module);
    return;
  }

  if (ktl::optional cfi_check = module->cfi_check_function()) [[likely]] {
    const uintptr_t module_cfi_check = static_cast<uintptr_t>(*cfi_check);
    auto* const check = reinterpret_cast<CfiCheckFunction*>(module_cfi_check);
    (*check)(key, entry, diag_data);
  } else {
    printf(
        "WARNING: CFI-checked cross-DSO indirect call site {{{pc:%p}}}"
        " into module %.*s with no __cfi_check;"
        " typeid %#" PRIx64 " for call to {{{pc:%p}}} diag data %p\n",
        caller, static_cast<int>(module->name().size()), module->name().data(), key, entry,
        diag_data);
  }
}
