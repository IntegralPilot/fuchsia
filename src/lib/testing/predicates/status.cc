// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "status.h"

namespace testing_predicates {

zx_status_t internal::StatusValue(zx_status_t status) { return status; }

::testing::AssertionResult CmpOk(const char* l_expr, zx_status_t l) {
  if (l == ZX_OK) {
    return ::testing::AssertionSuccess();
  }

  return ::testing::AssertionFailure()
         << l_expr << " is " << zx_status_get_string(l) << ", expected ZX_OK.";
}

::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr, zx_status_t l,
                                     zx_status_t r) {
  if (l == r) {
    return ::testing::AssertionSuccess();
  }

  return ::testing::AssertionFailure() << "Value of: " << l_expr << "\n"
                                       << "  Actual: " << zx_status_get_string(l) << "\n"
                                       << "Expected: " << r_expr << "\n"
                                       << "Which is: " << zx_status_get_string(r);
}
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr, zx_status_t l,
                                     const fit::result<zx_status_t>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t>& l, zx_status_t r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}
::testing::AssertionResult CmpStatus(const char* l_expr, const char* r_expr,
                                     const fit::result<zx_status_t>& l,
                                     const fit::result<zx_status_t>& r) {
  return CmpStatus(l_expr, r_expr, internal::StatusValue(l), internal::StatusValue(r));
}

}  // namespace testing_predicates
