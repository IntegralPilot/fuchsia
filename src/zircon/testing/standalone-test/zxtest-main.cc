// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>

#include <array>
#include <tuple>
#include <vector>

#include <zxtest/zxtest.h>

// This is the same as zxtest's default main() except that it checks the kernel
// command line for gtest arguments and passes them through to the test.
// Since this is run directly from boot there's no way for the user to pass
// a "normal" argc/argv.
namespace standalone {

int TestMain() {
  std::vector<const char*> argv({"standalone-test"});

  constexpr auto make_options = [](auto&&... prefix) {
    return std::array{standalone::Option{prefix}...};
  };
  // Match all the options handled in zxtest::Options::FromArgs.
  auto options = make_options("--gtest_filter=",                  //
                              "--gtest_repeat=",                  //
                              "--gtest_list_tests",               //
                              "--gtest_shuffle",                  //
                              "--gtest_also_run_disabled_tests",  //
                              "--gtest_repeat=",                  //
                              "--gtest_random_seed=",             //
                              "--gtest_break_on_failure");

  constexpr auto get_options = [](auto&... options) { standalone::GetOptions({options...}); };
  std::apply(get_options, options);

  for (const auto& opt : options) {
    if (!opt.option.empty()) {
      argv.push_back(opt.option.c_str());
    }
  }

  const int argc = static_cast<int>(argv.size());
  return RUN_ALL_TESTS(argc, const_cast<char**>(argv.data()));
}

}  // namespace standalone
