// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/bringup/bin/netsvc/args.h"

#include <zxtest/zxtest.h>

#include "src/bringup/bin/netsvc/netsvc_config.h"

namespace {
constexpr char kInterface[] = "/dev/whatever/whatever";

TEST(ArgsTest, NetsvcNoneProvided) {
  int argc = 1;
  const char* argv[] = {"netsvc"};
  const char* error = nullptr;
  auto config = netsvc_config::Config();
  config.advertise() = true;
  NetsvcArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_FALSE(args.netboot);
  ASSERT_FALSE(args.print_nodename_and_exit);
  ASSERT_TRUE(args.advertise);
  ASSERT_FALSE(args.all_features);
  ASSERT_TRUE(args.interface.empty());
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, NetsvcStructuredConfigProvided) {
  int argc = 1;
  const char* argv[] = {"netsvc"};
  const char* error = nullptr;
  auto config = netsvc_config::Config();
  config.advertise() = true;
  config.primary_interface() = kInterface;
  NetsvcArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_FALSE(args.netboot);
  ASSERT_FALSE(args.print_nodename_and_exit);
  ASSERT_TRUE(args.advertise);
  ASSERT_FALSE(args.all_features);
  ASSERT_EQ(args.interface, kInterface);
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, NetsvcAllProvided) {
  int argc = 7;
  const char* argv[] = {
      "netsvc",         "--netboot",   "--nodename", "--advertise",
      "--all-features", "--interface", kInterface,
  };
  auto config = netsvc_config::Config();
  const char* error = nullptr;
  NetsvcArgs args;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  ASSERT_TRUE(args.netboot);
  ASSERT_TRUE(args.print_nodename_and_exit);
  ASSERT_TRUE(args.advertise);
  ASSERT_TRUE(args.all_features);
  ASSERT_EQ(args.interface, std::string(kInterface));
  ASSERT_EQ(error, nullptr);
}

TEST(ArgsTest, NetsvcValidation) {
  int argc = 2;
  const char* argv[] = {
      "netsvc",
      "--interface",
  };
  auto config = netsvc_config::Config();
  const char* error = nullptr;
  NetsvcArgs args;
  ASSERT_LT(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0);
  ASSERT_TRUE(args.interface.empty());
  ASSERT_TRUE(strstr(error, "interface"));
}

TEST(ArgsTest, LogPackets) {
  int argc = 2;
  const char* argv[] = {
      "netsvc",
      "--log-packets",
  };
  auto config = netsvc_config::Config();
  NetsvcArgs args;
  EXPECT_FALSE(args.log_packets);
  const char* error = nullptr;
  ASSERT_EQ(ParseArgs(argc, const_cast<char**>(argv), config, &error, &args), 0, "%s", error);
  EXPECT_TRUE(args.log_packets);
}

}  // namespace
