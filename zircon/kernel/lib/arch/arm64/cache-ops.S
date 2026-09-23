// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/arm64/system-asm.h>
#include <lib/arch/asm.h>
#include <lib/arch/cache.h>

.text

// Scratch register, not saved across function calls.
tmp             .req x16

.function CleanDataCacheRange, global
  data_cache_range_op cvac

  ret
  speculation_postfence
.end_function

.function CleanInvalidateDataCacheRange, global
  data_cache_range_op civac

  ret
  speculation_postfence
.end_function

.function InvalidateDataCacheRange, global
  data_cache_range_op ivac

  ret
  speculation_postfence
.end_function

.function InvalidateInstructionCacheRange, global
  instruction_cache_range_invalidate

  ret
  speculation_postfence
.end_function
