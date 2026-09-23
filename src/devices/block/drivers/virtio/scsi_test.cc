// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "scsi.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fake-bti/bti.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <lib/virtio/backends/fake.h>

#include <chrono>
#include <memory>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

using Queue = virtio::ScsiDevice::Queue;

namespace {

// Fake virtio 'backend' for a virtio-scsi device.
class FakeBackendForScsi : public virtio::FakeBackend {
 public:
  explicit FakeBackendForScsi(zx_handle_t fake_bti = ZX_HANDLE_INVALID,
                              bool* out_device_reset_called = nullptr)
      : virtio::FakeBackend(
            /*queue_sizes=*/{{Queue::CONTROL, 128}, {Queue::REQUEST, 128}, {Queue::EVENT, 128}}),
        fake_bti_(fake_bti),
        out_device_reset_called_(out_device_reset_called) {
    // TODO(venkateshs): Sane defaults for these registers.
    AddClassRegister(offsetof(virtio_scsi_config, num_queues), 1);
    AddClassRegister(offsetof(virtio_scsi_config, seg_max), 1);
    AddClassRegister(offsetof(virtio_scsi_config, max_sectors),
                     virtio::ScsiDevice::kMaxXferSectors);
    AddClassRegister(offsetof(virtio_scsi_config, cmd_per_lun), 1);
    AddClassRegister(offsetof(virtio_scsi_config, event_info_size), 1);
    AddClassRegister(offsetof(virtio_scsi_config, sense_size), 1);
    AddClassRegister(offsetof(virtio_scsi_config, cdb_size), 1);
    AddClassRegister(offsetof(virtio_scsi_config, max_channel), static_cast<uint16_t>(1));
    AddClassRegister(offsetof(virtio_scsi_config, max_target), static_cast<uint16_t>(1));
    AddClassRegister(offsetof(virtio_scsi_config, max_lun), 1);
  }

  uint64_t ReadFeatures() override {
    uint64_t bitmap = FakeBackend::ReadFeatures();
    bitmap |= VIRTIO_F_VERSION_1;
    return bitmap;
  }

  zx_status_t SetRing(uint16_t index, uint16_t count, zx_paddr_t pa_desc, zx_paddr_t pa_avail,
                      zx_paddr_t pa_used) override {
    FakeBackend::SetRing(index, count, pa_desc, pa_avail, pa_used);
    if (index == Queue::REQUEST) {
      used_offset_ = pa_used - FAKE_BTI_PHYS_ADDR;
      avail_offset_ = pa_avail - FAKE_BTI_PHYS_ADDR;
      desc_offset_ = pa_desc - FAKE_BTI_PHYS_ADDR;
    }
    return ZX_OK;
  }

  void RingKick(uint16_t ring_index) override {
    FakeBackend::RingKick(ring_index);
    if (fake_bti_ == ZX_HANDLE_INVALID || ring_index != Queue::REQUEST) {
      return;
    }

    fake_bti_pinned_vmo_info_t vmos[16];
    size_t count = 0;
    zx_status_t status = fake_bti_get_pinned_vmos(fake_bti_, vmos, 16, &count);
    if (status != ZX_OK || count < 1) {
      return;
    }

    union __PACKED Used {
      vring_used head;
      struct __PACKED {
        uint8_t header[sizeof(vring_used)];
        vring_used_elem elements[128];
      };
    } used;
    union __PACKED Avail {
      vring_avail head;
      struct __PACKED {
        uint8_t header[sizeof(vring_avail)];
        uint16_t ring[128];
      };
    } avail;

    if (zx_vmo_read(vmos[1].vmo, &used, vmos[1].offset + used_offset_, sizeof(used)) != ZX_OK) {
      return;
    }
    if (zx_vmo_read(vmos[1].vmo, &avail, vmos[1].offset + avail_offset_, sizeof(avail)) != ZX_OK) {
      return;
    }

    while (used.head.idx != avail.head.idx) {
      size_t index = used.head.idx & (128 - 1);
      used.elements[index].id = avail.ring[index];
      used.elements[index].len = 0;
      ++used.head.idx;
    }
    zx_vmo_write(vmos[1].vmo, &used, vmos[1].offset + used_offset_, sizeof(used));
  }

  void DeviceReset() override {
    device_reset_called_ = true;
    if (out_device_reset_called_) {
      *out_device_reset_called_ = true;
    }
    virtio::FakeBackend::DeviceReset();
  }
  bool device_reset_called() const { return device_reset_called_; }
  void clear_device_reset_called() { device_reset_called_ = false; }

 private:
  zx_handle_t fake_bti_ = ZX_HANDLE_INVALID;
  size_t desc_offset_ = 0;
  size_t avail_offset_ = 0;
  size_t used_offset_ = 0;
  bool device_reset_called_ = false;
  bool* out_device_reset_called_ = nullptr;
};

TEST(ScsiTest, Init) {
  // TODO: Need unit tests that actually instantiate the driver, at which point this logger can be
  // removed.
  fdf_testing::ScopedGlobalLogger logger;

  std::unique_ptr<virtio::Backend> backend = std::make_unique<FakeBackendForScsi>();
  zx::bti bti(ZX_HANDLE_INVALID);

  virtio::ScsiDevice scsi(/*parent=*/nullptr, std::move(bti), std::move(backend));
  auto status = scsi.Init();
  EXPECT_NE(status, ZX_OK);
}

TEST(ScsiTest, InitSuccess) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  virtio::ScsiDevice scsi(/*parent=*/nullptr, std::move(bti), std::move(backend));
  EXPECT_OK(scsi.Init());
}

TEST(ScsiTest, QueueCommandCallback) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  virtio::ScsiDevice scsi(/*parent=*/nullptr, std::move(bti), std::move(backend));
  ASSERT_OK(scsi.Init());

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};
  bool callback_invoked = false;
  zx_status_t callback_status = ZX_ERR_INTERNAL;
  scsi.QueueCommand(
      /*target=*/0, /*lun=*/0, cdb, /*is_write=*/false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) {
        callback_invoked = true;
        callback_status = status;
      },
      /*data=*/nullptr, /*vmar_mapped=*/false);

  EXPECT_FALSE(callback_invoked);
  scsi.IrqRingUpdate();
  EXPECT_TRUE(callback_invoked);
  EXPECT_OK(callback_status);
}

class TestController : public scsi::Controller {
 public:
  zx_status_t ExecuteCommandSync(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                                 iovec data) override {
    return ZX_OK;
  }
  void ExecuteCommandsAsync(uint8_t target, uint16_t lun,
                            std::span<scsi::ScsiRequest> batch) override {
    for (auto& req : batch) {
      reqs_.push_back(std::move(req));
    }
  }
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() override { return root_node_; }
  std::string_view driver_name() const override { return "test"; }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const override { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() override { return outgoing_; }
  const std::optional<std::string>& driver_node_name() const override { return node_name_; }
  fdf::Logger& driver_logger() override { return logger_; }

  zx::result<std::unique_ptr<scsi::BlockDevice>> BindBlockDevice(
      uint8_t target, uint16_t lun, uint32_t max_transfer_bytes,
      scsi::DeviceOptions device_options) override {
    return zx::ok(scsi::BlockDevice::CreateForTesting(this, target, lun, device_options, 1024,
                                                      zx_system_get_page_size()));
  }

  std::vector<scsi::ScsiRequest> reqs_;

 private:
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  std::shared_ptr<fdf::Namespace> incoming_;
  std::shared_ptr<fdf::OutgoingDirectory> outgoing_;
  std::optional<std::string> node_name_;
  fdf::Logger logger_{};
};

TEST(ScsiTest, QueueCommandScsiRequest) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  virtio::ScsiDevice scsi(/*parent=*/nullptr, std::move(bti), std::move(backend));
  ASSERT_OK(scsi.Init());

  TestController controller;
  zx::result block_dev = controller.BindBlockDevice(
      0, 0, fuchsia_storage_block::wire::kMaxTransferUnbounded, scsi::DeviceOptions::Default());
  ASSERT_OK(block_dev);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  block_server::Request request = {
      .request_id = 1,
      .operation =
          {
              .tag = block_server::Operation::Tag::Read,
              .read =
                  {
                      .device_block_offset = 0,
                      .block_count = 1,
                      ._unused = 0,
                      .vmo_offset = 0,
                      .options = {},
                  },
          },
      .trace_flow_id = 0,
      .vmo = vmo.borrow(),
  };

  block_dev.value()->OnRequests(std::span(&request, 1));
  ASSERT_EQ(controller.reqs_.size(), 1u);

  auto scsi_req = std::move(controller.reqs_[0]);
  iovec cdb = {
      .iov_base = const_cast<uint8_t*>(scsi_req.cdb().data()),
      .iov_len = scsi_req.cdb().size(),
  };

  zx_vaddr_t mapped_addr;
  ASSERT_OK(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                        scsi_req.data_vmo()->get(), scsi_req.vmo_offset(),
                        scsi_req.transfer_length_bytes(), &mapped_addr));

  bool callback_invoked = false;
  zx_status_t callback_status = ZX_ERR_INTERNAL;

  scsi.QueueCommand(
      /*target=*/0, /*lun=*/0, cdb, scsi_req.is_write(), scsi_req.data_vmo(), scsi_req.vmo_offset(),
      scsi_req.transfer_length_bytes(),
      [&callback_invoked, &callback_status, req = std::move(scsi_req)](zx_status_t status) mutable {
        callback_invoked = true;
        callback_status = status;
        req.Complete(status);
      },
      reinterpret_cast<void*>(mapped_addr), /*vmar_mapped=*/true);

  EXPECT_FALSE(callback_invoked);
  EXPECT_EQ(scsi.active_ios(), 1u);
  scsi.IrqRingUpdate();
  EXPECT_TRUE(callback_invoked);
  EXPECT_OK(callback_status);
  EXPECT_EQ(scsi.active_ios(), 0u);
}

class TestScsiDriver : public virtio::ScsiDriver {
 public:
  using scsi::Controller::block_devs_;
  using virtio::ScsiDriver::scsi_device;
  using virtio::ScsiDriver::set_scsi_device;

  zx::result<std::unique_ptr<scsi::BlockDevice>> BindBlockDevice(
      uint8_t target, uint16_t lun, uint32_t max_transfer_bytes,
      scsi::DeviceOptions device_options) override {
    return zx::ok(scsi::BlockDevice::CreateForTesting(this, target, lun, device_options, 1024,
                                                      zx_system_get_page_size()));
  }
};

TEST(ScsiTest, DriverExecuteCommandsAsync) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  TestScsiDriver driver;
  virtio::ScsiDevice* scsi_dev_ptr = scsi.get();
  driver.set_scsi_device(std::move(scsi));

  zx::result block_dev = driver.BindBlockDevice(
      0, 0, fuchsia_storage_block::wire::kMaxTransferUnbounded, scsi::DeviceOptions::Default());
  ASSERT_OK(block_dev);
  scsi::BlockDevice* dev = nullptr;
  {
    std::lock_guard<std::mutex> lock(driver.lock());
    driver.block_devs_[0][0] = std::move(block_dev.value());
    dev = driver.block_devs_[0][0].get();
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  block_server::Request request = {
      .request_id = 1,
      .operation =
          {
              .tag = block_server::Operation::Tag::Read,
              .read =
                  {
                      .device_block_offset = 0,
                      .block_count = 1,
                      ._unused = 0,
                      .vmo_offset = 0,
                      .options = {},
                  },
          },
      .trace_flow_id = 0,
      .vmo = vmo.borrow(),
  };

  dev->OnRequests(std::span(&request, 1));
  EXPECT_EQ(scsi_dev_ptr->active_ios(), 1u);
  scsi_dev_ptr->IrqRingUpdate();
  EXPECT_EQ(scsi_dev_ptr->active_ios(), 0u);

  {
    std::lock_guard<std::mutex> lock(driver.lock());
    driver.block_devs_.clear();
  }
}

TEST(ScsiTest, ReleaseCompletesPendingCommands) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  bool callback_invoked = false;
  zx_status_t callback_status = ZX_OK;

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};
  scsi->QueueCommand(
      /*target=*/0, /*lun=*/0, cdb, /*is_write=*/false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) {
        callback_invoked = true;
        callback_status = status;
      },
      /*data=*/nullptr, /*vmar_mapped=*/false);

  EXPECT_FALSE(callback_invoked);

  // Releasing the device before the command is completed should drain pending
  // IOs and complete them with ZX_ERR_IO_NOT_PRESENT.
  scsi->Release();

  EXPECT_TRUE(callback_invoked);
  EXPECT_EQ(callback_status, ZX_ERR_IO_NOT_PRESENT);
}

TEST(ScsiTest, ReleaseCompletesPendingScsiRequest) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  TestController controller;
  zx::result block_dev = controller.BindBlockDevice(
      0, 0, fuchsia_storage_block::wire::kMaxTransferUnbounded, scsi::DeviceOptions::Default());
  ASSERT_OK(block_dev);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  block_server::Request request = {
      .request_id = 1,
      .operation =
          {
              .tag = block_server::Operation::Tag::Read,
              .read =
                  {
                      .device_block_offset = 0,
                      .block_count = 1,
                      ._unused = 0,
                      .vmo_offset = 0,
                      .options = {},
                  },
          },
      .trace_flow_id = 0,
      .vmo = vmo.borrow(),
  };

  block_dev.value()->OnRequests(std::span(&request, 1));
  ASSERT_EQ(controller.reqs_.size(), 1u);

  auto scsi_req = std::move(controller.reqs_[0]);
  iovec cdb = {
      .iov_base = const_cast<uint8_t*>(scsi_req.cdb().data()),
      .iov_len = scsi_req.cdb().size(),
  };

  zx_vaddr_t mapped_addr;
  ASSERT_OK(zx_vmar_map(zx_vmar_root_self(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0,
                        scsi_req.data_vmo()->get(), scsi_req.vmo_offset(),
                        scsi_req.transfer_length_bytes(), &mapped_addr));

  bool callback_invoked = false;
  zx_status_t callback_status = ZX_OK;

  scsi->QueueCommand(
      /*target=*/0, /*lun=*/0, cdb, scsi_req.is_write(), scsi_req.data_vmo(), scsi_req.vmo_offset(),
      scsi_req.transfer_length_bytes(),
      [&callback_invoked, &callback_status, req = std::move(scsi_req)](zx_status_t status) mutable {
        callback_invoked = true;
        callback_status = status;
        req.Complete(status);
      },
      reinterpret_cast<void*>(mapped_addr), /*vmar_mapped=*/true);

  EXPECT_FALSE(callback_invoked);
  EXPECT_EQ(scsi->active_ios(), 1u);

  // If Release did not complete the pending request, ScsiRequest's destructor would assert.
  scsi->Release();

  EXPECT_TRUE(callback_invoked);
  EXPECT_EQ(callback_status, ZX_ERR_IO_NOT_PRESENT);
  EXPECT_EQ(scsi->active_ios(), 0u);
}

TEST(ScsiTest, QueueCommandAfterReleaseReturnsError) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  scsi->Release();

  bool callback_invoked = false;
  zx_status_t callback_status = ZX_OK;

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};
  scsi->QueueCommand(
      /*target=*/0, /*lun=*/0, cdb, /*is_write=*/false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) {
        callback_invoked = true;
        callback_status = status;
      },
      /*data=*/nullptr, /*vmar_mapped=*/false);

  EXPECT_TRUE(callback_invoked);
  EXPECT_EQ(callback_status, ZX_ERR_IO_NOT_PRESENT);
}

TEST(ScsiTest, DriverStopDrainsPendingRequests) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  TestScsiDriver driver;
  virtio::ScsiDevice* scsi_dev_ptr = scsi.get();
  driver.set_scsi_device(std::move(scsi));

  zx::result block_dev = driver.BindBlockDevice(
      0, 0, fuchsia_storage_block::wire::kMaxTransferUnbounded, scsi::DeviceOptions::Default());
  ASSERT_OK(block_dev);
  scsi::BlockDevice* dev = nullptr;
  {
    std::lock_guard<std::mutex> lock(driver.lock());
    driver.block_devs_[0][0] = std::move(block_dev.value());
    dev = driver.block_devs_[0][0].get();
  }

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  block_server::Request request = {
      .request_id = 1,
      .operation =
          {
              .tag = block_server::Operation::Tag::Read,
              .read =
                  {
                      .device_block_offset = 0,
                      .block_count = 1,
                      ._unused = 0,
                      .vmo_offset = 0,
                      .options = {},
                  },
          },
      .trace_flow_id = 0,
      .vmo = vmo.borrow(),
  };

  dev->OnRequests(std::span(&request, 1));
  EXPECT_EQ(scsi_dev_ptr->active_ios(), 1u);

  bool stop_completed = false;
  driver.Stop(fdf::StopCompleter([&](zx::result<> result) {
    EXPECT_OK(result);
    stop_completed = true;
  }));

  EXPECT_TRUE(stop_completed);
  EXPECT_EQ(scsi_dev_ptr->active_ios(), 0u);
}

TEST(ScsiTest, EncodeLun) {
  // Test that the virtio-scsi device correctly encodes single-level LUN structures.

  // Test encoding of target=1, LUN=1.
  struct virtio_scsi_req_cmd req = {};
  virtio::ScsiDevice::FillLUNStructure(&req, /*target=*/1, /*lun=*/1);
  EXPECT_EQ(req.lun[0], 1);
  EXPECT_EQ(req.lun[1], 1);
  // Expect flat addressing, single-level LUN structure.
  EXPECT_EQ(req.lun[2], 0x40 | 0x0);
  EXPECT_EQ(req.lun[3], 0x1);

  memset(&req, 0, sizeof(req));

  // Test encoding of target=0, LUN=8191.
  virtio::ScsiDevice::FillLUNStructure(&req, /*target=*/0, /*lun=*/8191);
  EXPECT_EQ(req.lun[0], 1);
  EXPECT_EQ(req.lun[1], 0);
  EXPECT_EQ(req.lun[2], 0x40 | 0x1F);
  EXPECT_EQ(req.lun[3], 0xFF);

  memset(&req, 0, sizeof(req));
  // Test encoding of target=0, LUN=16383 (highest allowed LUN).
  virtio::ScsiDevice::FillLUNStructure(&req, /*target=*/0, /*lun=*/16383);
  EXPECT_EQ(req.lun[0], 1);
  EXPECT_EQ(req.lun[1], 0);
  EXPECT_EQ(req.lun[2], 0x40 | 0x3F);
  EXPECT_EQ(req.lun[3], 0xFF);
}

TEST(ScsiTest, ReleaseCleansUpAllPendingTxnsAtomically) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};

  uint32_t active_ios_at_first_callback = 999;
  bool first_callback_called = false;
  bool second_callback_called = false;

  scsi->QueueCommand(
      0, 0, cdb, false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) {
        first_callback_called = true;
        active_ios_at_first_callback = scsi->active_ios();
      },
      nullptr, false);

  scsi->QueueCommand(
      0, 0, cdb, false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) { second_callback_called = true; }, nullptr, false);

  EXPECT_EQ(scsi->active_ios(), 2u);

  // Releasing the device must atomically reset and free all pending IO slots before
  // invoking any of the callbacks.
  scsi->Release();

  EXPECT_TRUE(first_callback_called);
  EXPECT_TRUE(second_callback_called);
  EXPECT_EQ(active_ios_at_first_callback, 0u);
  EXPECT_EQ(scsi->active_ios(), 0u);
}

TEST(ScsiTest, ReleaseCallsDeviceResetBeforeCleanup) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  bool device_reset_called = false;
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get(), &device_reset_called);
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());
  device_reset_called = false;

  bool callback_invoked = false;
  bool reset_called_during_cleanup = false;

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};
  scsi->QueueCommand(
      0, 0, cdb, false, zx::unowned_vmo(), 0, 0,
      [&](zx_status_t status) {
        callback_invoked = true;
        reset_called_during_cleanup = device_reset_called;
      },
      nullptr, false);

  EXPECT_FALSE(device_reset_called);
  scsi->Release();

  EXPECT_TRUE(callback_invoked);
  EXPECT_TRUE(reset_called_during_cleanup);
  EXPECT_TRUE(device_reset_called);
}

class AsyncShutdownBlockDevice : public scsi::BlockDevice {
 public:
  AsyncShutdownBlockDevice(scsi::Controller* controller, uint8_t target, uint16_t lun)
      : scsi::BlockDevice(controller, target, lun, scsi::DeviceOptions::Default()) {}

  void ShutdownAsync(fit::callback<void()> callback) override {
    shutdown_callback_ = std::move(callback);
  }

  void CompleteShutdown() {
    if (shutdown_callback_) {
      shutdown_callback_();
    }
  }

  bool HasShutdownCallback() const { return static_cast<bool>(shutdown_callback_); }

 private:
  fit::callback<void()> shutdown_callback_;
};

TEST(ScsiTest, StopCompletesAsynchronously) {
  TestScsiDriver driver;
  auto dev = std::make_unique<AsyncShutdownBlockDevice>(&driver, 0, 0);
  auto* dev_ptr = dev.get();
  {
    std::lock_guard<std::mutex> lock(driver.lock());
    driver.block_devs_[0][0] = std::move(dev);
  }

  bool stop_completed = false;
  driver.Stop(fdf::StopCompleter([&](zx::result<> result) {
    EXPECT_OK(result);
    stop_completed = true;
  }));

  // Stop() must return without blocking synchronously on dev->ShutdownAsync().
  EXPECT_FALSE(stop_completed);
  EXPECT_TRUE(dev_ptr->HasShutdownCallback());

  // Triggering the async callback completes Stop().
  dev_ptr->CompleteShutdown();
  EXPECT_TRUE(stop_completed);
}

TEST(ScsiTest, GetIOReturnsNullWhenReleased) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  scsi->Release();
  EXPECT_EQ(scsi->GetIOForTesting(), nullptr);
  EXPECT_EQ(scsi->active_ios(), 0u);
}

TEST(ScsiTest, ReleaseWhileWaitingForIoSlot) {
  fdf_testing::ScopedGlobalLogger logger;
  zx_handle_t bti_handle;
  ASSERT_OK(fake_bti_create(&bti_handle));
  zx::bti bti(bti_handle);
  auto backend = std::make_unique<FakeBackendForScsi>(bti.get());
  auto scsi = std::make_unique<virtio::ScsiDevice>(/*scsi_driver=*/nullptr, std::move(bti),
                                                   std::move(backend));
  ASSERT_OK(scsi->Init());

  uint8_t cdb_bytes[6] = {0};
  iovec cdb = {cdb_bytes, sizeof(cdb_bytes)};

  // Fill all MAX_IOS (16) slots.
  size_t callbacks_invoked = 0;
  for (int i = 0; i < virtio::MAX_IOS; i++) {
    scsi->QueueCommand(
        0, 0, cdb, false, zx::unowned_vmo(), 0, 0, [&](zx_status_t status) { callbacks_invoked++; },
        nullptr, false);
  }
  EXPECT_EQ(scsi->active_ios(), static_cast<uint32_t>(virtio::MAX_IOS));

  std::atomic<bool> thread_started = false;
  bool blocked_callback_called = false;
  zx_status_t blocked_callback_status = ZX_OK;

  std::thread waiting_thread([&]() {
    thread_started = true;
    scsi->QueueCommand(
        0, 0, cdb, false, zx::unowned_vmo(), 0, 0,
        [&](zx_status_t status) {
          blocked_callback_called = true;
          blocked_callback_status = status;
        },
        nullptr, false);
  });

  while (!thread_started.load()) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  // Allow time for the thread to block inside GetIO on ioslot_cv_.
  std::this_thread::sleep_for(std::chrono::milliseconds(10));

  scsi->Release();
  waiting_thread.join();

  EXPECT_EQ(callbacks_invoked, static_cast<size_t>(virtio::MAX_IOS));
  EXPECT_TRUE(blocked_callback_called);
  EXPECT_EQ(blocked_callback_status, ZX_ERR_IO_NOT_PRESENT);
  EXPECT_EQ(scsi->active_ios(), 0u);
}

}  // anonymous namespace
