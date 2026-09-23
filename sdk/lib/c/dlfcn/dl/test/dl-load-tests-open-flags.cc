// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests.h"

namespace {

using dl::testing::DlTests;
using dl::testing::TestModule;
using dl::testing::TestShlib;

TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

// If RTLD_NOLOAD is passed and the module is not found, NULL is returned but an
// error is not expected.
TYPED_TEST(DlTests, RtldNoLoadBasic) {
  const std::string kRet17File = TestModule("ret17");

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_EQ(open.value(), nullptr);

  // TODO(https://fxbug.dev/325494556): check that dlerror() returns NULL.
}

// Test that RTLD_NODELETE prevents a module from being unloaded by dlclose().
// dlopen ret17, RTLD_NODELETE)
// dlclose(ret17)
// dlopen(ret17, RTLD_NOLOAD) and expect the module to still be loaded.
TYPED_TEST(DlTests, RtldNoDeleteBasic) {
  const std::string kRet17File = TestModule("ret17");

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  // Test that dlclose will succeed, but not unload the module.
  EXPECT_TRUE(this->DlClose(open.value()).is_ok());

  auto reopen = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(reopen.is_ok()) << reopen.error_value();
  // The open handle should still be valid memory.
  EXPECT_EQ(reopen.value(), open.value());

  ASSERT_TRUE(this->DlClose(reopen.value()).is_ok());
}

// Test that "promoting" a module with RTLD_NODELETE will prevent the module
// from being unloaded.
// dlopen ret17
// dlopen ret17 w/ RTLD_NO_DELETE
// dlopen ret17 to try to "unset" RTLD_NODELETE
// dlclose ret17 and expect ret17 to still be loaded.
TYPED_TEST(DlTests, RtldNoDeletePromotion) {
  const std::string kRet17File = TestModule("ret17");

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto promoted =
      this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE | RTLD_NOLOAD);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  // Expect the same handle is returned.
  EXPECT_EQ(promoted.value(), open.value());

  // Test that once RTLD_NODELETE is set, it cannot be "unset" by its omittance
  // from a subsequent dlopen call.
  auto try_unset = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(try_unset.is_ok()) << try_unset.error_value();
  // Expect the same handle is returned.
  EXPECT_EQ(try_unset.value(), promoted.value());

  // Call dlclose on all the handles.
  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
  ASSERT_TRUE(this->DlClose(promoted.value()).is_ok());
  ASSERT_TRUE(this->DlClose(try_unset.value()).is_ok());

  auto still_loaded = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(still_loaded.is_ok()) << still_loaded.error_value();
  // The try_unset handle should still be valid memory.
  EXPECT_EQ(still_loaded.value(), try_unset.value());

  // A final corresponding dlclose() to resolve the TestFixture's tracking of
  // open modules.
  ASSERT_TRUE(this->DlClose(still_loaded.value()).is_ok());
}

// Test that dlopen a dep with RTLD_NODELETE will not affect a module that is
// subsequently dlopen-ed and depends on the dep.
// dlopen foo-v1 with RTLD_NODELETE
// dlopen has-foo-v1
//  - foo-v1
// dlclose foo-v1, dlclose has-foo-v1 and expect only foo-v1 to still be loaded.
TYPED_TEST(DlTests, RtldNoDeleteDep) {
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");

  this->Needed({kFooV1File});

  auto foo = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE);
  ASSERT_TRUE(foo.is_ok()) << foo.error_value();
  EXPECT_TRUE(foo.value());

  this->Needed({kHasFooV1File});

  auto has_foo = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(has_foo.is_ok()) << has_foo.error_value();
  EXPECT_TRUE(has_foo.value());

  // dlclose the modules
  ASSERT_TRUE(this->DlClose(has_foo.value()).is_ok());
  ASSERT_TRUE(this->DlClose(foo.value()).is_ok());

  // Check that the root is no longer loaded.
  if constexpr (TestFixture::kDlCloseUnloadsModules) {
    auto check_unloaded = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
    ASSERT_TRUE(check_unloaded.is_ok()) << check_unloaded.error_value();
    EXPECT_FALSE(check_unloaded.value());
  }

  // Check that the dependency is still loaded.
  auto still_loaded = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(still_loaded.is_ok()) << still_loaded.error_value();
  // The foo handle should still point to valid memory.
  EXPECT_EQ(still_loaded.value(), foo.value());

  // A final corresponding dlclose() to resolve the TestFixture's tracking of
  // open modules.
  ASSERT_TRUE(this->DlClose(still_loaded.value()).is_ok());
}

// Test that a root module can still be unloaded after its dep gets promoted
// with RTLD_NODELETE
// dlopen has-foo-v1
//  - foo-v1
// dlopen foo-v1 with RTLD_NODELETE
// dlclose has-foo-v1 and expect foo-v1 to still be loaded.
TYPED_TEST(DlTests, RtldNoDeleteDepPromotion) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");

  this->Needed({kHasFooV1File, kFooV1File});
  auto has_foo = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(has_foo.is_ok()) << has_foo.error_value();
  EXPECT_TRUE(has_foo.value());

  // Promote the dependency with RTLD_NODELETE.
  auto foo = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE | RTLD_NOLOAD);
  ASSERT_TRUE(foo.is_ok()) << foo.error_value();
  EXPECT_TRUE(foo.value());

  // Close the root and dependency.
  ASSERT_TRUE(this->DlClose(has_foo.value()).is_ok());
  ASSERT_TRUE(this->DlClose(foo.value()).is_ok());

  // Check that the root is no longer loaded.
  if constexpr (TestFixture::kDlCloseUnloadsModules) {
    auto check_unloaded = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
    ASSERT_TRUE(check_unloaded.is_ok()) << check_unloaded.error_value();
    EXPECT_FALSE(check_unloaded.value());
  }

  // Check that the dependency is still loaded.
  auto still_loaded = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(still_loaded.is_ok()) << still_loaded.error_value();
  // The foo handle should still point to valid memory.
  EXPECT_EQ(still_loaded.value(), foo.value());

  // A final corresponding dlclose() to resolve the TestFixture's tracking of
  // open modules.
  ASSERT_TRUE(this->DlClose(still_loaded.value()).is_ok());
}

}  // namespace
