// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-iterate-phdr-tests.h"
#include "dl-load-tests.h"
#include "startup-symbols.h"

namespace {

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

using dl::testing::GetPhdrInfoForModule;
using dl::testing::IsUndefinedSymbolErrMsg;
using dl::testing::RunFunction;
using dl::testing::TestModule;
using dl::testing::TestShlib;
using dl::testing::TestSym;

// These are test scenarios that test symbol resolution with RTLD_GLOBAL.

// Test that a non-global module can depend on a previously loaded global
// module, and that dlsym() will be able to access the global module's symbols
// from the non-global module.
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen has-foo-v2:
//    - foo-v2 -> foo() returns 7
// call foo() from has-foo-v2 and expect foo() to return 7 (from previously
// loaded RTLD_GLOBAL foo-v2).
TYPED_TEST(DlTests, GlobalDep) {
  const auto kGlobalFooV2File = TestShlib("libld-dep-foo-v2");
  const auto kHasFooV2File = TestShlib("libhas-foo-v2");
  constexpr int64_t kReturnValueFromGlobalDep = 7;

  this->Needed({kGlobalFooV2File});

  auto open_global_foo_v2 = this->DlOpen(kGlobalFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v2.is_ok()) << open_global_foo_v2.error_value();
  EXPECT_TRUE(open_global_foo_v2.value());

  auto global_foo_v2_foo = this->DlSym(open_global_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(global_foo_v2_foo.is_ok()) << global_foo_v2_foo.error_value();
  ASSERT_TRUE(global_foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(global_foo_v2_foo.value()), kReturnValueFromGlobalDep);

  this->Needed({kHasFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  ASSERT_TRUE(has_foo_v2_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kReturnValueFromGlobalDep);

  auto has_foo_v2_foo = this->DlSym(open_has_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v2_foo.is_ok()) << has_foo_v2_foo.error_value();
  ASSERT_TRUE(has_foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_foo.value()), kReturnValueFromGlobalDep);

  ASSERT_TRUE(this->DlClose(open_global_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
}

// Test that loaded global module will take precedence over dependency ordering.
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen has-foo-v1:
//    - foo-v1 -> foo() returns 2
// call foo() from has-foo-v1 and expect foo() to return 7 (from previously
// loaded RTLD_GLOBAL foo-v2).
TYPED_TEST(DlTests, GlobalPrecedence) {
  const std::string kGlobalFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kGlobalFooV2File});

  auto open_global_foo_v2 = this->DlOpen(kGlobalFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v2.is_ok()) << open_global_foo_v2.error_value();
  EXPECT_TRUE(open_global_foo_v2.value());

  auto global_foo_v2_foo = this->DlSym(open_global_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(global_foo_v2_foo.is_ok()) << global_foo_v2_foo.error_value();
  ASSERT_TRUE(global_foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(global_foo_v2_foo.value()), kFooV2ReturnValue);

  this->Needed({kHasFooV1File, kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto has_foo_v1_call_foo = this->DlSym(open_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v1_call_foo.is_ok()) << has_foo_v1_call_foo.error_value();
  ASSERT_TRUE(has_foo_v1_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_call_foo.value()), kFooV2ReturnValue);

  // dlsym will always use dependency ordering from the local scope when looking
  // up a symbol
  auto has_foo_v1_foo = this->DlSym(open_has_foo_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v1_foo.is_ok()) << has_foo_v1_foo.error_value();
  ASSERT_TRUE(has_foo_v1_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_global_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
}

// Test that RTLD_GLOBAL applies to deps and load order will take precedence in
// subsequent symbol lookups:
// dlopen RTLD_GLOBAL has-foo-v1:
//   - foo-v1 -> foo() returns 2
// dlopen RTLD_GLOBAL has-foo-v2:
//   - foo-v2 -> foo() returns 7
// call foo from has-foo-v2 and expect 2.
TYPED_TEST(DlTests, GlobalPrecedenceDeps) {
  const std::string GlobalFooV1ParentFile = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({GlobalFooV1ParentFile, kFooV1File});

  auto open_global_has_foo_v1 = this->DlOpen(GlobalFooV1ParentFile.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_has_foo_v1.is_ok()) << open_global_has_foo_v1.error_value();
  EXPECT_TRUE(open_global_has_foo_v1.value());

  auto global_has_foo_v1_call_foo =
      this->DlSym(open_global_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(global_has_foo_v1_call_foo.is_ok()) << global_has_foo_v1_call_foo.error_value();
  ASSERT_TRUE(global_has_foo_v1_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(global_has_foo_v1_call_foo.value()), kFooV1ReturnValue);

  this->Needed({kHasFooV2File, kFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  ASSERT_TRUE(has_foo_v2_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kFooV1ReturnValue);

  // dlsym will always use dependency ordering from the local scope when looking
  // up a symbol
  auto has_foo_v2_foo = this->DlSym(open_has_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v2_foo.is_ok()) << has_foo_v2_foo.error_value();
  ASSERT_TRUE(has_foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_foo.value()), kFooV2ReturnValue);

  ASSERT_TRUE(this->DlClose(open_global_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
}

// Test that missing dep will use global symbol if there's a loaded global
// module with the same symbol
// dlopen RTLD global defines-missing-sym -> missing_sym() returns 2
// dlopen missing-sym -> TestStart() returns 2 + missing_sym():
//  - missing-sym-dep (does not have missing_sym())
// call missing_sym() from missing-sym and expect 6 (4 + 2 from previously
// loaded module).
TYPED_TEST(DlTests, GlobalSatisfiesMissingSymbol) {
  const std::string kGlobalDefinesSymFile = TestShlib("libld-dep-defines-missing-sym");
  const std::string kMissingSymFile = TestModule("missing-sym");
  const std::string kMissingSymDepFile = TestShlib("libld-dep-missing-sym-dep");
  constexpr int64_t kReturnValue = 6;

  this->Needed({kGlobalDefinesSymFile});

  auto open_global_defines_sym =
      this->DlOpen(kGlobalDefinesSymFile.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_defines_sym.is_ok()) << open_global_defines_sym.error_value();
  EXPECT_TRUE(open_global_defines_sym.value());

  this->ExpectRootModule(kMissingSymFile);
  this->Needed({kMissingSymDepFile});

  auto open_missing_sym = this->DlOpen(kMissingSymFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_missing_sym.is_ok()) << open_missing_sym.error_value();
  EXPECT_TRUE(open_missing_sym.value());

  auto global_sym = this->DlSym(open_missing_sym.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(global_sym.is_ok()) << global_sym.error_value();
  ASSERT_TRUE(global_sym.value());

  EXPECT_EQ(RunFunction<int64_t>(global_sym.value()), kReturnValue);

  const std::string symbol_name = TestSym("missing-sym");

  // dlsym will not be able to find the global symbol from the local scope
  auto local_sym = this->DlSym(open_missing_sym.value(), symbol_name.c_str());
  ASSERT_TRUE(local_sym.is_error()) << local_sym.error_value();
  EXPECT_THAT(local_sym.error_value().take_str(),
              IsUndefinedSymbolErrMsg(symbol_name, kMissingSymFile));

  ASSERT_TRUE(this->DlClose(open_global_defines_sym.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_missing_sym.value()).is_ok());
}

// Test dlopen-ing a previously non-global loaded module with RTLD_GLOBAL will
// make the module and all of its deps global.
// dlopen has-foo-v1:
//   - foo-v1 -> foo() returns 2
// dlopen has-foo-v2:
//   - foo-v2 -> foo() returns 7
// dlopen RTLD_GLOBAL has-foo-v2.
// dlopen bar-v1 -> bar_v1() calls foo().
//      - foo-v1 -> foo() returns 2
// Call foo() from bar_v1() and get 7 from RTLD_GLOBAL foo-v2
TYPED_TEST(DlTests, UpdateModeToGlobal) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kBarV1File = TestShlib("libbar-v1");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kHasFooV1File, kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto has_foo_v1_foo = this->DlSym(open_has_foo_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v1_foo.is_ok()) << has_foo_v1_foo.error_value();
  EXPECT_TRUE(has_foo_v1_foo.value());

  // Confirm the resolved symbol is from the local dependency.
  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_foo.value()), kFooV1ReturnValue);

  this->Needed({kHasFooV2File, kFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_foo = this->DlSym(open_has_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v2_foo.is_ok()) << has_foo_v2_foo.error_value();
  EXPECT_TRUE(has_foo_v2_foo.value());

  // Confirm the resolved symbol is from the local dependency.
  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_foo.value()), kFooV2ReturnValue);

  // Dlopen the file and promote it and its dep (foo-v2) to global modules
  // (expecting the files to already be loaded).
  auto open_global_has_foo_v2 =
      this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_global_has_foo_v2.is_ok()) << open_global_has_foo_v2.error_value();
  EXPECT_TRUE(open_global_has_foo_v2.value());

  EXPECT_EQ(open_has_foo_v2.value(), open_global_has_foo_v2.value());

  this->Needed({kBarV1File});

  // Dlopen a new file that depends on a previously-loaded non-global dependency.
  auto open_bar_v1 = this->DlOpen(kBarV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_bar_v1.is_ok()) << open_bar_v1.error_value();
  EXPECT_TRUE(open_bar_v1.value());

  // Expect the resolved symbol to be provided by the global module.
  auto bar_v1 = this->DlSym(open_bar_v1.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(bar_v1.is_ok()) << bar_v1.error_value();
  EXPECT_TRUE(bar_v1.value());

  if constexpr (TestFixture::kStrictLoadOrderPriority) {
    // Musl will prioritize the symbol that was loaded first (from foo-v1),
    // even though the file does not have global scope.
    EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kFooV1ReturnValue);
  } else {
    // Glibc & libdl will use the first global module that contains the symbol
    // (foo-v2)
    EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kFooV2ReturnValue);
  }

  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_bar_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_global_has_foo_v2.value()).is_ok());
}

// Test that promoting a module to a global module, will change the "global load
// order" of the dynamic linker's bookkeeping list.
// dlopen foo-v1 -> foo() returns 2
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen RTLD_GLOBAL foo-v1
// dlopen has-foo-v1
//  - foo-v1 -> foo() returns 7
// call foo() from has-foo-v1 and expect 2 from the first loaded global foo-v2
TYPED_TEST(DlTests, GlobalModuleOrdering) {
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kFooV1File});

  auto open_foo_v1 = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_foo_v1.is_ok()) << open_foo_v1.error_value();
  EXPECT_TRUE(open_foo_v1.value());

  auto foo_v1_foo = this->DlSym(open_foo_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(foo_v1_foo.is_ok()) << foo_v1_foo.error_value();
  EXPECT_TRUE(foo_v1_foo.value());

  // Validity check foo() return value from foo-v1.
  EXPECT_EQ(RunFunction<int64_t>(foo_v1_foo.value()), kFooV1ReturnValue);

  this->Needed({kFooV2File});

  auto open_global_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v2.is_ok()) << open_global_foo_v2.error_value();
  EXPECT_TRUE(open_global_foo_v2.value());

  auto global_foo_v2_foo = this->DlSym(open_global_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(global_foo_v2_foo.is_ok()) << global_foo_v2_foo.error_value();
  EXPECT_TRUE(global_foo_v2_foo.value());

  // Validity check foo() return value from foo-v2.
  EXPECT_EQ(RunFunction<int64_t>(global_foo_v2_foo.value()), kFooV2ReturnValue);

  // Promote foo-v1 to a global module (expecting it to already be loaded).
  auto open_global_foo_v1 = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_NOLOAD | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v1.is_ok()) << open_global_foo_v1.error_value();
  EXPECT_TRUE(open_global_foo_v1.value());

  EXPECT_EQ(open_foo_v1.value(), open_global_foo_v1.value());

  this->Needed({kHasFooV1File});

  // Test that has-foo-v1 will now resolve its foo symbol from the recently
  // promoted global foo-v1 module because it comes first in the load order of
  // global modules.
  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto has_foo_v1_call_foo = this->DlSym(open_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v1_call_foo.is_ok()) << has_foo_v1_call_foo.error_value();
  EXPECT_TRUE(has_foo_v1_call_foo.value());

  if constexpr (TestFixture::kStrictLoadOrderPriority) {
    // Musl will prioritize the symbol from the (global) module that was loaded
    // first (foo-v1).
    EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_call_foo.value()), kFooV1ReturnValue);
  } else {
    // Glibc will use the module that was loaded first _with_ RTLD_GLOBAL.
    EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_call_foo.value()), kFooV2ReturnValue);
  }

  ASSERT_TRUE(this->DlClose(open_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_global_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_global_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
}

// This tests that calling dlopen(..., RTLD_GLOBAL) multiple times on a module
// will not change the "global load order"
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen RTLD_GLOBAL foo-v1 -> foo() returns 2
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 2
// dlopen bar-v1 -> bar_v1() calls foo().
//    - foo-v1 -> foo() returns 2
// Call foo() from bar-v1 and get 7 from global foo-v2, because foo-v2 was
// loaded first with RTLD_GLOBAL, and its order did not change.
TYPED_TEST(DlTests, GlobalModuleOrderingMultiDlopen) {
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kBarV1File = TestShlib("libbar-v1");
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kFooV2File});

  auto open_global_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v2.is_ok()) << open_global_foo_v2.error_value();
  EXPECT_TRUE(open_global_foo_v2.value());

  this->Needed({kFooV1File});

  auto open_global_foo_v1 = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v1.is_ok()) << open_global_foo_v1.error_value();
  EXPECT_TRUE(open_global_foo_v1.value());

  auto open_global_foo_v2_again =
      this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_global_foo_v2_again.is_ok()) << open_global_foo_v2_again.error_value();
  EXPECT_TRUE(open_global_foo_v2_again.value());

  EXPECT_EQ(open_global_foo_v2.value(), open_global_foo_v2_again.value());

  this->ExpectRootModule(kBarV1File);

  auto open_bar_v1 = this->DlOpen(kBarV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_bar_v1.is_ok()) << open_bar_v1.error_value();
  EXPECT_TRUE(open_bar_v1.value());

  auto bar_v1 = this->DlSym(open_bar_v1.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(bar_v1.is_ok()) << bar_v1.error_value();
  EXPECT_TRUE(bar_v1.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kFooV2ReturnValue);

  ASSERT_TRUE(this->DlClose(open_global_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_global_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_global_foo_v2_again.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_bar_v1.value()).is_ok());
}

// This tests that calling dlopen(..., RTLD_GLOBAL) with a previously loaded
// global dependency will not change the "global load order"
// dlopen RTLD_GLOBAL foo-v2 -> foo() returns 7
// dlopen RTLD_GLOBAL multiple-foo-deps:
//    - foo-v1 -> foo() returns 2
//    - foo-v2 -> foo() returns 7
// dlopen bar-v1 -> bar_v1() calls foo().
//    - foo-v1 -> foo() returns 2
// Call foo() from bar-v1 and get 7 from global foo-v2, because foo-v2 was
// loaded first with RTLD_GLOBAL, and its order did not change.
TYPED_TEST(DlTests, GlobalModuleOrderingOfDeps) {
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kMultiFooDepsFile = TestModule("multiple-foo-deps");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kBarV1File = TestShlib("libbar-v1");
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kFooV2File});

  auto open_global_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_global_foo_v2.is_ok()) << open_global_foo_v2.error_value();
  EXPECT_TRUE(open_global_foo_v2.value());

  this->ExpectRootModule(kMultiFooDepsFile);
  this->Needed({kFooV1File});

  auto open_multi_foo_deps = this->DlOpen(kMultiFooDepsFile.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_multi_foo_deps.is_ok()) << open_multi_foo_deps.error_value();
  EXPECT_TRUE(open_multi_foo_deps.value());

  auto multi_foo_deps_call_foo =
      this->DlSym(open_multi_foo_deps.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(multi_foo_deps_call_foo.is_ok()) << multi_foo_deps_call_foo.error_value();
  EXPECT_TRUE(multi_foo_deps_call_foo.value());

  // Validity check foo() return value from first loaded (global) foo-v2.
  EXPECT_EQ(RunFunction<int64_t>(multi_foo_deps_call_foo.value()), kFooV2ReturnValue);

  this->ExpectRootModule(kBarV1File);

  auto open_bar_v1 = this->DlOpen(kBarV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_bar_v1.is_ok()) << open_bar_v1.error_value();
  EXPECT_TRUE(open_bar_v1.value());

  auto bar_v1 = this->DlSym(open_bar_v1.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(bar_v1.is_ok()) << bar_v1.error_value();
  EXPECT_TRUE(bar_v1.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kFooV2ReturnValue);

  ASSERT_TRUE(this->DlClose(open_global_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_multi_foo_deps.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_bar_v1.value()).is_ok());
}

// Test that promoting only the dependency of module to global does not affect
// the root module or sibling dep's visibility.
// dlopen multiple-foo-deps:
//  - foo-v1 -> foo() returns 2
//  - foo-v2 -> foo() returns 7
// dlopen RTLD_GLOBAL foo-v2
// dlopen has-foo-v1:
//  - foo-v1 -> foo() returns 2
// call_foo() from has-foo-v1 returns 7 from global foo-v2.
TYPED_TEST(DlTests, GlobalPromotionOfDep) {
  const std::string kMultiFooDepsFile = TestModule("multiple-foo-deps");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->ExpectRootModule(kMultiFooDepsFile);
  this->Needed({kFooV1File, kFooV2File});

  auto open_multi_foo_deps = this->DlOpen(kMultiFooDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_multi_foo_deps.is_ok()) << open_multi_foo_deps.error_value();
  EXPECT_TRUE(open_multi_foo_deps.value());

  // Promote the foo-v2 dependency to global.
  auto open_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_NOLOAD | RTLD_GLOBAL);
  ASSERT_TRUE(open_foo_v2.is_ok()) << open_foo_v2.error_value();
  EXPECT_TRUE(open_foo_v2.value());

  this->Needed({kHasFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  // call_foo() will return foo(). This will resolve the symbol from the global
  // module foo-v2 instead of its local dep (foo-v1).
  auto call_foo = this->DlSym(open_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(call_foo.is_ok()) << call_foo.error_value();
  EXPECT_TRUE(call_foo.value());

  if (TestFixture::kStrictLoadOrderPriority) {
    // Musl will prioritize the symbol that was loaded first (from foo-v1), even
    // though the file does not have global scope.
    EXPECT_EQ(RunFunction<int64_t>(call_foo.value()), kFooV1ReturnValue);
  } else {
    // Glibc & libdl will use the foo symbol that was promoted to global over
    // any local symbols that were loaded first.
    EXPECT_EQ(RunFunction<int64_t>(call_foo.value()), kFooV2ReturnValue);
  }

  ASSERT_TRUE(this->DlClose(open_multi_foo_deps.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
}

// Test that a module and its dependencies remain a global module after they
// have been promoted (ie they cannot be demoted).
// dlopen RTLD_GLOBAL has-foo-v1:
//    - foo-v1 -> foo() returns 2
// dlopen RTLD_LOCAL has-foo-v1:
// dlopen has-foo-v2:
//    - foo-v2 -> foo() returns 7
// call_foo() from has-foo-v2, and it should return 2 from the still global foo-v1.
TYPED_TEST(DlTests, GlobalPersistence) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kBarV2File = TestShlib("libbar-v2");
  constexpr int64_t kFooV1ReturnValue = 2;

  this->ExpectRootModule(kHasFooV1File);
  this->Needed({kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto reopen_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  this->ExpectRootModule(kHasFooV2File);
  this->Needed({kFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  EXPECT_TRUE(has_foo_v2_call_foo.value());

  // Expect to resolve to the global foo() symbol from foo-v1.
  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(reopen_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
}

// TODO(https://fxbug.dev/362604713)
// - Test applying RTLD_GLOBAL to a node along the circular dependency chain.

// Test startup modules are global modules managed by the dynamic linker
// - (startup module) foo-v1 -> foo() returns 2
// - (startup module) foo-v2 -> foo() returns 7
// dlopen(foo-v1, RTLD_NOLOAD) and expect the module to be loaded.
// dlopen(foo-v2, RTLD_NOLOAD) and expect the module to be loaded.
// dlopen has-foo-v2:
//  - foo-v2 -> foo() returns 7
// call foo from has-foo-v2 and expect 2 from global startup module that was
// loaded first (foo-v1).
TYPED_TEST(DlTests, StartupModulesBasic) {
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  // Make sure foo-v1, foo-v2 are linked in with this test by making direct
  // calls to their unique symbols.
  EXPECT_EQ(foo_v1_StartupModulesBasic(), kFooV1ReturnValue);
  EXPECT_EQ(foo_v2_StartupModulesBasic(), kFooV2ReturnValue);

  // Expect libld-dep-foo-v1/libld-dep-foo-v2 to already have been loaded at startup.
  auto open_foo_v1 = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_foo_v1.is_ok()) << open_foo_v1.error_value();
  EXPECT_TRUE(open_foo_v1.value());

  auto open_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_foo_v2.is_ok()) << open_foo_v2.error_value();
  EXPECT_TRUE(open_foo_v2.value());

  this->Needed({kHasFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  ASSERT_TRUE(has_foo_v2_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
}

// Test that global modules that are dlopen-ed are ordered after startup modules.
// (startup module) has-foo-v1:
//     - foo-v1 -> foo() returns 2
// dlopen RTLD_GLOBAL has-foo-v2:
//  - foo-v2 -> foo() returns 7
// call foo from has-foo-v2 and expect 2 from global startup module that was
// loaded first (foo-v1).
TYPED_TEST(DlTests, StartupModulesPriorityOverGlobal) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;

  // Make sure has-foo-v1 is linked in with this test by making a direct call to
  // its unique symbol.
  EXPECT_EQ(call_foo_v1_StartupModulesPriorityOverGlobal(), kFooV1ReturnValue);

  // Expect libhas-foo-v1 to already have been loaded at startup.
  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  this->Needed({kHasFooV2File, kFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_GLOBAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  ASSERT_TRUE(has_foo_v2_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
}

// A simple test that you can look up symbols from a startup module's dep.
// Unlike previous tests, this test does not rely on the symbol resolution
// performed by dlopen (since startup modules are already resolved and loaded at
// program execution). This tests that calling dlopen() on a startup module will
// properly reference dependencies so that dlsym() can search through the deps
// for the requested symbol when given a handle for a startup module.
// (startup module) has-foo-v1:
//    - foo-v1 -> foo() returns 2
// dlopen(has-foo-v1)
// call foo() from has-foo-v1 and expect 2.
TYPED_TEST(DlTests, StartupModulesDep) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  constexpr int64_t kFooV1ReturnValue = 2;

  // Make sure has-foo-v1 is linked in with this test by making a direct call to
  // its unique symbol.
  EXPECT_EQ(call_foo_v1_StartupModulesDep(), kFooV1ReturnValue);

  // Expect libhas-foo-v1 to already have been loaded at startup.
  auto open = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto foo = this->DlSym(open.value(), TestSym("foo").c_str());
  ASSERT_TRUE(foo.is_ok()) << foo.error_value();
  ASSERT_TRUE(foo.value());

  EXPECT_EQ(RunFunction<int64_t>(foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that dlsym() will return the first symbol found from the startup
// module's dep tree. This is a more extensive version of the above test, where
// dlsym() will look up symbols multiple levels down the dependency chain of the
// startup module, in the expected BFS order.
// (startup module) multiple-transitive-foo-deps:
//   - has-foo-v1:
//     - foo-v1 -> foo() returns 2
//   - has-foo-v2:
//     - foo-v2 -> foo() returns 7
// dlopen(multiple-transitive-foo-deps)
// call foo() from multiple-transitive-foo-deps and expect 2 from foo-v1, then
// call foo_v2() from multiple-transitive-foo-deps and expect 7 from foo-v2.
// dlopen(has-foo-v2) and call foo() and expect 7 from foo-v2.
TYPED_TEST(DlTests, StartupModulesDepOrder) {
  const std::string kMultipleTransitiveFooDepsFile = TestShlib("libmultiple-transitive-foo-deps");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  // Since multiple-transitive-foo-deps does not export any symbols itself, make
  // sure it's linked in with this test binary by checking that it's present in
  // dl_iterate_phdr output.
  GetPhdrInfoForModule(*this, kMultipleTransitiveFooDepsFile);

  auto open_root =
      this->DlOpen(kMultipleTransitiveFooDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_root.is_ok()) << open_root.error_value();
  EXPECT_TRUE(open_root.value());

  auto root_foo = this->DlSym(open_root.value(), TestSym("foo").c_str());
  ASSERT_TRUE(root_foo.is_ok()) << root_foo.error_value();
  ASSERT_TRUE(root_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(root_foo.value()), kFooV1ReturnValue);

  // Test that we can still resolve foo_v2() from the root startup module.
  auto root_foo_v2 = this->DlSym(open_root.value(), TestSym("foo_v2").c_str());
  ASSERT_TRUE(root_foo_v2.is_ok()) << root_foo_v2.error_value();
  ASSERT_TRUE(root_foo_v2.value());

  EXPECT_EQ(RunFunction<int64_t>(root_foo_v2.value()), kFooV2ReturnValue);

  // To make sure the dependency's module tree is filled out correctly, dlopen
  // the dep has-foo-v2 and make sure its "foo" symbol will resolve from foo_v2()
  auto open_dep = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_dep.is_ok()) << open_dep.error_value();
  EXPECT_TRUE(open_dep.value());

  auto dep_foo = this->DlSym(open_dep.value(), TestSym("foo").c_str());
  ASSERT_TRUE(dep_foo.is_ok()) << dep_foo.error_value();
  ASSERT_TRUE(dep_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(dep_foo.value()), kFooV2ReturnValue);

  ASSERT_TRUE(this->DlClose(open_dep.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_root.value()).is_ok());
}

}  // namespace
