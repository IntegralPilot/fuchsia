// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/testing/diagnostics.h>

#include <string_view>
#include <utility>

#include "load-tests.h"

namespace ld::testing {
namespace {

using elfldltl::testing::ExpectedErrorList;
using elfldltl::testing::ExpectReport;

// These are a convenience functions to specify that a specific dependency
// should or should not be found in the Needed set.
constexpr std::pair<std::string_view, bool> Found(std::string_view name) { return {name, true}; }

constexpr std::pair<std::string_view, bool> NotFound(std::string_view name) {
  return {name, false};
}

TYPED_TEST(LdLoadFailureTests, MissingSymbol) {
  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({"libld-dep-missing-sym-dep.so"}));

  ASSERT_NO_FATAL_FAILURE(  //
      this->LoadAndFail("missing-sym", ExpectedErrorList{
                                           ExpectReport{
                                               "undefined symbol: ",
                                               "missing_sym",
                                           },
                                       }));
}

TYPED_TEST(LdLoadFailureTests, MissingDependency) {
  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(this->Needed({NotFound("libmissing-dep-dep.so")}));

  ASSERT_NO_FATAL_FAILURE(  //
      this->LoadAndFail("missing-dep", ExpectedErrorList{
                                           ExpectReport{
                                               "cannot open dependency: ",
                                               "libmissing-dep-dep.so",
                                           },
                                       }));
}

TYPED_TEST(LdLoadFailureTests, MissingTransitiveDependency) {
  ASSERT_NO_FATAL_FAILURE(this->Init());

  ASSERT_NO_FATAL_FAILURE(
      this->Needed({Found("libhas-missing-dep.so"), NotFound("libmissing-dep-dep.so")}));

  ASSERT_NO_FATAL_FAILURE(  //
      this->LoadAndFail("missing-transitive-dep", ExpectedErrorList{
                                                      ExpectReport{
                                                          "cannot open dependency: ",
                                                          "libmissing-dep-dep.so",
                                                      },
                                                  }));
}

}  // namespace
}  // namespace ld::testing
