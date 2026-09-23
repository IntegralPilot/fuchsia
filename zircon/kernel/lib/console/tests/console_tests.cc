// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/console.h>

#if CONSOLE_ENABLED

#include <lib/unittest/unittest.h>
#include <string.h>

extern const cmd __start_commands[];
extern const cmd __stop_commands[];

namespace {

bool console_abi_test() {
  BEGIN_TEST;
  static_assert(sizeof(cmd_args) == 40, "cmd_args size mismatch");
  static_assert(alignof(cmd_args) == 8, "cmd_args align mismatch");
  static_assert(sizeof(cmd) == 32, "cmd size mismatch");
  static_assert(alignof(cmd) == 8, "cmd align mismatch");
  END_TEST;
}

// This test verifies that a command registered in Rust (via the `static_command!` macro
// in `tests/src/lib.rs`) is correctly placed in the `.data.rel.ro.commands` linker section
// and is visible from C++.
bool command_visibility_test() {
  BEGIN_TEST;

  bool found = false;
  for (const cmd* c = __start_commands; c != __stop_commands; c++) {
    if (strcmp(c->cmd_str, "mock_success") == 0) {
      found = true;
      EXPECT_EQ(0, strcmp("mock_success help", c->help_str));
      EXPECT_EQ(CMD_AVAIL_NORMAL, c->availability_mask);
      break;
    }
  }

  ASSERT_TRUE(found);

  END_TEST;
}

extern "C" bool command_visibility_from_rust_test();

bool command_visibility_rust_test() {
  BEGIN_TEST;
  EXPECT_TRUE(command_visibility_from_rust_test());
  END_TEST;
}

UNITTEST_START_TESTCASE(console_tests)
UNITTEST("test console ABI compatibility", console_abi_test)
UNITTEST("test command visibility in C++", command_visibility_test)
UNITTEST("test command visibility in Rust", command_visibility_rust_test)
UNITTEST_END_TESTCASE(console_tests, "console", "Tests for Console")

}  // namespace

#endif  // CONSOLE_ENABLED
