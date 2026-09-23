// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/dump/depth_printer.h>
#include <lib/unittest/unittest.h>
#include <string-file.h>

#include <ktl/array.h>

namespace {

bool depth_printer_smoke_test() {
  BEGIN_TEST;

  constexpr ktl::string_view kExpected =
      "Hello\n\
        World\n\
One\n\
Two\n\
[2 items not emitted]\n\
        One\n\
        Two\n\
        Goodbye\n";

  ktl::array<char, 2048> storage;
  StringFile buffer(storage);

  dump::DepthPrinter no_depth(&buffer, 0);
  dump::DepthPrinter depth(&buffer, 4);

  no_depth.Emit("Hello");
  depth.Emit("World");
  no_depth.BeginList(2);
  no_depth.Emit("One");
  no_depth.Emit("Two");
  no_depth.Emit("Three");
  no_depth.Emit("Four");
  no_depth.EndList();
  depth.BeginList(4);
  depth.Emit("One");
  depth.Emit("Two");
  depth.EndList();
  depth.Emit("Goodbye");

  auto used = buffer.used_region();
  EXPECT_EQ(kExpected.size(), used.size());
  EXPECT_BYTES_EQ(reinterpret_cast<const uint8_t*>(kExpected.data()),
                  reinterpret_cast<const uint8_t*>(used.data()),
                  ktl::min(kExpected.size(), used.size()));

  END_TEST;
}

}  // anonymous namespace

// Use the function name as the test name
#define VA_UNITTEST(fname) UNITTEST(#fname, fname)

UNITTEST_START_TESTCASE(depth_printer_tests)
VA_UNITTEST(depth_printer_smoke_test)
UNITTEST_END_TESTCASE(depth_printer_tests, "depth_printer", "depth_printer tests")
