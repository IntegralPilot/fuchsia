// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/affine/ratio.h>
#include <lib/counters.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>

#include <cstdint>

#include <platform/timer.h>

KCOUNTER_DECLARE(chainlock_backoffs, "chainlock.backoffs", Sum)
KCOUNTER_DECLARE(chainlock_backoffs_max, "chainlock.backoffs_max", Max)
KCOUNTER_DECLARE(chainlock_contention_ns, "chainlock.contention_ns", Sum)
KCOUNTER_DECLARE(chainlock_contention_ns_max, "chainlock.contention_ns_max", Max)

void ChainLockTransaction::OnBackoff() {
  chainlock_backoffs.Add(1);
  chainlock_backoffs_max.Max(static_cast<int64_t>(backoff_count()));
}

void ChainLockTransaction::UpdateContentionCounters(zx_duration_mono_ticks_t contention_ticks) {
  const affine::Ratio& ticks_to_time = timer_get_ticks_to_time_ratio();
  const zx_duration_t contention_ns = ticks_to_time.Scale(contention_ticks);
  chainlock_contention_ns.Add(contention_ns);
  chainlock_contention_ns_max.Max(contention_ns);
}
