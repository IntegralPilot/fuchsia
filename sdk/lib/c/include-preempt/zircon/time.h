// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_ZIRCON_TIME_H_
#define PREEMPT_ZIRCON_TIME_H_

// TODO(https://fxbug.dev/42105189): See adjacent compiler.h for full details.

#pragma push_macro("add_overflow")
#pragma push_macro("sub_overflow")
#pragma push_macro("mul_overflow")

#undef add_overflow
#undef sub_overflow
#undef mul_overflow

#define add_overflow __builtin_add_overflow
#define sub_overflow __builtin_sub_overflow
#define mul_overflow __builtin_mul_overflow

#include_next <zircon/time.h>

#pragma pop_macro("add_overflow")
#pragma pop_macro("sub_overflow")
#pragma pop_macro("mul_overflow")

#endif  // PREEMPT_ZIRCON_TIME_H_
