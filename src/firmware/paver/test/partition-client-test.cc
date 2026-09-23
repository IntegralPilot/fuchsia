// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/partition-client.h"

#include <lib/component/incoming/cpp/clone.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fdio.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/hw/gpt.h>

#include <iostream>
#include <memory>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/firmware/paver/test/test-utils.h"

namespace {

using device_watcher::RecursiveWaitForFile;
using driver_integration_test::IsolatedDevmgr;

class FakePartitionClient final : public paver::PartitionClient {
 public:
  explicit FakePartitionClient(size_t block_size, size_t partition_size)
      : block_size_(block_size), partition_size_(partition_size) {}

  zx::result<size_t> GetBlockSize() final {
    if (result_ == ZX_OK) {
      return zx::ok(block_size_);
    }
    return zx::error(result_);
  }
  zx::result<size_t> GetPartitionSize() final {
    if (result_ == ZX_OK) {
      return zx::ok(partition_size_);
    }
    return zx::error(result_);
  }
  zx::result<> Read(const zx::vmo& vmo, size_t size) final {
    read_called_ = true;
    if (size > partition_size_) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::make_result(result_);
  }
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final {
    write_called_ = true;
    if (vmo_size > partition_size_) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::make_result(result_);
  }
  zx::result<> Trim() final {
    trim_called_ = true;
    return zx::make_result(result_);
  }
  zx::result<> Flush() final {
    flush_called_ = true;
    return zx::make_result(result_);
  }

  bool read_called() { return read_called_; }
  bool write_called() { return write_called_; }
  bool trim_called() { return trim_called_; }
  bool flush_called() { return flush_called_; }

  void set_result(zx_status_t result) { result_ = result; }

 private:
  size_t block_size_;
  size_t partition_size_;

  bool read_called_ = false;
  bool write_called_ = false;
  bool trim_called_ = false;
  bool flush_called_ = false;
  zx_status_t result_ = ZX_OK;
};

TEST(PartitionCopyClientTest, ConstructEmpty) { paver::PartitionCopyClient({}); }

TEST(PartitionCopyClientTest, ConstructSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));
}

TEST(PartitionCopyClientTest, GetBlockSizeSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  auto status = client.GetBlockSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 10);

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.GetBlockSize());
}

TEST(PartitionCopyClientTest, GetPartitionSizeSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  auto status = client.GetPartitionSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 100);

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.GetPartitionSize());
}

TEST(PartitionCopyClientTest, ReadSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Read(vmo, 0));
  ASSERT_TRUE(fake_ref->read_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Read(vmo, 0));
}

TEST(PartitionCopyClientTest, WriteSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Write(vmo, 0));
  ASSERT_TRUE(fake_ref->write_called());
  ASSERT_FALSE(fake_ref->trim_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Write(vmo, 0));
  ASSERT_TRUE(fake_ref->trim_called());
}

TEST(PartitionCopyClientTest, TrimSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Trim());
  ASSERT_TRUE(fake_ref->trim_called());

  fake_ref->set_result(ZX_ERR_NOT_SUPPORTED);
  ASSERT_NOT_OK(client.Trim());
}

TEST(PartitionCopyClientTest, FlushSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake_ref = fake.get();
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Flush());
  ASSERT_TRUE(fake_ref->flush_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Flush());
}

TEST(PartitionCopyClientTest, GetChannelSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));
}

TEST(PartitionCopyClientTest, BlockFdSinglePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  partitions.push_back(std::move(fake));
  paver::PartitionCopyClient client(std::move(partitions));
}

TEST(PartitionCopyClientTest, ConstructMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));
}

TEST(PartitionCopyClientTest, GetBlockSizeMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  auto status = client.GetBlockSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 70);

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  status = client.GetBlockSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 7);

  fake_ref2->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.GetBlockSize());
}

TEST(PartitionCopyClientTest, GetPartitionSizeMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  auto status = client.GetPartitionSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 90);

  fake_ref2->set_result(ZX_ERR_ACCESS_DENIED);
  status = client.GetPartitionSize();
  ASSERT_OK(status);
  ASSERT_EQ(status.value(), 100);

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.GetPartitionSize());
}

TEST(PartitionCopyClientTest, ReadMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Read(vmo, 0));
  ASSERT_TRUE(fake_ref->read_called());
  ASSERT_FALSE(fake_ref2->read_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_OK(client.Read(vmo, 0));
  ASSERT_TRUE(fake_ref2->read_called());

  fake_ref2->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Read(vmo, 0));
}

TEST(PartitionCopyClientTest, WriteMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Write(vmo, 0));
  ASSERT_TRUE(fake_ref->write_called());
  ASSERT_TRUE(fake_ref2->write_called());
  ASSERT_FALSE(fake_ref->trim_called());
  ASSERT_FALSE(fake_ref->trim_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_OK(client.Write(vmo, 0));
  ASSERT_TRUE(fake_ref->trim_called());
  ASSERT_FALSE(fake_ref2->trim_called());

  fake_ref2->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Write(vmo, 0));
}

TEST(PartitionCopyClientTest, TrimMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Trim());
  ASSERT_TRUE(fake_ref->trim_called());
  ASSERT_TRUE(fake_ref2->trim_called());

  fake_ref->set_result(ZX_ERR_NOT_SUPPORTED);
  ASSERT_NOT_OK(client.Trim());
}

TEST(PartitionCopyClientTest, FlushMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  auto fake_ref = fake.get();
  auto fake_ref2 = fake2.get();
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));

  zx::vmo vmo;
  ASSERT_OK(client.Flush());
  ASSERT_TRUE(fake_ref->flush_called());
  ASSERT_TRUE(fake_ref2->flush_called());

  fake_ref->set_result(ZX_ERR_ACCESS_DENIED);
  ASSERT_NOT_OK(client.Flush());
}

TEST(PartitionCopyClientTest, GetChannelMultiplePartitions) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));
}

TEST(PartitionCopyClientTest, BlockFdMultilplePartition) {
  std::vector<std::unique_ptr<paver::PartitionClient>> partitions;
  auto fake = std::make_unique<FakePartitionClient>(10, 100);
  auto fake2 = std::make_unique<FakePartitionClient>(7, 90);
  partitions.push_back(std::move(fake));
  partitions.push_back(std::move(fake2));
  paver::PartitionCopyClient client(std::move(partitions));
}

class FixedOffsetBlockPartitionClientTest : public PaverTest {
 public:
  void SetUp() override {
    PaverTest::SetUp();
    IsolatedDevmgr::Args args;
    args.disable_block_watcher = true;

    ASSERT_OK(IsolatedDevmgr::Create(&args, &devmgr_));

    constexpr uint8_t kEmptyType[GPT_GUID_LEN] = GUID_EMPTY_VALUE;
    fbl::unique_fd svc_root;
    ASSERT_OK(fdio_fd_create(devmgr_.RealmExposedDir().TakeHandle().release(),
                             svc_root.reset_and_get_address()));
    ASSERT_NO_FATAL_FAILURE(BlockDevice::Create(&gpt_dev_, svc_root, kEmptyType, 2, 512));
  }

  // Creates a BlockPartitionClient which will read/write the entire device.
  zx::result<std::unique_ptr<paver::BlockPartitionClient>> RawClient() {
    return paver::BlockPartitionClient::Create(gpt_dev_->GetConnector());
  }

  // Creates a FixedOffsetBlockPartitionClient which will read/write with a partition
  // and buffer offset
  zx::result<std::unique_ptr<paver::FixedOffsetBlockPartitionClient>> FixedOffsetClient(
      size_t partition_offset, size_t buffer_offset) {
    return paver::FixedOffsetBlockPartitionClient::Create(gpt_dev_->GetConnector(),
                                                          partition_offset, buffer_offset);
  }

  fidl::ClientEnd<fuchsia_io::Directory> GetSvcRoot() { return devmgr_.RealmExposedDir(); }

 private:
  IsolatedDevmgr devmgr_;
  std::unique_ptr<BlockDevice> gpt_dev_;
};

// Writes |data| to |client|.
// Call with ASSERT_NO_FATAL_FAILURE().
void Write(std::unique_ptr<paver::PartitionClient> client, std::string_view data) {
  // Write data to a VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(data.size(), 0, &vmo));
  ASSERT_OK(vmo.write(data.data(), 0, data.size()));

  // Write VMO to the client.
  ASSERT_OK(client->Write(vmo, data.size()));
}

// Reads |size| bytes from |client| into |data|.
// Call with ASSERT_NO_FATAL_FAILURE().
void Read(std::unique_ptr<paver::PartitionClient> client, std::string* data, size_t size) {
  if (data->size() < size) {
    data->resize(size);
  }

  // Read client to a VMO.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(data->size(), 0, &vmo));
  ASSERT_OK(client->Read(vmo, data->size()));

  // Read VMO to data.
  ASSERT_OK(vmo.read(data->data(), 0, data->size()));
}

TEST_F(FixedOffsetBlockPartitionClientTest, GetPartitionSize) {
  {
    auto status = RawClient()->GetPartitionSize();
    ASSERT_OK(status);
    ASSERT_EQ(1024, status.value());
  }

  {
    // GetPartitionSize size should not count block 0.
    auto status = FixedOffsetClient(1, 0)->GetPartitionSize();
    ASSERT_OK(status);
    ASSERT_EQ(512, status.value());
  }
}

TEST_F(FixedOffsetBlockPartitionClientTest, ReadOffsetedPartition) {
  const std::string block0(512, '0');
  const std::string firmware(512, 'F');

  zx::result raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*raw_client), block0 + firmware));

  // Bootloader read should skip block 0.
  zx::result fixed_client = FixedOffsetClient(1, 0);
  ASSERT_OK(fixed_client);
  std::string actual;
  ASSERT_NO_FATAL_FAILURE(Read(std::move(*fixed_client), &actual, 512));
  ASSERT_EQ(firmware, actual);
}

TEST_F(FixedOffsetBlockPartitionClientTest, WriteOffsetdPartition) {
  const std::string block0(512, '0');
  const std::string firmware(512, 'F');

  zx::result raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*raw_client), block0 + block0));
  zx::result fixed_client = FixedOffsetClient(1, 0);
  ASSERT_OK(fixed_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*fixed_client), firmware));

  // Bootloader write should have skipped block 0.
  std::string actual;
  raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Read(std::move(*raw_client), &actual, 1024));
  ASSERT_EQ(block0 + firmware, actual);
}

TEST_F(FixedOffsetBlockPartitionClientTest, ReadPartitionOffsetedBuffer) {
  size_t block_size = 512;
  const std::string initial(block_size, '0');

  zx::result raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*raw_client), initial));

  // Bootloader should read to |actual| from offset 512
  std::string actual(2 * block_size, 'F');
  zx::result fixed_client = FixedOffsetClient(0, 1);
  ASSERT_OK(fixed_client);
  ASSERT_NO_FATAL_FAILURE(Read(std::move(*fixed_client), &actual, block_size));
  ASSERT_EQ(initial, actual.substr(block_size, block_size));
}

TEST_F(FixedOffsetBlockPartitionClientTest, WritePartitionOffsetedBuffer) {
  size_t block_size = 512;
  const std::string initial(block_size, '0');
  const std::string firmware = std::string(block_size, 'A') + std::string(block_size, 'B');

  zx::result raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*raw_client), initial));
  zx::result fixed_client = FixedOffsetClient(0, 1);
  ASSERT_OK(fixed_client);
  ASSERT_NO_FATAL_FAILURE(Write(std::move(*fixed_client), firmware));

  // Bootloader should write only 'B' to storage
  std::string actual;
  raw_client = RawClient();
  ASSERT_OK(raw_client);
  ASSERT_NO_FATAL_FAILURE(Read(std::move(*raw_client), &actual, block_size));
  ASSERT_EQ(firmware.substr(block_size), actual);
}

}  // namespace
