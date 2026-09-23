// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <gtest/gtest-spi.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace {

// NOTE(https://github.com/google/googletest/issues/4740): EXPECT_FATAL_FAILURE
// chokes on templates with two variables (something about the comma confuses
// the macro). So, use these aliases to keep it happy.
using fit_result_status_int = fit::result<zx_status_t, int>;
using fit_result_string_int = fit::result<std::string, int>;

class PredicatesTest : public ::testing::Test {
 public:
  // Declare some constants with zx_status_t values to assert on error messages.
  static constexpr zx_status_t kStatusOk = ZX_OK;
  static constexpr zx_status_t kStatusErrInternal = ZX_ERR_INTERNAL;
  static constexpr zx_status_t kStatusErrNotFound = ZX_ERR_NOT_FOUND;
};

TEST_F(PredicatesTest, CompareOk) {
  constexpr const char* kErrorMsg = "kStatusErrInternal is ZX_ERR_INTERNAL, expected ZX_OK.";
  // Test failure and error message.
  EXPECT_FATAL_FAILURE(ASSERT_OK(kStatusErrInternal), kErrorMsg);
  EXPECT_NONFATAL_FAILURE(EXPECT_OK(kStatusErrInternal), kErrorMsg);
  // Test success case.
  ASSERT_OK(kStatusOk);
  EXPECT_OK(kStatusOk);
}

TEST_F(PredicatesTest, CompareStatus) {
  constexpr const char* kErrorMsg =
      "Value of: kStatusErrNotFound\n  Actual: ZX_ERR_NOT_FOUND\nExpected: "
      "kStatusErrInternal\nWhich is: ZX_ERR_INTERNAL";
  // Test failure and error message.
  EXPECT_FATAL_FAILURE(ASSERT_STATUS(kStatusErrNotFound, kStatusErrInternal), kErrorMsg);
  EXPECT_NONFATAL_FAILURE(EXPECT_STATUS(kStatusErrNotFound, kStatusErrInternal), kErrorMsg);
  // Test success case.
  ASSERT_STATUS(kStatusErrInternal, ZX_ERR_INTERNAL);
  EXPECT_STATUS(kStatusErrInternal, ZX_ERR_INTERNAL);
}

TEST_F(PredicatesTest, CompareOkFitResult) {
  fit::result<std::string, int> ok_result = fit::ok(100);
  ASSERT_OK(ok_result);
  EXPECT_OK(ok_result);

  constexpr const char* kErrorMsg = "error_result is fit::error(o hai), expected fit::ok().";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_string_int error_result = fit::error("o hai");
        ASSERT_OK(error_result);
      },
      kErrorMsg);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_string_int error_result = fit::error("o hai");
        EXPECT_OK(error_result);
      },
      kErrorMsg);
}

TEST_F(PredicatesTest, CompareOkFitResultWithNoValue) {
  fit::result<std::string> ok_result = fit::ok();
  ASSERT_OK(ok_result);
  EXPECT_OK(ok_result);

  constexpr const char* kErrorMsg = "error_result is fit::error(o hai), expected fit::ok().";
  EXPECT_FATAL_FAILURE(
      {
        fit::result<std::string> error_result = fit::error("o hai");
        ASSERT_OK(error_result);
      },
      kErrorMsg);
  EXPECT_NONFATAL_FAILURE(
      {
        fit::result<std::string> error_result = fit::error("o hai");
        EXPECT_OK(error_result);
      },
      kErrorMsg);
}

TEST_F(PredicatesTest, CompareStatusFitOkWithNoValue) {
  fit::result<zx_status_t> ok_result = fit::ok();
  ASSERT_STATUS(ok_result, ZX_OK);
  ASSERT_STATUS(ZX_OK, ok_result);
  ASSERT_STATUS(ok_result, fit::ok());
  ASSERT_STATUS(fit::ok(), ok_result);
  EXPECT_STATUS(ok_result, fit::ok());
  EXPECT_STATUS(fit::ok(), ok_result);

  constexpr const char* kErrorMsgOkResultComparedWithErrorResult =
      "Value of: ok_result\n"
      "  Actual: ZX_OK\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        fit::result<zx_status_t> ok_result = fit::ok();
        fit::result<zx_status_t> error_result = fit::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(ok_result, error_result);
      },
      kErrorMsgOkResultComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit::result<zx_status_t> ok_result = fit::ok();
        fit::result<zx_status_t> error_result = fit::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(ok_result, error_result);
      },
      kErrorMsgOkResultComparedWithErrorResult);
}

TEST_F(PredicatesTest, CompareStatusFitErrorWithNoValue) {
  fit::result<zx_status_t> error_result = fit::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, ZX_ERR_INTERNAL);
  EXPECT_STATUS(error_result, ZX_ERR_INTERNAL);
  ASSERT_STATUS(ZX_ERR_INTERNAL, error_result);
  EXPECT_STATUS(ZX_ERR_INTERNAL, error_result);

  ASSERT_STATUS(error_result, fit::error(ZX_ERR_INTERNAL));
  EXPECT_STATUS(error_result, fit::error(ZX_ERR_INTERNAL));
  ASSERT_STATUS(fit::error(ZX_ERR_INTERNAL), error_result);
  EXPECT_STATUS(fit::error(ZX_ERR_INTERNAL), error_result);

  fit::result<zx_status_t> error_result2 = fit::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, error_result2);
  EXPECT_STATUS(error_result, error_result2);

  constexpr const char* kErrorMsgErrorResultComparedWithOkResult =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: ok_result\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        fit::result<zx_status_t> error_result = fit::error(ZX_ERR_INTERNAL);
        fit::result<zx_status_t> ok_result = fit::ok();
        ASSERT_STATUS(error_result, ok_result);
      },
      kErrorMsgErrorResultComparedWithOkResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit::result<zx_status_t> error_result = fit::error(ZX_ERR_INTERNAL);
        fit::result<zx_status_t> ok_result = fit::ok();
        EXPECT_STATUS(error_result, ok_result);
      },
      kErrorMsgErrorResultComparedWithOkResult);

  constexpr const char* kErrorMsgErrorResultComparedWithErrorResult =
      "Value of: error_result1\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: error_result2\n"
      "Which is: ZX_ERR_NOT_FOUND";
  EXPECT_FATAL_FAILURE(
      {
        fit::result<zx_status_t> error_result1 = fit::error(ZX_ERR_INTERNAL);
        fit::result<zx_status_t> error_result2 = fit::error(ZX_ERR_NOT_FOUND);
        ASSERT_STATUS(error_result1, error_result2);
      },
      kErrorMsgErrorResultComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit::result<zx_status_t> error_result1 = fit::error(ZX_ERR_INTERNAL);
        fit::result<zx_status_t> error_result2 = fit::error(ZX_ERR_NOT_FOUND);
        EXPECT_STATUS(error_result1, error_result2);
      },
      kErrorMsgErrorResultComparedWithErrorResult);
}

TEST_F(PredicatesTest, CompareStatusFitOkWithValue) {
  fit_result_status_int ok_result = fit::ok(100);
  ASSERT_STATUS(ok_result, ZX_OK);
  ASSERT_STATUS(ZX_OK, ok_result);
  EXPECT_STATUS(ok_result, ZX_OK);
  EXPECT_STATUS(ZX_OK, ok_result);
  ASSERT_STATUS(ok_result, fit::ok());
  ASSERT_STATUS(fit::ok(), ok_result);
  EXPECT_STATUS(ok_result, fit::ok());
  EXPECT_STATUS(fit::ok(), ok_result);

  constexpr const char* kErrorMsgOkResultComparedWithErrorLiteral =
      "Value of: ok_result\n"
      "  Actual: ZX_OK\n"
      "Expected: fit::error((-1))\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int ok_result = fit::ok(100);
        ASSERT_STATUS(ok_result, fit::error(ZX_ERR_INTERNAL));
      },
      kErrorMsgOkResultComparedWithErrorLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int ok_result = fit::ok(100);
        EXPECT_STATUS(ok_result, fit::error(ZX_ERR_INTERNAL));
      },
      kErrorMsgOkResultComparedWithErrorLiteral);

  constexpr const char* kErrorMsgErrorLiteralComparedWithOkResult =
      "Value of: fit::error((-1))\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: ok_result\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int ok_result = fit::ok(100);
        ASSERT_STATUS(fit::error(ZX_ERR_INTERNAL), ok_result);
      },
      kErrorMsgErrorLiteralComparedWithOkResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int ok_result = fit::ok(100);
        EXPECT_STATUS(fit::error(ZX_ERR_INTERNAL), ok_result);
      },
      kErrorMsgErrorLiteralComparedWithOkResult);
}

TEST_F(PredicatesTest, CompareStatusFitErrorWithValue) {
  fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, ZX_ERR_INTERNAL);
  EXPECT_STATUS(error_result, ZX_ERR_INTERNAL);
  ASSERT_STATUS(ZX_ERR_INTERNAL, error_result);
  EXPECT_STATUS(ZX_ERR_INTERNAL, error_result);
  ASSERT_STATUS(error_result, fit::error(ZX_ERR_INTERNAL));
  EXPECT_STATUS(error_result, fit::error(ZX_ERR_INTERNAL));
  ASSERT_STATUS(fit::error(ZX_ERR_INTERNAL), error_result);
  EXPECT_STATUS(fit::error(ZX_ERR_INTERNAL), error_result);

  constexpr const char* kErrorMsgErrorResultComparedWithOkLiteral =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: fit::ok()\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(error_result, fit::ok());
      },
      kErrorMsgErrorResultComparedWithOkLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(error_result, fit::ok());
      },
      kErrorMsgErrorResultComparedWithOkLiteral);

  constexpr const char* kErrorMsgOkLiteralComparedWithErrorResult =
      "Value of: fit::ok()\n"
      "  Actual: ZX_OK\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(fit::ok(), error_result);
      },
      kErrorMsgOkLiteralComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(fit::ok(), error_result);
      },
      kErrorMsgOkLiteralComparedWithErrorResult);

  constexpr const char* kErrorMsgErrorResultComparedWithErrorLiteral =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: fit::error((-25))\n"
      "Which is: ZX_ERR_NOT_FOUND";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(error_result, fit::error(ZX_ERR_NOT_FOUND));
      },
      kErrorMsgErrorResultComparedWithErrorLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(error_result, fit::error(ZX_ERR_NOT_FOUND));
      },
      kErrorMsgErrorResultComparedWithErrorLiteral);

  constexpr const char* kErrorMsgErrorLiteralComparedWithErrorResult =
      "Value of: fit::error((-25))\n"
      "  Actual: ZX_ERR_NOT_FOUND\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(fit::error(ZX_ERR_NOT_FOUND), error_result);
      },
      kErrorMsgErrorLiteralComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        fit_result_status_int error_result = fit::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(fit::error(ZX_ERR_NOT_FOUND), error_result);
      },
      kErrorMsgErrorLiteralComparedWithErrorResult);
}

TEST_F(PredicatesTest, CompareOkZxResult) {
  zx::result<int> ok_result = zx::ok(100);
  ASSERT_OK(ok_result);
  EXPECT_OK(ok_result);

  constexpr const char* kErrorMsg = "error_result is ZX_ERR_NOT_FOUND, expected ZX_OK.";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_NOT_FOUND);
        ASSERT_OK(error_result);
      },
      kErrorMsg);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_NOT_FOUND);
        EXPECT_OK(error_result);
      },
      kErrorMsg);
}

TEST_F(PredicatesTest, CompareOkZxResultWithNoValue) {
  zx::result<> ok_result = zx::ok();
  ASSERT_OK(ok_result);
  EXPECT_OK(ok_result);

  constexpr const char* kErrorMsg = "error_result is ZX_ERR_NOT_FOUND, expected ZX_OK.";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<> error_result = zx::error(ZX_ERR_NOT_FOUND);
        ASSERT_OK(error_result);
      },
      kErrorMsg);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<> error_result = zx::error(ZX_ERR_NOT_FOUND);
        EXPECT_OK(error_result);
      },
      kErrorMsg);
}

TEST_F(PredicatesTest, CompareStatusZxOkWithNoValue) {
  zx::result<> ok_result = zx::ok();
  ASSERT_STATUS(ok_result, ZX_OK);
  ASSERT_STATUS(ZX_OK, ok_result);
  EXPECT_STATUS(ok_result, ZX_OK);
  EXPECT_STATUS(ZX_OK, ok_result);
  ASSERT_STATUS(ok_result, zx::ok());
  ASSERT_STATUS(zx::ok(), ok_result);
  EXPECT_STATUS(ok_result, zx::ok());
  EXPECT_STATUS(zx::ok(), ok_result);

  constexpr const char* kErrorMsgOkResultComparedWithErrorResult =
      "Value of: ok_result\n"
      "  Actual: ZX_OK\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<> ok_result = zx::ok();
        zx::result<> error_result = zx::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(ok_result, error_result);
      },
      kErrorMsgOkResultComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<> ok_result = zx::ok();
        zx::result<> error_result = zx::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(ok_result, error_result);
      },
      kErrorMsgOkResultComparedWithErrorResult);
}

TEST_F(PredicatesTest, CompareStatusZxErrorWithNoValue) {
  zx::result<> error_result = zx::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, ZX_ERR_INTERNAL);
  EXPECT_STATUS(error_result, ZX_ERR_INTERNAL);
  ASSERT_STATUS(ZX_ERR_INTERNAL, error_result);
  EXPECT_STATUS(ZX_ERR_INTERNAL, error_result);
  ASSERT_STATUS(error_result, zx::error(ZX_ERR_INTERNAL));
  EXPECT_STATUS(error_result, zx::error(ZX_ERR_INTERNAL));
  ASSERT_STATUS(zx::error(ZX_ERR_INTERNAL), error_result);
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), error_result);

  zx::result<> error_result2 = zx::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, error_result2);
  EXPECT_STATUS(error_result, error_result2);

  constexpr const char* kErrorMsgErrorResultComparedWithOkResult =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: ok_result\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<> error_result = zx::error(ZX_ERR_INTERNAL);
        zx::result<> ok_result = zx::ok();
        ASSERT_STATUS(error_result, ok_result);
      },
      kErrorMsgErrorResultComparedWithOkResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<> error_result = zx::error(ZX_ERR_INTERNAL);
        zx::result<> ok_result = zx::ok();
        EXPECT_STATUS(error_result, ok_result);
      },
      kErrorMsgErrorResultComparedWithOkResult);

  constexpr const char* kErrorMsgErrorResultComparedWithErrorResult =
      "Value of: error_result1\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: error_result2\n"
      "Which is: ZX_ERR_NOT_FOUND";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<> error_result1 = zx::error(ZX_ERR_INTERNAL);
        zx::result<> error_result2 = zx::error(ZX_ERR_NOT_FOUND);
        ASSERT_STATUS(error_result1, error_result2);
      },
      kErrorMsgErrorResultComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<> error_result1 = zx::error(ZX_ERR_INTERNAL);
        zx::result<> error_result2 = zx::error(ZX_ERR_NOT_FOUND);
        EXPECT_STATUS(error_result1, error_result2);
      },
      kErrorMsgErrorResultComparedWithErrorResult);
}

TEST_F(PredicatesTest, CompareStatusZxOkWithValue) {
  zx::result<int> ok_result = zx::ok(100);
  ASSERT_STATUS(ok_result, ZX_OK);
  ASSERT_STATUS(ZX_OK, ok_result);
  EXPECT_STATUS(ok_result, ZX_OK);
  EXPECT_STATUS(ZX_OK, ok_result);
  ASSERT_STATUS(ok_result, zx::ok());
  ASSERT_STATUS(zx::ok(), ok_result);
  EXPECT_STATUS(ok_result, zx::ok());
  EXPECT_STATUS(zx::ok(), ok_result);

  constexpr const char* kErrorMsgOkResultComparedWithErrorLiteral =
      "Value of: ok_result\n"
      "  Actual: ZX_OK\n"
      "Expected: zx::error((-1))\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> ok_result = zx::ok(100);
        ASSERT_STATUS(ok_result, zx::error(ZX_ERR_INTERNAL));
      },
      kErrorMsgOkResultComparedWithErrorLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> ok_result = zx::ok(100);
        EXPECT_STATUS(ok_result, zx::error(ZX_ERR_INTERNAL));
      },
      kErrorMsgOkResultComparedWithErrorLiteral);

  constexpr const char* kErrorMsgErrorLiteralComparedWithOkResult =
      "Value of: zx::error((-1))\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: ok_result\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> ok_result = zx::ok(100);
        ASSERT_STATUS(zx::error(ZX_ERR_INTERNAL), ok_result);
      },
      kErrorMsgErrorLiteralComparedWithOkResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> ok_result = zx::ok(100);
        EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), ok_result);
      },
      kErrorMsgErrorLiteralComparedWithOkResult);
}

TEST_F(PredicatesTest, CompareStatusZxErrorWithValue) {
  zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
  ASSERT_STATUS(error_result, ZX_ERR_INTERNAL);
  EXPECT_STATUS(error_result, ZX_ERR_INTERNAL);
  ASSERT_STATUS(ZX_ERR_INTERNAL, error_result);
  EXPECT_STATUS(ZX_ERR_INTERNAL, error_result);
  ASSERT_STATUS(error_result, zx::error(ZX_ERR_INTERNAL));
  EXPECT_STATUS(error_result, zx::error(ZX_ERR_INTERNAL));
  ASSERT_STATUS(zx::error(ZX_ERR_INTERNAL), error_result);
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), error_result);

  constexpr const char* kErrorMsgErrorResultComparedWithOkLiteral =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: zx::ok()\n"
      "Which is: ZX_OK";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(error_result, zx::ok());
      },
      kErrorMsgErrorResultComparedWithOkLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(error_result, zx::ok());
      },
      kErrorMsgErrorResultComparedWithOkLiteral);

  constexpr const char* kErrorMsgOkLiteralComparedWithErrorResult =
      "Value of: zx::ok()\n"
      "  Actual: ZX_OK\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(zx::ok(), error_result);
      },
      kErrorMsgOkLiteralComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(zx::ok(), error_result);
      },
      kErrorMsgOkLiteralComparedWithErrorResult);

  constexpr const char* kErrorMsgErrorResultComparedWithErrorLiteral =
      "Value of: error_result\n"
      "  Actual: ZX_ERR_INTERNAL\n"
      "Expected: zx::error((-25))\n"
      "Which is: ZX_ERR_NOT_FOUND";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(error_result, zx::error(ZX_ERR_NOT_FOUND));
      },
      kErrorMsgErrorResultComparedWithErrorLiteral);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(error_result, zx::error(ZX_ERR_NOT_FOUND));
      },
      kErrorMsgErrorResultComparedWithErrorLiteral);

  constexpr const char* kErrorMsgErrorLiteralComparedWithErrorResult =
      "Value of: zx::error((-25))\n"
      "  Actual: ZX_ERR_NOT_FOUND\n"
      "Expected: error_result\n"
      "Which is: ZX_ERR_INTERNAL";
  EXPECT_FATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        ASSERT_STATUS(zx::error(ZX_ERR_NOT_FOUND), error_result);
      },
      kErrorMsgErrorLiteralComparedWithErrorResult);
  EXPECT_NONFATAL_FAILURE(
      {
        zx::result<int> error_result = zx::error(ZX_ERR_INTERNAL);
        EXPECT_STATUS(zx::error(ZX_ERR_NOT_FOUND), error_result);
      },
      kErrorMsgErrorLiteralComparedWithErrorResult);
}

}  // namespace
