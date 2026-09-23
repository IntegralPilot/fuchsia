// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_LOG_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_LOG_H_

#include <lib/syslog/structured_backend/fuchsia_syslog.h>
#include <zircon/compiler.h>

#define VLOG_ENABLED 0

#if (VLOG_ENABLED)
#define VLOGF(format, ...) LOGF(format, ##__VA_ARGS__)
#else
#define VLOGF(...) \
  do {             \
  } while (0)
#endif

#define LOGF(format, ...)             \
  do {                                \
    LOG(INFO, format, ##__VA_ARGS__); \
  } while (0)

#define FUCHSIA_LOG_WARN FUCHSIA_LOG_WARNING
#define LOG(severity, format, formatted_args...)                                            \
  do {                                                                                      \
    codec_impl::internal::log_via_environment((FUCHSIA_LOG_##severity), __FILE__, __LINE__, \
                                              "(%s) " format, __func__, ##formatted_args);  \
  } while (0)

#define DBG_LINE() LOG(INFO, "")

namespace codec_impl {
namespace internal {

const char* BaseName(const char* path);

void log_via_environment(FuchsiaLogSeverity severity, const char* file, int line, const char* msg,
                         ...) __PRINTFLIKE(4, 5);

}  // namespace internal
}  // namespace codec_impl

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_INCLUDE_LIB_MEDIA_CODEC_IMPL_LOG_H_
