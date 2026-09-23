// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_ULIB_ZXTEST_INCLUDE_ZXTEST_CPP_ASSERT_STREAMS_H_
#define ZIRCON_SYSTEM_ULIB_ZXTEST_INCLUDE_ZXTEST_CPP_ASSERT_STREAMS_H_

#include <zxtest/cpp/internal.h>

#include "streams_helper.h"

// Helper to trick the compiler into ignoring lack of braces on the else branch.
// We CANNOT introduce braces at that point, since it would prevent the stream operator to take
// place.
#define LIB_ZXTEST_DISABLE_DANGLING_ELSE \
  switch (0)                             \
  case 0:                                \
  default:  // NOLINT

// Basic assert macro implementation with stream support.
#define LIB_ZXTEST_CHECK_VAR(op, expected, actual, fatal, file, line, desc, ...)                \
  LIB_ZXTEST_DISABLE_DANGLING_ELSE                                                              \
  if (auto assertion = zxtest::internal::EvaluateConditionForStream(                            \
          actual, expected, #actual, #expected, {.filename = file, .line_number = line}, fatal, \
          LIB_ZXTEST_COMPARE_FN(op), LIB_ZXTEST_DEFAULT_PRINTER, LIB_ZXTEST_DEFAULT_PRINTER);   \
      assertion == nullptr) {                                                                   \
  } else                                                                                        \
    LIB_ZXTEST_RETURN_TAG(fatal)                                                                \
  (*assertion) << desc << " "

#define LIB_ZXTEST_CHECK_VAR_STATUS(op, expected, actual, fatal, file, line, desc, ...)         \
  LIB_ZXTEST_DISABLE_DANGLING_ELSE                                                              \
  if (auto assertion = zxtest::internal::EvaluateConditionForStream(                            \
          zxtest::internal::GetStatus(actual), zxtest::internal::GetStatus(expected), #actual,  \
          #expected, {.filename = file, .line_number = line}, fatal, LIB_ZXTEST_COMPARE_FN(op), \
          LIB_ZXTEST_STATUS_PRINTER, LIB_ZXTEST_STATUS_PRINTER);                                \
      assertion == nullptr) {                                                                   \
  } else                                                                                        \
    LIB_ZXTEST_RETURN_TAG(fatal)                                                                \
  (*assertion) << desc << " "

#define LIB_ZXTEST_CHECK_VAR_COERCE(op, expected, actual, coerce_type, fatal, file, line, desc, \
                                    ...)                                                        \
  LIB_ZXTEST_DISABLE_DANGLING_ELSE                                                              \
  if (auto assertion = zxtest::internal::EvaluateConditionForStream(                            \
          actual, expected, #actual, #expected, {.filename = file, .line_number = line}, fatal, \
          [&](const auto& expected_, const auto& actual_) {                                     \
            using DecayType = typename std::decay<coerce_type>::type;                           \
            return op(static_cast<const DecayType&>(actual_),                                   \
                      static_cast<const DecayType&>(expected_));                                \
          },                                                                                    \
          LIB_ZXTEST_DEFAULT_PRINTER, LIB_ZXTEST_DEFAULT_PRINTER);                              \
      assertion == nullptr) {                                                                   \
  } else                                                                                        \
    LIB_ZXTEST_RETURN_TAG(fatal)                                                                \
  (*assertion) << desc << " "

#define LIB_ZXTEST_CHECK_VAR_BYTES(op, expected, actual, size, fatal, file, line, desc, ...)   \
  LIB_ZXTEST_DISABLE_DANGLING_ELSE                                                             \
  if (auto assertion = zxtest::internal::EvaluateConditionForStream(                           \
          zxtest::internal::ToPointer(actual), zxtest::internal::ToPointer(expected), #actual, \
          #expected, {.filename = file, .line_number = line}, fatal,                           \
          LIB_ZXTEST_COMPARE_3_FN(op, size), LIB_ZXTEST_HEXDUMP_PRINTER(size),                 \
          LIB_ZXTEST_HEXDUMP_PRINTER(size));                                                   \
      assertion == nullptr) {                                                                  \
  } else                                                                                       \
    LIB_ZXTEST_RETURN_TAG(fatal)                                                               \
  (*assertion) << desc << " "

#define LIB_ZXTEST_FAIL_NO_RETURN(fatal, desc, ...)                                        \
  zxtest::internal::StreamableFail({.filename = __FILE__, .line_number = __LINE__}, fatal, \
                                   zxtest::Runner::GetInstance()->GetScopedTraces())       \
      << desc << " "

#define LIB_ZXTEST_ASSERT_ERROR(has_errors, fatal, desc, ...) \
  LIB_ZXTEST_DISABLE_DANGLING_ELSE                            \
  if (!has_errors) {                                          \
  } else                                                      \
    LIB_ZXTEST_RETURN_TAG(fatal) LIB_ZXTEST_FAIL_NO_RETURN(fatal, desc, ##__VA_ARGS__)

#define FAIL(...) LIB_ZXTEST_RETURN_TAG(true) LIB_ZXTEST_FAIL_NO_RETURN(true, "", ##__VA_ARGS__)

#define ZXTEST_SKIP(...)      \
  LIB_ZXTEST_RETURN_TAG(true) \
  zxtest::internal::StreamableSkip({.filename = __FILE__, .line_number = __LINE__})

#endif  // ZIRCON_SYSTEM_ULIB_ZXTEST_INCLUDE_ZXTEST_CPP_ASSERT_STREAMS_H_
