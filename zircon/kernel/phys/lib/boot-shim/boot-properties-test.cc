// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/boot-properties.h"

#include <lib/linux-boot-config/linux-boot-config.h>

#include <utility>
#include <vector>

#include <zxtest/zxtest.h>

namespace {

TEST(BootPropertiesTest, Empty) {
  boot_shim::BootProperties props("");
  EXPECT_NOT_OK(props.GetProperty("foo"));
}

TEST(BootPropertiesTest, CmdlineSimple) {
  boot_shim::BootProperties props("foo=bar baz=qux");
  auto res = props.GetProperty("foo");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "bar");

  auto res2 = props.GetProperty("baz");
  ASSERT_OK(res2);
  EXPECT_EQ(res2.value(), "qux");

  EXPECT_NOT_OK(props.GetProperty("missing"));
}

TEST(BootPropertiesTest, CmdlineFlag) {
  boot_shim::BootProperties props("enable_feature other=1");
  auto res = props.GetProperty("enable_feature");
  ASSERT_OK(res);
  EXPECT_TRUE(res.value().empty());
}

TEST(BootPropertiesTest, CmdlineQuotes) {
  boot_shim::BootProperties props("key1=\"quoted_val\" key2=\"another_val\"");
  auto res1 = props.GetProperty("key1");
  ASSERT_OK(res1);
  EXPECT_EQ(res1.value(), "quoted_val");

  auto res2 = props.GetProperty("key2");
  ASSERT_OK(res2);
  EXPECT_EQ(res2.value(), "another_val");
}

TEST(BootPropertiesTest, CmdlinePrefixCollision) {
  boot_shim::BootProperties props("prefix_other=wrong prefix=correct");
  auto res = props.GetProperty("prefix");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "correct");
}

TEST(BootPropertiesTest, CmdlineLastWins) {
  boot_shim::BootProperties props("key=first key=second");
  auto res = props.GetProperty("key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "second");
}

TEST(BootPropertiesTest, BootconfigLookup) {
  constexpr std::string_view kBootconfigData =
      "kernel.param = foo\n"
      "driver.option = \"value1\"\n"
      "system.mode = normal\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("system.mode=recovery", bootconfig);

  // Bootconfig takes precedence over cmdline.
  auto mode = props.GetProperty("system.mode");
  ASSERT_OK(mode);
  EXPECT_EQ(mode.value(), "normal");

  auto option = props.GetProperty("driver.option");
  ASSERT_OK(option);
  EXPECT_EQ(option.value(), "value1");

  auto param = props.GetProperty("kernel.param");
  ASSERT_OK(param);
  EXPECT_EQ(param.value(), "foo");
}

TEST(BootPropertiesTest, BootconfigFallbackToCmdline) {
  constexpr std::string_view kBootconfigData = "kernel.param = foo\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("fallback_key=cmdline_value", bootconfig);

  auto res = props.GetProperty("fallback_key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "cmdline_value");
}

TEST(BootPropertiesTest, BootconfigHierarchical) {
  constexpr std::string_view kBootconfigData =
      "system {\n"
      "  name = target\n"
      "  version = \"2.0\"\n"
      "}\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  auto name = props.GetProperty("system.name");
  ASSERT_OK(name);
  EXPECT_EQ(name.value(), "target");

  auto ver = props.GetProperty("system.version");
  ASSERT_OK(ver);
  EXPECT_EQ(ver.value(), "2.0");
}

TEST(BootPropertiesTest, BootconfigLastWins) {
  constexpr std::string_view kBootconfigData =
      "test.key = first\n"
      "test.key = second\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  auto res = props.GetProperty("test.key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "second");
}

TEST(BootPropertiesTest, BootconfigArrayReturnsFirst) {
  constexpr std::string_view kBootconfigData = "test.key = \"first\", \"second\", \"third\"\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  auto res = props.GetProperty("test.key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "first");
}

TEST(BootPropertiesTest, BootconfigAppendDoesNotOverwriteBase) {
  constexpr std::string_view kBootconfigData =
      "test.key = base\n"
      "test.key += appended\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  auto res = props.GetProperty("test.key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "base");
}

TEST(BootPropertiesTest, BootconfigOverrideOverwritesBase) {
  constexpr std::string_view kBootconfigData =
      "test.key = base\n"
      "test.key := overridden\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  auto res = props.GetProperty("test.key");
  ASSERT_OK(res);
  EXPECT_EQ(res.value(), "overridden");
}

TEST(BootPropertiesTest, EnumeratePropertyBootconfig) {
  constexpr std::string_view kBootconfigData = "test.key = \"alpha\", \"beta\"\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("", bootconfig);

  std::vector<std::pair<std::string_view, linux_boot_config::Value::Action>> visited;
  props.EnumerateProperty("test.key",
                          [&](std::string_view val, linux_boot_config::Value::Action act) {
                            visited.emplace_back(val, act);
                          });

  ASSERT_EQ(visited.size(), 2);
  EXPECT_EQ(visited[0].first, "alpha");
  EXPECT_EQ(visited[0].second, linux_boot_config::Value::Action::kDefine);
  EXPECT_EQ(visited[1].first, "beta");
  EXPECT_EQ(visited[1].second, linux_boot_config::Value::Action::kAppend);
}

TEST(BootPropertiesTest, EnumeratePropertyCmdline) {
  boot_shim::BootProperties props("test.key=cmdline_val");

  std::vector<std::pair<std::string_view, linux_boot_config::Value::Action>> visited;
  props.EnumerateProperty("test.key",
                          [&](std::string_view val, linux_boot_config::Value::Action act) {
                            visited.emplace_back(val, act);
                          });

  ASSERT_EQ(visited.size(), 1);
  EXPECT_EQ(visited[0].first, "cmdline_val");
  EXPECT_EQ(visited[0].second, linux_boot_config::Value::Action::kDefine);
}

TEST(BootPropertiesTest, EnumeratePropertyBootconfigFallback) {
  constexpr std::string_view kBootconfigData = "other.key = other_val\n";

  linux_boot_config::LinuxBootConfig bootconfig(kBootconfigData);
  boot_shim::BootProperties props("test.key=cmdline_fallback", bootconfig);

  std::vector<std::pair<std::string_view, linux_boot_config::Value::Action>> visited;
  props.EnumerateProperty("test.key",
                          [&](std::string_view val, linux_boot_config::Value::Action act) {
                            visited.emplace_back(val, act);
                          });

  ASSERT_EQ(visited.size(), 1);
  EXPECT_EQ(visited[0].first, "cmdline_fallback");
  EXPECT_EQ(visited[0].second, linux_boot_config::Value::Action::kDefine);
}

}  // namespace
