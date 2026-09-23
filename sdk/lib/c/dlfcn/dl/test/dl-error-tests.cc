// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests.h"
namespace {

using dl::testing::IsFileNotFoundErrMsg;
using dl::testing::IsUndefinedSymbolErrMsg;
using dl::testing::TestModule;
using dl::testing::TestSym;

using dl::testing::DlSystemTests;

// TODO(https://fxbug.dev/478035413): These tests are only testing DlSystemTests
// implementations to verify their behavior, but to do so DlSystemsTests fixture
// is modified to keep track of dlerror state. Consider an alternative to using
// the DlSystemTests fixture for these tests and just invoke the system dl
// calls directly (see https://fxrev.dev/1342024/comment/4ffd8a74_16725ea3/).

// TODO(https://fxbug.dev/342028933): Add test for error reporting after
// dlclose().

// Test that dlerror will report a failure from dlopen.
TEST_F(DlSystemTests, DlErrorDlOpen) {
  const std::string kDoesNotExistFile = TestModule("does-not-exist");

  this->ExpectMissing(kDoesNotExistFile);

  auto open = this->DlOpen(kDoesNotExistFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_error());
  // Disarm the error object: we will check dlerror manually.
  std::ignore = std::move(open.error_value()).take();

  auto error = this->DlError();
  EXPECT_TRUE(error);
  EXPECT_THAT(error->take_str(), IsFileNotFoundErrMsg(kDoesNotExistFile));
}

// Test that dlerror will report a failure from dlsym.
TEST_F(DlSystemTests, DlErrorDlSym) {
  const std::string kRet17File = TestModule("ret17");
  const std::string kDoesNotExistSym = TestSym("does-not-exist");

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok());
  ASSERT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), kDoesNotExistSym.c_str());
  EXPECT_TRUE(sym.is_error());

  auto error = this->DlError();
  EXPECT_TRUE(error);
  EXPECT_THAT(error->take_str(), IsUndefinedSymbolErrMsg(kDoesNotExistSym, kRet17File));

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that dlerror will report a failure from dlinfo.
TEST_F(DlSystemTests, DlErrorDlInfo) {
  const std::string kRet17File = TestModule("ret17");
  struct link_map *link_map = nullptr;

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto res = this->DlInfo(open.value(), -1, &link_map);
  ASSERT_TRUE(res.is_error());

  auto error = this->DlError();
  EXPECT_TRUE(error);

  if constexpr (kEmitsDlInfoUnsupportedValue) {
    EXPECT_EQ(error->take_str(), "Unsupported request -1");
  } else {
    EXPECT_EQ(error->take_str(), "unsupported dlinfo request");
  }

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that in the event of multiple errors, dlerror() will report the failure
// from the last error to have occurred.
TEST_F(DlSystemTests, DlErrorMultiple) {
  const std::string kDoesNotExistFile = TestModule("does-not-exist");
  const std::string kAnotherDoesNotExistFile = TestModule("another-does-not-exist");

  this->ExpectMissing(kDoesNotExistFile);

  auto open_dne = this->DlOpen(kDoesNotExistFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_dne.is_error());
  std::ignore = std::move(open_dne.error_value()).take();

  this->ExpectMissing(kAnotherDoesNotExistFile);

  auto open_another_dne = this->DlOpen(kAnotherDoesNotExistFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_another_dne.is_error());
  std::ignore = std::move(open_another_dne.error_value()).take();

  auto error = this->DlError();
  EXPECT_TRUE(error);
  EXPECT_THAT(error->take_str(), IsFileNotFoundErrMsg(kAnotherDoesNotExistFile));
}

// Test that dlerror will clear its error state if a subsequent function call
// succeeds.
TEST_F(DlSystemTests, DlErrorEphemeral) {
  const std::string kDoesNotExistFile = TestModule("does-not-exist");
  const std::string kRet17File = TestModule("ret17");

  this->ExpectMissing(kDoesNotExistFile);

  auto open_dne = this->DlOpen(kDoesNotExistFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_dne.is_error());
  std::ignore = std::move(open_dne.error_value()).take();

  this->ExpectRootModule(kRet17File);

  auto ret17_open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(ret17_open.is_ok());

  // Test that dlerror() returns null, indicating the error from the first
  // dlopen has been cleared.
  auto error = this->DlError();
  EXPECT_FALSE(error);

  ASSERT_TRUE(this->DlClose(ret17_open.value()).is_ok());
}
}  // namespace
