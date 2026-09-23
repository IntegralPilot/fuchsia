// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

class MultiThreads : public F2fsFakeDevTestFixture {
 public:
  MultiThreads() : F2fsFakeDevTestFixture() {}
};

class MultiThreadsWithLFS : public MultiThreads, public testing::WithParamInterface<bool> {
 public:
  MultiThreadsWithLFS() : MultiThreads() {
    mount_options_.SetValue(MountOption::kForceLfs, GetParam());
  }
};

TEST_P(MultiThreadsWithLFS, Truncate) {
  zx::result test_file = root_dir_->Create("test2", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  {
    fbl::RefPtr<f2fs::File> vn = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file));

    constexpr int kNTry = 1000;
    uint8_t buf[kBlockSize * 2] = {1};
    FileTester::AppendToFile(vn.get(), buf, sizeof(buf));
    std::thread writer = std::thread([&]() {
      bool run = true;
      std::thread truncate = std::thread([&]() {
        while (run) {
          ASSERT_EQ(vn->Truncate(0), ZX_OK);
        }
      });
      for (int i = 0; i < kNTry; ++i) {
        size_t out_actual;
        ASSERT_EQ(FileTester::Write(vn.get(), buf, sizeof(buf), 0, &out_actual), ZX_OK);
      }
      run = false;
      truncate.join();
    });
    writer.join();
    vn->Close();
  }
}

TEST_P(MultiThreadsWithLFS, Write) {
  zx::result test_file1 = root_dir_->Create("test1", fs::CreationType::kFile);
  ASSERT_TRUE(test_file1.is_ok()) << test_file1.status_string();
  zx::result test_file2 = root_dir_->Create("test2", fs::CreationType::kFile);
  ASSERT_TRUE(test_file2.is_ok()) << test_file2.status_string();
  fbl::RefPtr<f2fs::File> vn1 = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file1));
  fbl::RefPtr<f2fs::File> vn2 = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file2));

  constexpr uint32_t kNumFiles = 2;
  uint32_t file_size =
      (fs_->GetSuperblockInfo().GetTotalBlockCount() - kNrCursegType * kDefaultBlocksPerSegment) /
      kNumFiles;
  uint8_t buf[kBlockSize] = {1};
  // 2 iterations are enough to touch every block and trigger gc.
  for (int i = 0; i < 2; ++i) {
    std::thread writer1 = std::thread([&]() {
      for (uint32_t i = 0; i < file_size; ++i) {
        size_t out_actual;
        ASSERT_EQ(FileTester::Write(vn1.get(), buf, sizeof(buf), i * kBlockSize, &out_actual),
                  ZX_OK);
        ASSERT_EQ(out_actual, sizeof(buf));
      }
    });

    std::thread writer2 = std::thread([&]() {
      for (uint32_t i = 0; i < file_size; ++i) {
        size_t out_actual;
        ASSERT_EQ(FileTester::Write(vn2.get(), buf, sizeof(buf), i * kBlockSize, &out_actual),
                  ZX_OK);
        ASSERT_EQ(out_actual, sizeof(buf));
      }
    });

    writer1.join();
    writer2.join();
  }
  vn1->Close();
  vn2->Close();
}

const std::array<bool, 2> kAllocParams = {false, true};
INSTANTIATE_TEST_SUITE_P(/*no prefix*/, MultiThreadsWithLFS, ::testing::ValuesIn(kAllocParams));

TEST_F(MultiThreads, Create) {
  zx::result child = root_dir_->Create("dir", fs::CreationType::kDirectory);
  ASSERT_TRUE(child.is_ok()) << child.status_string();
  {
    fbl::RefPtr<Dir> child_dir = fbl::RefPtr<Dir>::Downcast(*std::move(child));
    constexpr int kNThreads = 10;
    constexpr int kNEntries = 100;
    std::thread threads[kNThreads];
    for (auto nThread = 0; nThread < kNThreads; ++nThread) {
      threads[nThread] = std::thread([nThread, child_dir]() {
        // Create dentries more than MaxInlineDentry() to trigger ConvertInlineDir().
        for (uint32_t child_count = 0; child_count < kNEntries; ++child_count) {
          umode_t mode = child_count % 2 == 0 ? S_IFDIR : S_IFREG;
          auto child_name = child_count + nThread * kNEntries;
          FileTester::CreateChild(child_dir.get(), mode, std::to_string(child_name));
        }
      });
    }

    for (auto& thread : threads) {
      thread.join();
    }

    // Verify dentries.
    for (uint32_t child = 0; child < kNThreads * kNEntries; ++child) {
      fbl::RefPtr<fs::Vnode> child_vn;
      FileTester::Lookup(child_dir.get(), std::to_string(child), &child_vn);
      ASSERT_TRUE(child_vn);
      ASSERT_EQ(fbl::RefPtr<Dir>::Downcast(child_vn)->IsDir(), (child % 2) == 0);
      ASSERT_EQ(child_vn->Close(), ZX_OK);
    }

    // It should not have inline entires.
    FileTester::CheckNonInlineDir(child_dir.get());
    ASSERT_EQ(child_dir->Close(), ZX_OK);
  }
}

TEST_F(MultiThreads, Unlink) {
  std::string dir_name("dir");
  zx::result child = root_dir_->Create(dir_name, fs::CreationType::kDirectory);
  ASSERT_TRUE(child.is_ok()) << child.status_string();
  {
    fbl::RefPtr<Dir> child_dir = fbl::RefPtr<Dir>::Downcast(*std::move(child));

    constexpr int kNThreads = 10;
    constexpr int kNEntries = 100;
    std::thread threads[kNThreads];

    // create child vnodes.
    for (uint32_t child = 0; child < kNThreads * kNEntries; ++child) {
      umode_t mode = child % 2 == 0 ? S_IFDIR : S_IFREG;
      FileTester::CreateChild(child_dir.get(), mode, std::to_string(child));
    }

    for (auto nThread = 0; nThread < kNThreads; ++nThread) {
      threads[nThread] = std::thread([nThread, child_dir]() {
        // Each thread deletes dentries to make |child_dir| empty.
        for (uint32_t child_count = 0; child_count < kNEntries; ++child_count) {
          auto child_name = child_count + nThread * kNEntries;
          bool is_dir = child_name % 2 == 0;
          FileTester::DeleteChild(child_dir.get(), std::to_string(child_name), is_dir);
        }
      });
    }

    for (auto& thread : threads) {
      thread.join();
    }

    // If |child_dir| is empty, it should be successful.
    FileTester::DeleteChild(root_dir_.get(), dir_name);
    ASSERT_EQ(child_dir->Close(), ZX_OK);
  }
}

}  // namespace
}  // namespace f2fs
