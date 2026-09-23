// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <span>
#include <unordered_set>

#include <gtest/gtest.h>

#include "safemath/safe_conversions.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using FileTest = F2fsFakeDevTestFixture;

TEST_F(FileTest, BlkAddrLevel) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  char buf[kBlockSize];
  uint32_t level = 0;

  for (size_t i = 0; i < kBlockSize; ++i) {
    buf[i] = static_cast<char>(rand());
  }

  // fill kAddrsPerInode blocks
  for (int i = 0; i < kAddrsPerInode; ++i) {
    FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);
  }

  // check direct node #1 is not available yet
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, level);

  // fill one more block
  FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);

  // check direct node #1 is available
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, ++level);

  // fill direct node #1
  for (int i = 1; i < kAddrsPerBlock; ++i) {
    FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);
  }

  // check direct node #2 is not available yet
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, level);

  // fill one more block
  FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);

  // check direct node #2 is available
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, ++level);

  // fill direct node #2
  for (int i = 1; i < kAddrsPerBlock; ++i) {
    FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);
  }

  // check indirect node #1 is not available yet
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, level);

  // fill one more block
  FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);

  // check indirect node #1 is available
  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, ++level);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, NidAndBlkaddrAllocFree) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  char buf[kBlockSize];

  for (size_t i = 0; i < kBlockSize; ++i) {
    buf[i] = static_cast<char>(rand() % 128);
  }

  // Fill until direct nodes are full
  unsigned int level = 2;
  for (int i = 0; i < kAddrsPerInode + kAddrsPerBlock * 2; ++i) {
    FileTester::AppendToFile(test_file_ptr, buf, kBlockSize);
  }

  test_file_ptr->SyncFile(false);

  MapTester::CheckNodeLevel(fs_.get(), test_file_ptr, level);

  // Build nid and blkaddr set
  std::unordered_set<nid_t> nid_set;
  std::unordered_set<block_t> blkaddr_set;

  nid_set.insert(test_file_ptr->Ino());
  {
    LockedPage ipage;
    ASSERT_EQ(fs_->GetNodeManager().GetNodePage(test_file_ptr->Ino(), &ipage), ZX_OK);
    Inode *inode = &(ipage->GetAddress<Node>()->i);

    for (int i = 0; i < kNidsPerInode; ++i) {
      if (inode->i_nid[i] != 0U)
        nid_set.insert(inode->i_nid[i]);
    }

    for (int i = 0; i < kAddrsPerInode; ++i) {
      ASSERT_NE(inode->i_addr[i], kNullAddr);
      blkaddr_set.insert(inode->i_addr[i]);
    }

    for (int i = 0; i < 2; ++i) {
      LockedPage direct_node_page;
      ASSERT_EQ(fs_->GetNodeManager().GetNodePage(inode->i_nid[i], &direct_node_page), ZX_OK);
      DirectNode *direct_node = &(direct_node_page->GetAddress<Node>()->dn);

      for (int j = 0; j < kAddrsPerBlock; j++) {
        ASSERT_NE(direct_node->addr[j], kNullAddr);
        blkaddr_set.insert(direct_node->addr[j]);
      }
    }
  }

  ASSERT_EQ(nid_set.size(), level + 1);
  ASSERT_EQ(blkaddr_set.size(), static_cast<uint32_t>(kAddrsPerInode + kAddrsPerBlock * 2));

  // After writing checkpoint, check if nids are removed from free nid list
  // Also, for allocated blkaddr, check if corresponding bit is set in valid bitmap of segment
  fs_->SyncFs(false);

  MapTester::CheckNidsInuse(fs_.get(), nid_set);
  MapTester::CheckBlkaddrsInuse(fs_.get(), blkaddr_set);

  // Remove file, writing checkpoint, then check if nids are added to free nid list
  // Also, for allocated blkaddr, check if corresponding bit is cleared in valid bitmap of segment
  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;

  root_dir_->Unlink("test", false);
  fs_->SyncFs(false);

  MapTester::CheckNidsFree(fs_.get(), nid_set);
  MapTester::CheckBlkaddrsFree(fs_.get(), blkaddr_set);
  test_file_vn = nullptr;
}

TEST_F(FileTest, FileReadExceedFileSize) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  uint32_t data_size = kBlockSize * 7 / 4;
  uint32_t read_location = kBlockSize * 5 / 4;

  auto w_buf = std::make_unique<char[]>(data_size);
  auto r_buf = std::make_unique<char[]>(read_location + kBlockSize);

  for (size_t i = 0; i < data_size; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  // Write data
  FileTester::AppendToFile(test_file_ptr, w_buf.get(), data_size);
  ASSERT_EQ(test_file_ptr->GetSize(), data_size);

  size_t out;
  // Read first part of file
  ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf.get(), read_location, 0, &out), ZX_OK);
  ASSERT_EQ(out, read_location);
  // Read excess file size, then check if actual read size does not exceed the end of file
  ASSERT_EQ(
      FileTester::Read(test_file_ptr, r_buf.get() + read_location, kBlockSize, read_location, &out),
      ZX_OK);
  ASSERT_EQ(out, data_size - read_location);

  ASSERT_EQ(memcmp(r_buf.get(), w_buf.get(), data_size), 0);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, Truncate) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  constexpr uint32_t data_size = Page::Size() * 2;

  char w_buf[data_size];
  char r_buf[data_size * 2];
  std::array<char, data_size> zero = {0};

  for (size_t i = 0; i < data_size; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  size_t out;
  ASSERT_EQ(FileTester::Write(test_file_ptr, w_buf, data_size, 0, &out), ZX_OK);
  ASSERT_EQ(test_file_ptr->GetSize(), out);

  // Truncate to a smaller size, and verify its content and size.
  size_t after = Page::Size() / 2;
  ASSERT_EQ(test_file_ptr->Truncate(after), ZX_OK);
  ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf, data_size, 0, &out), ZX_OK);
  ASSERT_EQ(out, after);
  ASSERT_EQ(test_file_ptr->GetSize(), out);
  ASSERT_EQ(std::memcmp(r_buf, w_buf, after), 0);

  {
    // Check if its vmo is zeroed after |after|.
    LockedPage page;
    test_file_ptr->GrabLockedPage(after / Page::Size(), &page);
    page->Read(r_buf);
    ASSERT_EQ(std::memcmp(r_buf, w_buf, after), 0);
    ASSERT_EQ(std::memcmp(&r_buf[after], zero.data(), Page::Size() - after), 0);
    ASSERT_TRUE(page->IsDirty());
  }

  ASSERT_EQ(FileTester::Write(test_file_ptr, w_buf, data_size, 0, &out), ZX_OK);
  ASSERT_EQ(test_file_ptr->GetSize(), out);

  // Truncate to a large size, and verify its content and size.
  after = data_size + Page::Size() / 2;
  ASSERT_EQ(test_file_ptr->Truncate(after), ZX_OK);
  ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf, after, 0, &out), ZX_OK);
  ASSERT_EQ(out, after);
  ASSERT_EQ(std::memcmp(r_buf, w_buf, data_size), 0);
  ASSERT_EQ(std::memcmp(&r_buf[data_size], zero.data(), after - data_size), 0);

  // Clear all dirty pages.
  test_file_ptr->Writeback(false, true);
  test_file_ptr->Writeback(true, true);

  // Truncate to a smaller size, and check the page state and content.
  after = Page::Size() / 2;
  ASSERT_EQ(test_file_ptr->Truncate(after), ZX_OK);
  {
    LockedPage page;
    test_file_ptr->GrabLockedPage(after / Page::Size(), &page);
    page->Read(r_buf);
    ASSERT_EQ(std::memcmp(r_buf, w_buf, after), 0);
    ASSERT_EQ(std::memcmp(&r_buf[after], zero.data(), Page::Size() - after), 0);
    ASSERT_TRUE(page->IsDirty());
  }

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, WritebackWhileTruncate) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  constexpr size_t written_blocks = 1024;

  zx::result file_or = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(file_or.is_ok()) << file_or.status_string();
  fbl::RefPtr<File> file = fbl::RefPtr<File>::Downcast(std::move(*file_or));
  char w_buf[Page::Size()] = {
      0,
  };

  for (size_t i = 0; i < written_blocks; ++i) {
    size_t offset = Page::Size() * i;
    ASSERT_EQ(FileTester::Write(file.get(), w_buf, Page::Size(), offset, &offset), ZX_OK);
    ASSERT_EQ(file->GetSize(), Page::Size() * i + offset);
  }

  // Schedule writeback tasks for 1024 files
  for (size_t i = 0; i < written_blocks; ++i) {
    std::string name = "test" + std::to_string(i);
    zx::result file_or = root_dir_->Create(name, fs::CreationType::kFile);
    ASSERT_TRUE(file_or.is_ok());
    fbl::RefPtr<File> file = fbl::RefPtr<File>::Downcast(std::move(*file_or));

    size_t offset = 0;
    ASSERT_EQ(FileTester::Write(file.get(), w_buf, Page::Size(), offset, &offset), ZX_OK);
    ASSERT_EQ(file->GetSize(), Page::Size());
    ASSERT_EQ(file->Writeback(false, true), 1UL);
    ASSERT_EQ(file->Close(), ZX_OK);
  }

  // Test the case where writeback pages are assigned addrs but invalidated before writing them to
  // disk. Because of the pre-scheduled tasks, file->Truncate() executes in prior to the writeback
  // task requsting write IOs for |file|.
  ASSERT_EQ(file->Writeback(false, true), written_blocks);
  file->Truncate(0);
  for (size_t i = 0; i < written_blocks; ++i) {
    LockedPage page;
    ASSERT_EQ(file->GrabLockedPage(i, &page), ZX_OK);
    ASSERT_EQ(page->GetBlockAddr(), kNullAddr);
  }

  ASSERT_EQ(file->Close(), ZX_OK);
  file = nullptr;
}

TEST_F(FileTest, MixedSizeWrite) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  std::array<size_t, 5> num_pages = {1, 2, 4, 8, 16};
  size_t total_pages = 0;
  for (auto i : num_pages) {
    total_pages += i;
  }
  size_t data_size = kBlockSize * total_pages;
  auto w_buf = std::make_unique<char[]>(data_size);

  for (size_t i = 0; i < data_size; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  // Write data for various sizes
  char *w_buf_iter = w_buf.get();
  for (auto i : num_pages) {
    size_t cur_size = i * kBlockSize;
    FileTester::AppendToFile(test_file_ptr, w_buf_iter, cur_size);
    w_buf_iter += cur_size;
  }
  ASSERT_EQ(test_file_ptr->GetSize(), data_size);

  // Read verify for each page
  auto r_buf = std::make_unique<char[]>(kBlockSize);
  w_buf_iter = w_buf.get();
  for (size_t i = 0; i < total_pages; ++i) {
    size_t out;
    ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, i * kBlockSize, &out),
              ZX_OK);
    ASSERT_EQ(out, kBlockSize);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, kBlockSize), 0);
    w_buf_iter += kBlockSize;
  }

  // Read verify again after clearing file cache
  {
    test_file_ptr->Writeback(true, true);
    test_file_ptr->ResetFileCache();
  }
  w_buf_iter = w_buf.get();
  for (size_t i = 0; i < total_pages; ++i) {
    size_t out;
    ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, i * kBlockSize, &out),
              ZX_OK);
    ASSERT_EQ(out, kBlockSize);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, kBlockSize), 0);
    w_buf_iter += kBlockSize;
  }

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, LargeChunkReadWrite) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<File> test_file_vn = fbl::RefPtr<File>::Downcast(*std::move(test_file));

  constexpr size_t kNumPage = 256;
  constexpr size_t kDataSize = kBlockSize * kNumPage;
  std::vector<char> w_buf(kDataSize, 0);

  for (size_t i = 0; i < kDataSize; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  FileTester::AppendToFile(test_file_vn.get(), w_buf.data(), kDataSize);
  ASSERT_EQ(test_file_vn->GetSize(), kDataSize);

  // Read verify again after clearing file cache
  {
    test_file_vn->Writeback(true, true);
    test_file_vn->ResetFileCache();
  }
  std::vector<char> r_buf(kDataSize, 0);
  FileTester::ReadFromFile(test_file_vn.get(), r_buf.data(), kDataSize, 0);
  ASSERT_EQ(memcmp(w_buf.data(), r_buf.data(), kDataSize), 0);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, MixedSizeWriteUnaligned) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  std::array<size_t, 5> num_pages = {1, 2, 4, 8, 16};
  size_t total_pages = 0;
  for (auto i : num_pages) {
    total_pages += i;
  }
  size_t unalign = 1000;
  size_t data_size = kBlockSize * total_pages + unalign;
  auto w_buf = std::make_unique<char[]>(data_size);

  for (size_t i = 0; i < data_size; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  // Write some data for unalignment
  FileTester::AppendToFile(test_file_ptr, w_buf.get(), unalign);
  ASSERT_EQ(test_file_ptr->GetSize(), unalign);

  // Write data for various sizes
  char *w_buf_iter = w_buf.get() + unalign;
  for (auto i : num_pages) {
    size_t cur_size = i * kBlockSize;
    FileTester::AppendToFile(test_file_ptr, w_buf_iter, cur_size);
    w_buf_iter += cur_size;
  }
  ASSERT_EQ(test_file_ptr->GetSize(), data_size);

  // Read verify for each page
  auto r_buf = std::make_unique<char[]>(kBlockSize);
  w_buf_iter = w_buf.get();
  for (size_t i = 0; i < total_pages; ++i) {
    size_t out;
    ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, i * kBlockSize, &out),
              ZX_OK);
    ASSERT_EQ(out, kBlockSize);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, kBlockSize), 0);
    w_buf_iter += kBlockSize;
  }

  // Read verify for last unaligned data
  {
    size_t out;
    ASSERT_EQ(
        FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, total_pages * kBlockSize, &out),
        ZX_OK);
    ASSERT_EQ(out, unalign);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, unalign), 0);
  }

  // Read verify again after clearing file cache
  {
    test_file_ptr->Writeback(true, true);
    test_file_vn->ResetFileCache();
  }
  w_buf_iter = w_buf.get();
  for (size_t i = 0; i < total_pages; ++i) {
    size_t out;
    ASSERT_EQ(FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, i * kBlockSize, &out),
              ZX_OK);
    ASSERT_EQ(out, kBlockSize);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, kBlockSize), 0);
    w_buf_iter += kBlockSize;
  }
  {
    size_t out;
    ASSERT_EQ(
        FileTester::Read(test_file_ptr, r_buf.get(), kBlockSize, total_pages * kBlockSize, &out),
        ZX_OK);
    ASSERT_EQ(out, unalign);
    ASSERT_EQ(memcmp(r_buf.get(), w_buf_iter, unalign), 0);
  }

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, OutOfSpace) {
  std::vector<fbl::RefPtr<VnodeF2fs>> vnodes;
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
  zx::result vnode_or = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(vnode_or.is_ok());
  fbl::RefPtr<File> file = fbl::RefPtr<File>::Downcast(*std::move(vnode_or));
  size_t num_blocks = 0;
  uint8_t buf[Page::Size()] = {1};
  size_t out;
  // Fill data until it meets ZX_ERR_NO_SPACE
  while (true) {
    size_t before = file->GetBlockCount();
    zx_status_t ret =
        FileTester::Write(file.get(), buf, sizeof(buf), num_blocks * sizeof(buf), &out);
    size_t after = file->GetBlockCount();
    if (ret == ZX_OK) {
      ASSERT_GT(after, before);
      ++num_blocks;
      continue;
    }
    ASSERT_EQ(before, after);
    ASSERT_EQ(ret, ZX_ERR_NO_SPACE);
    break;
  }
  {
    // The last page we tried should be truncated
    fbl::RefPtr<Page> page;
    file->FindPage(num_blocks, &page);
    ASSERT_TRUE(!page || !page->IsUptodate());
    zx::result addr_or = file->FindAddresses(num_blocks, 1);
    ASSERT_TRUE(addr_or.is_ok());
    ASSERT_EQ(addr_or->front(), kNullAddr);
  }
  size_t size = file->GetSize();
  ASSERT_TRUE(size / kBlockSize > kDefaultBlocksPerSegment);
  vnodes.push_back(file);
  // Secure free blocks as many as a segment
  file->Truncate(size - kDefaultBlocksPerSegment * kBlockSize);
  // Create new files to consume blocks until it meets ZX_ERR_NO_SPACE
  while (true) {
    size_t inodes_before = superblock_info.GetValidInodeCount();
    size_t nodes_before = superblock_info.GetValidNodeCount();
    size_t nids_before = fs_->GetNodeManager().GetFreeNidCount();
    zx::result child_or = root_dir_->Create(std::to_string(--num_blocks), fs::CreationType::kFile);
    size_t inodes_after = superblock_info.GetValidInodeCount();
    size_t nodes_after = superblock_info.GetValidNodeCount();
    size_t nids_after = fs_->GetNodeManager().GetFreeNidCount();
    if (child_or.is_ok()) {
      ASSERT_GT(inodes_after, inodes_before);
      ASSERT_GT(nodes_after, nodes_before);
      ASSERT_GT(nids_before, nids_after);
      child_or->Close();
      vnodes.push_back(fbl::RefPtr<VnodeF2fs>::Downcast(*child_or));
      continue;
    }
    ASSERT_EQ(inodes_before, inodes_after);
    ASSERT_EQ(nodes_before, nodes_after);
    ASSERT_EQ(nids_before, nids_after);
    ASSERT_EQ(child_or.error_value(), ZX_ERR_NO_SPACE);

    zx::result dir_or =
        root_dir_->Create(std::to_string(--num_blocks), fs::CreationType::kDirectory);
    inodes_after = superblock_info.GetValidInodeCount();
    nodes_after = superblock_info.GetValidNodeCount();
    nids_after = fs_->GetNodeManager().GetFreeNidCount();
    ASSERT_EQ(dir_or.status_value(), ZX_ERR_NO_SPACE);
    break;
  }
  file->Close();
  FileTester::DeleteChildren(vnodes, root_dir_, vnodes.size());
}

TEST_F(FileTest, BasicXattrSetGet) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  std::string name = "testname";
  std::array<uint8_t, 5> value = {'x', 'a', 't', 't', 'r'};

  // Initially xattr block is not allocated
  ASSERT_EQ(test_file_ptr->XattrNid(), 0U);

  // Create xattr
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_OK);

  // Xattr block allocated
  ASSERT_NE(test_file_ptr->XattrNid(), 0U);

  // Get and verify
  std::array<uint8_t, kMaxXattrValueLength> buf;
  buf.fill(0);
  zx::result<size_t> result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(*result, value.size());
  ASSERT_EQ(std::memcmp(buf.data(), value.data(), value.size()), 0);

  // Modify xattr
  value = {'h', 'e', 'l', 'l', 'o'};
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_OK);

  // Get and verify
  buf.fill(0);
  result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(*result, value.size());
  ASSERT_EQ(std::memcmp(buf.data(), value.data(), value.size()), 0);

  // Remount and verify again
  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;

  FileTester::Unmount(std::move(fs_), &bc_);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::MountWithOptions(loop_.dispatcher(), mount_options_, &bc_, &fs_);
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));

  fbl::RefPtr<fs::Vnode> test_vn;
  FileTester::Lookup(root_dir_.get(), "test", &test_vn);
  test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(test_vn));
  test_file_ptr = static_cast<File *>(test_file_vn.get());

  buf.fill(0);
  result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(*result, value.size());
  ASSERT_EQ(std::memcmp(buf.data(), value.data(), value.size()), 0);

  // Remove xattr, then get xattr is failed
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, std::span<const uint8_t>(),
                                                XattrOption::kNone),
            ZX_OK);
  result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_NOT_FOUND);

  // Check xattr block deallocated
  ASSERT_EQ(test_file_ptr->XattrNid(), 0U);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, XattrFill) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  // Set xattr until no remaining space
  std::vector<std::pair<std::string, std::vector<uint8_t>>> xattrs;
  std::string current_name;
  std::vector<uint8_t> current_value;
  for (uint32_t i = 0;; ++i) {
    // name string: "a", "ab", "abc", ..., "abcdefgh", "b", "bc", "bcd", ...
    if (i % kMaxNameLen == 0) {
      current_name.clear();
    }
    current_name.push_back(static_cast<std::string::value_type>(
        'a' + (i / kMaxNameLen + i % kMaxNameLen) % ('z' - 'a' + 1)));

    // value string: "a", "ab", "abc", ..., "abc...xyz", "abc...xyza", "abc...xyzab", ...
    current_value.push_back(static_cast<std::string::value_type>('a' + i % ('z' - 'a' + 1)));

    auto ret = test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, current_name, current_value,
                                                   XattrOption::kNone);

    if (ret != ZX_OK) {
      ASSERT_EQ(ret, ZX_ERR_NO_SPACE);
      break;
    }

    xattrs.emplace_back(current_name, current_value);
  }

  // Get and verify
  std::array<uint8_t, kMaxXattrValueLength> buf;
  for (auto &i : xattrs) {
    buf.fill(0);
    zx::result<size_t> result =
        test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, i.first, buf);
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(*result, i.second.size());
    ASSERT_EQ(std::memcmp(buf.data(), i.second.data(), i.second.size()), 0);
  }

  // Remove half of xattrs
  for (uint32_t i = 0; i < xattrs.size(); i += 2) {
    ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, xattrs[i].first,
                                                  std::span<const uint8_t>(), XattrOption::kNone),
              ZX_OK);
  }

  // Get and verify
  for (uint32_t i = 0; i < xattrs.size(); ++i) {
    buf.fill(0);
    zx::result<size_t> result =
        test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, xattrs[i].first, buf);

    // Removed
    if (i % 2 == 0) {  // removed
      ASSERT_TRUE(result.is_error());
      ASSERT_EQ(result.error_value(), ZX_ERR_NOT_FOUND);
    } else {  // exist
      ASSERT_TRUE(result.is_ok());
      ASSERT_EQ(*result, xattrs[i].second.size());
      ASSERT_EQ(std::memcmp(buf.data(), xattrs[i].second.data(), xattrs[i].second.size()), 0);
    }
  }

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, XattrException) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  // Error for empty name
  std::string name;
  std::array<uint8_t, 5> value = {'x', 'a', 't', 't', 'r'};

  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_ERR_INVALID_ARGS);

  std::array<uint8_t, kMaxXattrValueLength> buf;
  buf.fill(0);
  zx::result<size_t> result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_INVALID_ARGS);

  // Error for name length exceed limit
  name.clear();
  for (uint32_t i = 0; i < kMaxNameLen; ++i) {
    name.push_back('a');
  }
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_OK);

  name.push_back('a');
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_ERR_OUT_OF_RANGE);

  buf.fill(0);
  result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_OUT_OF_RANGE);

  // Error for value length exceed limit
  name = "12345678";
  std::array<uint8_t, kMaxXattrValueLength + 1> value_large;
  value_large.fill(0);
  ASSERT_EQ(
      test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value_large, XattrOption::kNone),
      ZX_ERR_OUT_OF_RANGE);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

TEST_F(FileTest, XattrFlagException) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  std::string name = "test";
  std::array<uint8_t, 5> value = {'x', 'a', 't', 't', 'r'};

  // Create xattr
  ASSERT_EQ(
      test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kCreate),
      ZX_OK);

  std::array<uint8_t, kMaxXattrValueLength> buf;
  // Get and verify
  buf.fill(0);
  zx::result<size_t> result = test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, name, buf);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(*result, value.size());
  ASSERT_EQ(std::memcmp(buf.data(), value.data(), value.size()), 0);

  // Error for create xattr that is already exist
  value[0] = '0';
  ASSERT_EQ(
      test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kCreate),
      ZX_ERR_ALREADY_EXISTS);

  // Error for replace xattr that is not exist
  name = "test2";
  ASSERT_EQ(
      test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kReplace),
      ZX_ERR_NOT_FOUND);

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

}  // namespace
}  // namespace f2fs
