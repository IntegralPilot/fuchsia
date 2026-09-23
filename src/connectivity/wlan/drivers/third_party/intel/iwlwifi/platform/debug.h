// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_DEBUG_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_DEBUG_H_

// This file contains Fuchsia-specific debug compilation support.

#include <stddef.h>
#include <stdio.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <wlan/drivers/log.h>

#if defined(__cplusplus)
extern "C" {
#endif  // defined(__cplusplus)

struct device;

// NEEDS_TYPES
#define WARN(cond, y, z...) (!!(cond))
#define BUILD_BUG_ON(x) ZX_ASSERT(!(x))

#define __WARN_ON(x, count_down)                                              \
  ({                                                                          \
    static size_t count = 1;                                                  \
    count -= count_down;                                                      \
    if (count && !!(x)) {                                                     \
      char log_str[] = #x;                                                    \
      lwarn("WARN_ON(%s) is asserted in %s:%d", log_str, __FILE__, __LINE__); \
    }                                                                         \
    !!(x);                                                                    \
  })

#define WARN_ON(x) __WARN_ON(x, 0)
#define WARN_ON_ONCE(x) __WARN_ON(x, 1)

#define iwl_assert_lock_held(x) ZX_DEBUG_ASSERT(mtx_trylock(x) == thrd_busy)

// In the future we may want to add other filters for a more granular view of debug/trace log
// output. You should define any additional log filters and tags here.
#define IWL_LOG_DEBUG_FILTER (1U)
#define IWL_LOG_DEBUG_TAG "iwlwifi-debug"

// If you want to log debug messages, this should be changed to some combinations of filters above.
// By default, debug messages are not displayed.
#define IWL_LOG_FILTER_SETTING (0U)

#define LOG_BUF_LEN 255

// Print a message to the kernel, of different severities. Removes the trailing newline that is
// unnecessary for zxlogf() and creates blank lines in the logs. It's a bit ugly, but it allows
// us to reduce the diffs between our code and the original driver code. If that is no longer a
// concern at some point in the future, we can remove the newlines in the format strings and
// this extra step. This potential future improvement is tracked by fxbug.dev/80797.
#define IWL_LOG(log_func, fmt, args...)                               \
  do {                                                                \
    char log_buf[LOG_BUF_LEN];                                        \
    size_t buf_len = snprintf(log_buf, LOG_BUF_LEN - 1, fmt, ##args); \
    log_buf[LOG_BUF_LEN - 1] = '\0';                                  \
    ZX_ASSERT(buf_len >= 0);                                          \
    ZX_ASSERT(buf_len < LOG_BUF_LEN);                                 \
    if (buf_len > 0 && log_buf[buf_len - 1] == '\n') {                \
      log_buf[buf_len - 1] = '\0';                                    \
    }                                                                 \
    log_func("%s", log_buf);                                          \
  } while (0)

#define IWL_LOG_FILTER_TAG(log_func, filter, tag, fmt, args...)       \
  do {                                                                \
    char log_buf[LOG_BUF_LEN];                                        \
    size_t buf_len = snprintf(log_buf, LOG_BUF_LEN - 1, fmt, ##args); \
    log_buf[LOG_BUF_LEN - 1] = '\0';                                  \
    ZX_ASSERT(buf_len >= 0);                                          \
    ZX_ASSERT(buf_len < LOG_BUF_LEN);                                 \
    if (buf_len > 0 && log_buf[buf_len - 1] == '\n') {                \
      log_buf[buf_len - 1] = '\0';                                    \
    }                                                                 \
    log_func(filter, tag, "%s", log_buf);                             \
  } while (0)

#define __iwl_err(dev, rfkill_prefix, only_trace, fmt, args...) IWL_LOG(lerror, fmt, ##args)
#define __iwl_warn(dev, fmt, args...) IWL_LOG(lwarn, fmt, ##args)
#define __iwl_info(dev, fmt, args...) IWL_LOG(linfo, fmt, ##args)
#define __iwl_crit(dev, fmt, args...) IWL_LOG(lerror, fmt, ##args)
#define __iwl_dbg(dev, level, limit, function, fmt, args...) \
  IWL_LOG_FILTER_TAG(ltrace, IWL_LOG_DEBUG_FILTER, IWL_LOG_DEBUG_TAG, fmt, ##args)

#define __iwl_err_throttle(dev, rfkill_prefix, only_trace, fmt, args...) \
  IWL_LOG(lthrottle_error, fmt, ##args)
#define __iwl_warn_throttle(dev, fmt, args...) IWL_LOG(lthrottle_warn, fmt, ##args)
#define __iwl_info_throttle(dev, fmt, args...) IWL_LOG(lthrottle_info, fmt, ##args)
#define __iwl_crit_throttle(dev, fmt, args...) IWL_LOG(lthrottle_error, fmt, ##args)
#define __iwl_dbg_throttle(dev, level, limit, function, fmt, args...) \
  IWL_LOG_FILTER_TAG(lthrottle_trace, IWL_LOG_DEBUG_FILTER, IWL_LOG_DEBUG_TAG, fmt, ##args)

// Publish a core dump.
zx_status_t iwl_debug_core_dump(struct device* dev, const char* core_dump_name,
                                const char** core_dump_ptrs, size_t* core_dump_sizes,
                                size_t core_dump_count);

#if defined(__cplusplus)
}  // extern "C"
#endif  // defined(__cplusplus)

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_DEBUG_H_
