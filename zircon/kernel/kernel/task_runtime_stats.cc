// Copyright 2021 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/task_runtime_stats.h"

#include <lib/affine/ratio.h>

#include <platform/timer.h>

TaskRuntimeStats::operator zx_info_task_runtime_t() const {
  const affine::Ratio ticks_to_time = timer_get_ticks_to_time_ratio();
  return {.cpu_time = ticks_to_time.Scale(cpu_ticks),
          .queue_time = ticks_to_time.Scale(queue_ticks),
          .page_fault_time = ticks_to_time.Scale(page_fault_ticks),
          .lock_contention_time = ticks_to_time.Scale(lock_contention_ticks)};
}
