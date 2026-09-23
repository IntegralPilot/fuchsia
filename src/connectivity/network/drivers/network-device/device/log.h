// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_LOG_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_LOG_H_

#ifdef NETDEV_DRIVER

#include <sdk/lib/driver/logging/cpp/logger.h>
#define LOG_ERROR(msg) FDF_LOG(ERROR, msg)
#define LOG_WARN(msg) FDF_LOG(WARNING, msg)
#define LOG_INFO(msg) FDF_LOG(INFO, msg)
#define LOG_TRACE(msg) FDF_LOG(DEBUG, msg)

#define LOGF_ERROR(fmt, ...) FDF_LOG(ERROR, fmt, ##__VA_ARGS__)
#define LOGF_WARN(fmt, ...) FDF_LOG(WARNING, fmt, ##__VA_ARGS__)
#define LOGF_INFO(fmt, ...) FDF_LOG(INFO, fmt, ##__VA_ARGS__)
#define LOGF_TRACE(fmt, ...) FDF_LOG(DEBUG, fmt, ##__VA_ARGS__)

#else  // !NETDEV_DRIVER

#include <sdk/lib/syslog/cpp/macros.h>  // nogncheck

#include "src/connectivity/network/drivers/network-device/log/log.h"  // nogncheck

inline constexpr const char* kLogTag = "network-device";

#define LOG_ERROR(msg)               \
  do {                               \
    FX_LOGST(ERROR, kLogTag) << msg; \
  } while (0)
#define LOG_WARN(msg)                  \
  do {                                 \
    FX_LOGST(WARNING, kLogTag) << msg; \
  } while (0)
#define LOG_INFO(msg)               \
  do {                              \
    FX_LOGST(INFO, kLogTag) << msg; \
  } while (0)
#define LOG_TRACE(msg)               \
  do {                               \
    FX_LOGST(TRACE, kLogTag) << msg; \
  } while (0)

#define LOGF_ERROR(fmt, ...) \
  Logf(fuchsia_logging::LogSeverity::Error, kLogTag, __FILE__, __LINE__, fmt, ##__VA_ARGS__)
#define LOGF_WARN(fmt, ...) \
  Logf(fuchsia_logging::LogSeverity::Warn, kLogTag, __FILE__, __LINE__, fmt, ##__VA_ARGS__)
#define LOGF_INFO(fmt, ...) \
  Logf(fuchsia_logging::LogSeverity::Info, kLogTag, __FILE__, __LINE__, fmt, ##__VA_ARGS__)
#define LOGF_TRACE(fmt, ...) \
  Logf(fuchsia_logging::LogSeverity::Trace, kLogTag, __FILE__, __LINE__, fmt, ##__VA_ARGS__)

#endif  // !NETDEV_DRIVER

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_LOG_H_
