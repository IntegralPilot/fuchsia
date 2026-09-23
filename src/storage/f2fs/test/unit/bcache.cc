// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bcache.h"

#include <fidl/fuchsia.storage.block/cpp/wire.h>

#include <gtest/gtest.h>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using block_client::FakeBlockDevice;

TEST(BCacheTest, Trim) {
  {
    auto device = std::make_unique<FakeBlockDevice>(
        FakeBlockDevice::Config{.block_count = kDefaultSectorCount,
                                .block_size = kDefaultSectorSize,
                                .supports_trim = false});
    ASSERT_TRUE(device);
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    ASSERT_TRUE(bc_or.is_ok());

    fuchsia_storage_block::wire::BlockInfo info;
    bc_or->BlockGetInfo(&info);
    block_t end_blk = static_cast<block_t>(bc_or->Maxblk() / (kBlockSize / info.block_size));
    ASSERT_EQ(bc_or->Trim(0, end_blk), ZX_ERR_NOT_SUPPORTED);
  }
  {
    auto device = std::make_unique<FakeBlockDevice>(
        FakeBlockDevice::Config{.block_count = kDefaultSectorCount,
                                .block_size = kDefaultSectorSize,
                                .supports_trim = true});
    ASSERT_TRUE(device);
    auto bc_or = CreateBcacheMapper(std::move(device), true);
    ASSERT_TRUE(bc_or.is_ok());

    fuchsia_storage_block::wire::BlockInfo info;
    bc_or->BlockGetInfo(&info);
    block_t end_blk = static_cast<block_t>(bc_or->Maxblk() / (kBlockSize / info.block_size));
    ASSERT_EQ(bc_or->Trim(0, end_blk), ZX_OK);
  }
}

TEST(BCacheTest, Exception) {
  // Test zero block_size exception case
  {
    std::unique_ptr<f2fs::BcacheMapper> bc;
    auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
        .block_count = kDefaultSectorCount, .block_size = 0, .supports_trim = false});
    ASSERT_TRUE(device);
    auto bc_or = CreateBcacheMapper(std::move(device));
    ASSERT_TRUE(bc_or.is_error());
    ASSERT_EQ(bc_or.status_value(), ZX_ERR_INVALID_ARGS);
  }
  // Test block_count overflow exception case
  {
    std::unique_ptr<f2fs::BcacheMapper> bc;
    auto device = std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
        .block_count = static_cast<uint64_t>(std::numeric_limits<uint32_t>::max()) * 8,
        .block_size = kDefaultSectorSize,
        .supports_trim = true});
    ASSERT_TRUE(device);
    auto bc_or = CreateBcacheMapper(std::move(device));
    ASSERT_TRUE(bc_or.is_error());
    ASSERT_EQ(bc_or.status_value(), ZX_ERR_OUT_OF_RANGE);
  }
}

TEST(BCacheTest, VmoidReuse) {
  std::unique_ptr<f2fs::BcacheMapper> bc;
  auto device =
      std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{.block_count = kDefaultSectorCount,
                                                                .block_size = kDefaultSectorSize,
                                                                .supports_trim = false});
  ASSERT_TRUE(device);

  auto bc_or = CreateBcacheMapper(std::move(device));
  ASSERT_FALSE(bc_or.is_error());

  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  storage::Vmoid vmoid;
  // When the BcacheMapper is created, BlockAttachVmo() is called once internally. Therefore, the
  // expected_vmoid starts at BLOCK_VMOID_INVALID + 2.
  for (vmoid_t expected_vmoid = BLOCK_VMOID_INVALID + 2;
       expected_vmoid < std::numeric_limits<vmoid_t>::max(); ++expected_vmoid) {
    ASSERT_EQ(bc_or->BlockAttachVmo(vmo, &vmoid), ZX_OK);
    ASSERT_EQ(vmoid.get(), expected_vmoid);
    ASSERT_EQ(bc_or->BlockDetachVmo(std::move(vmoid)), ZX_OK);
  }

  ASSERT_EQ(bc_or->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  ASSERT_EQ(vmoid.get(), BLOCK_VMOID_INVALID + 2);
  ASSERT_EQ(bc_or->BlockDetachVmo(std::move(vmoid)), ZX_OK);
}

TEST(BCacheTest, Flag) {
  std::unique_ptr<f2fs::BcacheMapper> bc;
  auto device =
      std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{.block_count = kDefaultSectorCount,
                                                                .block_size = kDefaultSectorSize,
                                                                .supports_trim = false});
  ASSERT_TRUE(device);

  fuchsia_storage_block::wire::DeviceFlag test_flag =
      fuchsia_storage_block::wire::DeviceFlag::kReadonly |
      fuchsia_storage_block::wire::DeviceFlag::kRemovable |
      fuchsia_storage_block::wire::DeviceFlag::kTrimSupport |
      fuchsia_storage_block::wire::DeviceFlag::kFuaSupport;

  device->SetInfoFlags(test_flag);

  auto bc_or = CreateBcacheMapper(std::move(device));
  ASSERT_FALSE(bc_or.is_error());

  fuchsia_storage_block::wire::BlockInfo info;
  bc_or->BlockGetInfo(&info);
  ASSERT_EQ(test_flag, info.flags);
}

class BcacheMapperTest
    : public testing::Test,
      public testing::WithParamInterface<std::pair<std::vector<uint64_t>, std::vector<uint32_t>>> {
 public:
  BcacheMapperTest() : block_counts_(GetParam().first), block_sizes_(GetParam().second) {}

  void SetUp() override {
    ASSERT_EQ(block_counts_.size(), block_sizes_.size());

    for (size_t i = 0; i < block_counts_.size(); ++i) {
      fake_block_devices_.push_back(std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
          .block_count = block_counts_[i], .block_size = block_sizes_[i], .supports_trim = true}));
    }

    auto bc_or = CreateBcacheMapper(std::move(fake_block_devices_), true);
    ASSERT_TRUE(bc_or.is_ok());

    bcache_ = std::move(bc_or.value());
  }

 protected:
  uint64_t GetMiddlePositionOfDevice(uint32_t num) {
    ZX_ASSERT(num < block_counts_.size());

    uint64_t start_off = 0;
    uint64_t end_off = 0;
    for (size_t i = 0; i < num; ++i) {
      start_off += block_counts_[i] * block_sizes_[i] / bcache_->BlockSize();
    }
    end_off = start_off + block_counts_[num] * block_sizes_[num] / bcache_->BlockSize() - 1;

    return (start_off + end_off) / 2;
  }

  uint64_t GetEndPositionOfDevice(uint32_t num) {
    ZX_ASSERT(num < block_counts_.size());

    uint64_t end_off = 0;
    for (size_t i = 0; i <= num; ++i) {
      end_off += block_counts_[i] * block_sizes_[i] / bcache_->BlockSize();
    }
    end_off--;

    return end_off;
  }

  const std::vector<uint64_t> block_counts_;
  const std::vector<uint32_t> block_sizes_;
  std::vector<std::unique_ptr<block_client::BlockDevice>> fake_block_devices_;
  std::unique_ptr<BcacheMapper> bcache_;
};

TEST_P(BcacheMapperTest, BlockGetInfo) {
  fuchsia_storage_block::wire::BlockInfo info;
  bcache_->BlockGetInfo(&info);

  // Check block size
  ASSERT_EQ(info.block_size, *std::max_element(block_sizes_.begin(), block_sizes_.end()));

  // Check block count
  uint64_t byte_size = 0;
  bcache_->ForEachBcache([&](Bcache *underlying_bc) {
    byte_size += underlying_bc->Maxblk() * underlying_bc->BlockSize();
  });

  ASSERT_EQ(info.block_count * info.block_size, byte_size);
}

TEST_P(BcacheMapperTest, AttachDetachVmo) {
  zx::vmo vmo;
  ASSERT_EQ(zx::vmo::create(zx_system_get_page_size(), 0, &vmo), ZX_OK);

  storage::Vmoid vmoid;
  ASSERT_EQ(bcache_->BlockAttachVmo(vmo, &vmoid), ZX_OK);
  ASSERT_EQ(bcache_->BlockDetachVmo(std::move(vmoid)), ZX_OK);
}

TEST_P(BcacheMapperTest, RunRequests) {
  std::vector<std::pair<uint64_t, uint64_t>> dev_offset_and_length = {};

  for (uint32_t start = 0; start < block_counts_.size(); ++start) {
    for (uint32_t end = start; end < block_counts_.size(); ++end) {
      dev_offset_and_length.emplace_back(
          GetMiddlePositionOfDevice(start),
          GetEndPositionOfDevice(end) - GetMiddlePositionOfDevice(start) + 1);
    }
  }

  std::vector<char> pattern(bcache_->BlockSize(), 0b10101010);

  for (const auto &[dev_offset, length] : dev_offset_and_length) {
    storage::VmoBuffer buffer;
    ASSERT_EQ(buffer.Initialize(bcache_.get(), bcache_->Maxblk(), bcache_->BlockSize(), "test"),
              ZX_OK);

    for (size_t i = 0; i < length; ++i) {
      std::memcpy(buffer.Data(dev_offset + i), pattern.data(), bcache_->BlockSize());
    }

    ASSERT_EQ(bcache_->RunRequests(
                  {storage::BufferedOperation{.vmoid = buffer.vmoid(),
                                              .op =
                                                  {
                                                      .type = storage::OperationType::kWrite,
                                                      .vmo_offset = dev_offset,
                                                      .dev_offset = dev_offset,
                                                      .length = length,
                                                  }}}),
              ZX_OK);

    buffer.Zero(dev_offset, length);

    ASSERT_EQ(bcache_->RunRequests(
                  {storage::BufferedOperation{.vmoid = buffer.vmoid(),
                                              .op =
                                                  {
                                                      .type = storage::OperationType::kRead,
                                                      .vmo_offset = dev_offset,
                                                      .dev_offset = dev_offset,
                                                      .length = length,
                                                  }}}),
              ZX_OK);

    for (size_t i = 0; i < length; ++i) {
      auto ptr = buffer.Data(dev_offset + i);
      ASSERT_EQ(std::memcmp(ptr, pattern.data(), bcache_->BlockSize()), 0);
    }
  }
}

std::set<uint64_t> GetRandomValuesWithoutDuplicates(uint64_t max, uint32_t count) {
  std::set<uint64_t> random_numbers;
  while (random_numbers.size() != count) {
    uint64_t random_number =
        static_cast<uint64_t>((static_cast<uint64_t>(rand()) << 32 | rand())) % max;

    random_numbers.insert(random_number);
  }
  return random_numbers;
}

TEST_P(BcacheMapperTest, RunRequestsRandom) {
  constexpr uint32_t kBatchSize = 3;
  srand(testing::UnitTest::GetInstance()->random_seed());

  auto random_offsets = GetRandomValuesWithoutDuplicates(bcache_->Maxblk(), kBatchSize * 2);

  char random_pattern[kBatchSize];
  {
    std::vector<storage::BufferedOperation> operations;
    storage::VmoBuffer buffer[kBatchSize];

    uint32_t iteration = 0;
    uint64_t prev = 0;
    for (auto offset : random_offsets) {
      if (iteration % 2 == 0) {
        prev = offset;
      } else {
        uint64_t length = offset - prev;
        ASSERT_EQ(
            buffer[iteration / 2].Initialize(bcache_.get(), length, bcache_->BlockSize(), "test"),
            ZX_OK);
        random_pattern[iteration / 2] = static_cast<char>(rand() % 128);
        for (size_t i = 0; i < length; ++i) {
          std::memset(buffer[iteration / 2].Data(i), random_pattern[iteration / 2],
                      bcache_->BlockSize());
        }
        operations.push_back(storage::BufferedOperation{.vmoid = buffer[iteration / 2].vmoid(),
                                                        .op = storage::Operation{
                                                            .type = storage::OperationType::kWrite,
                                                            .vmo_offset = 0,
                                                            .dev_offset = prev,
                                                            .length = length,
                                                        }});
      }
      ++iteration;
    }
    ASSERT_EQ(bcache_->RunRequests(operations), ZX_OK);
  }

  // Read Verify
  {
    std::vector<storage::BufferedOperation> operations;
    storage::VmoBuffer buffer[kBatchSize];

    uint32_t iteration = 0;
    uint64_t prev = 0;
    for (auto offset : random_offsets) {
      if (iteration % 2 == 0) {
        prev = offset;
      } else {
        uint64_t length = offset - prev;
        ASSERT_EQ(
            buffer[iteration / 2].Initialize(bcache_.get(), length, bcache_->BlockSize(), "test"),
            ZX_OK);
        operations.push_back(storage::BufferedOperation{.vmoid = buffer[iteration / 2].vmoid(),
                                                        .op = storage::Operation{
                                                            .type = storage::OperationType::kRead,
                                                            .vmo_offset = 0,
                                                            .dev_offset = prev,
                                                            .length = length,
                                                        }});
      }
      ++iteration;
    }
    ASSERT_EQ(bcache_->RunRequests(operations), ZX_OK);

    for (uint32_t i = 0; i < kBatchSize; ++i) {
      for (uint32_t block = 0; block < buffer[i].capacity(); ++block) {
        char *data = static_cast<char *>(buffer[i].Data(block));
        for (uint32_t byte = 0; byte < buffer[i].BlockSize(); ++byte) {
          ASSERT_EQ(data[byte], random_pattern[i]);
        }
      }
    }
  }
}

class F2fsMountedBcacheMapperTest : public BcacheMapperTest {
 public:
  F2fsMountedBcacheMapperTest() : BcacheMapperTest() {}
  void MkfsOnMultiFakeDevWithOptions(std::unique_ptr<BcacheMapper> *bc, const MkfsOptions &options,
                                     std::vector<uint64_t> block_counts,
                                     std::vector<uint32_t> block_sizes, bool btrim) {
    ASSERT_EQ(block_counts.size(), block_sizes.size());
    std::vector<std::unique_ptr<block_client::BlockDevice>> devices;
    for (size_t i = 0; i < block_counts.size(); ++i) {
      devices.push_back(std::make_unique<FakeBlockDevice>(FakeBlockDevice::Config{
          .block_count = block_counts[i], .block_size = block_sizes[i], .supports_trim = btrim}));
    }

    auto bc_or = CreateBcacheMapper(std::move(devices), true);
    ASSERT_TRUE(bc_or.is_ok());

    MkfsWorker mkfs(std::move(*bc_or), options);
    auto ret = mkfs.DoMkfs();
    ASSERT_EQ(ret.is_error(), false);
    *bc = std::move(*ret);
  }

  void SetUp() override {
    BcacheMapperTest::SetUp();
    MkfsOnMultiFakeDevWithOptions(&bc_, MkfsOptions{}, block_counts_, block_sizes_, true);
    FileTester::MountWithOptions(loop_.dispatcher(), MountOptions{}, &bc_, &fs_);

    fbl::RefPtr<VnodeF2fs> root;
    FileTester::CreateRoot(fs_.get(), &root);

    root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
  }

  void TearDown() override {
    ASSERT_EQ(root_dir_->Close(), ZX_OK);
    root_dir_ = nullptr;
    FileTester::Unmount(std::move(fs_), &bc_);

    FsckWorker fsck(std::move(bc_), FsckOptions{.repair = false});
    ASSERT_EQ(fsck.Run(), ZX_OK);
  }

 protected:
  std::unique_ptr<f2fs::BcacheMapper> bc_;
  std::unique_ptr<F2fs> fs_;
  fbl::RefPtr<Dir> root_dir_;
  async::Loop loop_ = async::Loop(&kAsyncLoopConfigAttachToCurrentThread);
};

TEST_P(F2fsMountedBcacheMapperTest, ReadWrite) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<VnodeF2fs> test_file_vn = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file));
  File *test_file_ptr = static_cast<File *>(test_file_vn.get());

  std::array<size_t, 5> num_pages = {1, 2, 4};
  size_t total_pages = 0;
  for (auto i : num_pages) {
    total_pages += i;
  }
  const size_t data_size = kBlockSize * total_pages;
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

  // Read verify again after clearing file cache
  char r_buf[kBlockSize];
  {
    test_file_ptr->Writeback(true, true);
    test_file_ptr->ResetFileCache();
  }
  w_buf_iter = w_buf.get();
  for (size_t i = 0; i < total_pages; ++i) {
    FileTester::ReadFromFile(test_file_ptr, r_buf, kBlockSize, i * kBlockSize);
    ASSERT_EQ(memcmp(r_buf, w_buf_iter, kBlockSize), 0);
    w_buf_iter += kBlockSize;
  }

  ASSERT_EQ(test_file_vn->Close(), ZX_OK);
  test_file_vn = nullptr;
}

const std::array<std::pair<std::vector<uint64_t>, std::vector<uint32_t>>, 3> kBlockCountsAndSizes =
    {{{{65536, 65536, 65536, 65536},
       {kDefaultSectorSize, kDefaultSectorSize, kDefaultSectorSize, kDefaultSectorSize}},
      {{10, 10, 10, 200000},
       {kDefaultSectorSize, kDefaultSectorSize, kDefaultSectorSize, kDefaultSectorSize}},
      {{65535, 32768, 65535, 32768},
       {kDefaultSectorSize, kDefaultSectorSize * 2, kDefaultSectorSize, kDefaultSectorSize * 2}}}};

INSTANTIATE_TEST_SUITE_P(BcacheMapperTest, BcacheMapperTest,
                         ::testing::ValuesIn(kBlockCountsAndSizes));

INSTANTIATE_TEST_SUITE_P(F2fsMountedBcacheMapperTest, F2fsMountedBcacheMapperTest,
                         ::testing::ValuesIn(kBlockCountsAndSizes));

}  // namespace
}  // namespace f2fs
