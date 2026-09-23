// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_KERNEL_VDSO_VDSO_CALCULATE_TIME_H_
#define SRC_STARNIX_KERNEL_VDSO_VDSO_CALCULATE_TIME_H_

#include <stdint.h>
#include <zircon/time.h>

constexpr uint32_t kNanosecondsPerSecond = 1'000'000'000;
constexpr int64_t kUtcInvalid = 0;

// Defined by vdso.ld.
//
// This declaration needs to be explicitly annotated with the hidden visibility
// attribute to let the compiler know that the definition for vvar can be found
// within the VDSO itself and does not need to include an entry in the dynamic
// relocation table to resolve the address for vvar at runtime. Without this,
// the loader will try to resolve the entry at runtime but doing that requires
// write access to the VDSO memory mapping (to modify the entry itself) which
// starnix does not provide (VDSO mapping only has readable/executable) so we
// end up segmentation faulting when the loader performs the write. This issue
// has so far only been observed when using Android's init[1] on aarch64. See
// this[2] gist to see how the symbol tables differ on x86_64 and aarch64 with
// an example.
//
// Note that the VDSO is compiled with -fvisibility=hidden but this only affects
// the _default symbol visibility for definitions_[3]; the compiler still needs
// to be told that the definition for vvar can be found in the VDSO itself.
//
// [1]:
// https://cs.android.com/android/platform/superproject/main/+/main:system/core/init/main.cpp;drc=813871767921010aaccae39f4bbaaf78d21211e6
// [2]: https://gist.github.com/ghananigans/a642ec3b7854f54c3694c55db22de2fe
// [3]: https://clang.llvm.org/docs/ClangCommandLineReference.html#cmdoption-clang-fvisibility
__attribute__((__visibility__("hidden"))) extern "C" char vvar;

// Defined by vdso.ld.
//
// This declaration is explicitly annotated as hidden for the same reason as vvar is above.
// The type of this variable is irrelevant and arbitrarily chosen to be char, as only the
// address of the variable is used.
__attribute__((__visibility__("hidden"))) extern "C" char time_values;

// Returns monotonic time in hardware ticks.
// This is equivalent to calling zx_ticks_get, so long as the ticks are userspace accessible. If
// they are not, then this will return ZX_TIME_INFINITE_PAST, and the caller must invoke a full
// clock_gettime syscall.
zx_instant_mono_ticks_t calculate_monotonic_ticks();

// Similar to above, but uses the boot timeline for the calculation instead.
zx_instant_boot_ticks_t calculate_boot_ticks();

// Returns monotonic time in nanoseconds.
// This is equivalent to calling zx_clock_get_monotonic, so long as the ticks are userspace
// accessible. If they are not, then this will return ZX_TIME_INFINITE_PAST, and the caller must
// invoke a full clock_gettime syscall.
zx_instant_mono_t calculate_monotonic_time_nsec();

// Similar to above, but uses the boot clock timeline for the calculation instead.
zx_instant_boot_t calculate_boot_time_nsec();

// Returns utc time in nanoseconds
int64_t calculate_utc_time_nsec();

#endif  // SRC_STARNIX_KERNEL_VDSO_VDSO_CALCULATE_TIME_H_
