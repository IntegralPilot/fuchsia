// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests.h"

namespace {

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

using dl::testing::Found;
using dl::testing::IsDepFileNotFoundErrMsg;
using dl::testing::IsFileNotFoundErrMsg;
using dl::testing::IsUndefinedSymbolErrMsg;
using dl::testing::NotFound;
using dl::testing::TestModule;
using dl::testing::TestShlib;
using dl::testing::TestSym;

TYPED_TEST(DlTests, InvalidMode) {
  const std::string kRet17File = TestModule("ret17");

  if constexpr (!TestFixture::kCanValidateMode) {
    GTEST_SKIP() << "test requires dlopen to validate mode argment";
  }

  int bad_mode = -1;
  // The sanitizer runtimes (on non-Fuchsia hosts) intercept dlopen calls with
  // RTLD_DEEPBIND and make them fail without really calling -ldl's dlopen to
  // see if it would fail anyway.  So avoid having that flag set in the bad
  // mode argument.
#ifdef RTLD_DEEPBIND
  bad_mode &= ~RTLD_DEEPBIND;
#endif
  // Make sure the bad_mode does not produce a false positive with RTLD_NOLOAD
  // checks by the test fixture.
  bad_mode &= ~RTLD_NOLOAD;

  auto open = this->DlOpen(kRet17File.c_str(), bad_mode);
  ASSERT_TRUE(open.is_error());
  EXPECT_EQ(open.error_value().take_str(), "invalid mode parameter")
      << "for mode argument " << bad_mode;
}

TYPED_TEST(DlTests, NotFound) {
  const std::string kDoesNotExistFile = TestModule("does-not-exist");

  this->ExpectMissing(kDoesNotExistFile);

  auto open = this->DlOpen(kDoesNotExistFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_error());
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(open.error_value().take_str(), "does-not-exist.NotFound.module.so not found");
  } else {
    EXPECT_THAT(open.error_value().take_str(), IsFileNotFoundErrMsg(kDoesNotExistFile));
  }
}

// TODO(https://fxbug.dev/339028040): Test missing symbol in transitive dep.
// Load a module that depends on libld-dep-a.so, but this dependency does not
// provide the c symbol referenced by the root module, so relocation fails.
TYPED_TEST(DlTests, MissingSymbol) {
  const std::string kMissingSymFile = TestModule("missing-sym");
  const std::string kMissingSymDepFile = TestShlib("libld-dep-missing-sym-dep");

  this->ExpectRootModule(kMissingSymFile);
  this->Needed({kMissingSymDepFile});

  auto open = this->DlOpen(kMissingSymFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_error());
  EXPECT_THAT(open.error_value().take_str(),
              IsUndefinedSymbolErrMsg(TestSym("missing_sym"), kMissingSymFile));
}

// TODO(https://fxbug.dev/339662473): Test simple case of transitive missing
// symbol.
// dlopen missing-transitive-symbol:
//  - missing-transitive-sym
//    - has-missing-sym does not define missing_sym()
// call missing_sym() from missing-transitive-symbol, and expect symbol not found

// Try to load a module that has a (direct) dependency that cannot be found.
TYPED_TEST(DlTests, MissingDependency) {
  const std::string kMissingDepFile = TestModule("missing-dep");
  const std::string kMissingDepDepFile = TestShlib("libmissing-dep-dep");

  this->ExpectRootModule(kMissingDepFile);
  this->Needed({NotFound(kMissingDepDepFile)});

  auto open = this->DlOpen(kMissingDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_error());

  // TODO(https://fxbug.dev/336633049): Harmonize "not found" error messages
  // between implementations.
  // Expect that the dependency lib to missing-dep.module.so cannot be found.
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(open.error_value().take_str(),
              "cannot open dependency: libmissing-dep-dep.MissingDependency.so");
  } else {
    EXPECT_THAT(open.error_value().take_str(),
                IsDepFileNotFoundErrMsg(kMissingDepFile, kMissingDepDepFile));
  }
}

// Try to load a module where the dependency of its direct dependency (i.e. a
// transitive dependency of the root module) cannot be found.
TYPED_TEST(DlTests, MissingTransitiveDependency) {
  const std::string kMissingTransitiveDepFile = TestModule("missing-transitive-dep");
  const std::string kHasMissingDepFile = TestShlib("libhas-missing-dep");
  const std::string kMissingDepDepFile = TestShlib("libmissing-dep-dep");

  this->ExpectRootModule(kMissingTransitiveDepFile);
  this->Needed({Found(kHasMissingDepFile), NotFound(kMissingDepDepFile)});

  auto open = this->DlOpen(kMissingTransitiveDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  // TODO(https://fxbug.dev/336633049): Harmonize "not found" error messages
  // between implementations.
  // Expect that the dependency lib to libhas-missing-dep.so cannot be found.
  if constexpr (TestFixture::kCanMatchExactError) {
    EXPECT_EQ(open.error_value().take_str(),
              "cannot open dependency: libmissing-dep-dep.MissingTransitiveDependency.so");
  } else {
    EXPECT_THAT(open.error_value().take_str(),
                IsDepFileNotFoundErrMsg(kHasMissingDepFile, kMissingDepDepFile));
  }
}

}  // namespace
