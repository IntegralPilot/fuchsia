// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "block.h"

#include <fidl/fuchsia.storage.block/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/fake-bti/bti.h>
#include <lib/sync/completion.h>
#include <lib/virtio/backends/fake.h>

#include <condition_variable>
#include <cstdint>
#include <memory>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace {

constexpr uint64_t kCapacity = 200;
constexpr uint64_t kSizeMax = 4000;
constexpr uint64_t kSegMax = 1024;
constexpr uint64_t kBlkSize = 1024;
constexpr uint64_t kVmoOffsetBlocks = 1;
const uint16_t kRingSize = 128;  // Should match block.h

// Fake virtio 'backend' for a virtio-block device.
class FakeBackendForBlock : public virtio::FakeBackend {
 public:
  FakeBackendForBlock(zx_handle_t fake_bti)
      : virtio::FakeBackend({{0, 1024}}), fake_bti_(fake_bti) {
    // Fill out a block config:
    virtio_blk_config config;
    memset(&config, 0, sizeof(config));
    config.capacity = kCapacity;
    config.size_max = kSizeMax;
    config.seg_max = kSegMax;
    config.blk_size = kBlkSize;

    for (uint16_t i = 0; i < sizeof(config); ++i) {
      AddClassRegister(i, reinterpret_cast<uint8_t*>(&config)[i]);
    }
  }

  void set_status(uint8_t status) { status_ = status; }

  uint64_t ReadFeatures() override {
    uint64_t bitmap = FakeBackend::ReadFeatures();

    // Declare support for VIRTIO_F_VERSION_1.
    bitmap |= VIRTIO_F_VERSION_1;

    return bitmap;
  }

  void RingKick(uint16_t ring_index) override {
    FakeBackend::RingKick(ring_index);

    fake_bti_pinned_vmo_info_t vmos[16];
    size_t count;
    ASSERT_OK(fake_bti_get_pinned_vmos(fake_bti_, vmos, 16, &count));
    ASSERT_LE(size_t{2}, count);

    union __PACKED Used {
      vring_used head;
      struct __PACKED {
        uint8_t header[sizeof(vring_used)];
        vring_used_elem elements[kRingSize];
      };
    } used;
    union __PACKED Avail {
      vring_avail head;
      struct __PACKED {
        uint8_t header[sizeof(vring_avail)];
        uint16_t ring[kRingSize];
      };
    } avail;

    // This assumes that the ring is in the first VMO.
    ASSERT_OK(zx_vmo_read(vmos[0].vmo, &used, vmos[0].offset + used_offset_, sizeof(used)));
    ASSERT_OK(zx_vmo_read(vmos[0].vmo, &avail, vmos[0].offset + avail_offset_, sizeof(avail)));

    if (avail.head.idx != used.head.idx) {
      ASSERT_EQ(avail.head.idx, used.head.idx + 1);  // We can only handle one queued entry.

      size_t index = used.head.idx & (kRingSize - 1);

      // Read the descriptors.
      vring_desc descriptors[kRingSize];
      ASSERT_OK(zx_vmo_read(vmos[0].vmo, descriptors, vmos[0].offset + desc_offset_,
                            sizeof(descriptors)));

      // Find the last descriptor.
      vring_desc* desc = &descriptors[avail.ring[index]];
      uint16_t count = 1;
      uint16_t data_descriptor_idx = UINT16_MAX;
      while (desc->flags & VRING_DESC_F_NEXT) {
        if (desc->addr % zx_system_get_page_size() == kBlkSize * kVmoOffsetBlocks) {
          last_data_offset_ = desc->addr - FAKE_BTI_PHYS_ADDR;
          data_descriptor_idx = count;
        }
        desc = &descriptors[desc->next];
        ++count;
      }
      // The second-last descriptor describes the first page of data transfer (the first descriptor
      // is the head descriptor).
      if (data_descriptor_idx != UINT16_MAX) {
        ZX_ASSERT_MSG(data_descriptor_idx == count - 1,
                      "The second-last descriptor should point to data");
      }

      // It should be the status descriptor.
      ASSERT_EQ(uint32_t{1}, desc->len);

      // This assumes the results are in the second VMO.
      size_t offset = vmos[1].offset + desc->addr - FAKE_BTI_PHYS_ADDR;
      ASSERT_OK(zx_vmo_write(vmos[1].vmo, &status_, offset, sizeof(status_)));

      used.elements[index].id = avail.ring[index];
      used.elements[index].len = count;

      ++used.head.idx;

      ASSERT_OK(zx_vmo_write(vmos[0].vmo, &used, vmos[0].offset + used_offset_, sizeof(used)));

      // Trigger an interrupt.
      uint8_t isr_status;
      ReadRegister(kISRStatus, &isr_status);
      isr_status |= VIRTIO_ISR_QUEUE_INT;
      SetRegister(kISRStatus, isr_status);

      std::scoped_lock lock(mutex_);
      interrupt_ = true;
      cond_.notify_all();
    }
  }

  zx_status_t SetRing(uint16_t index, uint16_t count, zx_paddr_t pa_desc, zx_paddr_t pa_avail,
                      zx_paddr_t pa_used) override {
    FakeBackend::SetRing(index, count, pa_desc, pa_avail, pa_used);
    used_offset_ = pa_used - FAKE_BTI_PHYS_ADDR;
    avail_offset_ = pa_avail - FAKE_BTI_PHYS_ADDR;
    desc_offset_ = pa_desc - FAKE_BTI_PHYS_ADDR;
    ZX_ASSERT(count == kRingSize);
    return ZX_OK;
  }

  zx_status_t InterruptValid() override {
    std::scoped_lock lock(mutex_);
    return terminate_ ? ZX_ERR_CANCELED : ZX_OK;
  }

  zx::result<uint32_t> WaitForInterrupt() override {
    std::unique_lock<std::mutex> lock(mutex_);
    for (;;) {
      if (terminate_)
        return zx::error(ZX_ERR_CANCELED);
      if (interrupt_)
        return zx::ok(0);
      cond_.wait(lock);
    }
  }

  void InterruptAck(uint32_t key) override {
    std::scoped_lock lock(mutex_);
    interrupt_ = false;
  }

  void Terminate() override {
    std::scoped_lock lock(mutex_);
    terminate_ = true;
    cond_.notify_all();
  }

  // Used to peek at the data offset of the last submitted request.
  static std::atomic<uint64_t> last_data_offset_;

 private:
  // The vring offsets.
  size_t used_offset_ = 0;
  size_t avail_offset_ = 0;
  size_t desc_offset_ = 0;

  zx_handle_t fake_bti_;

  std::mutex mutex_;
  std::condition_variable cond_;
  bool terminate_ = false;
  bool interrupt_ = false;

  // The status returned for any operations.
  uint8_t status_ = VIRTIO_BLK_S_OK;
};

class TestBlockDriver : public virtio::BlockDriver {
 public:
  // Modify to configure the behaviour of this test driver.
  static uint8_t backend_status_;

  explicit TestBlockDriver() : BlockDriver() {}

  virtio::BlockDevice& block_device() const { return virtio::BlockDriver::block_device(); }

 protected:
  zx::result<std::unique_ptr<virtio::BlockDevice>> CreateBlockDevice(
      const fdf::Namespace& incoming) override {
    zx::bti bti(ZX_HANDLE_INVALID);
    zx_status_t status = fake_bti_create(bti.reset_and_get_address());
    if (status != ZX_OK) {
      return zx::error(status);
    }
    auto backend = std::make_unique<FakeBackendForBlock>(bti.get());
    backend->set_status(backend_status_);

    return zx::ok(
        std::make_unique<virtio::BlockDevice>(std::move(bti), std::move(backend), logger()));
  }
};

uint8_t TestBlockDriver::backend_status_;
std::atomic<uint64_t> FakeBackendForBlock::last_data_offset_ = 0;

class TestConfig final {
 public:
  using DriverType = TestBlockDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

// Provides control primitives for tests that issue IO requests to the device.
class BlockDriverTest : public ::testing::Test {
 public:
  void StartDriver(uint8_t status = VIRTIO_BLK_S_OK) {
    zx::event token;
    ASSERT_OK(zx::event::create(0, &token));
    StartDriverWithNodeToken(std::move(token), status);
  }
  void StartDriverWithNodeToken(zx::event node_token, uint8_t status = VIRTIO_BLK_S_OK) {
    TestBlockDriver::backend_status_ = status;
    zx::result<> result = driver_test().StartDriverWithCustomStartArgs(
        [&](fdf::DriverStartArgs& args) { args.node_token(std::move(node_token)); });
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  zx::result<std::unique_ptr<block_client::RemoteBlockDevice>> CreateClient() {
    auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
    driver_test().RunInDriverContext([&](TestBlockDriver& driver) {
      driver.block_device().ServeRequests(std::move(volume_server));
    });
    return block_client::RemoteBlockDevice::Create(std::move(volume_client));
  }

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
};

TEST_F(BlockDriverTest, QueueOne) {
  StartDriver();

  zx::result client = CreateClient();
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client.value()->BlockGetInfo(&info));
  const size_t kLen = info.max_transfer_size + kBlkSize;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kLen, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client.value()->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest request = {.command = {.opcode = BLOCK_OPCODE_READ},
                              .vmoid = vmoid,
                              .length = 0,
                              .vmo_offset = kVmoOffsetBlocks,
                              .dev_offset = 0};
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, client.value()->FifoTransaction(&request, 1));

  request.length = kCapacity + 1;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, client.value()->FifoTransaction(&request, 1));
}

TEST_F(BlockDriverTest, CheckQuery) {
  StartDriver();

  zx::result client = CreateClient();
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client.value()->BlockGetInfo(&info));
  ASSERT_EQ(info.block_size, kBlkSize);
  ASSERT_EQ(info.block_count, kCapacity);
  ASSERT_GE(info.max_transfer_size, zx_system_get_page_size());
}

TEST_F(BlockDriverTest, ReadOk) {
  StartDriver();

  zx::result client = CreateClient();
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client.value()->BlockGetInfo(&info));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client.value()->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest request = {.command = {.opcode = BLOCK_OPCODE_READ},
                              .vmoid = vmoid,
                              .length = 1,
                              .vmo_offset = kVmoOffsetBlocks,
                              .dev_offset = 0};

  EXPECT_OK(client.value()->FifoTransaction(&request, 1));
}

TEST_F(BlockDriverTest, ReadError) {
  StartDriver(VIRTIO_BLK_S_IOERR);

  zx::result client = CreateClient();
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client.value()->BlockGetInfo(&info));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client.value()->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest request = {.command = {.opcode = BLOCK_OPCODE_READ},
                              .vmoid = vmoid,
                              .length = 1,
                              .vmo_offset = kVmoOffsetBlocks,
                              .dev_offset = 0};

  ASSERT_EQ(ZX_ERR_IO, client.value()->FifoTransaction(&request, 1));
}

TEST_F(BlockDriverTest, Trim) {
  StartDriver();

  zx::result client = CreateClient();
  ASSERT_OK(client);

  BlockFifoRequest request = {.command = {.opcode = BLOCK_OPCODE_TRIM},
                              .vmoid = 0,
                              .length = 1,
                              .vmo_offset = 0,
                              .dev_offset = 0};

  EXPECT_OK(client.value()->FifoTransaction(&request, 1));
}

TEST_F(BlockDriverTest, BlockServer) {
  StartDriver();

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&](TestBlockDriver& driver) {
    driver.block_device().ServeRequests(std::move(volume_server));
  });
  zx::result client = block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  const size_t kLen = info.max_transfer_size + kBlkSize;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kLen, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  // It doesn't matter if we leak the ID.
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {.command =
           {
               .opcode = BLOCK_OPCODE_WRITE,
           },
       .vmoid = vmoid,
       .length = 3,
       .vmo_offset = 0,
       .dev_offset = 0},
      {.command =
           {
               .opcode = BLOCK_OPCODE_READ,
           },
       .vmoid = vmoid,
       .length = 3,
       .vmo_offset = 10,
       .dev_offset = 100},
      {.command =
           {
               .opcode = BLOCK_OPCODE_TRIM,
           },
       .vmoid = 0,
       .length = 3,
       .vmo_offset = 0,
       .dev_offset = 3},
  };

  EXPECT_OK(client->FifoTransaction(requests, 3));

  BlockFifoRequest big_request = {.command =
                                      {
                                          .opcode = BLOCK_OPCODE_WRITE,
                                      },
                                  .vmoid = vmoid,
                                  .length = static_cast<uint32_t>(kCapacity),
                                  .vmo_offset = 0,
                                  .dev_offset = 0};

  EXPECT_OK(client->FifoTransaction(&big_request, 1));
}

TEST_F(BlockDriverTest, BarriersOk) {
  StartDriver();

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&](TestBlockDriver& driver) {
    driver.block_device().ServeRequests(std::move(volume_server));
  });
  zx::result client = block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  const size_t kLen = info.max_transfer_size + kBlkSize;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kLen, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  // It doesn't matter if we leak the ID.
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {.command =
           {
               .opcode = BLOCK_OPCODE_WRITE,
               .flags = BLOCK_IO_FLAG_PRE_BARRIER,
           },
       .vmoid = vmoid,
       .length = 3,
       .vmo_offset = 0,
       .dev_offset = 0},
  };

  EXPECT_OK(client->FifoTransaction(requests, 1));
}

TEST_F(BlockDriverTest, BarriersError) {
  StartDriver(VIRTIO_BLK_S_IOERR);

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&](TestBlockDriver& driver) {
    driver.block_device().ServeRequests(std::move(volume_server));
  });
  zx::result client = block_client::RemoteBlockDevice::Create(std::move(volume_client));
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  const size_t kLen = info.max_transfer_size + kBlkSize;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kLen, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  // It doesn't matter if we leak the ID.
  vmoid_t vmoid = owned_vmoid.TakeId();

  BlockFifoRequest requests[] = {
      {.command =
           {
               .opcode = BLOCK_OPCODE_WRITE,
               .flags = BLOCK_IO_FLAG_PRE_BARRIER,
           },
       .vmoid = vmoid,
       .length = 3,
       .vmo_offset = 0,
       .dev_offset = 0},
  };

  ASSERT_EQ(ZX_ERR_IO, client->FifoTransaction(requests, 1));
}

TEST_F(BlockDriverTest, NodeToken) {
  zx::event token;
  ASSERT_OK(zx::event::create(0, &token));
  zx::event token_copy;
  ASSERT_OK(token.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

  StartDriverWithNodeToken(std::move(token));

  zx::result connect_result =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Token>();
  ASSERT_OK(connect_result);

  fidl::SyncClient<fuchsia_driver_token::NodeToken> client(std::move(connect_result.value()));

  auto get_result = client->Get();

  ASSERT_OK(get_result);

  zx_info_handle_basic_t info1, info2;
  ASSERT_EQ(token_copy.get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr, nullptr),
            ZX_OK);
  ASSERT_EQ(
      get_result->token().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);
  ASSERT_EQ(info1.koid, info2.koid);
}

TEST_F(BlockDriverTest, UnalignedVmoOffset) {
  StartDriver();

  zx::result client = CreateClient();
  ASSERT_OK(client);

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client.value()->BlockGetInfo(&info));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(static_cast<uint64_t>(zx_system_get_page_size()) * 2, 0, &vmo));

  storage::Vmoid owned_vmoid;
  EXPECT_OK(client.value()->BlockAttachVmo(vmo, &owned_vmoid));
  vmoid_t vmoid = owned_vmoid.TakeId();

  // Use an offset of 1 block. With kBlkSize=1024, this translates to 1024 byte offset.
  BlockFifoRequest request = {.command = {.opcode = BLOCK_OPCODE_READ},
                              .vmoid = vmoid,
                              .length = 1,
                              .vmo_offset = 1,
                              .dev_offset = 0};

  FakeBackendForBlock::last_data_offset_ = 0xCAFE;

  EXPECT_OK(client.value()->FifoTransaction(&request, 1));

  EXPECT_EQ(FakeBackendForBlock::last_data_offset_, 1024ul);
}

TEST_F(BlockDriverTest, MapperService) {
  StartDriver();

  zx::result connect_result =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Mapper>();
  ASSERT_OK(connect_result);

  fidl::SyncClient<fuchsia_storage_block::Mapper> client(std::move(connect_result.value()));
  auto [session_client, session_server] =
      fidl::Endpoints<fuchsia_storage_block::MapperSession>::Create();

  zx::vmo mapping_vmo;
  ASSERT_OK(zx::vmo::create(512 * 1024, 0, &mapping_vmo));

  fuchsia_storage_block::MapperOpenSessionRequest request;
  request.session(std::move(session_server));
  request.mapping_vmo(std::move(mapping_vmo));
  auto open_result = client->OpenSession(std::move(request));
  ASSERT_OK(open_result);
}

TEST_F(BlockDriverTest, ConnectMapper) {
  StartDriver();

  auto [volume_client, volume_server] = fidl::Endpoints<fuchsia_storage_block::Block>::Create();
  driver_test().RunInDriverContext([&](TestBlockDriver& driver) {
    driver.block_device().ServeRequests(std::move(volume_server));
  });

  fidl::SyncClient<fuchsia_storage_block::Block> block_client(std::move(volume_client));
  auto [mapper_client, mapper_server] = fidl::Endpoints<fuchsia_storage_block::Mapper>::Create();
  auto connect_result = block_client->ConnectMapper(std::move(mapper_server));
  ASSERT_OK(connect_result);

  auto [session_client, session_server] =
      fidl::Endpoints<fuchsia_storage_block::MapperSession>::Create();

  zx::vmo mapping_vmo;
  ASSERT_OK(zx::vmo::create(512 * 1024, 0, &mapping_vmo));

  fidl::SyncClient<fuchsia_storage_block::Mapper> sync_mapper_client(std::move(mapper_client));
  fuchsia_storage_block::MapperOpenSessionRequest request;
  request.session(std::move(session_server));
  request.mapping_vmo(std::move(mapping_vmo));
  auto open_result = sync_mapper_client->OpenSession(std::move(request));
  ASSERT_OK(open_result);
}

FUCHSIA_DRIVER_EXPORT2(TestBlockDriver);

}  // anonymous namespace
