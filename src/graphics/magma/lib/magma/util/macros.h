// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_MACROS_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_MACROS_H_

#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

// Files #including macros.h may assume that it #includes inttypes.h.
// So, for convenience, they don't need to follow "#include-what-you-use" for that header.
#include <inttypes.h>
#include <lib/magma/platform/platform_logger.h>

#ifndef MAGMA_DEBUG_INTERNAL_USE_ONLY
#error MAGMA_DEBUG_INTERNAL_USE_ONLY not defined, your gn foo needs magma_util_config
#endif

namespace magma {

static constexpr bool kDebug = MAGMA_DEBUG_INTERNAL_USE_ONLY;

#define MAGMA_DASSERT(x)                                                               \
  do {                                                                                 \
    if (magma::kDebug && !(x)) {                                                       \
      magma::PlatformLogger::Log(magma::PlatformLogger::LOG_ERROR, __FILE__, __LINE__, \
                                 "MAGMA_DASSERT: %s", #x);                             \
      abort();                                                                         \
    }                                                                                  \
  } while (0)

#define MAGMA_DMESSAGE(format, ...)                                                           \
  do {                                                                                        \
    if (magma::kDebug) {                                                                      \
      magma::PlatformLogger::Log(magma::PlatformLogger::LOG_INFO, __FILE__, __LINE__, format, \
                                 ##__VA_ARGS__);                                              \
    }                                                                                         \
  } while (0)

static constexpr bool kMagmaDretEnable = kDebug;

#define MAGMA_DRET(ret)                                                               \
  (magma::kMagmaDretEnable && (ret) != 0                                              \
   ? magma::PlatformLogger::Log(magma::PlatformLogger::LOG_ERROR, __FILE__, __LINE__, \
                                "Returning error %" PRId64, (int64_t)(ret)),          \
   (ret) : (ret))

#define MAGMA_DRET_MSG(ret, format, ...)                                                \
  (magma::kMagmaDretEnable && (ret) != 0                                                \
   ? magma::PlatformLogger::Log(magma::PlatformLogger::LOG_ERROR, __FILE__, __LINE__,   \
                                "Returning error %" PRId64 ": " format, (int64_t)(ret), \
                                ##__VA_ARGS__),                                         \
   (ret) : (ret))

#define MAGMA_DRETF(ret, format, ...)                                                 \
  (magma::kMagmaDretEnable && !(ret)                                                  \
   ? magma::PlatformLogger::Log(magma::PlatformLogger::LOG_ERROR, __FILE__, __LINE__, \
                                "Returning false: " format, ##__VA_ARGS__),           \
   (ret) : (ret))

#define MAGMA_DRETP(ret, format, ...)                                                 \
  (magma::kMagmaDretEnable && ((ret) == nullptr)                                      \
   ? magma::PlatformLogger::Log(magma::PlatformLogger::LOG_ERROR, __FILE__, __LINE__, \
                                "Returning null: " format, ##__VA_ARGS__),            \
   (ret) : (ret))

#define MAGMA_UNIMPLEMENTED(...)          \
  do {                                    \
    DLOG("UNIMPLEMENTED: " #__VA_ARGS__); \
    MAGMA_DASSERT(false);                 \
  } while (0)

#define MAGMA_ATTRIBUTE_UNUSED __attribute__((unused))

}  // namespace magma

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_UTIL_MACROS_H_
