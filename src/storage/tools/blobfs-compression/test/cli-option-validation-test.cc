// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>

#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/storage/tools/blobfs-compression/blobfs-compression.h"

namespace blobfs_compress {
namespace {

class CliOptionValidationTest : public ::testing::Test {
 public:
  CliOptionValidationTest() : test_dir_(files::ScopedTempDir("/tmp")) {}

 protected:
  files::ScopedTempDir test_dir_;
};

TEST_F(CliOptionValidationTest, NoSourceFileNoOutputFile) {
  CompressionCliOptionStruct options_missing_source;
  ASSERT_EQ(ValidateCliOptions(options_missing_source), ZX_ERR_INVALID_ARGS);
}

TEST_F(CliOptionValidationTest, OutputFileOnly) {
  CompressionCliOptionStruct options_missing_source = {
      .compressed_file = "test",
  };
  ASSERT_EQ(ValidateCliOptions(options_missing_source), ZX_ERR_INVALID_ARGS);
}

TEST_F(CliOptionValidationTest, ValidSourceFileNoOutputFile) {
  const std::string file_path = test_dir_.path() + "/valid_file";
  ASSERT_TRUE(files::WriteFile(file_path, "hello"));
  CompressionCliOptionStruct options_valid = {
      .source_file = file_path,
  };
  options_valid.source_file_fd.reset(open(file_path.c_str(), O_RDONLY));
  ASSERT_EQ(ValidateCliOptions(options_valid), ZX_OK);
}

TEST_F(CliOptionValidationTest, ValidEmptyExistingSourceFileNoOutputFile) {
  const std::string file_path = test_dir_.path() + "/valid_empty_file";
  ASSERT_TRUE(files::WriteFile(file_path, ""));
  CompressionCliOptionStruct options_valid = {
      .source_file = file_path,
  };
  options_valid.source_file_fd.reset(open(file_path.c_str(), O_RDONLY));
  ASSERT_EQ(ValidateCliOptions(options_valid), ZX_OK);
}

TEST_F(CliOptionValidationTest, SourceFileIsDirectory) {
  const std::string dir_path = test_dir_.path() + "/directory";
  ASSERT_TRUE(files::CreateDirectory(dir_path));
  CompressionCliOptionStruct options_valid = {
      .source_file = dir_path,
  };
  options_valid.source_file_fd.reset(open(dir_path.c_str(), O_DIRECTORY | O_RDONLY));
  ASSERT_EQ(ValidateCliOptions(options_valid), ZX_ERR_NOT_FILE);
}

TEST_F(CliOptionValidationTest, ValidSourceFileValidOutputFile) {
  const std::string source_path = test_dir_.path() + "/source_file";
  const std::string output_path = test_dir_.path() + "/output_file";
  ASSERT_TRUE(files::WriteFile(source_path, "hello"));
  CompressionCliOptionStruct options_valid = {
      .source_file = source_path,
      .compressed_file = output_path,
  };
  options_valid.source_file_fd.reset(open(source_path.c_str(), O_RDONLY));
  options_valid.compressed_file_fd.reset(
      open(output_path.c_str(), O_RDWR | O_CREAT | O_TRUNC, S_IRUSR));
  ASSERT_EQ(ValidateCliOptions(options_valid), ZX_OK);
}

TEST_F(CliOptionValidationTest, ValidSourceFileInvalidOutputFile) {
  const std::string source_path = test_dir_.path() + "/source_file";
  const std::string invalid_output_path = test_dir_.path() + "/output_directory";
  ASSERT_TRUE(files::WriteFile(source_path, "hello"));
  ASSERT_TRUE(files::CreateDirectory(invalid_output_path));
  CompressionCliOptionStruct options_valid = {
      .source_file = source_path,
      .compressed_file = invalid_output_path,
  };
  options_valid.source_file_fd.reset(open(source_path.c_str(), O_RDONLY));
  // Open directory as file.
  options_valid.compressed_file_fd.reset(
      open(invalid_output_path.c_str(), O_RDWR | O_CREAT | O_TRUNC, S_IRUSR));
  ASSERT_EQ(ValidateCliOptions(options_valid), ZX_ERR_BAD_PATH);
}

}  // namespace
}  // namespace blobfs_compress
