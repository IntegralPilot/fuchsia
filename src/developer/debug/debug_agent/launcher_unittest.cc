// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/launcher.h"

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.debugger/cpp/fidl.h>

#include <gtest/gtest.h>

namespace {

TEST(DebugAgentLauncherTest, CreateChildDecl) {
  fuchsia_debugger::LaunchOptions options_false;
  options_false.monitor_root_job(false);

  auto decl_false = DebugAgentLauncher::CreateChildDecl("test_agent_false", options_false);
  EXPECT_EQ(decl_false.name(), "test_agent_false");
  ASSERT_TRUE(decl_false.config_overrides().has_value());
  ASSERT_EQ(decl_false.config_overrides()->size(), 1u);
  EXPECT_EQ(decl_false.config_overrides().value()[0].key(), "monitor_root_job");
  ASSERT_TRUE(decl_false.config_overrides().value()[0].value().has_value());
  ASSERT_TRUE(decl_false.config_overrides().value()[0].value()->single().has_value());
  ASSERT_TRUE(decl_false.config_overrides().value()[0].value()->single()->bool_().has_value());
  EXPECT_FALSE(decl_false.config_overrides().value()[0].value()->single()->bool_().value());

  fuchsia_debugger::LaunchOptions options_true;
  options_true.monitor_root_job(true);

  auto decl_true = DebugAgentLauncher::CreateChildDecl("test_agent_true", options_true);
  EXPECT_EQ(decl_true.name(), "test_agent_true");
  ASSERT_TRUE(decl_true.config_overrides().has_value());
  ASSERT_EQ(decl_true.config_overrides()->size(), 1u);
  EXPECT_EQ(decl_true.config_overrides().value()[0].key(), "monitor_root_job");
  ASSERT_TRUE(decl_true.config_overrides().value()[0].value().has_value());
  ASSERT_TRUE(decl_true.config_overrides().value()[0].value()->single().has_value());
  ASSERT_TRUE(decl_true.config_overrides().value()[0].value()->single()->bool_().has_value());
  EXPECT_TRUE(decl_true.config_overrides().value()[0].value()->single()->bool_().value());

  fuchsia_debugger::LaunchOptions options_empty;
  auto decl_empty = DebugAgentLauncher::CreateChildDecl("test_agent_empty", options_empty);
  EXPECT_EQ(decl_empty.name(), "test_agent_empty");
  EXPECT_FALSE(decl_empty.config_overrides().has_value());
}

}  // namespace
