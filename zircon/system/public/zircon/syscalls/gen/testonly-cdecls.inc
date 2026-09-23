// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#ifndef _ZX_SYSCALL_DECL
#error "<zircon/testonly-syscalls.h> is the public API header"
#endif

_ZX_SYSCALL_DECL(syscall_test_handle_create, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2,
                 (return_value, out),
                 (zx_status_t return_value,
                  _ZX_SYSCALL_ANNO(acquire_handle("Fuchsia")) zx_handle_t* out))

_ZX_SYSCALL_DECL(syscall_test_rust_handle, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (handle, value),
                 (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, uint32_t* value))

_ZX_SYSCALL_DECL(syscall_test_rust_inoutptr, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (ptr),
                 (int32_t* ptr))

_ZX_SYSCALL_DECL(syscall_test_rust_inptr, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (ptr, value),
                 (const int32_t* ptr, int32_t* value))

_ZX_SYSCALL_DECL(syscall_test_rust_outptr, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (value, ptr),
                 (int32_t value, int32_t* ptr))

_ZX_SYSCALL_DECL(syscall_test_rust_wrapper, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3, (a, b, c),
                 (int32_t a, int32_t b, int32_t c))

_ZX_SYSCALL_DECL(syscall_test_rust_0, zx_status_t, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

_ZX_SYSCALL_DECL(syscall_test_rust_1, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (a), (int32_t a))

_ZX_SYSCALL_DECL(syscall_test_rust_2, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (a, b),
                 (int32_t a, int32_t b))

_ZX_SYSCALL_DECL(syscall_test_rust_3, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3, (a, b, c),
                 (int32_t a, int32_t b, int32_t c))

_ZX_SYSCALL_DECL(syscall_test_rust_4, zx_status_t, _ZX_SYSCALL_EXTERN_C, 4, (a, b, c, d),
                 (int32_t a, int32_t b, int32_t c, int32_t d))

_ZX_SYSCALL_DECL(syscall_test_rust_5, zx_status_t, _ZX_SYSCALL_EXTERN_C, 5, (a, b, c, d, e),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e))

_ZX_SYSCALL_DECL(syscall_test_rust_6, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6, (a, b, c, d, e, f),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f))

_ZX_SYSCALL_DECL(syscall_test_rust_7, zx_status_t, _ZX_SYSCALL_EXTERN_C, 7, (a, b, c, d, e, f, g),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f, int32_t g))

_ZX_SYSCALL_DECL(syscall_test_rust_8, zx_status_t, _ZX_SYSCALL_EXTERN_C, 8,
                 (a, b, c, d, e, f, g, h),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f, int32_t g,
                  int32_t h))

_ZX_SYSCALL_DECL(syscall_test_widening_signed_narrow, int64_t, _ZX_SYSCALL_EXTERN_C, 4,
                 (a, b, c, d), (int64_t a, int32_t b, int16_t c, int8_t d))

_ZX_SYSCALL_DECL(syscall_test_widening_signed_wide, int64_t, _ZX_SYSCALL_EXTERN_C, 4, (a, b, c, d),
                 (int64_t a, int32_t b, int16_t c, int8_t d))

_ZX_SYSCALL_DECL(syscall_test_widening_unsigned_narrow, uint64_t, _ZX_SYSCALL_EXTERN_C, 4,
                 (a, b, c, d), (uint64_t a, uint32_t b, uint16_t c, uint8_t d))

_ZX_SYSCALL_DECL(syscall_test_widening_unsigned_wide, uint64_t, _ZX_SYSCALL_EXTERN_C, 4,
                 (a, b, c, d), (uint64_t a, uint32_t b, uint16_t c, uint8_t d))

_ZX_SYSCALL_DECL(syscall_test_wrapper, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3, (a, b, c),
                 (int32_t a, int32_t b, int32_t c))

_ZX_SYSCALL_DECL(syscall_test_0, zx_status_t, _ZX_SYSCALL_EXTERN_C, 0, (), (void))

_ZX_SYSCALL_DECL(syscall_test_1, zx_status_t, _ZX_SYSCALL_EXTERN_C, 1, (a), (int32_t a))

_ZX_SYSCALL_DECL(syscall_test_2, zx_status_t, _ZX_SYSCALL_EXTERN_C, 2, (a, b),
                 (int32_t a, int32_t b))

_ZX_SYSCALL_DECL(syscall_test_3, zx_status_t, _ZX_SYSCALL_EXTERN_C, 3, (a, b, c),
                 (int32_t a, int32_t b, int32_t c))

_ZX_SYSCALL_DECL(syscall_test_4, zx_status_t, _ZX_SYSCALL_EXTERN_C, 4, (a, b, c, d),
                 (int32_t a, int32_t b, int32_t c, int32_t d))

_ZX_SYSCALL_DECL(syscall_test_5, zx_status_t, _ZX_SYSCALL_EXTERN_C, 5, (a, b, c, d, e),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e))

_ZX_SYSCALL_DECL(syscall_test_6, zx_status_t, _ZX_SYSCALL_EXTERN_C, 6, (a, b, c, d, e, f),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f))

_ZX_SYSCALL_DECL(syscall_test_7, zx_status_t, _ZX_SYSCALL_EXTERN_C, 7, (a, b, c, d, e, f, g),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f, int32_t g))

_ZX_SYSCALL_DECL(syscall_test_8, zx_status_t, _ZX_SYSCALL_EXTERN_C, 8, (a, b, c, d, e, f, g, h),
                 (int32_t a, int32_t b, int32_t c, int32_t d, int32_t e, int32_t f, int32_t g,
                  int32_t h))
