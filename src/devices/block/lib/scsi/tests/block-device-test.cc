// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/fit/function.h>
#include <lib/scsi/block-device.h>
#include <lib/scsi/controller.h>
#include <lib/sync/cpp/completion.h>
#include <sys/types.h>

#include <map>
#include <memory>
#include <optional>
#include <queue>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace scsi {
namespace {
constexpr uint32_t kBlockSize = 512;
}  // namespace

// Controller for test; allows us to set expectations and fakes command responses.
class TestController : public fdf::DriverBase2, public Controller {
 public:
  using IOCallbackType = fit::function<zx_status_t(uint8_t, uint16_t, iovec, bool, iovec)>;
  static constexpr char kDriverName[] = "scsi-test";

  TestController() : fdf::DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override {
    incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
    node_name_ = context.node_name();
    parent_node_.Bind(take_node());

    auto [controller_client_end, controller_server_end] =
        fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
    auto [node_client_end, node_server_end] =
        fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

    node_controller_.Bind(std::move(controller_client_end));
    root_node_.Bind(std::move(node_client_end));

    fidl::Arena arena;

    const auto args =
        fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, name()).Build();

    fidl::WireResult result =
        parent_node_->AddChild(args, std::move(controller_server_end), std::move(node_server_end));
    if (!result.ok()) {
      fdf::error("Failed to add child: {}", result.status_string());
      return zx::error(result.status());
    }

    zx_status_t status = zx::event::create(0, &node_token_);
    if (status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok();
  }

  ~TestController() { ZX_ASSERT(times_ == 0); }

  // Init the state required for testing async IOs.
  zx_status_t AsyncIoInit() {
    {
      fbl::AutoLock lock(&lock_);
      queued_ios_ = {};
      worker_thread_exit_ = false;
    }
    auto cb = [](void* arg) -> int { return static_cast<TestController*>(arg)->WorkerThread(); };
    if (thrd_create_with_name(&worker_thread_, cb, this, "scsi-test-controller") != thrd_success) {
      printf("%s: Failed to create worker thread\n", __FILE__);
      return ZX_ERR_INTERNAL;
    }
    return ZX_OK;
  }

  // De-Init the state required for testing async IOs.
  void AsyncIoRelease() {
    {
      fbl::AutoLock lock(&lock_);
      worker_thread_exit_ = true;
      cv_.Signal();
    }
    thrd_join(worker_thread_, nullptr);
    fbl::AutoLock lock(&lock_);
    queued_ios_ = {};
  }

  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() override { return root_node_; }
  std::string_view driver_name() const override { return name(); }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const override { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() override { return outgoing(); }
  async_dispatcher_t* driver_async_dispatcher() const { return dispatcher(); }
  const std::optional<std::string>& driver_node_name() const override { return node_name_; }
  fdf::Logger& driver_logger() override { return logger(); }
  zx::event node_token() const override {
    zx::event token;
    zx_status_t status = node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token);
    if (status != ZX_OK) {
      return {};
    }
    return token;
  }

  void ExecuteCommandsAsync(uint8_t target, uint16_t lun, std::span<ScsiRequest> batch) override {
    fbl::AutoLock lock(&lock_);
    for (auto& req : batch) {
      auto io = std::make_unique<QueuedIo>();
      io->target = target;
      io->lun = lun;
      std::span<const uint8_t> cdb = req.cdb();
      EXPECT_EQ(req.opcode(), static_cast<Opcode>(cdb[0]));
      EXPECT_EQ(req.block_size(), kBlockSize);
      EXPECT_EQ(req.transfer_length_bytes(), req.transfer_length() * kBlockSize);
      memcpy(reinterpret_cast<void*>(&io->cdbptr), cdb.data(), cdb.size());
      io->cdb.iov_base = &io->cdbptr;
      io->cdb.iov_len = cdb.size();
      io->is_write = req.is_write();
      io->data_vmo = req.data_vmo();
      io->vmo_offset_bytes = req.vmo_offset();
      io->transfer_bytes = req.transfer_length_bytes();
      io->scsi_req = std::move(req);
      queued_ios_.push(std::move(io));
      cv_.Signal();
    }
  }

  zx_status_t ExecuteCommandSync(uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                                 iovec data) override {
    EXPECT_TRUE(do_io_);
    EXPECT_GT(times_, 0);

    if (!do_io_ || times_ == 0) {
      return ZX_ERR_INTERNAL;
    }

    auto status = do_io_(target, lun, cdb, is_write, data);
    if (--times_ == 0) {
      decltype(do_io_) empty;
      do_io_.swap(empty);
    }
    return status;
  }

  void ExpectCall(IOCallbackType do_io, int times) {
    do_io_.swap(do_io);
    times_ = times;
  }

 private:
  IOCallbackType do_io_;
  int times_ = 0;

  int WorkerThread() {
    while (true) {
      std::unique_ptr<QueuedIo> io;
      {
        fbl::AutoLock lock(&lock_);
        while (queued_ios_.empty() && !worker_thread_exit_) {
          cv_.Wait(&lock_);
        }
        if (worker_thread_exit_) {
          break;
        }
        io = std::move(queued_ios_.front());
        queued_ios_.pop();
      }
      if (!io) {
        continue;
      }

      std::unique_ptr<uint8_t[]> temp_buffer;
      zx_status_t status = ZX_OK;

      if (io->data_vmo->is_valid() && io->transfer_bytes > 0) {
        temp_buffer = std::make_unique<uint8_t[]>(io->transfer_bytes);
        // In case of WRITE command, populate the temp buffer with data from VMO.
        if (io->is_write) {
          status = zx_vmo_read(io->data_vmo->get(), temp_buffer.get(), io->vmo_offset_bytes,
                               io->transfer_bytes);
          if (status != ZX_OK) {
            io->scsi_req->Complete(status);
            continue;
          }
        }
      }

      iovec data_to_pass = {temp_buffer.get(), io->transfer_bytes};
      if (io->scsi_req.has_value() && io->scsi_req->immediate_data().size() > 0) {
        data_to_pass = {const_cast<uint8_t*>(io->scsi_req->immediate_data().data()),
                        io->scsi_req->immediate_data().size()};
      }

      status = ExecuteCommandSync(io->target, io->lun, io->cdb, io->is_write, data_to_pass);

      // In case of READ command, populate the VMO with data from temp buffer.
      if (status == ZX_OK && !io->is_write && io->data_vmo->is_valid() && io->transfer_bytes > 0) {
        status = zx_vmo_write(io->data_vmo->get(), temp_buffer.get(), io->vmo_offset_bytes,
                              io->transfer_bytes);
      }

      io->scsi_req->Complete(status);
    }
    return ZX_OK;
  }

  struct QueuedIo {
    uint8_t target;
    uint16_t lun;
    // Deep copy of the CDB.
    union {
      Read16CDB readcdb;
      Write16CDB writecdb;
    } cdbptr;
    iovec cdb;
    bool is_write;
    zx::unowned_vmo data_vmo;
    zx_off_t vmo_offset_bytes;
    size_t transfer_bytes;
    std::optional<ScsiRequest> scsi_req;
  };

  // These are the state for testing Async IOs.
  // The test enqueues Async IOs and pokes the worker thread, which
  // does the IO, and calls back.
  fbl::Mutex lock_;
  fbl::ConditionVariable cv_;
  thrd_t worker_thread_;
  bool worker_thread_exit_ __TA_GUARDED(lock_);
  std::queue<std::unique_ptr<QueuedIo>> queued_ios_ __TA_GUARDED(lock_);

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;

  zx::event node_token_;

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<std::string> node_name_;
};

class TestConfig final {
 public:
  using DriverType = TestController;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class BlockDeviceTest : public ::testing::Test {
 public:
  static constexpr uint8_t kTarget = 5;
  static constexpr uint16_t kLun = 1;
  static constexpr int kTransferSize = 32 * 1024;
  static constexpr uint64_t kFakeBlocks = 0x128000000;

  using DiskBlock = unsigned char[kBlockSize];

  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_OK(result);
    SetUpCommands();
  }
  void TearDown() override {
    ShutdownDevice(std::move(device_));
    zx::result<> result = driver_test().StopDriver();
    ASSERT_OK(result);
  }
  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  void ShutdownDevice(std::unique_ptr<BlockDevice> dev) {
    if (!dev) {
      return;
    }
    libsync::Completion completion;
    driver_test().RunInDriverContext([&](TestController& controller) {
      dev->ShutdownAsync([&completion]() { completion.Signal(); });
    });
    completion.Wait();
    driver_test().RunInDriverContext([&](TestController& controller) { dev.reset(); });
  }

  void SetUpCommands(uint64_t block_count = kFakeBlocks) {
    default_seq_ = 0;
    const bool is_large = block_count > UINT32_MAX;
    const int total_times = is_large ? 10 : 9;
    // Set up default command expectations.
    driver_test().RunInDriverContext([this, block_count, is_large,
                                      total_times](TestController& controller) {
      controller.ExpectCall(
          [this, block_count, is_large](uint8_t target, uint16_t lun, iovec cdb, bool is_write,
                                        iovec data) -> auto {
            EXPECT_EQ(target, kTarget);
            EXPECT_EQ(lun, kLun);

            int seq = default_seq_++;
            if (!is_large && seq >= 5) {
              // When block_count <= UINT32_MAX, ReadCapacity16 is skipped,
              // so skip case 5 in the sequence.
              seq++;
            }

            switch (seq) {
              case 0: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::INQUIRY);
                EXPECT_FALSE(is_write);
                break;
              }
              case 1: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::TEST_UNIT_READY);
                EXPECT_FALSE(is_write);
                break;
              }
              case 2: {
                if (cdb.iov_len == 6) {
                  ModeSense6CDB decoded_cdb = {};
                  memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                  EXPECT_EQ(decoded_cdb.opcode, Opcode::MODE_SENSE_6);
                  EXPECT_EQ(decoded_cdb.page_code(), PageCode::kAllPageCode);
                  EXPECT_EQ(decoded_cdb.disable_block_descriptors(), true);
                  EXPECT_FALSE(is_write);
                  Mode6ParameterHeader header = {};
                  memcpy(data.iov_base, reinterpret_cast<char*>(&header), sizeof(header));
                } else {
                  EXPECT_EQ(cdb.iov_len, size_t{10});
                  ModeSense10CDB decoded_cdb = {};
                  memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                  EXPECT_EQ(decoded_cdb.opcode, Opcode::MODE_SENSE_10);
                  EXPECT_EQ(decoded_cdb.page_code(), PageCode::kAllPageCode);
                  EXPECT_EQ(decoded_cdb.disable_block_descriptors(), true);
                  EXPECT_FALSE(is_write);
                  Mode10ParameterHeader header = {};
                  memcpy(data.iov_base, reinterpret_cast<char*>(&header), sizeof(header));
                }
                break;
              }
              case 3: {
                if (cdb.iov_len == 6) {
                  ModeSense6CDB decoded_cdb = {};
                  memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                  EXPECT_EQ(decoded_cdb.opcode, Opcode::MODE_SENSE_6);
                  EXPECT_EQ(decoded_cdb.page_code(), PageCode::kCachingPageCode);
                  EXPECT_EQ(decoded_cdb.disable_block_descriptors(), true);
                  EXPECT_FALSE(is_write);
                  Mode6ParameterHeader header = {};
                  memcpy(data.iov_base, reinterpret_cast<char*>(&header), sizeof(header));
                  CachingModePage response = {};
                  response.set_page_code(static_cast<uint8_t>(PageCode::kCachingPageCode));
                  response.set_write_cache_enabled(true);
                  memcpy(static_cast<char*>(data.iov_base) + sizeof(header),
                         reinterpret_cast<char*>(&response), sizeof(response));
                } else {
                  EXPECT_EQ(cdb.iov_len, size_t{10});
                  ModeSense10CDB decoded_cdb = {};
                  memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                  EXPECT_EQ(decoded_cdb.opcode, Opcode::MODE_SENSE_10);
                  EXPECT_EQ(decoded_cdb.page_code(), PageCode::kCachingPageCode);
                  EXPECT_EQ(decoded_cdb.disable_block_descriptors(), true);
                  EXPECT_FALSE(is_write);
                  Mode10ParameterHeader header = {};
                  memcpy(data.iov_base, reinterpret_cast<char*>(&header), sizeof(header));
                  CachingModePage response = {};
                  response.set_page_code(static_cast<uint8_t>(PageCode::kCachingPageCode));
                  response.set_write_cache_enabled(true);
                  memcpy(static_cast<char*>(data.iov_base) + sizeof(header),
                         reinterpret_cast<char*>(&response), sizeof(response));
                }
                break;
              }
              case 4: {
                EXPECT_EQ(cdb.iov_len, size_t{10});
                ReadCapacity10CDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::READ_CAPACITY_10);
                EXPECT_FALSE(is_write);
                ReadCapacity10ParameterData response = {};
                if (is_large) {
                  response.returned_logical_block_address = htobe32(UINT32_MAX);
                } else {
                  response.returned_logical_block_address =
                      htobe32(static_cast<uint32_t>(block_count - 1));
                }
                response.block_length_in_bytes = htobe32(kBlockSize);
                memcpy(data.iov_base, reinterpret_cast<char*>(&response), sizeof(response));
                break;
              }
              case 5: {
                EXPECT_EQ(cdb.iov_len, size_t{16});
                ReadCapacity16CDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::READ_CAPACITY_16);
                EXPECT_EQ(decoded_cdb.service_action, 0x10);
                EXPECT_FALSE(is_write);
                ReadCapacity16ParameterData response = {};
                response.returned_logical_block_address = htobe64(block_count - 1);
                response.block_length_in_bytes = htobe32(kBlockSize);
                memcpy(data.iov_base, reinterpret_cast<char*>(&response), sizeof(response));
                break;
              }
              case 6: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::INQUIRY);
                EXPECT_EQ(decoded_cdb.page_code, scsi::InquiryCDB::kPageListVpdPageCode);
                EXPECT_FALSE(is_write);
                VPDPageList vpd_page_list = {};
                vpd_page_list.peripheral_qualifier_device_type = 0;
                vpd_page_list.page_code = InquiryCDB::kPageListVpdPageCode;
                vpd_page_list.page_length = 2;
                vpd_page_list.pages[0] = InquiryCDB::kBlockLimitsVpdPageCode;
                vpd_page_list.pages[1] = InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
                memcpy(data.iov_base, reinterpret_cast<char*>(&vpd_page_list),
                       sizeof(vpd_page_list));
                break;
              }
              case 7: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::INQUIRY);
                EXPECT_EQ(decoded_cdb.page_code, InquiryCDB::kBlockLimitsVpdPageCode);
                EXPECT_FALSE(is_write);
                VPDBlockLimits block_limits = {};
                block_limits.peripheral_qualifier_device_type = 0;
                block_limits.page_code = scsi::InquiryCDB::kBlockLimitsVpdPageCode;
                block_limits.maximum_unmap_lba_count = htobe32(UINT32_MAX);
                memcpy(data.iov_base, &block_limits, sizeof(block_limits));
                break;
              }
              case 8: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::INQUIRY);
                EXPECT_EQ(decoded_cdb.page_code, InquiryCDB::kPageListVpdPageCode);
                EXPECT_FALSE(is_write);
                VPDPageList vpd_page_list = {};
                vpd_page_list.peripheral_qualifier_device_type = 0;
                vpd_page_list.page_code = InquiryCDB::kPageListVpdPageCode;
                vpd_page_list.page_length = 2;
                vpd_page_list.pages[0] = InquiryCDB::kBlockLimitsVpdPageCode;
                vpd_page_list.pages[1] = InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
                memcpy(data.iov_base, reinterpret_cast<char*>(&vpd_page_list),
                       sizeof(vpd_page_list));
                break;
              }
              case 9: {
                EXPECT_EQ(cdb.iov_len, size_t{6});
                InquiryCDB decoded_cdb = {};
                memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
                EXPECT_EQ(decoded_cdb.opcode, Opcode::INQUIRY);
                EXPECT_EQ(decoded_cdb.page_code, InquiryCDB::kLogicalBlockProvisioningVpdPageCode);
                EXPECT_FALSE(is_write);
                VPDLogicalBlockProvisioning provisioning = {};
                provisioning.peripheral_qualifier_device_type = 0;
                provisioning.page_code = scsi::InquiryCDB::kLogicalBlockProvisioningVpdPageCode;
                provisioning.set_lbpu(true);
                provisioning.set_provisioning_type(0x02);  // The logical unit is thin provisioned
                memcpy(data.iov_base, &provisioning, sizeof(provisioning));
                break;
              }
            }

            return ZX_OK;
          },
          total_times);
    });
  }

  zx::result<PostProcess> CheckScsiStatus(
      StatusCode status_code,
      std::optional<std::reference_wrapper<FixedFormatSenseDataHeader>> sense_data) {
    return driver_test().RunInDriverContext<zx::result<PostProcess>>(
        [&](TestController& controller) {
          return controller.CheckScsiStatus(status_code, sense_data);
        });
  }

 protected:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  int default_seq_ = 0;
  std::unique_ptr<BlockDevice> device_;
};

// Test that we can create a block device when the underlying controller successfully executes CDBs.
TEST_F(BlockDeviceTest, TestCreateDestroy) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });
}

// Test that we can create a block device when the underlying controller successfully executes CDBs.
TEST_F(BlockDeviceTest, TestCreateDestroyWithModeSense10) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/false,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });
}

TEST_F(BlockDeviceTest, ScsiComplete) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });

  StatusMessage status_message = {HostStatusCode::kOk, StatusCode::GOOD};

  FixedFormatSenseDataHeader sense_data;
  sense_data.set_response_code(SenseDataResponseCodes::kFixedCurrentInformation);
  sense_data.set_filemark(false);
  sense_data.set_eom(false);
  sense_data.set_ili(false);
  sense_data.set_sense_key(SenseKey::NO_SENSE);
  // ASC=00h, ASCQ=00h, NO ADDITIONAL SENSE INFORMATION
  sense_data.additional_sense_code = 0x0;
  sense_data.additional_sense_code_qualifier = 0x0;

  // Success with sense data
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_OK(controller.ScsiComplete(status_message, sense_data));
  });

  // Success without sense data (standard SCSI/UFS behavior for GOOD status)
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_OK(controller.ScsiComplete(status_message, std::nullopt));
  });

  // CHECK_CONDITION without sense data should fail with ZX_ERR_INVALID_ARGS
  StatusMessage check_condition_message = {HostStatusCode::kOk, StatusCode::CHECK_CONDITION};
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_EQ(controller.ScsiComplete(check_condition_message, std::nullopt).status_value(),
              ZX_ERR_INVALID_ARGS);
  });

  // Abort
  status_message.host_status_code = HostStatusCode::kAbort;
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_EQ(controller.ScsiComplete(status_message, sense_data).status_value(),
              ZX_ERR_IO_REFUSED);
  });

  // Unexpected host status value
  status_message.host_status_code = HostStatusCode::kUnknown;
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_EQ(controller.ScsiComplete(status_message, sense_data).status_value(), ZX_ERR_BAD_STATE);
  });

  // Error handling
  status_message.host_status_code = HostStatusCode::kTimeout;
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_EQ(controller.ScsiComplete(status_message, sense_data).status_value(), ZX_ERR_TIMED_OUT);
  });

  // Retry
  status_message.host_status_code = HostStatusCode::kRequeue;
  driver_test().RunInDriverContext([&](TestController& controller) {
    EXPECT_EQ(controller.ScsiComplete(status_message, sense_data).status_value(), ZX_ERR_BAD_STATE);
  });
}

TEST_F(BlockDeviceTest, CheckScsiStatus) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });

  FixedFormatSenseDataHeader sense_data;
  sense_data.set_response_code(SenseDataResponseCodes::kFixedCurrentInformation);
  sense_data.set_filemark(false);
  sense_data.set_eom(false);
  sense_data.set_ili(false);
  sense_data.set_sense_key(SenseKey::NO_SENSE);
  // ASC=00h, ASCQ=00h, NO ADDITIONAL SENSE INFORMATION
  sense_data.additional_sense_code = 0x0;
  sense_data.additional_sense_code_qualifier = 0x0;

  // StatusCode::GOOD, TASK_ABORTED
  {
    EXPECT_OK(CheckScsiStatus(StatusCode::GOOD, sense_data));
    EXPECT_OK(CheckScsiStatus(StatusCode::GOOD, std::nullopt));
    EXPECT_OK(CheckScsiStatus(StatusCode::TASK_ABORTED, std::nullopt));
  }

  // StatusCode::CHECK_CONDITION
  {
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNone);

    auto missing_sense = CheckScsiStatus(StatusCode::CHECK_CONDITION, std::nullopt);
    EXPECT_EQ(missing_sense.status_value(), ZX_ERR_INVALID_ARGS);
  }

  // StatusCode::TASK_SET_FULL
  {
    auto post_process = CheckScsiStatus(StatusCode::TASK_SET_FULL, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNeedsRetry);
  }

  // StatusCode::BUSY
  {
    auto post_process = CheckScsiStatus(StatusCode::BUSY, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNeedsRetry);
  }

  // Not supported status codes
  {
    auto post_process = CheckScsiStatus(StatusCode::CONDITION_MET, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(BlockDeviceTest, CheckSenseData) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });

  FixedFormatSenseDataHeader sense_data;
  sense_data.set_response_code(SenseDataResponseCodes::kFixedCurrentInformation);
  sense_data.set_filemark(false);
  sense_data.set_eom(false);
  sense_data.set_ili(false);
  sense_data.set_sense_key(SenseKey::NO_SENSE);
  // ASC=00h, ASCQ=00h, NO ADDITIONAL SENSE INFORMATION
  sense_data.additional_sense_code = 0x0;
  sense_data.additional_sense_code_qualifier = 0x0;

  // Invalid response code
  {
    sense_data.set_response_code(SenseDataResponseCodes::kDescriptorCurrentInformation);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_NOT_SUPPORTED);

    sense_data.set_response_code(SenseDataResponseCodes::kFixedCurrentInformation);
  }

  // Invalid FILEMARK, EOM, ILI
  {
    sense_data.set_filemark(true);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_INVALID_ARGS);
    sense_data.set_filemark(false);
  }

  // Invalid EOM
  {
    sense_data.set_eom(true);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_INVALID_ARGS);
    sense_data.set_eom(false);
  }

  // Invalid ILI
  {
    sense_data.set_ili(true);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_INVALID_ARGS);
    sense_data.set_ili(false);
  }

  // SenseKey::NO_SENSE
  {
    sense_data.set_sense_key(SenseKey::NO_SENSE);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNone);
  }

  // SenseKey::RECOVERED_ERROR
  {
    sense_data.set_sense_key(SenseKey::NO_SENSE);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNone);
  }

  // SenseKey::ABORTED_COMMAND
  {
    sense_data.set_sense_key(SenseKey::ABORTED_COMMAND);
    sense_data.additional_sense_code = 0x10;  // DIF
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_IO_DATA_INTEGRITY);

    // ASC=0x2e, ASCQ=0x01: COMMAND TIMEOUT BEFORE PROCESSING
    sense_data.additional_sense_code = 0x2e;
    sense_data.additional_sense_code_qualifier = 0x01;
    post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_TIMED_OUT);
    sense_data.additional_sense_code = 0;
    sense_data.additional_sense_code_qualifier = 0;

    post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNeedsRetry);
  }

  // SenseKey::NOT_READY, UNIT_ATTENTION
  {
    // Expected UNIT_ATTENTION
    sense_data.set_sense_key(SenseKey::UNIT_ATTENTION);
    driver_test().RunInDriverContext([&](TestController& controller) {
      controller.SetExpectCheckConditionOrUnitAttention(true);
    });
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNeedsRetry);

    // Unit is not ready
    driver_test().RunInDriverContext([&](TestController& controller) {
      controller.SetExpectCheckConditionOrUnitAttention(false);
    });
    // ASC=0x04, ASCQ=0x01: LOGICAL UNIT IS IN PROCESS OF BECOMING READY
    sense_data.additional_sense_code = 0x04;
    sense_data.additional_sense_code_qualifier = 0x01;
    post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_OK(post_process);
    EXPECT_EQ(post_process.value(), PostProcess::kNeedsRetry);
    sense_data.additional_sense_code = 0;
    sense_data.additional_sense_code_qualifier = 0;

    post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_BAD_STATE);
  }

  // Not supported
  {
    sense_data.set_sense_key(SenseKey::MEDIUM_ERROR);
    auto post_process = CheckScsiStatus(StatusCode::CHECK_CONDITION, sense_data);
    EXPECT_EQ(post_process.status_value(), ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(BlockDeviceTest, BlockServerGetName) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);

  const fidl::WireResult result = fidl::WireCall(client_end.value())->GetName();
  ASSERT_TRUE(result.ok());
  ASSERT_OK(result.value().status);
  EXPECT_EQ(result.value().name.get(), "lun1");
}

TEST_F(BlockDeviceTest, BlockServerRead) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  // Configure back-end behavior for reads.
  std::map<uint64_t, DiskBlock> blocks;
  DiskBlock& test_block_1 = blocks[1];
  memset(test_block_1, 0xAB, sizeof(DiskBlock));

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{16});
          Read16CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::READ_16);
          EXPECT_FALSE(is_write);
          // Expecting 1 block request
          EXPECT_EQ(be32toh(decoded_cdb.transfer_length), uint32_t{1});
          uint64_t block_to_read = be64toh(decoded_cdb.logical_block_address);
          const DiskBlock& data_to_return = blocks.at(block_to_read);
          memcpy(data.iov_base, data_to_return, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);

  auto remote_device_result =
      block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
  ASSERT_OK(remote_device_result);
  auto client = std::move(remote_device_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  EXPECT_EQ(info.block_size, kBlockSize);
  EXPECT_EQ(info.block_count, kFakeBlocks);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  DiskBlock check_buffer = {};
  ASSERT_OK(vmo.read(check_buffer, 0, sizeof(DiskBlock)));
  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(check_buffer[i], 0xAB);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerWrite) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  std::map<uint64_t, DiskBlock> blocks;

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{16});
          Write16CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::WRITE_16);
          EXPECT_TRUE(is_write);
          EXPECT_EQ(be32toh(decoded_cdb.transfer_length), uint32_t{1});
          uint64_t block_to_write = be64toh(decoded_cdb.logical_block_address);
          memcpy(blocks[block_to_write], data.iov_base, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  DiskBlock write_buffer;
  memset(write_buffer, 0xCD, sizeof(DiskBlock));
  ASSERT_OK(vmo.write(write_buffer, 0, sizeof(DiskBlock)));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(blocks[1][i], 0xCD);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerRead12) {
  constexpr uint64_t kSmallBlockCount = 1024;
  SetUpCommands(kSmallBlockCount);
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  std::map<uint64_t, DiskBlock> blocks;
  DiskBlock& test_block_1 = blocks[1];
  memset(test_block_1, 0xAB, sizeof(DiskBlock));

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{12});
          Read12CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::READ_12);
          EXPECT_FALSE(is_write);
          EXPECT_EQ(be32toh(decoded_cdb.transfer_length), uint32_t{1});
          uint32_t block_to_read = be32toh(decoded_cdb.logical_block_address);
          const DiskBlock& data_to_return = blocks.at(block_to_read);
          memcpy(data.iov_base, data_to_return, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);

  auto remote_device_result =
      block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
  ASSERT_OK(remote_device_result);
  auto client = std::move(remote_device_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  EXPECT_EQ(info.block_size, kBlockSize);
  EXPECT_EQ(info.block_count, kSmallBlockCount);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  DiskBlock check_buffer = {};
  ASSERT_OK(vmo.read(check_buffer, 0, sizeof(DiskBlock)));
  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(check_buffer[i], 0xAB);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerWrite12) {
  constexpr uint64_t kSmallBlockCount = 1024;
  SetUpCommands(kSmallBlockCount);
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  std::map<uint64_t, DiskBlock> blocks;

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{12});
          Write12CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::WRITE_12);
          EXPECT_TRUE(is_write);
          EXPECT_EQ(be32toh(decoded_cdb.transfer_length), uint32_t{1});
          uint32_t block_to_write = be32toh(decoded_cdb.logical_block_address);
          memcpy(blocks[block_to_write], data.iov_base, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  DiskBlock write_buffer;
  memset(write_buffer, 0xCD, sizeof(DiskBlock));
  ASSERT_OK(vmo.write(write_buffer, 0, sizeof(DiskBlock)));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(blocks[1][i], 0xCD);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerRead10) {
  constexpr uint64_t kSmallBlockCount = 1024;
  SetUpCommands(kSmallBlockCount);
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/false));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  std::map<uint64_t, DiskBlock> blocks;
  DiskBlock& test_block_1 = blocks[1];
  memset(test_block_1, 0xAB, sizeof(DiskBlock));

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{10});
          Read10CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::READ_10);
          EXPECT_FALSE(is_write);
          EXPECT_EQ(be16toh(decoded_cdb.transfer_length), uint16_t{1});
          uint32_t block_to_read = be32toh(decoded_cdb.logical_block_address);
          const DiskBlock& data_to_return = blocks.at(block_to_read);
          memcpy(data.iov_base, data_to_return, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);

  auto remote_device_result =
      block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
  ASSERT_OK(remote_device_result);
  auto client = std::move(remote_device_result.value());

  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client->BlockGetInfo(&info));
  EXPECT_EQ(info.block_size, kBlockSize);
  EXPECT_EQ(info.block_count, kSmallBlockCount);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  DiskBlock check_buffer = {};
  ASSERT_OK(vmo.read(check_buffer, 0, sizeof(DiskBlock)));
  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(check_buffer[i], 0xAB);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerWrite10) {
  constexpr uint64_t kSmallBlockCount = 1024;
  SetUpCommands(kSmallBlockCount);
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/false));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  std::map<uint64_t, DiskBlock> blocks;

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [&blocks](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{10});
          Write10CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::WRITE_10);
          EXPECT_TRUE(is_write);
          EXPECT_EQ(be16toh(decoded_cdb.transfer_length), uint16_t{1});
          uint32_t block_to_write = be32toh(decoded_cdb.logical_block_address);
          memcpy(blocks[block_to_write], data.iov_base, sizeof(DiskBlock));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  DiskBlock write_buffer;
  memset(write_buffer, 0xCD, sizeof(DiskBlock));
  ASSERT_OK(vmo.write(write_buffer, 0, sizeof(DiskBlock)));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  for (uint i = 0; i < sizeof(DiskBlock); i++) {
    EXPECT_EQ(blocks[1][i], 0xCD);
  }

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerFlush) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{10});
          SynchronizeCache10CDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::SYNCHRONIZE_CACHE_10);
          EXPECT_FALSE(is_write);
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_FLUSH},
      .vmoid = BLOCK_VMOID_INVALID,
      .length = 0,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerTrim) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  driver_test().RunInDriverContext([&](TestController& controller) {
    controller.ExpectCall(
        [](uint8_t target, uint16_t lun, iovec cdb, bool is_write, iovec data) -> auto {
          EXPECT_EQ(cdb.iov_len, size_t{10});
          UnmapCDB decoded_cdb = {};
          memcpy(&decoded_cdb, cdb.iov_base, cdb.iov_len);
          EXPECT_EQ(decoded_cdb.opcode, Opcode::UNMAP);
          EXPECT_TRUE(is_write);
          EXPECT_EQ(data.iov_len, sizeof(UnmapParameterListHeader) + sizeof(UnmapBlockDescriptor));
          return ZX_OK;
        },
        /*times=*/1);
    controller.AsyncIoInit();
  });

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_TRIM},
      .vmoid = BLOCK_VMOID_INVALID,
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 1,
  };

  ASSERT_OK(client->FifoTransaction(&request, 1));

  driver_test().RunInDriverContext(
      [&](TestController& controller) { controller.AsyncIoRelease(); });
}

TEST_F(BlockDeviceTest, BlockServerInvalidIoRange) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  auto client_end =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(client_end);
  auto client = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  storage::Vmoid owned_vmoid;
  ASSERT_OK(client->BlockAttachVmo(vmo, &owned_vmoid));

  // Issue a read request that exceeds the device capacity (kFakeBlocks)
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_READ},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = kFakeBlocks + 10,  // Out of range
  };

  // Verify that the transaction is correctly rejected with ZX_ERR_OUT_OF_RANGE
  ASSERT_EQ(client->FifoTransaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

  ASSERT_OK(client->BlockDetachVmo(std::move(owned_vmoid)));
}

TEST_F(BlockDeviceTest, BlockServerService) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  zx::result volume_connect =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(instance_name);
  ASSERT_OK(volume_connect);

  zx::result token_connect =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Token>(instance_name);
  ASSERT_OK(token_connect);

  driver_test().RunInNodeContext(
      [](fdf_testing::TestNode& node) { ASSERT_EQ(size_t{1}, node.children().size()); });
}

TEST_F(BlockDeviceTest, NodeToken) {
  driver_test().RunInDriverContext([&](TestController& controller) {
    auto result =
        BlockDevice::Bind(&controller, kTarget, kLun, kTransferSize,
                          DeviceOptions(/*check_unmap_support=*/true, /*use_mode_sense_6=*/true,
                                        /*use_read_write_12=*/true));
    ASSERT_OK(result);
    device_ = std::move(result.value());
  });
  std::string instance_name(device_->DeviceName().c_str());

  zx::result connect_result =
      driver_test().Connect<fuchsia_hardware_block_volume::Service::Token>(instance_name);
  ASSERT_OK(connect_result);

  fidl::SyncClient<fuchsia_driver_token::NodeToken> client(std::move(connect_result.value()));
  auto get_result = client->Get();
  ASSERT_TRUE(get_result.is_ok());

  zx_info_handle_basic_t info1, info2;
  driver_test().RunInDriverContext([&](TestController& controller) {
    ASSERT_EQ(controller.node_token().get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr,
                                               nullptr),
              ZX_OK);
  });
  ASSERT_EQ(
      get_result->token().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);
  ASSERT_EQ(info1.koid, info2.koid);
}

}  // namespace scsi

FUCHSIA_DRIVER_EXPORT2(scsi::TestController);
