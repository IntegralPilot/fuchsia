// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TEST_DL_LOAD_TESTS_H_
#define LIB_C_DLFCN_DL_TEST_DL_LOAD_TESTS_H_

#include <format>
#include <string>
#include <string_view>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "dl-impl-tests.h"
#include "dl-system-tests.h"

// It's too much hassle the generate ELF test modules on a system where the
// host code is not usually built with ELF, so don't bother trying to test any
// of the ELF-loading logic on such hosts.  Unfortunately this means not
// discovering any <dlfcn.h> API differences from another non-ELF system that
// has that API, such as macOS.
#ifndef __ELF__
#error "This file should not be used on non-ELF hosts."
#endif

namespace dl::testing {

// This lists the test fixture classes to run DlTests tests against. The
// DlImplTests fixture is a framework for testing the implementation in
// libdl and the DlSystemTests fixture proxies to the system-provided dynamic
// linker. These tests ensure that both dynamic linker implementations meet
// expectations and behave the same way, with exceptions noted within the test.
using TestTypes = ::testing::Types<
#ifdef __Fuchsia__
    dl::testing::DlImplLoadZirconTests,
#endif
// TODO(https://fxbug.dev/324650368): Test fixtures currently retrieve files
// from different prefixed locations depending on the platform. Find a way
// to use a singular API to return the prefixed path specific to the platform so
// that the TestPosix fixture can run on Fuchsia as well.
#ifndef __Fuchsia__
    // libdl's POSIX test fixture can also be tested on Fuchsia and is included
    // for any ELF supported host.
    dl::testing::DlImplLoadPosixTests,
#endif
    dl::testing::DlSystemTests>;

struct TestNames {
  template <typename T>
  static std::string GetName(int i) {
    return std::string(T::kName);
  }
};

// This must be repeated inside each file's anonymous namespace:
// ```
// using dl::testing::DlTests;
// TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);
// ```
template <class Fixture>
using DlTests = Fixture;

// Cast `symbol` into a function returning type T and run it.
template <typename T>
inline T RunFunction([[gnu::nonnull]] void* symbol) {
  auto func_ptr = reinterpret_cast<T (*)()>(reinterpret_cast<uintptr_t>(symbol));
  return func_ptr();
}

// A matcher that uses a MatchesRegex object that matches the format of the
// error messages for dlopen() and dlsym() when a symbol is undefined.
// Example: `EXPECT_THAT(msg, IsUndefinedSymbolErrMsg(name, module));`
MATCHER_P2(IsUndefinedSymbolErrMsg, symbol_name, module_name,
           std::format("error for undefined symbol {} in module {}", symbol_name, module_name)) {
  return ::testing::ExplainMatchResult(
      ::testing::MatchesRegex(std::format(
          // Emitted by Fuchsia-musl when dlsym fails to locate the symbol.
          "Symbol not found: {}"
          // Emitted by Fuchsia-musl when relocation fails to resolve the symbol.
          "|.*Error relocating {}: {}: symbol not found"
          // Emitted by Linux-glibc and Libdl.
          "|.*{}: undefined symbol: {}",
          symbol_name, module_name, symbol_name, module_name, symbol_name)),
      arg, result_listener);
}

MATCHER_P(IsFileNotFoundErrMsg, file, std::format("error for missing file {}", file)) {
  return ::testing::ExplainMatchResult(
      ::testing::MatchesRegex(std::format(
          // emitted by Fuchsia-musl
          "Error loading shared library .*{}: ZX_ERR_NOT_FOUND"
          // emitted by Linux-glibc
          "|.*{}: cannot open shared object file: No such file or directory",
          file, file)),
      arg, result_listener);
}

MATCHER_P2(IsDepFileNotFoundErrMsg, file, dep_file,
           std::format("error for missing dep file {} for {}", dep_file, file)) {
  return ::testing::ExplainMatchResult(
      ::testing::MatchesRegex(std::format(
          // emitted by Fuchsia-musl
          "Error loading shared library .*{}: ZX_ERR_NOT_FOUND \\(needed by {}\\)"
          // emitted by Linux-glibc
          "|.*{}: cannot open shared object file: No such file or directory",
          dep_file, file, dep_file)),
      arg, result_listener);
}

// These are a convenience functions to specify that a specific dependency
// should or should not be found in the Needed set.

constexpr std::pair<std::string_view, bool> Found(std::string_view name) { return {name, true}; }

constexpr std::pair<std::string_view, bool> NotFound(std::string_view name) {
  return {name, false};
}

// Helper functions that will suffix strings with the current test name.

inline std::string TestSym(std::string_view symbol) {
  const ::testing::TestInfo* const test_info =
      ::testing::UnitTest::GetInstance()->current_test_info();
  return std::string{symbol} + "_" + test_info->name();
}

inline std::string TestModule(std::string_view symbol) {
  const ::testing::TestInfo* const test_info =
      ::testing::UnitTest::GetInstance()->current_test_info();
  return std::string{symbol} + "." + test_info->name() + ".module.so";
}

inline std::string TestShlib(std::string_view symbol) {
  const ::testing::TestInfo* const test_info =
      ::testing::UnitTest::GetInstance()->current_test_info();
  return std::string{symbol} + "." + test_info->name() + ".so";
}

}  // namespace dl::testing

#endif  // LIB_C_DLFCN_DL_TEST_DL_LOAD_TESTS_H_
