// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "load-tests.h"

namespace ld::testing {
namespace {

TYPED_TEST(LdLoadTests, TlsExecOnly) {
  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-exec-only"));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

TYPED_TEST(LdLoadTests, TlsShlibOnly) {
  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-shlib-only"));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

TYPED_TEST(LdLoadTests, TlsExecShlib) {
  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-exec-shlib"));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

TYPED_TEST(LdLoadTests, TlsInitialExecAccess) {
  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-ie-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-ie"));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

TYPED_TEST(LdLoadTests, TlsGlobalDynamicAccess) {
  constexpr int64_t kReturnValue = 17;
  constexpr int64_t kSkipReturnValue = 77;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-gd"));

  const int64_t return_value = this->Run();

  // Check the log before the return value so we've handled it in case we skip.
  this->ExpectLog("");

  if (return_value == kSkipReturnValue) {
    GTEST_SKIP() << "tls-gd module compiled with TLSDESC";
  }

  EXPECT_EQ(return_value, kReturnValue);
}

TYPED_TEST(LdLoadTests, TlsDescAccess) {
  constexpr int64_t kReturnValue = 17;
  constexpr int64_t kSkipReturnValue = 77;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-desc-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-desc"));

  const int64_t return_value = this->Run();

  // Check the log before the return value so we've handled it in case we skip.
  this->ExpectLog("");

  if (return_value == kSkipReturnValue) {
    GTEST_SKIP() << "tls-desc module compiled without TLSDESC";
  }

  EXPECT_EQ(return_value, kReturnValue);
}

TYPED_TEST(LdLoadTests, TlsLocalDynamicAccess) {
  constexpr int64_t kReturnValue = 17;
  constexpr int64_t kSkipReturnValue = 77;

  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libtls-ld-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(this->Load("tls-ld"));

  const int64_t return_value = this->Run();

  // Check the log before the return value so we've handled it in case we skip.
  this->ExpectLog("");

  if (return_value == kSkipReturnValue) {
    GTEST_SKIP() << "tls-ld module compiled with TLSDESC";
  }

  EXPECT_EQ(return_value, kReturnValue);
}

}  // namespace
}  // namespace ld::testing
