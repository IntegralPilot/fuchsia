// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/ffi.h>
#include <platform/debug.h>

extern "C" {

int cpp_platform_dgetc(char* c, bool wait);
void cpp_platform_dputs_thread(const char* str, size_t len);
void cpp_platform_pputc(char c);
int cpp_platform_pgetc(char* c);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE int cpp_platform_dgetc(char* c, bool wait) { return platform_dgetc(c, wait); }
FFI_ALWAYS_INLINE void cpp_platform_dputs_thread(const char* str, size_t len) {
  platform_dputs_thread(str, len);
}
FFI_ALWAYS_INLINE void cpp_platform_pputc(char c) { platform_pputc(c); }
FFI_ALWAYS_INLINE int cpp_platform_pgetc(char* c) { return platform_pgetc(c); }

}  // extern "C"
