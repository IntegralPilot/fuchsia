// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/self.h>

#include "dl-iterate-phdr-tests.h"
#include "dl-load-tests.h"
namespace {

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

using ::testing::Contains;
using ::testing::Not;
using ::testing::Property;

using dl::testing::CollectModulePhdrInfo;
using dl::testing::GetGlobalCounters;
using dl::testing::ModuleInfoList;
using dl::testing::ModulePhdrInfo;

using dl::testing::RunFunction;
using dl::testing::TestModule;
using dl::testing::TestShlib;
using dl::testing::TestSym;

// Weak symbol resolution tests conditionalize on `ld::kResolverPolicy` to
// verify the symbol chosen based on the resolution strategy (e.g., strict
// link-order vs. strong-over-weak).
static_assert(ld::kResolverPolicy == elfldltl::ResolverPolicy::kStrongOverWeak);

// Test that the module data structure uses its own copy of the module's name,
// so that there is no dependency on the memory backing the original string
// pointer.
TYPED_TEST(DlTests, ModuleNameOwnership) {
  std::string file = TestModule("ret17");

  this->ExpectRootModule(file);
  auto open_first = this->DlOpen(file.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_first.is_ok()) << open_first.error_value();
  EXPECT_TRUE(open_first.value());

  // Changing the contents for this pointer should not affect the dynamic
  // linker's state.
  file = "FOO";

  // A lookup for the original filename should succeed.
  auto open_second = this->DlOpen(TestModule("ret17").c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_second.is_ok()) << open_second.error_value();
  EXPECT_TRUE(open_second.value());

  EXPECT_EQ(open_first.value(), open_second.value());

  ASSERT_TRUE(this->DlClose(open_first.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_second.value()).is_ok());
}

// Load a basic file with no dependencies.
TYPED_TEST(DlTests, Basic) {
  constexpr int64_t kReturnValue = 17;
  const std::string kRet17File = TestModule("ret17");

  this->ExpectRootModule(kRet17File);

  auto open = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  // Look up the TestSym("TestStart").c_str() function and call it, expecting it to return 17.
  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Load a file that performs relative relocations against itself. The TestStart
// function's return value is derived from the resolved symbols.
TYPED_TEST(DlTests, Relative) {
  constexpr int64_t kReturnValue = 17;
  const std::string kRelativeRelocFile = TestModule("relative-reloc");

  this->ExpectRootModule(kRelativeRelocFile);

  auto open = this->DlOpen(kRelativeRelocFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Load a file that performs symbolic relocations against itself. The TestStart
// functions' return value is derived from the resolved symbols.
TYPED_TEST(DlTests, Symbolic) {
  constexpr int64_t kReturnValue = 17;
  const std::string kSymbolicFile = TestModule("symbolic-reloc");

  this->ExpectRootModule(kSymbolicFile);

  auto open = this->DlOpen(kSymbolicFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Tests how symbols are resolved when presented with a weak and strong
// definition.
// dlopen one-weak-symbol:
//   - foo-v1 -> [[gnu::weak]] foo() returns 2
//   - foo-v2 -> foo() returns 7
// call call_foo from one-weak-symbol and expect it to resolve its foo symbol to
// 7 from foo-v2 for musl (which uses strong symbols over weak), or 2
// from from foo-v1 (for glibc which uses the first symbol encountered).
TYPED_TEST(DlTests, OneWeakSymbol) {
  const std::string kOneWeakSymbolFile = TestModule("one-weak-symbol");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1-weak");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->ExpectRootModule(kOneWeakSymbolFile);
  this->Needed({kFooV1File, kFooV2File});

  auto open = this->DlOpen(kOneWeakSymbolFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  switch (TestFixture::kResolverPolicy) {
    case elfldltl::ResolverPolicy::kStrictLinkOrder: {
      EXPECT_EQ(RunFunction<int64_t>(sym.value()), kFooV1ReturnValue);
      break;
    }
    case elfldltl::ResolverPolicy::kStrongOverWeak: {
      EXPECT_EQ(RunFunction<int64_t>(sym.value()), kFooV2ReturnValue);
      break;
    }
  }

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Tests how symbols are resolved when all definitions are weak.
// dlopen all-weak-symbols:
//   - foo-v1 -> [[gnu::weak]] foo() returns 2
//   - foo-v2 -> [[gnu::weak]] foo() returns 7
// call call_foo from one-weak-symbol and expect it to resolve its foo symbol to
// from foo-v1. All implementations will use the first weak definition found in
// absence of any strong definitions.
TYPED_TEST(DlTests, AllWeakSymbols) {
  const std::string kOneWeakSymbolFile = TestModule("all-weak-symbols");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1-weak");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2-weak");
  constexpr int64_t kFooV1ReturnValue = 2;

  this->ExpectRootModule(kOneWeakSymbolFile);
  this->Needed({kFooV1File, kFooV2File});

  auto open = this->DlOpen(kOneWeakSymbolFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test loading a module with relro protections.
TYPED_TEST(DlTests, Relro) {
  const std::string kRelroFile = TestModule("relro");

  this->ExpectRootModule(kRelroFile);

  auto open = this->DlOpen(kRelroFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  ASSERT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_DEATH(RunFunction<int64_t>(sym.value()), "");

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that calling dlopen twice on a file will return the same pointer,
// indicating that the dynamic linker is storing the module in its bookkeeping.
// dlsym() should return a pointer to the same symbol from the same module as
// well.
TYPED_TEST(DlTests, BasicModuleReuse) {
  const std::string kRet17File = TestModule("ret17");

  this->ExpectRootModule(kRet17File);

  auto first_open_ret17 = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(first_open_ret17.is_ok()) << first_open_ret17.error_value();
  EXPECT_TRUE(first_open_ret17.value());

  auto second_open_ret17 = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(second_open_ret17.is_ok()) << second_open_ret17.error_value();
  EXPECT_TRUE(second_open_ret17.value());

  EXPECT_EQ(first_open_ret17.value(), second_open_ret17.value());

  auto first_ret17_test_start = this->DlSym(first_open_ret17.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(first_ret17_test_start.is_ok()) << first_ret17_test_start.error_value();
  EXPECT_TRUE(first_ret17_test_start.value());

  auto second_ret17_test_start =
      this->DlSym(second_open_ret17.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(second_ret17_test_start.is_ok()) << second_ret17_test_start.error_value();
  EXPECT_TRUE(second_ret17_test_start.value());

  EXPECT_EQ(first_ret17_test_start.value(), second_ret17_test_start.value());

  ASSERT_TRUE(this->DlClose(first_open_ret17.value()).is_ok());
  ASSERT_TRUE(this->DlClose(second_open_ret17.value()).is_ok());
}

// Test that different mutually-exclusive kFiles that were dlopen-ed do not share
// pointers or resolved symbols.
TYPED_TEST(DlTests, UniqueModules) {
  const std::string kRet17File = TestModule("ret17");
  const std::string kRet23File = TestModule("ret23");

  this->ExpectRootModule(kRet17File);

  auto open_ret17 = this->DlOpen(kRet17File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_ret17.is_ok()) << open_ret17.error_value();
  EXPECT_TRUE(open_ret17.value());

  this->ExpectRootModule(kRet23File);

  auto open_ret23 = this->DlOpen(kRet23File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_ret23.is_ok()) << open_ret23.error_value();
  EXPECT_TRUE(open_ret23.value());

  EXPECT_NE(open_ret17.value(), open_ret23.value());

  auto ret17_test_start = this->DlSym(open_ret17.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(ret17_test_start.is_ok()) << ret17_test_start.error_value();
  EXPECT_TRUE(ret17_test_start.value());

  auto sym23_test_start = this->DlSym(open_ret23.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym23_test_start.is_ok()) << sym23_test_start.error_value();
  EXPECT_TRUE(sym23_test_start.value());

  EXPECT_NE(ret17_test_start.value(), sym23_test_start.value());

  EXPECT_EQ(RunFunction<int64_t>(ret17_test_start.value()), 17);
  EXPECT_EQ(RunFunction<int64_t>(sym23_test_start.value()), 23);

  ASSERT_TRUE(this->DlClose(open_ret17.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_ret23.value()).is_ok());
}

// Test that the same module can be located by either its DT_SONAME or filename.
// dlopen libfoo-filename (with DT_SONAME libbar-soname)
// dlopen libbar-soname and expect the same module handle for libfoo-filename.
// dlopen soname-filename-match:
//    - libbar-soname
// Expect dlopen for soname-filename-match to reuse libfoo-filename module.
// Expect only 2 modules are loaded: libfoo-filename, soname-filename-match.
TYPED_TEST(DlTests, SonameFilenameMatch) {
  const std::string kFilename = TestShlib("libfoo-filename");
  const std::string kSoname = TestShlib("libbar-soname");
  const std::string kParentFile = TestShlib("libsoname-filename-match");

  const size_t initial_loaded_count = GetGlobalCounters(this).adds;

  this->Needed({kFilename});

  // Open the module by its filename.
  auto open_filename = this->DlOpen(kFilename.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_filename.is_ok()) << open_filename.error_value();
  ASSERT_TRUE(open_filename.value());

  // Open the module by its DT_SONAME, expecting it to already be loaded and is
  // pointing to the same module as `open_filename`.
  auto open_soname = this->DlOpen(kSoname.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_soname.is_ok()) << open_soname.error_value();
  ASSERT_TRUE(open_soname.value());

  EXPECT_EQ(open_filename.value(), open_soname.value());

  // Expect only the parent-file to be fetched from the filesystem; its dep
  // should already be loaded.
  this->Needed({kParentFile});

  auto open_parent = this->DlOpen(kParentFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_parent.is_ok()) << open_parent.error_value();
  ASSERT_TRUE(open_parent.value());

  const size_t updated_loaded_count = GetGlobalCounters(this).adds;
  if constexpr (TestFixture::kInaccurateLoadCountAfterSonameMatch) {
    EXPECT_EQ(updated_loaded_count, initial_loaded_count + 3);
  } else {
    EXPECT_EQ(updated_loaded_count, initial_loaded_count + 2);
  }
  // Expect a file named libbar-soname to be absent from loaded modules.
  ModuleInfoList info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &info_list), 0);
  EXPECT_THAT(info_list,
              Not(Contains(Property(&ModulePhdrInfo::name, TestShlib("libbar-soname")))));

  ASSERT_TRUE(this->DlClose(open_parent.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_filename.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_soname.value()).is_ok());
}

// Test a linking session will match the DT_SONAME of a module that is in the
// loading queue but has not yet been loaded.
// dlopen soname-filename-dep:
//  - soname-filename-match
//    - libbar-soname
//  - libfoo-filename (with DT_SONAME libbar-soname)
// Since libbar-soname was enqueued before libfoo-filename was loaded, this
// tests if the loading logic can match libbar-soname with libfoo-filename's
// DT_SONAME. Glibc does perform this matching, but Musl/Libdl does not.
// Check there should be 3 modules that were loaded after this dlopen:
// soname-filename-loaded-dep, soname-filename-match, and libfoo-filename.
TYPED_TEST(DlTests, SonameFilenameDep) {
  const std::string kLibFooFile = TestShlib("libfoo-filename");
  const std::string kHasLibBarFile = TestShlib("libsoname-filename-match");
  const std::string kHasDepsFile = TestModule("soname-filename-loaded-dep");

  if constexpr (!TestFixture::kSonameLookupInPendingDeps) {
    GTEST_SKIP()
        << "Fixture should be able to look up DT_SONAME in pending deps in a linking session";
  }

  const size_t initial_loaded_count = GetGlobalCounters(this).adds;

  this->ExpectRootModule(kHasDepsFile);
  this->Needed({kHasLibBarFile, kLibFooFile});

  auto open_has_deps = this->DlOpen(kHasDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_deps.is_ok()) << open_has_deps.error_value();
  ASSERT_TRUE(open_has_deps.value());

  const size_t updated_loaded_count = GetGlobalCounters(this).adds;
  EXPECT_EQ(updated_loaded_count, initial_loaded_count + 3);

  // Expect a file named libar-soname to be absent from loaded modules.
  ModuleInfoList info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &info_list), 0);
  EXPECT_THAT(info_list,
              Not(Contains(Property(&ModulePhdrInfo::name, TestShlib("libbar-soname")))));

  ASSERT_TRUE(this->DlClose(open_has_deps.value()).is_ok());
}

// Test a linking session will match the DT_SONAME of a module that has already
// been loaded in the same linking session. This test is similar to above,
// except the module with the matching DT_SONAME has been loaded by the time the
// pending module is looking for a match.
// dlopen soname-filename-loaded-dep:
//  - libfoo-filename (with DT_SONAME libbar-soname)
//  - soname-filename-match
//    - libbar-soname
// Expect that libbar-soname will reuse the module for libfoo-filename, because
// libfoo-filename was loaded/decoded by the time libbar-soname is added to the
// loading queue.
// Check there should be 3 modules that were loaded after this dlopen:
// soname-filename-loaded-dep, libfoo-filename, and
// soname-filename-match
TYPED_TEST(DlTests, SonameFilenameLoadedDep) {
  const std::string kLibFooFile = TestShlib("libfoo-filename");
  const std::string kHasLibBarFile = TestShlib("libsoname-filename-match");
  const std::string kHasDepsFile = TestModule("soname-filename-loaded-dep");

  if constexpr (!TestFixture::kSonameLookupInLoadedDeps) {
    GTEST_SKIP()
        << "Fixture should be able to look up DT_SONAME in loaded deps in a linking session.";
  }

  const size_t initial_loaded_count = GetGlobalCounters(this).adds;

  this->ExpectRootModule(kHasDepsFile);
  this->Needed({kLibFooFile, kHasLibBarFile});

  auto open_has_deps = this->DlOpen(kHasDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_deps.is_ok()) << open_has_deps.error_value();
  ASSERT_TRUE(open_has_deps.value());

  const size_t updated_loaded_count = GetGlobalCounters(this).adds;

  if constexpr (TestFixture::kInaccurateLoadCountAfterSonameMatch) {
    // For some reason, Musl only counts one additional module.
    EXPECT_EQ(updated_loaded_count, initial_loaded_count + 1);
  } else {
    EXPECT_EQ(updated_loaded_count, initial_loaded_count + 3);
  }

  // Expect a file named libar-soname to be absent from loaded modules.
  ModuleInfoList info_list;
  EXPECT_EQ(this->DlIteratePhdr(CollectModulePhdrInfo, &info_list), 0);
  EXPECT_THAT(info_list,
              Not(Contains(Property(&ModulePhdrInfo::name, TestShlib("libbar-soname")))));

  ASSERT_TRUE(this->DlClose(open_has_deps.value()).is_ok());
}

// Test that passing a NULL or an empty string as the file arg will return a
// handle to the executable.
TYPED_TEST(DlTests, NullFileArgBasic) {
  const std::string kEmpty;

  auto null_open = this->DlOpen(NULL, RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(null_open.is_ok()) << null_open.error_value();
  ASSERT_TRUE(null_open.value());

  // To verify the module handle dlopen() returned is for the executable, we
  // that the module's link_map.l_ld points to dynamic section of this test
  // binary.
  EXPECT_EQ(this->ModuleLinkMap(null_open.value())->l_ld,
            reinterpret_cast<const Elf64_Dyn*>(elfldltl::Self<>::Dynamic().data()));

  auto empty_str_open = this->DlOpen(kEmpty.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(empty_str_open.is_ok()) << empty_str_open.error_value();
  EXPECT_TRUE(empty_str_open.value());

  // Expect that the handle for an empty string is the same as the handle for
  // a NULL argument, which is the executable.
  EXPECT_EQ(empty_str_open.value(), null_open.value());

  // TODO(https://fxbug.dev/339294119): TODO test that dlsym() will resolve
  // symbols correctly with this handle.

  ASSERT_TRUE(this->DlClose(null_open.value()).is_ok());
  ASSERT_TRUE(this->DlClose(empty_str_open.value()).is_ok());
}

}  // namespace
