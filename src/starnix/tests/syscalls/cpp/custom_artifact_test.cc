// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/stat.h>

#include <fstream>
#include <iostream>

#include <gtest/gtest.h>

namespace {

TEST(CustomArtifactTest, WriteFile) {
  struct stat buf;
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (stat("/custom_artifacts", &buf) == -1 && errno == ENOENT) {
    GTEST_SKIP() << "No /custom_artifacts found, skipping";
  }
  const std::string file_path = "/custom_artifacts/test_doc.txt";
  const std::string contents = "test content";
  {
    std::ofstream file(file_path);
    ASSERT_TRUE(file.is_open());
    file << contents << std::endl;
    file.close();
  }
  {
    std::ifstream file(file_path);
    std::string line;
    getline(file, line);
    ASSERT_EQ(line, contents);
    file.close();
  }
}

}  // namespace
