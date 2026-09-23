// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_H_

#include <lib/ktrace.h>

#include <kernel/lock_trace_config.h>

#define LOCK_TRACE_VARIABLE_NAME(name) FXT_CONCATENATE(name, __COUNTER__)

#define LOCK_TRACE_DURATION(label, args...)           \
  ktrace::Scope LOCK_TRACE_VARIABLE_NAME(duration_) = \
      KTRACE_BEGIN_SCOPE("kernel:contention", label, ##args)

#define LOCK_TRACE_DURATION_BEGIN(label, args...) \
  KTRACE_DURATION_BEGIN("kernel:contention", label, ##args)

#define LOCK_TRACE_DURATION_END(label, args...) \
  KTRACE_DURATION_END("kernel:contention", label, ##args)

#define LOCK_TRACE_FLOW_BEGIN(label, args...) KTRACE_FLOW_BEGIN("kernel:contention", label, ##args)

#define LOCK_TRACE_FLOW_STEP(label, args...) KTRACE_FLOW_STEP("kernel:contention", label, ##args)

#define LOCK_TRACE_FLOW_END(label, args...) KTRACE_FLOW_END("kernel:contention", label, ##args)

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_H_
