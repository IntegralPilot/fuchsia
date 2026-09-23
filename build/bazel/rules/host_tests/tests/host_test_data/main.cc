// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A single binary that can run several host_test() cases that depend on host_test_data()
// targets differently.

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>

#include <functional>
#include <map>
#include <string>

namespace {

// Read a file and return its contents as a string. exit() on error.
std::string ReadFile(const char* path) {
  FILE* f = fopen(path, "rb");
  if (!f) {
    fprintf(stderr, "ERROR: Could not open %s: %s\n", path, strerror(errno));
    exit(1);
  }
  std::string result;
  fseek(f, 0, SEEK_END);
  size_t file_size = ftell(f);
  result.resize(file_size);
  fseek(f, 0, SEEK_SET);

  if (fread(const_cast<char*>(result.data()), 1, file_size, f) != file_size) {
    fprintf(stderr, "ERROR: Could not read %s: %s\n", path, strerror(errno));
    exit(1);
  }

  fclose(f);
  return result;
}

// Check that the content of a ReadFile() result equals an expected value.
// Simply returns on success, exit() on failure with a message.
void AssertStringEquals(const std::string& actual, const std::string& expected, const char* path) {
  if (actual == expected)
    return;

  fprintf(
      stderr,
      "Invalid file content for %s\nACTUAL (%zu bytes)=======[\n%s\n]=== EXPECTED (%zu bytes)====[\n%s\n]=======\n",
      path, actual.size(), actual.c_str(), expected.size(), expected.c_str());
  exit(1);
}

void ReadAndCompareFile(const char* path, const std::string& expected) {
  std::string actual = ReadFile(path);
  AssertStringEquals(actual, expected, path);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) {
    fprintf(stderr, "Usage: %s <test_name>\n", argv[0]);
    return 1;
  }

  // test_bin.txt is a data dependency of the test binary and should thus always be available.
  ReadAndCompareFile("test_bin.txt", "bin\n");

  static const char kDataText1[] = "data1\n";
  static const char kDataText2[] = "data2\n";
  static const char kDataArtifactText[] = "artifact\n";

  // Keep this in check with BUILD.bazel.
  using TestMap = std::map<std::string, std::function<void()>>;

  auto check_files_list = []() {
    ReadAndCompareFile("test_data_1.txt", kDataText1);
    ReadAndCompareFile("test_data_2.txt", kDataText2);
    ReadAndCompareFile("artifact.txt", kDataArtifactText);
  };

  auto check_files_list_with_dest_dir = []() {
    ReadAndCompareFile("test_data/test_data_1.txt", kDataText1);
    ReadAndCompareFile("test_data/test_data_2.txt", kDataText2);
    ReadAndCompareFile("test_data/artifact.txt", kDataArtifactText);
  };

  auto check_explicit_map = []() {
    ReadAndCompareFile("data_1/test.txt", kDataText1);
    ReadAndCompareFile("subdir/data_2/test.txt", kDataText2);
    ReadAndCompareFile("obj/artifact", kDataArtifactText);
  };

  const TestMap test_map = {
      // First test only checks for test_bin.txt above.
      // clang-format off
      {"binary_data_dep", []() {}},
      {"data_deps", check_files_list},
      {"data_filegroup_deps", check_files_list},
      {"explicit_map", check_explicit_map},
      {"dest_dir", check_files_list_with_dest_dir},
      // clang-format on
  };

  auto it = test_map.find(std::string(argv[1]));
  if (it == test_map.end()) {
    fprintf(stderr, "ERROR: Unknown test name (%s), must be one of: ", argv[1]);
    for (const auto& pair : test_map) {
      fprintf(stderr, " %s", pair.first.c_str());
    }
    fprintf(stderr, "\n");
    return 1;
  }

  it->second();
  printf("ok\n");
  return 0;
}
