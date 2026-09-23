// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/device-name-provider/args.h"

#include <zxtest/zxtest.h>

#include "src/bringup/bin/device-name-provider/device_name_provider_config.h"

namespace {
constexpr char kInterface[] = "/dev/whatever/whatever";
constexpr char kNodename[] = "some-four-word-name";

TEST(ArgsTest, DeviceNameProviderNoneProvided) {
  int argc = 1;
  const char* argv[] = {"device-name-provider"};
  const char* error = nullptr;
  device_name_provider_config::Config config;
  DeviceNameProviderArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_TRUE(args.interface.empty());
  ASSERT_TRUE(args.nodename.empty());
  ASSERT_EQ(args.namegen, 1);
  ASSERT_EQ(args.devdir, kDefaultDevdir);
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, DeviceNameConfigCapabilityProvided) {
  int argc = 1;
  const char* argv[] = {"device-name-provider"};
  const char* error = nullptr;
  device_name_provider_config::Config config;
  config.primary_interface() = kInterface;
  DeviceNameProviderArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_EQ(args.interface, kInterface);
  ASSERT_TRUE(args.nodename.empty());
  ASSERT_EQ(args.namegen, 1);
  ASSERT_EQ(args.devdir, kDefaultDevdir);
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, DeviceNameProviderAllProvided) {
  int argc = 9;
  constexpr char kDevDir[] = "/foo";
  const char* argv[] = {"device-name-provider",
                        "--nodename",
                        kNodename,
                        "--interface",
                        kInterface,
                        "--devdir",
                        kDevDir,
                        "--namegen",
                        "0"};
  const char* error = nullptr;
  device_name_provider_config::Config config;
  DeviceNameProviderArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_EQ(args.interface, std::string(kInterface));
  ASSERT_EQ(args.nodename, std::string(kNodename));
  ASSERT_EQ(args.devdir, std::string(kDevDir));
  ASSERT_EQ(args.namegen, 0);
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, DeviceNameProviderValidation) {
  int argc = 2;
  const char* argv[] = {
      "device-name-provider",
      "--interface",
  };
  device_name_provider_config::Config config;
  DeviceNameProviderArgs args;
  const char* error = nullptr;
  ASSERT_LT(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0);
  ASSERT_TRUE(args.interface.empty());
  ASSERT_TRUE(strstr(error, "interface"));

  argc = 2;
  argv[1] = "--nodename";
  args.interface = "";
  args.nodename = "";
  args.namegen = 1;
  error = nullptr;
  ASSERT_LT(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0);
  ASSERT_TRUE(args.nodename.empty());
  ASSERT_TRUE(strstr(error, "nodename"));

  argc = 2;
  argv[1] = "--namegen";
  args.interface = "";
  args.nodename = "";
  args.namegen = 1;
  error = nullptr;
  ASSERT_LT(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0);
  ASSERT_EQ(args.namegen, 1);
  ASSERT_TRUE(strstr(error, "namegen"));
}
}  // namespace
