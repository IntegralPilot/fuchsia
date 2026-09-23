// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_CONFIG_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_CONFIG_H_

#ifndef LOCK_NAME_TRACING_ENABLED
#define LOCK_NAME_TRACING_ENABLED false
#endif

constexpr bool kLockNameTracingEnabled{LOCK_NAME_TRACING_ENABLED};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_LOCK_TRACE_CONFIG_H_
