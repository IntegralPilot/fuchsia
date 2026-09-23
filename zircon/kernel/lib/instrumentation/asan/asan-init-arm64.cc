// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/instrumentation/asan.h>
#include <stddef.h>
#include <stdint.h>

#include "asan-internal.h"

#include <ktl/enforce.h>

// TODO(https://fxbug.dev/379891035): Implement me in physboot!

void asan_map_shadow_for(uintptr_t start, size_t size) {}

void arch_asan_early_init() {}

void arch_asan_late_init() {}
