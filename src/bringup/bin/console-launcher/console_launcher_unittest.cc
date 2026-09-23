// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/console-launcher/console_launcher.h"

#include <zxtest/zxtest.h>

namespace {

TEST(SystemInstanceTest, CheckBootArgParsing) {
  console_launcher_config::Config config;
  config.console_shell() = true;
  config.use_virtio_console() = true;
  config.term() = "FAKE_TERM";
  config.autorun_boot() = "/boot/bin/ls+/dev/class/";
  config.autorun_system() = "/boot/bin/ls+/system";
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_TRUE(args->run_shell);
  ASSERT_EQ(args->term, "TERM=FAKE_TERM");
  ASSERT_TRUE(args->use_virtio_console);
  ASSERT_EQ(args->autorun_boot, "/boot/bin/ls+/dev/class/");
  ASSERT_EQ(args->autorun_system, "/boot/bin/ls+/system");
  ASSERT_EQ(args->virtcon_disabled, false);
}

TEST(SystemInstanceTest, CheckBootArgDefaultStrings) {
  console_launcher_config::Config config;
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_FALSE(args->run_shell);
  ASSERT_EQ(args->term, "TERM=uart");
  ASSERT_FALSE(args->use_virtio_console);
  ASSERT_EQ(args->autorun_boot, "");
  ASSERT_EQ(args->autorun_system, "");
}

// The defaults are that a system is not required, so zedboot will try to launch.
TEST(VirtconSetup, VirtconDefaults) {
  std::map<std::string, std::string> arguments;

  console_launcher_config::Config config;
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_FALSE(args->virtual_console_need_debuglog);
}

// Need debuglog should be true when netboot is true and netboot is not disabled.
TEST(VirtconSetup, VirtconNeedDebuglog) {
  console_launcher_config::Config config;
  config.netsvc_disable() = false;
  config.netsvc_netboot() = true;
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_TRUE(args->virtual_console_need_debuglog);
}

// If netboot is true but netsvc is disabled, don't start debuglog.
TEST(VirtconSetup, VirtconNetbootWithNetsvcDisabled) {
  console_launcher_config::Config config;
  config.netsvc_disable() = true;
  config.netsvc_netboot() = true;
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_FALSE(args->virtual_console_need_debuglog);
}

// Check that virtcon_disabled is propogated through to args correctly.
TEST(VirtconSetup, VirtconDisabled) {
  console_launcher_config::Config config;
  config.virtcon_disabled() = true;
  zx::result args = console_launcher::GetArguments(config);
  ASSERT_OK(args.status_value());

  ASSERT_TRUE(args->virtcon_disabled);
}

}  // namespace
