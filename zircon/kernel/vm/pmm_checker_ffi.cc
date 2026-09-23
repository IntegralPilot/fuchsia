// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/pmm_checker_ffi.h"

#include <kernel/ffi.h>
#include <ktl/memory.h>

#include "vm/pmm_checker.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void cpp_pmm_checker_init(ffi::Uninitialized<PmmChecker>* checker) {
  checker->Initialize();
}
FFI_ALWAYS_INLINE void cpp_pmm_checker_destroy(PmmChecker* checker) { ktl::destroy_at(checker); }

FFI_ALWAYS_INLINE bool cpp_pmm_checker_is_valid_fill_size(size_t fill_size) {
  return PmmChecker::IsValidFillSize(fill_size);
}

FFI_ALWAYS_INLINE void cpp_pmm_checker_set_fill_size(PmmChecker* checker, size_t fill_size) {
  checker->SetFillSize(fill_size);
}

FFI_ALWAYS_INLINE size_t cpp_pmm_checker_get_fill_size(const PmmChecker* checker) {
  return checker->GetFillSize();
}

FFI_ALWAYS_INLINE void cpp_pmm_checker_set_action(PmmChecker* checker, uint8_t action) {
  checker->SetAction(static_cast<CheckFailAction>(action));
}

FFI_ALWAYS_INLINE uint8_t cpp_pmm_checker_get_action(const PmmChecker* checker) {
  return static_cast<uint8_t>(checker->GetAction());
}

FFI_ALWAYS_INLINE bool cpp_pmm_checker_is_armed(const PmmChecker* checker) {
  return checker->IsArmed();
}

FFI_ALWAYS_INLINE void cpp_pmm_checker_arm(PmmChecker* checker) { checker->Arm(); }

FFI_ALWAYS_INLINE void cpp_pmm_checker_fill_pattern(const PmmChecker* checker, vm_page_t* page) {
  checker->FillPattern(page);
}

FFI_ALWAYS_INLINE bool cpp_pmm_checker_validate_pattern(const PmmChecker* checker,
                                                        vm_page_t* page) {
  return checker->ValidatePattern(page);
}

FFI_ALWAYS_INLINE void cpp_pmm_checker_assert_pattern(const PmmChecker* checker, vm_page_t* page) {
  checker->AssertPattern(page);
}

FFI_ALWAYS_INLINE void cpp_pmm_checker_print_status_stdout(const PmmChecker* checker) {
  checker->PrintStatus(stdout);
}

}  // extern "C"
