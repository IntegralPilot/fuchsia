// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/soname.h>
#include <lib/ld/testing/mock-debugdata.h>

#include "load-tests.h"

namespace ld::testing {
namespace {

TYPED_TEST(LdLoadTests, LdsvcConfig) {
  constexpr elfldltl::Soname<> kExecutableName{"ldsvc-config"};
  constexpr std::string_view kConfig = "test-libprefix";
  constexpr int64_t kReturnValue = 17;

  if constexpr (!TestFixture::kRunsLdStartup) {
    GTEST_SKIP() << "test only applies to startup dynamic linker";
  }

  ASSERT_NO_FATAL_FAILURE(this->Init());

  // We'll tell Load() to check for the expected PT_INTERP string, but that's
  // all it will do about the libprefix.  Explicitly prime the mock to expect
  // the Config() FIDL message from the startup dynamic linker.
  this->LdsvcExpectConfig(kConfig);

  // Now to prime the mock with a dependency VMO we must discern the actual
  // libprefix for how the test was built to match up where the dependency has
  // been packaged.  The usual test fixture machinery would find that via the
  // PT_INTERP prefix, but that's overridden to match kConfig for the test.
  // Instead, use the build-time record from the dependency.
  ASSERT_NO_FATAL_FAILURE(this->NeededViaLoadSet(  //
      kExecutableName, {"libld-dep-a.so"}));

  // The executable doesn't do anything interesting.  It's redundant with the
  // LdLoadTests.Basic test, except that its PT_INTERP has a prefix so the
  // startup dynamic linker should send the Config() FIDL request.
  ASSERT_NO_FATAL_FAILURE(this->Load(kExecutableName.str(), kConfig));

  EXPECT_EQ(this->Run(), kReturnValue);

  this->ExpectLog("");
}

}  // namespace
}  // namespace ld::testing
