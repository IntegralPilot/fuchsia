// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <iostream>
#include <optional>
#include <string>
#include <thread>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/storage/fs_test/fs_test_fixture.h"

namespace fs_test {
namespace {

using DirectoryMaxTest = FilesystemTest;

// Hopefully not pushing against any 'max file length' boundaries, but large enough to fill a
// directory quickly.
constexpr int kLargePathLength = 128;

TEST_P(DirectoryMaxTest, Max) {
  // Write the maximum number of files to a directory
  const std::string dir = "dir/";
  ASSERT_EQ(mkdir(GetPath(dir).c_str(), 0777), 0);

  class Context {
   public:
    explicit Context(std::string prefix) : path_prefix_(std::move(prefix)) {}

    void fill() {
      for (;; ++cnt_) {
        const std::string path = path_prefix_ + std::to_string(cnt_);
        fbl::unique_fd fd(open(path.c_str(), O_RDWR | O_CREAT | O_EXCL, 0644));
        if (!fd) {
          break;
        }
      }
    }

    void unlink_all() {
      for (cnt_ -= cnt_; cnt_ >= 0; cnt_--) {
        const std::string path = path_prefix_ + std::to_string(cnt_);
        int r = unlink(path.c_str());
        if (r != 0) {
          err_ = true;
          break;
        }
      }
    }

    bool err() const { return err_; }
    int count() const { return cnt_; }

   private:
    std::string path_prefix_;
    int cnt_ = 0;
    bool err_ = false;
  };

  const std::string path_prefix = GetPath(dir + std::string(kLargePathLength, '.'));
  std::vector<Context> contexts;
  for (int i = 0; i < 16; i++) {
    contexts.emplace_back(Context(path_prefix + std::to_string(i) + "_"));
  }

  std::vector<std::thread> fillers;
  for (auto& context : contexts) {
    fillers.emplace_back(std::thread([&context]() { context.fill(); }));
  }
  for (auto& thread : fillers) {
    thread.join();
  }

  int total_files = 0;
  for (const auto& context : contexts) {
    total_files += context.count();
  }
  std::cerr << "Wrote a total of " << total_files << "." << std::endl;

  std::cerr << "Starting unmount." << std::endl;
  EXPECT_EQ(fs().Unmount().status_value(), ZX_OK);
  std::cerr << "Starting fsck." << std::endl;
  EXPECT_EQ(fs().Fsck().status_value(), ZX_OK);
  std::cerr << "Starting mount." << std::endl;
  EXPECT_EQ(fs().Mount().status_value(), ZX_OK);

  std::cerr << "Time to clean up." << std::endl;

  std::vector<std::thread> unlinkers;
  for (auto& context : contexts) {
    unlinkers.emplace_back(std::thread([&context]() { context.unlink_all(); }));
  }
  for (auto& thread : unlinkers) {
    thread.join();
  }
  for (auto& context : contexts) {
    ASSERT_FALSE(context.err());
  }
  std::cerr << "Done." << std::endl;
}

INSTANTIATE_TEST_SUITE_P(
    /*no prefix*/, DirectoryMaxTest,
    testing::ValuesIn(MapAndFilterAllTestFilesystems(
        [](TestFilesystemOptions options) -> std::optional<TestFilesystemOptions> {
          // Filesystems such as memfs cannot run this test because they OOM (as expected, given
          // memory is the limiting factor).
          if (options.filesystem->GetTraits().in_memory)
            return std::nullopt;
          if (!options.filesystem->GetTraits().has_directory_size_limit &&
              !options.filesystem->GetTraits().has_min_volume_size) {
            // Fatfs is slow and, other than the root directory on FAT12/16, is limited by the size
            // of the ram-disk rather than a directory size limit, so use a small ram-disk to keep
            // run-time reasonable, and do the same for other filesystems that don't have a
            // directory size limit.
            options.device_block_count = options.filesystem->GetTraits().is_slow ? 256 : 4096;
          }
          return options;
        })),
    testing::PrintToStringParamName());

GTEST_ALLOW_UNINSTANTIATED_PARAMETERIZED_TEST(DirectoryMaxTest);

}  // namespace
}  // namespace fs_test
