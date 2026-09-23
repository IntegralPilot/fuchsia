// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/command_line_options.h"

#include <gtest/gtest.h>

namespace symbolizer {

namespace {

TEST(CommandLineOptionsTest, ValidOptions) {
  CommandLineOptions options;
  const char* argv[] = {"", "--ids-txt=path/to/ids.txt", "-s", "/symbol/path"};

  setenv("DEBUGINFOD_URLS", "", 1);
  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_TRUE(error.empty());
  ASSERT_TRUE(options.public_symbol_servers.empty());
  ASSERT_TRUE(options.private_symbol_servers.empty());
  ASSERT_EQ(options.ids_txts.size(), 1UL);
  ASSERT_EQ(options.ids_txts[0], "path/to/ids.txt");
  ASSERT_EQ(options.symbol_paths.size(), 1UL);
  ASSERT_EQ(options.symbol_paths[0], "/symbol/path");
}

TEST(CommandLineOptionsTest, SingleServerFromEnv) {
  CommandLineOptions options;
  const char* argv[] = {""};

  setenv("DEBUGINFOD_URLS", "gs://foo", 1);
  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_TRUE(error.empty());
  ASSERT_EQ(options.public_symbol_servers.size(), 1UL);
  ASSERT_EQ(options.private_symbol_servers.size(), 0UL);
  ASSERT_EQ(options.public_symbol_servers[0], "gs://foo");
}

TEST(CommandLineOptionsTest, MultipleServersFromEnv) {
  CommandLineOptions options;
  const char* argv[] = {""};

  setenv("DEBUGINFOD_URLS", "gs://foo gs://bar", 1);
  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_TRUE(error.empty());
  ASSERT_EQ(options.public_symbol_servers.size(), 2UL);
  ASSERT_EQ(options.private_symbol_servers.size(), 0UL);
  ASSERT_EQ(options.public_symbol_servers[0], "gs://foo");
  ASSERT_EQ(options.public_symbol_servers[1], "gs://bar");
}

TEST(CommandLineOptionsTest, DuplicateServerInEnvAndArgs) {
  CommandLineOptions options;
  // Servers specified with DEBUGINFOD_URLS will only ever conflict with public servers.
  const char* argv[] = {"", "--public-symbol-server", "gs://foo"};

  setenv("DEBUGINFOD_URLS", "gs://foo", 1);
  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_TRUE(error.empty());
  ASSERT_EQ(options.private_symbol_servers.size(), 0UL);
  ASSERT_EQ(options.public_symbol_servers.size(), 1UL);
  ASSERT_EQ(options.public_symbol_servers[0], "gs://foo");
}

TEST(CommandLineOptionsTest, NoServerInEnvVar) {
  CommandLineOptions options;
  const char* argv[] = {""};

  unsetenv("DEBUGINFOD_URLS");
  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_TRUE(error.empty());
  ASSERT_TRUE(options.public_symbol_servers.empty());
  ASSERT_TRUE(options.private_symbol_servers.empty());
}

TEST(CommandLineOptionsTest, InvalidOptions) {
  CommandLineOptions options;
  const char* argv[] = {"", "--invalid"};

  const Error error = ParseCommandLine(sizeof(argv) / sizeof(char*), argv, &options);
  ASSERT_FALSE(error.empty());
}

}  // namespace

}  // namespace symbolizer
