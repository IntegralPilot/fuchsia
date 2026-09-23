// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TESTING_PREDICATES_STATUS_H_
#define SRC_LIB_TESTING_PREDICATES_STATUS_H_

#include <lib/fit/result.h>
#include <zircon/status.h>

#include <gtest/gtest.h>

// Helper macro that asserts that `condition` equals `ZX_OK`, `fit::ok()`,
// or `zx::ok()`. Behaves similarly to `ASSERT_EQ(condition, ZX_OK)` but with
// prettier output.
#define ASSERT_OK(condition) \
  GTEST_PRED_FORMAT1_(::testing_predicates::CmpOk, condition, GTEST_FATAL_FAILURE_)
// Helper macro that expects that `condition` equals `ZX_OK`, `fit::ok()`,
// or `zx::ok()`. Behaves similarly to `EXPECT_EQ(condition, ZX_OK)` but with
// prettier output.
#define EXPECT_OK(condition) \
  GTEST_PRED_FORMAT1_(::testing_predicates::CmpOk, condition, GTEST_NONFATAL_FAILURE_)
// Helper macro that asserts equality between `zx_status_t` expressions `val1` and `val2`.
// Behaves similarly to `ASSERT_EQ(val1, val2)` but with prettier output.
#define ASSERT_STATUS(val1, val2) \
  GTEST_PRED_FORMAT2_(::testing_predicates::CmpStatus, val1, val2, GTEST_FATAL_FAILURE_)
// Helper macro that expects equality between `zx_status_t` expressions `val1` and `val2`.
// Behaves similarly to `EXPECT_EQ(val1, val2)` but with prettier output.
#define EXPECT_STATUS(val1, val2) \
  GTEST_PRED_FORMAT2_(::testing_predicates::CmpStatus, val1, val2, GTEST_NONFATAL_FAILURE_)

namespace testing_predicates {
::testing::AssertionResult CmpOk(const char* l_expr, zx_status_t l);

namespace internal {

zx_status_t StatusValue(zx_status_t status);
template <typename... Ts>
zx_status_t StatusValue(const fit::result<zx_status_t, Ts...>& r) {
  if (r.is_ok()) {
    return ZX_OK;
  }
  return r.error_value();
}

template <typename... Ts>
::testing::AssertionResult CmpOkImpl(const char* l_expr, const fit::result<zx_status_t, Ts...>& l) {
  return CmpOk(l_expr, StatusValue(l));
}

template <typename E, typename... Ts>
::testing::AssertionResult CmpOkImpl(const char* l_expr, const fit::result<E, Ts...>& l) {
  if (l.is_ok()) {
    return ::testing::AssertionSuccess();
  }
  return ::testing::AssertionFailure()
         << l_expr << " is fit::error(" << l.error_value() << "), expected fit::ok().";
}

}  // namespace internal

template <typename E>
::testing::AssertionResult CmpOk(const char* l_expr, const fit::result<E>& l) {
  return internal::CmpOkImpl(l_expr, l);
}

template <typename E, typename T>
::testing::AssertionResult CmpOk(const char* l_expr, const fit::result<E, T>& l) {
  return internal::CmpOkImpl(l_expr, l);
}

// Nine variants of CmpStatus, accepting zx_status_t, fit::result<zx_status_t>,
// or fit::result<zx_status_t, T> for each argument. The latter 8
// implementations all delegate to the first.
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr, zx_status_t l,
                                     zx_status_t r);
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr, zx_status_t l,
                                     const fit::result<zx_status_t>& r);
template <typename R>
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr, zx_status_t l,
                                     const fit::result<zx_status_t, R>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t>& l, zx_status_t r);
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t>& l,
                                     const fit::result<zx_status_t>& r);
template <typename R>
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t>& l,
                                     const fit::result<zx_status_t, R>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
template <typename L>
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t, L>& l, zx_status_t r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
template <typename L>
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t, L>& l,
                                     const fit::result<zx_status_t>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
template <typename L, typename R>
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t, L>& l,
                                     const fit::result<zx_status_t, R>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}

}  // namespace testing_predicates

#endif  // SRC_LIB_TESTING_PREDICATES_STATUS_H_
