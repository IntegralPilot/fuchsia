// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(LibraryTests, GoodLibraryMultipleFiles) {
  TestLibrary library;
  library.AddFile("good/fi-0040-a.test.fidl");
  library.AddFile("good/fi-0040-b.test.fidl");

  ASSERT_COMPILED(library);
}

TEST(LibraryTests, BadFilesDisagreeOnLibraryName) {
  TestLibrary library;
  library.AddFile("bad/fi-0040-a.test.fidl");
  library.AddFile("bad/fi-0040-b.test.fidl");

  library.ExpectFail(ErrFilesDisagreeOnLibraryName);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
