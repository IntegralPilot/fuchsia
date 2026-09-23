// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_TRACING_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_TRACING_H_

#include <lib/ktrace.h>

// Determines which subset of tracers are enabled when detailed tracing is
// enabled. When queue tracing is enabled the minimum trace level is COMMON.
#define LOCAL_KTRACE_LEVEL                                         \
  (SCHEDULER_TRACING_LEVEL == 0 && SCHEDULER_QUEUE_TRACING_ENABLED \
       ? KERNEL_SCHEDULER_TRACING_LEVEL_COMMON                     \
       : SCHEDULER_TRACING_LEVEL)

// The tracing levels used in this compilation unit.
#define KERNEL_SCHEDULER_TRACING_LEVEL_STANDARD 0
#define KERNEL_SCHEDULER_TRACING_LEVEL_COMMON 1
#define KERNEL_SCHEDULER_TRACING_LEVEL_BANDWIDTH 2
#define KERNEL_SCHEDULER_TRACING_LEVEL_QUEUE 3
#define KERNEL_SCHEDULER_TRACING_LEVEL_FLOW 4
#define KERNEL_SCHEDULER_TRACING_LEVEL_COUNTER 5
#define KERNEL_SCHEDULER_TRACING_LEVEL_DETAILED 6

// Evaluates to true if tracing is enabled for the given level.
#define LOCAL_KTRACE_LEVEL_ENABLED(level) \
  ((LOCAL_KTRACE_LEVEL) >= FXT_CONCATENATE(KERNEL_SCHEDULER_TRACING_LEVEL_, level))

#define LOCAL_KTRACE_INSTANT(level, string, args...) \
  KTRACE_CPU_INSTANT_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, ##args)

#define LOCAL_KTRACE_FLOW_BEGIN(level, string, flow_id, args...)                                   \
  KTRACE_CPU_FLOW_BEGIN_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, flow_id, \
                               ##args)

#define LOCAL_KTRACE_FLOW_END(level, string, flow_id, args...)                                   \
  KTRACE_CPU_FLOW_END_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, flow_id, \
                             ##args)

#define LOCAL_KTRACE_FLOW_STEP(level, string, flow_id, args...)                                   \
  KTRACE_CPU_FLOW_STEP_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, flow_id, \
                              ##args)

#define LOCAL_KTRACE_COUNTER(level, string, counter_id, args...)                                   \
  KTRACE_CPU_COUNTER_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, counter_id, \
                            ##args)

#define LOCAL_KTRACE_COUNTER_TIMESTAMP(level, string, timestamp, counter_id, args...)            \
  KTRACE_CPU_COUNTER_TIMESTAMP_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, \
                                      timestamp, counter_id, ##args)

#define LOCAL_KTRACE_BEGIN_SCOPE(level, string, args...) \
  KTRACE_CPU_BEGIN_SCOPE_ENABLE(LOCAL_KTRACE_LEVEL_ENABLED(level), "kernel:sched", string, ##args)

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_TRACING_H_
