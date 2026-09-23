// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests.h"

namespace {

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

using dl::testing::RunFunction;
using dl::testing::TestModule;
using dl::testing::TestShlib;
using dl::testing::TestSym;

// Load a module that depends on a symbol provided directly by a dependency.
TYPED_TEST(DlTests, BasicDep) {
  constexpr int64_t kReturnValue = 17;
  const std::string kBasicDepFile = TestModule("basic-dep");
  const std::string kDepAFile = TestShlib("libld-dep-a");

  this->ExpectRootModule(kBasicDepFile);
  this->Needed({kDepAFile});

  auto open = this->DlOpen(kBasicDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Load a module that depends on a symbols provided directly and transitively by
// several dependencies. Dependency ordering is serialized such that a module
// depends on a symbol provided by a dependency only one hop away
// (e.g. in its DT_NEEDED list):
TYPED_TEST(DlTests, IndirectDeps) {
  constexpr int64_t kReturnValue = 17;
  const std::string kIndirectDepsFile = TestModule("indirect-deps");
  const std::string kIndirectDepsAFile = TestShlib("libindirect-deps-a");
  const std::string IndirectDepsBFile = TestShlib("libindirect-deps-b");
  const std::string kIndirectDepsCFile = TestShlib("libindirect-deps-c");

  this->ExpectRootModule(kIndirectDepsFile);
  this->Needed({kIndirectDepsAFile, IndirectDepsBFile, kIndirectDepsCFile});

  auto open = this->DlOpen(kIndirectDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Load a module that depends on symbols provided directly and transitively by
// several dependencies. Dependency ordering is DAG-like where several modules
// share a dependency.
TYPED_TEST(DlTests, ManyDeps) {
  constexpr int64_t kReturnValue = 17;
  const std::string kManyDepsFile = TestModule("many-deps");
  const std::string kDepAFile = TestShlib("libld-dep-a");
  const std::string kDepBFile = TestShlib("libld-dep-b");
  const std::string kDepFFile = TestShlib("libld-dep-f");
  const std::string kDepCFile = TestShlib("libld-dep-c");
  const std::string kDepDFile = TestShlib("libld-dep-d");
  const std::string kDepEFile = TestShlib("libld-dep-e");

  this->ExpectRootModule(kManyDepsFile);
  this->Needed({kDepAFile, kDepBFile, kDepFFile, kDepCFile, kDepDFile, kDepEFile});

  auto open = this->DlOpen(kManyDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("TestStart").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that you can dlopen a dependency from a previously loaded module.
TYPED_TEST(DlTests, OpenDepDirectly) {
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");

  this->Needed({kHasFooV1File, kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  // dlopen kFooV1File expecting it to already be loaded.
  auto open_foo_v1 = this->DlOpen(kFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(open_foo_v1.is_ok()) << open_foo_v1.error_value();
  EXPECT_TRUE(open_foo_v1.value());

  // Test that dlsym will resolve the same symbol pointer from the shared
  // dependency between kHasFooV1File (open_has_foo_v1) and kFooV1File (open_foo_v1).
  auto has_foo_v1_foo = this->DlSym(open_has_foo_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(has_foo_v1_foo.is_ok()) << has_foo_v1_foo.error_value();
  ASSERT_TRUE(has_foo_v1_foo.value());

  auto foo_v2_foo = this->DlSym(open_foo_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(foo_v2_foo.is_ok()) << foo_v2_foo.error_value();
  ASSERT_TRUE(foo_v2_foo.value());

  EXPECT_EQ(has_foo_v1_foo.value(), foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_foo.value()), RunFunction<int64_t>(foo_v2_foo.value()));

  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_foo_v1.value()).is_ok());
}

// These are test scenarios that test symbol resolution from just the dependency
// graph, ie from the local scope of the dlopen-ed module.

// Test that dep ordering is preserved in the dependency graph.
// dlopen multiple-foo-deps -> calls foo()
//  - foo-v1 -> foo() returns 2
//  - foo-v2 -> foo() returns 7
// call foo() from multiple-foo-deps pointer and expect 2 from foo-v1.
TYPED_TEST(DlTests, DepOrder) {
  constexpr const int64_t kReturnValue = 2;
  const std::string kMultipleFooDepsFile = TestModule("multiple-foo-deps");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");

  this->ExpectRootModule(kMultipleFooDepsFile);
  this->Needed({kFooV1File, kFooV2File});

  auto open = this->DlOpen(kMultipleFooDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that transitive dep ordering is preserved the dependency graph.
// dlopen transitive-foo-dep -> calls foo()
//   - has-foo-v1:
//     - foo-v1 -> foo() returns 2
//   - foo-v2 -> foo() returns 7
// call foo() from transitive-foo-dep pointer and expect 7 from foo-v2.
TYPED_TEST(DlTests, TransitiveDepOrder) {
  constexpr const int64_t kReturnValue = 7;
  const std::string kTransitiveFooDepFile = TestModule("transitive-foo-dep");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");

  this->ExpectRootModule(kTransitiveFooDepFile);
  this->Needed({kHasFooV1File, kFooV2File, kFooV1File});

  auto open = this->DlOpen(kTransitiveFooDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that a module graph with a dependency cycle involving the root module
// will complete dlopen() & dlsym() calls.
// dlopen cyclic-dep-parent: foo() returns 2.
//   - has-cyclic-dep:
//     - cyclic-dep-parent
//     - foo-v1: foo() returns 2.
TYPED_TEST(DlTests, CyclicalDependency) {
  constexpr const int64_t kReturnValue = 2;
  const auto kCyclicDepParentFile = TestModule("cyclic-dep-parent");
  const auto kHasCyclicDepFile = TestShlib("libhas-cyclic-dep");
  const auto kFooV1File = TestShlib("libld-dep-foo-v1");

  this->ExpectRootModule(kCyclicDepParentFile);
  this->Needed({kHasCyclicDepFile, kFooV1File});

  auto open = this->DlOpen(kCyclicDepParentFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto sym = this->DlSym(open.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(sym.is_ok()) << sym.error_value();
  ASSERT_TRUE(sym.value());

  EXPECT_EQ(RunFunction<int64_t>(sym.value()), kReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// These are test scenarios that test symbol resolution from the local scope
// of the dlopen-ed module, when some (or all) of its dependencies have already
// been loaded.

// Test that dependency ordering is always preserved in the local symbol scope,
// even if a module with the same symbol was already loaded (with RTLD_LOCAL).
// dlopen foo-v2 -> foo() returns 7
// dlopen multiple-foo-deps:
//   - foo-v1 -> foo() returns 2
//   - foo-v2 -> foo() returns 7
// call foo() from multiple-foo-deps and expect 2 from foo-v1 because it is
// first in multiple-foo-deps local scope.
TYPED_TEST(DlTests, LocalPrecedence) {
  const std::string kMultipleFooDepsFile = TestModule("multiple-foo-deps");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kFooV2File});

  auto open_foo_v2 = this->DlOpen(kFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_foo_v2.is_ok()) << open_foo_v2.error_value();
  EXPECT_TRUE(open_foo_v2.value());

  auto foo_v2_foo = this->DlSym(open_foo_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(foo_v2_foo.is_ok()) << foo_v2_foo.error_value();
  ASSERT_TRUE(foo_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(foo_v2_foo.value()), kFooV2ReturnValue);

  this->ExpectRootModule(kMultipleFooDepsFile);

  this->Needed({kFooV1File});

  auto open_multiple_foo_deps = this->DlOpen(kMultipleFooDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_multiple_foo_deps.is_ok()) << open_multiple_foo_deps.error_value();
  EXPECT_TRUE(open_multiple_foo_deps.value());

  // Test the `foo` value that is used by the root module's `call_foo()` function.
  auto multiple_foo_deps_call_foo =
      this->DlSym(open_multiple_foo_deps.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(multiple_foo_deps_call_foo.is_ok()) << multiple_foo_deps_call_foo.error_value();
  ASSERT_TRUE(multiple_foo_deps_call_foo.value());

  if constexpr (TestFixture::kStrictLoadOrderPriority) {
    // Musl will prioritize the symbol that was loaded first (from libfoo-v2),
    // even though the file does not have global scope.
    EXPECT_EQ(RunFunction<int64_t>(multiple_foo_deps_call_foo.value()), kFooV2ReturnValue);
  } else {
    // Glibc & libdl will use the symbol that is first encountered in the
    // module's local scope, from libfoo-v1.
    EXPECT_EQ(RunFunction<int64_t>(multiple_foo_deps_call_foo.value()), kFooV1ReturnValue);
  }

  // dlsym will always use dependency ordering from the local scope when looking
  // up a symbol. Unlike `call_foo()`, `foo()` is provided by a dependency, and
  // both musl and glibc will only look for this symbol in the root module's
  // local-scope dependency set.
  auto multiple_foo_deps_foo = this->DlSym(open_multiple_foo_deps.value(), TestSym("foo").c_str());
  ASSERT_TRUE(multiple_foo_deps_foo.is_ok()) << multiple_foo_deps_foo.error_value();
  ASSERT_TRUE(multiple_foo_deps_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(multiple_foo_deps_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_multiple_foo_deps.value()).is_ok());
}

// Test the local symbol scope precedence is not affected by transitive
// dependencies that were already loaded.
// dlopen has-foo-v1:
//  - foo-v1 -> foo() returns 2
// dlopen transitive-foo-dep:
//   - has-foo-v1:
//     - foo-v1 -> foo() returns 2
//   - foo-v2 -> foo() returns 7
// call foo() from transitive-foo-deps and expect 7 from foo-v2 because
// it is first in multiple-transitive-foo-dep's local scope.
TYPED_TEST(DlTests, LocalPrecedenceTransitiveDeps) {
  const std::string kTransitiveFooDepFile = TestModule("transitive-foo-dep");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string FooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kHasFooV1File, kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto has_foo_v1_call_foo = this->DlSym(open_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v1_call_foo.is_ok()) << has_foo_v1_call_foo.error_value();
  ASSERT_TRUE(has_foo_v1_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_call_foo.value()), kFooV1ReturnValue);

  this->ExpectRootModule(kTransitiveFooDepFile);
  this->Needed({FooV2File});

  auto open_transitive_foo_dep = this->DlOpen(kTransitiveFooDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_transitive_foo_dep.is_ok()) << open_transitive_foo_dep.error_value();
  EXPECT_TRUE(open_transitive_foo_dep.value());

  auto transitive_foo_dep_call_fo =
      this->DlSym(open_transitive_foo_dep.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(transitive_foo_dep_call_fo.is_ok()) << transitive_foo_dep_call_fo.error_value();
  ASSERT_TRUE(transitive_foo_dep_call_fo.value());

  if (TestFixture::kStrictLoadOrderPriority) {
    // Musl will prioritize the symbol that was loaded first (from has-foo-v1 +
    // foo-v1), even though the file does not have global scope.
    EXPECT_EQ(RunFunction<int64_t>(transitive_foo_dep_call_fo.value()), kFooV1ReturnValue);
  } else {
    // Glibc & libdl will use the symbol that is first encountered in the
    // module's local scope, from foo-v2.
    EXPECT_EQ(RunFunction<int64_t>(transitive_foo_dep_call_fo.value()), kFooV2ReturnValue);
  }

  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_transitive_foo_dep.value()).is_ok());
}

// The following tests will test dlopen-ing a module and resolving symbols from
// transitive dependency modules, all of which have already been loaded.
// dlopen has-foo-v2:
//  - foo-v2 -> foo() returns 7
// dlopen has-foo-v1:
//  - foo-v1 -> foo() returns 2
// dlopen multiple-transitive-foo-deps:
//   - has-foo-v1:
//     - foo-v1 -> foo() returns 2
//   - has-foo-v2:
//     - foo-v2 -> foo() returns 7
// Call foo() from multiple-transitive-foo-deps and expect 2 from foo-v1,
// because it is first in multiple-transitive-foo-dep's local scope.
TYPED_TEST(DlTests, LoadedTransitiveDepOrder) {
  const std::string kMultipleTransitiveFooDepsFile = TestShlib("libmultiple-transitive-foo-deps");
  const std::string kHasFooV2File = TestShlib("libhas-foo-v2");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  const std::string kHasFooV1File = TestShlib("libhas-foo-v1");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->Needed({kHasFooV2File, kFooV2File});

  auto open_has_foo_v2 = this->DlOpen(kHasFooV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v2.is_ok()) << open_has_foo_v2.error_value();
  EXPECT_TRUE(open_has_foo_v2.value());

  auto has_foo_v2_call_foo = this->DlSym(open_has_foo_v2.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v2_call_foo.is_ok()) << has_foo_v2_call_foo.error_value();
  ASSERT_TRUE(has_foo_v2_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v2_call_foo.value()), kFooV2ReturnValue);

  this->Needed({kHasFooV1File, kFooV1File});

  auto open_has_foo_v1 = this->DlOpen(kHasFooV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_has_foo_v1.is_ok()) << open_has_foo_v1.error_value();
  EXPECT_TRUE(open_has_foo_v1.value());

  auto has_foo_v1_call_foo = this->DlSym(open_has_foo_v1.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(has_foo_v1_call_foo.is_ok()) << has_foo_v1_call_foo.error_value();
  ASSERT_TRUE(has_foo_v1_call_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(has_foo_v1_call_foo.value()), kFooV1ReturnValue);

  this->Needed({kMultipleTransitiveFooDepsFile});

  auto open_multiple_transitive_foo_deps =
      this->DlOpen(kMultipleTransitiveFooDepsFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_multiple_transitive_foo_deps.is_ok())
      << open_multiple_transitive_foo_deps.error_value();
  EXPECT_TRUE(open_multiple_transitive_foo_deps.value());

  auto multiple_transitive_foo_deps_call_foo =
      this->DlSym(open_multiple_transitive_foo_deps.value(), TestSym("call_foo").c_str());
  ASSERT_TRUE(multiple_transitive_foo_deps_call_foo.is_ok())
      << multiple_transitive_foo_deps_call_foo.error_value();
  ASSERT_TRUE(multiple_transitive_foo_deps_call_foo.value());

  // Both Glibc & Musl's agree on the resolved symbol here here, because
  // has-foo-v1 is looked up first (and it is already loaded), so its symbols
  // take precedence.
  EXPECT_EQ(RunFunction<int64_t>(multiple_transitive_foo_deps_call_foo.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open_has_foo_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_has_foo_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_multiple_transitive_foo_deps.value()).is_ok());
}

// Whereas the above tests test how symbols are resolved for the root module,
// the following tests will test how symbols are resolved for dependencies of
// the root module.

// Test that the load-order of the local scope has precedence in the symbol
// resolution for symbols used by dependencies (i.e. a sibling's symbol is used
// used when it's earlier in the load-order set).
// dlopen precedence-in-dep-resolution:
//   - bar-v1 -> bar_v1() calls foo().
//      - foo-v1 -> foo() returns 2
//   - bar-v2 -> bar_v2() calls foo().
//      - foo-v2 -> foo() returns 7
// bar_v1() from precedence-in-dep-resolution and expect 2.
// bar_v2() from precedence-in-dep-resolution and expect 2 from foo-v1.
TYPED_TEST(DlTests, PrecedenceInDepResolution) {
  const std::string kPrecedenceInDepResolutionFile = TestModule("precedence-in-dep-resolution");
  const std::string kBarV1File = TestShlib("libbar-v1");
  const std::string kBarV2File = TestShlib("libbar-v2");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kFooV1ReturnValue = 2;

  this->ExpectRootModule(kPrecedenceInDepResolutionFile);
  this->Needed({kBarV1File, kBarV2File, kFooV1File, kFooV2File});

  auto open = this->DlOpen(kPrecedenceInDepResolutionFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open.is_ok()) << open.error_value();
  EXPECT_TRUE(open.value());

  auto bar_v1 = this->DlSym(open.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(bar_v1.is_ok()) << bar_v1.error_value();
  ASSERT_TRUE(bar_v1.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kFooV1ReturnValue);

  auto bar_v2 = this->DlSym(open.value(), TestSym("bar_v2").c_str());
  ASSERT_TRUE(bar_v2.is_ok()) << bar_v2.error_value();
  ASSERT_TRUE(bar_v2.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v2.value()), kFooV1ReturnValue);

  ASSERT_TRUE(this->DlClose(open.value()).is_ok());
}

// Test that the root module symbols have precedence in the symbol resolution
// for symbols used by dependencies.
// dlopen root-precedence-in-dep-resolution: foo() returns 17
//   - bar-v1 -> bar_v1() calls foo().
//      - foo-v1 -> foo() returns 2
//   - bar-v2 -> bar_v2() calls foo().
//      - foo-v2 -> foo() returns 7
// call foo() from root-precedence-in-dep-resolution and expect 17.
// bar_v1() from root-precedence-in-dep-resolution and expect 17.
// bar_v2() from root-precedence-in-dep-resolution and expect 17.
TYPED_TEST(DlTests, RootPrecedenceInDepResolution) {
  const std::string kRootFile = TestModule("root-precedence-in-dep-resolution");
  const std::string kBarV1File = TestShlib("libbar-v1");
  const std::string kBarV2File = TestShlib("libbar-v2");
  const std::string kFooV1File = TestShlib("libld-dep-foo-v1");
  const std::string kFooV2File = TestShlib("libld-dep-foo-v2");
  constexpr int64_t kReturnValueFromRootModule = 17;
  constexpr int64_t kFooV1ReturnValue = 2;
  constexpr int64_t kFooV2ReturnValue = 7;

  this->ExpectRootModule(kRootFile);
  this->Needed({kBarV1File, kBarV2File, kFooV1File, kFooV2File});

  auto open_root = this->DlOpen(kRootFile.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_root.is_ok()) << open_root.error_value();
  EXPECT_TRUE(open_root.value());

  auto root_foo = this->DlSym(open_root.value(), TestSym("foo").c_str());
  ASSERT_TRUE(root_foo.is_ok()) << root_foo.error_value();
  ASSERT_TRUE(root_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(root_foo.value()), kReturnValueFromRootModule);

  auto root_bar_v1 = this->DlSym(open_root.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(root_bar_v1.is_ok()) << root_bar_v1.error_value();
  ASSERT_TRUE(root_bar_v1.value());

  EXPECT_EQ(RunFunction<int64_t>(root_bar_v1.value()), kReturnValueFromRootModule);

  auto root_bar_v2 = this->DlSym(open_root.value(), TestSym("bar_v2").c_str());
  ASSERT_TRUE(root_bar_v2.is_ok()) << root_bar_v2.error_value();
  ASSERT_TRUE(root_bar_v2.value());

  EXPECT_EQ(RunFunction<int64_t>(root_bar_v2.value()), kReturnValueFromRootModule);

  // Test that when we dlopen the dep directly, foo is resolved to the
  // transitive dependency, while bar_v1/bar_v2 continue to use the root
  // module's foo symbol.
  auto open_bar_v1 = this->DlOpen(kBarV1File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_bar_v1.is_ok()) << open_bar_v1.error_value();
  EXPECT_TRUE(open_bar_v1.value());

  auto foo1 = this->DlSym(open_bar_v1.value(), TestSym("foo").c_str());
  ASSERT_TRUE(foo1.is_ok()) << foo1.error_value();
  ASSERT_TRUE(foo1.value());

  EXPECT_EQ(RunFunction<int64_t>(foo1.value()), kFooV1ReturnValue);

  auto bar_v1 = this->DlSym(open_bar_v1.value(), TestSym("bar_v1").c_str());
  ASSERT_TRUE(bar_v1.is_ok()) << bar_v1.error_value();
  ASSERT_TRUE(bar_v1.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v1.value()), kReturnValueFromRootModule);

  auto open_bar_v2 = this->DlOpen(kBarV2File.c_str(), RTLD_NOW | RTLD_LOCAL);
  ASSERT_TRUE(open_bar_v2.is_ok()) << open_bar_v2.error_value();
  EXPECT_TRUE(open_bar_v2.value());

  auto bar_v2_foo = this->DlSym(open_bar_v2.value(), TestSym("foo").c_str());
  ASSERT_TRUE(bar_v2_foo.is_ok()) << bar_v2_foo.error_value();
  ASSERT_TRUE(bar_v2_foo.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v2_foo.value()), kFooV2ReturnValue);

  auto bar_v2 = this->DlSym(open_bar_v2.value(), TestSym("bar_v2").c_str());
  ASSERT_TRUE(bar_v2.is_ok()) << bar_v2.error_value();
  ASSERT_TRUE(bar_v2.value());

  EXPECT_EQ(RunFunction<int64_t>(bar_v2.value()), kReturnValueFromRootModule);

  ASSERT_TRUE(this->DlClose(open_bar_v1.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_bar_v2.value()).is_ok());
  ASSERT_TRUE(this->DlClose(open_root.value()).is_ok());
}

}  // namespace
