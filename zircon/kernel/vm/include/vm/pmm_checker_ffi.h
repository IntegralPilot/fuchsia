// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_CHECKER_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_CHECKER_FFI_H_

#include <zircon/compiler.h>

#include <kernel/ffi.h>
#include <vm/pmm_checker.h>

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
__BEGIN_CDECLS

FFI_ALWAYS_INLINE void cpp_pmm_checker_init(ffi::Uninitialized<PmmChecker>* checker);
FFI_ALWAYS_INLINE void cpp_pmm_checker_destroy(PmmChecker* checker);
FFI_ALWAYS_INLINE bool cpp_pmm_checker_is_valid_fill_size(size_t fill_size);
FFI_ALWAYS_INLINE void cpp_pmm_checker_set_fill_size(PmmChecker* checker, size_t fill_size);
FFI_ALWAYS_INLINE size_t cpp_pmm_checker_get_fill_size(const PmmChecker* checker);
FFI_ALWAYS_INLINE void cpp_pmm_checker_set_action(PmmChecker* checker, uint8_t action);
FFI_ALWAYS_INLINE uint8_t cpp_pmm_checker_get_action(const PmmChecker* checker);
FFI_ALWAYS_INLINE bool cpp_pmm_checker_is_armed(const PmmChecker* checker);
FFI_ALWAYS_INLINE void cpp_pmm_checker_arm(PmmChecker* checker);
FFI_ALWAYS_INLINE void cpp_pmm_checker_fill_pattern(const PmmChecker* checker, vm_page_t* page);
FFI_ALWAYS_INLINE bool cpp_pmm_checker_validate_pattern(const PmmChecker* checker, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_pmm_checker_assert_pattern(const PmmChecker* checker, vm_page_t* page);
FFI_ALWAYS_INLINE void cpp_pmm_checker_print_status_stdout(const PmmChecker* checker);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_PMM_CHECKER_FFI_H_
