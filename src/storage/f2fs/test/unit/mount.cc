// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using block_client::FakeBlockDevice;
constexpr uint32_t kMountVerifyTest = 0;
constexpr uint32_t kMountDisableExtTest = 1;
constexpr uint32_t kMountActiveLogsTest = 2;

void MountTestVerifyOptions(F2fs *fs, MountOptions &options) {
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();
  for (const auto &option : MountOptions::Iter()) {
    auto value_or = options.GetValue(option);
    ASSERT_TRUE(value_or.is_ok());
    switch (option) {
      case MountOption::kActiveLogs:
        ASSERT_EQ(static_cast<uint32_t>(superblock_info.GetActiveLogs()), *value_or);
        break;
      case MountOption::kDiscard:
      case MountOption::kBgGcOff:
      case MountOption::kDisableExtIdentify:
      case MountOption::kNoUserXAttr:
      case MountOption::kNoAcl:
      case MountOption::kDisableRollForward:
      case MountOption::kInlineXattr:
      case MountOption::kInlineDentry:
      case MountOption::kForceLfs:
        ASSERT_EQ(*value_or != 0, superblock_info.TestOpt(option));
        break;
      default:
        break;
    };
  }
  auto value_or = options.GetValue(MountOption::kMaxNum);
  ASSERT_EQ(value_or.error_value(), ZX_ERR_INVALID_ARGS);
}

void MountTestDisableExt(F2fs *fs, uint32_t expectation) {
  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs, &root);
  Dir *root_dir = static_cast<Dir *>(root.get());
  bool result = (expectation > 0);

  for (const char *ext_item : kMediaExtList) {
    std::string name = "test.";
    name += ext_item;
    // create regular files with cold file extensions
    zx::result vnode = root_dir->Create(name, fs::CreationType::kFile);
    ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
    File *file = static_cast<File *>(vnode.value().get());
    ASSERT_EQ(file->IsColdFile(), result);
    vnode->Close();
  }

  ASSERT_EQ(root->Close(), ZX_OK);
  root = nullptr;
}

void TestSegmentType(F2fs *fs, Dir *root_dir, std::string_view name, bool is_dir,
                     std::vector<CursegType> &out) {
  nid_t nid = 100;
  uint32_t inode_ofs = 0;
  uint32_t indirect_node_ofs = 3;
  CursegType type;
  zx::result vnode =
      root_dir->Create(name, is_dir ? fs::CreationType::kDirectory : fs::CreationType::kFile);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  VnodeF2fs *vn = static_cast<VnodeF2fs *>(vnode.value().get());

  // data block test
  {
    LockedPage page;
    vn->GrabLockedPage(0, &page);
    type = GetSegmentType(*page, PageType::kData, fs->GetSuperblockInfo().GetActiveLogs());
    out.push_back(type);
  }

  // Dnode block test
  {
    LockedPage page;
    fs->GetNodeVnode().GrabLockedPage(vn->Ino(), &page);
    NodePage *node_page = &page.GetPage<NodePage>();
    page.Zero();
    node_page->FillNodeFooter(static_cast<nid_t>(node_page->GetIndex()), vn->Ino(), inode_ofs);
    node_page->SetColdNode(vn->IsDir());
    type = GetSegmentType(*node_page, PageType::kNode, fs->GetSuperblockInfo().GetActiveLogs());
    out.push_back(type);
  }

  // indirect node block test
  {
    LockedPage page;
    fs->GetNodeVnode().GrabLockedPage(nid, &page);
    NodePage *node_page = &page.GetPage<NodePage>();
    page.Zero();
    node_page->FillNodeFooter(static_cast<nid_t>(node_page->GetIndex()), vn->Ino(),
                              indirect_node_ofs);
    node_page->SetColdNode(vn->IsDir());
    type = GetSegmentType(*node_page, PageType::kNode, fs->GetSuperblockInfo().GetActiveLogs());
    out.push_back(type);
  }

  vnode->Close();
}

void MountTestActiveLogs(F2fs *fs, MountOptions options) {
  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs, &root);
  Dir *root_dir = static_cast<Dir *>(root.get());
  const char *filenames[] = {"dir", "warm.exe", "cold.mp4"};
  std::vector<CursegType> results(3, CursegType::kNoCheckType);
  auto num_logs_or = options.GetValue(MountOption::kActiveLogs);
  ASSERT_TRUE(num_logs_or.is_ok());
  size_t num_logs = *num_logs_or;

  constexpr int dir_file = 0;
  constexpr int warm_file = 1;
  [[maybe_unused]] constexpr int cold_file = 2;

  [[maybe_unused]] constexpr int data_block = 0;
  constexpr int dnode_block = 1;
  constexpr int indirect_node_block = 2;

  for (int i = 0; i < 3; ++i) {
    results.clear();
    TestSegmentType(fs, root_dir, filenames[i], (i == dir_file), results);
    for (int j = 0; j < 3; ++j) {
      CursegType type = results[j];
      if (j == indirect_node_block) {
        if (num_logs > 2)
          ASSERT_EQ(type, CursegType::kCursegColdNode);
        else
          ASSERT_EQ(type, CursegType::kCursegHotNode);
      } else if (j == dnode_block) {
        if (i == dir_file || num_logs == 2) {
          ASSERT_EQ(type, CursegType::kCursegHotNode);
        } else if (num_logs == 6) {
          ASSERT_EQ(type, CursegType::kCursegWarmNode);
        } else
          ASSERT_EQ(type, CursegType::kCursegColdNode);

      } else {  // data block case
        if (i == dir_file || num_logs == 2) {
          ASSERT_EQ(type, CursegType::kCursegHotData);
        } else {
          if (i == warm_file && num_logs == 6)
            ASSERT_EQ(type, CursegType::kCursegWarmData);
          else
            ASSERT_EQ(type, CursegType::kCursegColdData);
        }
      }
    }
  }

  ASSERT_EQ(root->Close(), ZX_OK);
  root = nullptr;
}

void MountTestMain(MountOptions &options, uint32_t test, uint32_t priv) {
  std::unique_ptr<f2fs::BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  switch (test) {
    case kMountVerifyTest:
      MountTestVerifyOptions(fs.get(), options);
      break;
    case kMountDisableExtTest:
      MountTestDisableExt(fs.get(), priv);
      break;
    case kMountActiveLogsTest:
      MountTestActiveLogs(fs.get(), options);
      break;
    default:
      ASSERT_EQ(0, 1);
      break;
  };

  FileTester::Unmount(std::move(fs), &bc);
}

TEST(MountTest, Verify) {
  MountOptions options;
  MountTestMain(options, kMountVerifyTest, 0);
}

TEST(MountTest, DisableExtOptions) {
  constexpr uint32_t ShouldNotBeCold = 0;
  MountOptions options{};
  ASSERT_EQ(options.SetValue(MountOption::kDisableExtIdentify, 1), ZX_OK);
  MountTestMain(options, kMountDisableExtTest, ShouldNotBeCold);
}

TEST(MountTest, EnableExtOptions) {
  constexpr uint32_t ShouldBeCold = 1;
  MountOptions options{};
  ASSERT_EQ(options.SetValue(MountOption::kDisableExtIdentify, 0), ZX_OK);
  MountTestMain(options, kMountDisableExtTest, ShouldBeCold);
}

TEST(MountTest, ActiveLogsOptions) {
  for (uint32_t i = 2; i <= 6; i += 2) {
    MountOptions options{};
    ASSERT_EQ(options.SetValue(MountOption::kActiveLogs, i), ZX_OK);
    MountTestMain(options, kMountActiveLogsTest, 0);
  }
  MountOptions option{};
  ASSERT_EQ(option.SetValue(MountOption::kActiveLogs, kMaxActiveLogs), ZX_ERR_INVALID_ARGS);
}

TEST(MountTest, EnableDiscardOptions) {
  std::unique_ptr<f2fs::BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  MountOptions options{};
  ASSERT_EQ(options.SetValue(MountOption::kDiscard, 1), ZX_OK);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  ASSERT_TRUE(fs->GetSuperblockInfo().TestOpt(MountOption::kDiscard));

  FileTester::Unmount(std::move(fs), &bc);
}

}  // namespace
}  // namespace f2fs
