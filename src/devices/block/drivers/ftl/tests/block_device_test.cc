// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "block_device.h"

#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/cpp/vmo/types.h>

#include <cstddef>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace ftl {
namespace {

constexpr uint32_t kPageSize = 1024;
constexpr uint32_t kNumPages = 20;
constexpr char kMagic = 'f';
constexpr uint8_t kGuid[ZBI_PARTITION_GUID_LEN] = {'g', 'u', 'i', 'd'};
constexpr uint32_t kWearCount = 1337;
constexpr uint32_t kInitialBadBlocks = 3;
constexpr uint32_t kRunningBadBlocks = 4;

bool CheckPattern(const void* buffer, size_t size, char pattern = kMagic) {
  const char* data = reinterpret_cast<const char*>(buffer);
  for (; size; size--) {
    if (*data++ != pattern) {
      return false;
    }
  }
  return true;
}

class FakeNand : public ddk::NandProtocol<FakeNand> {
 public:
  static const nand_protocol_ops_t kOps;

  FakeNand() : proto_({&kOps, this}) {}

  nand_protocol_t* proto() { return &proto_; }

  // Nand protocol:
  void NandQuery(nand_info_t* out_info, size_t* out_nand_op_size) {
    *out_info = {};
    out_info->page_size = 1024;
    out_info->oob_size = 8;
    out_info->pages_per_block = 4;
    out_info->num_blocks = 10;
    out_info->ecc_bits = 12;
    memcpy(out_info->partition_guid, kGuid, sizeof(kGuid));
    *out_nand_op_size = sizeof(nand_operation_t);
  }

  void NandQueue(nand_operation_t* operation, nand_queue_callback callback, void* cookie) {}

  zx_status_t NandGetFactoryBadBlockList(uint32_t* out_bad_blocks_list, size_t bad_blocks_count,
                                         size_t* out_bad_blocks_actual) {
    return ZX_ERR_BAD_STATE;
  }

 private:
  nand_protocol_t proto_;
};

class FakeVolume final : public ftl::Volume {
 public:
  explicit FakeVolume(ftl::BlockDevice* device) : device_(device) {
    data_.resize(static_cast<size_t>(kNumPages) * kPageSize, kMagic);
  }
  ~FakeVolume() final {}

  bool written() const { return written_; }
  bool flushed() const { return flushed_; }
  bool formatted() const { return formatted_; }
  bool leveled() const { return leveled_; }
  bool trimmed() const { return trimmed_; }
  uint32_t first_page() const { return first_page_; }
  int num_pages() const { return num_pages_; }

  // Volume interface.
  const char* Init(std::unique_ptr<ftl::NdmDriver> driver) final {
    device_->OnVolumeAdded(kPageSize, kNumPages);
    return nullptr;
  }
  const char* ReAttach() final { return nullptr; }
  zx_status_t Read(uint32_t first_page, int num_pages, void* buffer) final {
    OnOperation();
    first_page_ = first_page;
    num_pages_ = num_pages;
    memcpy(buffer, data_.data() + (static_cast<size_t>(first_page) * kPageSize),
           num_pages * kPageSize);
    return ZX_OK;
  }
  zx_status_t Write(uint32_t first_page, int num_pages, const void* buffer) final {
    OnOperation();
    first_page_ = first_page;
    num_pages_ = num_pages;
    written_ = true;
    memcpy(data_.data() + (static_cast<size_t>(first_page) * kPageSize), buffer,
           num_pages * kPageSize);
    return ZX_OK;
  }
  zx_status_t Format() final {
    formatted_ = true;
    return ZX_OK;
  }
  zx_status_t FormatAndLevel() final {
    leveled_ = true;
    return ZX_OK;
  }
  zx_status_t Mount() final { return ZX_OK; }
  zx_status_t Unmount() final { return ZX_OK; }
  zx_status_t Flush() final {
    OnOperation();
    flushed_ = true;
    return ZX_OK;
  }
  zx_status_t Trim(uint32_t first_page, uint32_t num_pages) final {
    OnOperation();
    trimmed_ = true;
    first_page_ = first_page;
    num_pages_ = num_pages;
    memset(data_.data() + (static_cast<size_t>(first_page) * kPageSize), 1, num_pages * kPageSize);
    return ZX_OK;
  }

  zx_status_t GarbageCollect() final { return ZX_OK; }

  zx_status_t GetStats(Stats* stats) final {
    *stats = {};
    stats->wear_count = wear_count_;
    stats->initial_bad_blocks = initial_bad_blocks_;
    stats->running_bad_blocks = running_bad_blocks_;
    stats->worn_blocks_detected = worn_blocks_detected_;
    return ZX_OK;
  }

  zx_status_t GetCounters(Counters* counters) final {
    counters->wear_count = wear_count_;
    counters->initial_bad_blocks = initial_bad_blocks_;
    counters->running_bad_blocks = running_bad_blocks_;
    counters->worn_blocks_detected = worn_blocks_detected_;
    return ZX_OK;
  }

  zx_status_t GetNewWearLeveling(bool* state) final {
    *state = false;
    return ZX_OK;
  }

  zx_status_t SetNewWearLeveling(bool state) final { return ZX_OK; }

  void UpdateWearCount(uint32_t wear_count) { wear_count_ = wear_count; }

  void UpdateInitialBadBlockCount(uint32_t initial_bad_blocks) {
    initial_bad_blocks_ = initial_bad_blocks;
  }
  void UpdateRunningBadBlockCount(uint32_t running_bad_blocks) {
    running_bad_blocks_ = running_bad_blocks;
  }

  void UpdateWornBlocksCount(uint32_t worn_blocks_detected) {
    worn_blocks_detected_ = worn_blocks_detected;
  }

  void SetOnOperation(fit::function<void()> callback) { on_operation_ = std::move(callback); }

 private:
  void OnOperation() {
    if (on_operation_) {
      on_operation_();
    }
  }

  ftl::BlockDevice* device_;
  std::vector<uint8_t> data_;

  uint32_t first_page_ = 0;
  int num_pages_ = 0;
  uint32_t wear_count_ = kWearCount;
  uint32_t initial_bad_blocks_ = kInitialBadBlocks;
  uint32_t running_bad_blocks_ = kRunningBadBlocks;
  uint32_t worn_blocks_detected_ = 0;
  fit::function<void()> on_operation_;
  bool written_ = false;
  bool flushed_ = false;
  bool formatted_ = false;
  bool leveled_ = false;
  bool trimmed_ = false;
};

class TestFtlBlockDevice : public ftl::BlockDevice {
 public:
  TestFtlBlockDevice() : ftl::BlockDevice() {
    auto volume = std::make_unique<FakeVolume>(static_cast<ftl::BlockDevice*>(this));
    volume_ptr_ = volume.get();
    SetVolumeForTest(std::move(volume));
  }

  FakeVolume* volume() { return volume_ptr_; }

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(
        fdf_internal::DriverServer2<TestFtlBlockDevice>::initialize,
        fdf_internal::DriverServer2<TestFtlBlockDevice>::destroy);
  }

 private:
  FakeVolume* volume_ptr_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    compat::DeviceServer::BanjoConfig banjo_config;
    banjo_config.callbacks[ZX_PROTOCOL_NAND] = [this]() {
      return compat::DeviceServer::GenericProtocol{
          .ops = fake_nand_->proto()->ops,
          .ctx = fake_nand_->proto()->ctx,
      };
    };

    compat_server_.Initialize("default", std::nullopt, std::move(banjo_config));
    zx_status_t status =
        compat_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok();
  }

 private:
  std::unique_ptr<FakeNand> fake_nand_ = std::make_unique<FakeNand>();
  compat::DeviceServer compat_server_;
};

class TestConfig {
 public:
  using DriverType = TestFtlBlockDevice;
  using EnvironmentType = TestEnvironment;
};

const nand_protocol_ops_t FakeNand::kOps = {
    .query =
        [](void* ctx, nand_info_t* out_info, size_t* out_nand_op_size) {
          static_cast<FakeNand*>(ctx)->NandQuery(out_info, out_nand_op_size);
        },
    .queue =
        [](void* ctx, nand_operation_t* op, nand_queue_callback cb, void* cookie) {
          if (op->command == NAND_OP_READ) {
            uint8_t buf[2048];
            memset(buf, 0xff, sizeof(buf));
            ASSERT_OK(zx_vmo_write(op->rw.data_vmo, buf, op->rw.offset_data_vmo * 1024,
                                   op->rw.length * 1024));
            memset(buf, 0xff, 16);
            ASSERT_OK(
                zx_vmo_write(op->rw.oob_vmo, buf, op->rw.offset_oob_vmo * 8, op->rw.length * 8));
          }
          async::PostTask(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                          [cb, cookie, op]() { cb(cookie, ZX_OK, op); });
        },
    .get_factory_bad_block_list = [](void* ctx, uint32_t* out, size_t count,
                                     size_t* actual) { return ZX_ERR_BAD_STATE; },
};

class BlockDeviceTest : public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(zx::event::create(0, &node_token_));
    zx::event token_copy;
    ASSERT_OK(node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));

    ASSERT_OK(driver_test_.StartDriverWithCustomStartArgs(
        [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); }));
    fidl::ClientEnd<fuchsia_io::Directory> svc_dir = driver_test_.ConnectToDriverSvcDir();
    zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
    ASSERT_OK(service);
    zx::result client_end = service->connect_volume();
    ASSERT_OK(client_end);
    client_ = block_client::RemoteBlockDevice::Create(std::move(client_end.value())).value();
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  void Read() {
    char buffer[kPageSize];
    memset(buffer, 'f', sizeof(buffer));
    fidl::ClientEnd<fuchsia_io::Directory> svc_dir = driver_test_.ConnectToDriverSvcDir();
    zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
    ASSERT_OK(service);
    zx::result read_client = service->connect_volume();
    ASSERT_OK(read_client);
    ASSERT_OK(
        block_client::SingleReadBytes(read_client.value(), buffer, sizeof(buffer), 3 * kPageSize));
  }

  void Write() {
    char buffer[kPageSize];
    memset(buffer, 'f', sizeof(buffer));
    fidl::ClientEnd<fuchsia_io::Directory> svc_dir = driver_test_.ConnectToDriverSvcDir();
    zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
    ASSERT_OK(service);
    zx::result write_client = service->connect_volume();
    ASSERT_OK(write_client);
    ASSERT_OK(block_client::SingleWriteBytes(write_client.value(), buffer, sizeof(buffer),
                                             5 * kPageSize));
  }

  void Flush() const {
    BlockFifoRequest requests[1] = {};
    requests[0].command = {.opcode = BLOCK_OPCODE_FLUSH, .flags = 0};
    ASSERT_OK(client_->FifoTransaction(requests, 1));
  }

  void Trim() const {
    BlockFifoRequest requests[1] = {};
    requests[0].command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
    requests[0].length = 2;
    requests[0].dev_offset = 3;
    ASSERT_OK(client_->FifoTransaction(requests, 1));
  }

  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;

  std::unique_ptr<block_client::RemoteBlockDevice> client_;
  zx::event node_token_;
};

TEST_F(BlockDeviceTest, GetInfo) {
  fuchsia_storage_block::wire::BlockInfo info;
  ASSERT_OK(client_->BlockGetInfo(&info));

  EXPECT_EQ(info.block_count, kNumPages);
  EXPECT_EQ(info.block_size, kPageSize);
  EXPECT_EQ(info.max_transfer_size, fuchsia_storage_block::wire::kMaxTransferUnbounded);
  EXPECT_TRUE(
      static_cast<uint32_t>(info.flags & fuchsia_storage_block::wire::DeviceFlag::kTrimSupport));
}

TEST_F(BlockDeviceTest, ReadWrite) {
  char buffer[kPageSize * 2];
  memset(buffer, 0, sizeof(buffer));

  auto svc_dir = driver_test_.ConnectToDriverSvcDir();
  zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
  ASSERT_OK(service);
  zx::result read_client = service->connect_volume();
  ASSERT_OK(read_client);
  ASSERT_OK(
      block_client::SingleReadBytes(read_client.value(), buffer, sizeof(buffer), 3 * kPageSize));

  driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    EXPECT_TRUE(driver.volume()->written() == false);
    EXPECT_EQ(2, driver.volume()->num_pages());
    EXPECT_EQ(3, driver.volume()->first_page());
  });
  EXPECT_TRUE(CheckPattern(buffer, kPageSize * 2));

  memset(buffer, kMagic, sizeof(buffer));
  zx::result write_client = service->connect_volume();
  ASSERT_OK(write_client);

  ASSERT_OK(
      block_client::SingleWriteBytes(write_client.value(), buffer, sizeof(buffer), 5 * kPageSize));

  driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    EXPECT_TRUE(driver.volume()->written());
    EXPECT_EQ(2, driver.volume()->num_pages());
    EXPECT_EQ(5, driver.volume()->first_page());
  });
}

TEST(BlockDeviceTest, Lifetime) {
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test;
  zx::event node_token;
  ASSERT_OK(zx::event::create(0, &node_token));
  zx::event token_copy;
  ASSERT_OK(node_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));
  ASSERT_OK(driver_test.StartDriverWithCustomStartArgs(
      [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); }));
  ASSERT_OK(driver_test.StopDriver());
}

TEST_F(BlockDeviceTest, Trim) {
  BlockFifoRequest requests[1] = {};
  requests[0].command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
  requests[0].length = 2;
  requests[0].dev_offset = kNumPages;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, client_->FifoTransaction(requests, 1));

  requests[0].dev_offset = 3;
  ASSERT_OK(client_->FifoTransaction(requests, 1));
  driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    EXPECT_TRUE(driver.volume()->trimmed());
    EXPECT_EQ(2, driver.volume()->num_pages());
    EXPECT_EQ(3, driver.volume()->first_page());
  });

  char buffer[kPageSize * 2];
  memset(buffer, 0, sizeof(buffer));
  auto svc_dir = driver_test_.ConnectToDriverSvcDir();
  zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
  ASSERT_OK(service);
  zx::result read_client = service->connect_volume();
  ASSERT_OK(read_client);
  ASSERT_OK(
      block_client::SingleReadBytes(read_client.value(), buffer, sizeof(buffer), 3 * kPageSize));

  EXPECT_TRUE(CheckPattern(buffer, sizeof(buffer), 1));
}

TEST_F(BlockDeviceTest, Flush) {
  BlockFifoRequest requests[1] = {};
  requests[0].command = {.opcode = BLOCK_OPCODE_FLUSH, .flags = 0};
  ASSERT_OK(client_->FifoTransaction(requests, 1));
  driver_test_.RunInDriverContext(
      [](TestFtlBlockDevice& driver) { EXPECT_TRUE(driver.volume()->flushed()); });
}

TEST_F(BlockDeviceTest, QueueMultiple) {
  char buffer[kPageSize * 2];
  memset(buffer, 'f', sizeof(buffer));

  auto svc_dir = driver_test_.ConnectToDriverSvcDir();
  zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
  ASSERT_OK(service);

  zx::result write_client = service->connect_volume();
  ASSERT_OK(write_client);
  ASSERT_OK(
      block_client::SingleWriteBytes(write_client.value(), buffer, sizeof(buffer), 5 * kPageSize));

  zx::result read_client = service->connect_volume();
  ASSERT_OK(read_client);
  ASSERT_OK(
      block_client::SingleReadBytes(read_client.value(), buffer, sizeof(buffer), 3 * kPageSize));

  driver_test_.RunInDriverContext(
      [](TestFtlBlockDevice& driver) { EXPECT_TRUE(driver.volume()->written()); });
}

TEST_F(BlockDeviceTest, GetInspectVmoContainsCountersAndWearCount) {
  zx::vmo vmo;
  driver_test_.RunInDriverContext([&vmo](TestFtlBlockDevice& driver) {
    vmo = static_cast<ftl::BlockDevice&>(driver).DuplicateInspectVmo();
  });

  auto base_hierarchy = inspect::ReadFromVmo(vmo).take_value();
  auto* hierarchy = base_hierarchy.GetByPath({"ftl"});
  ASSERT_NOT_NULL(hierarchy);

  auto* property =
      hierarchy->node().get_property<inspect::UintPropertyValue>("nand.erase_block.max_wear");
  ASSERT_NOT_NULL(property);
  EXPECT_EQ(property->value(), kWearCount);

  auto* property_initial =
      hierarchy->node().get_property<inspect::UintPropertyValue>("nand.initial_bad_blocks");
  ASSERT_NOT_NULL(property_initial);
  EXPECT_EQ(property_initial->value(), kInitialBadBlocks);

  auto* property_running =
      hierarchy->node().get_property<inspect::UintPropertyValue>("nand.running_bad_blocks");
  ASSERT_NOT_NULL(property_running);
  EXPECT_EQ(property_running->value(), kRunningBadBlocks);

  auto* property_worn =
      hierarchy->node().get_property<inspect::UintPropertyValue>("nand.worn_blocks_detected");
  ASSERT_NOT_NULL(property_worn);
  EXPECT_EQ(property_worn->value(), 0);
}

TEST_F(BlockDeviceTest, NodeToken) {
  fidl::ClientEnd<fuchsia_io::Directory> svc_dir = driver_test_.ConnectToDriverSvcDir();
  zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
  ASSERT_OK(service);
  zx::result client_end = service->connect_token();
  ASSERT_OK(client_end);

  fidl::SyncClient<fuchsia_driver_token::NodeToken> client(std::move(client_end.value()));
  auto get_result = client->Get();
  ASSERT_TRUE(get_result.is_ok());

  zx_info_handle_basic_t info1, info2;
  ASSERT_EQ(node_token_.get_info(ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr, nullptr),
            ZX_OK);
  ASSERT_EQ(
      get_result->token().get_info(ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr),
      ZX_OK);
  ASSERT_EQ(info1.koid, info2.koid);
}

void ReadProperties(const zx::vmo& vmo, std::map<std::string, uint64_t>& counters,
                    std::map<std::string, double>& rates) {
  auto base_hierarchy = inspect::ReadFromVmo(vmo).take_value();
  auto* hierarchy = base_hierarchy.GetByPath({"ftl"});
  for (const auto& property_name : ftl::Metrics::GetPropertyNames<inspect::UintProperty>()) {
    auto* property = hierarchy->node().get_property<inspect::UintPropertyValue>(property_name);
    ASSERT_NOT_NULL(property, "Missing Inspect Property: %s", property_name.c_str());
    counters[property_name] = property->value();
  }

  for (const auto& property_name : ftl::Metrics::GetPropertyNames<inspect::DoubleProperty>()) {
    auto* property = hierarchy->node().get_property<inspect::DoublePropertyValue>(property_name);
    ASSERT_NOT_NULL(property, "Missing Inspect Property: %s", property_name.c_str());
    rates[property_name] = property->value();
  }
}

void VerifyInspectMetrics(BlockDeviceTest* fixture, const std::string& block_metric_prefix,
                          fit::function<std::string()> clear_op,
                          fit::function<void()> trigger_metric_update_op) {
  fixture->driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    driver.volume()->UpdateWearCount(0);
    driver.volume()->UpdateInitialBadBlockCount(0);
    driver.volume()->UpdateRunningBadBlockCount(0);
  });

  std::map<std::string, uint64_t> counters;
  std::map<std::string, double> rates;
  std::map<std::string, uint64_t> expected_counters;
  std::map<std::string, double> expected_rates;

  // Random operation to trigger a metric update.
  expected_counters[clear_op()]++;

  zx::vmo vmo;
  fixture->driver_test_.RunInDriverContext([&vmo](TestFtlBlockDevice& driver) {
    vmo = static_cast<ftl::BlockDevice&>(driver).DuplicateInspectVmo();
  });

  ReadProperties(vmo, counters, rates);
  for (const auto& counter : counters) {
    EXPECT_EQ(counter.second, expected_counters[counter.first],
              "Property %s had initial non zero counter.", counter.first.c_str());
  }

  // The counters are cleared before any operation.
  fixture->driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    driver.volume()->SetOnOperation([&driver]() {
      auto& counters = driver.nand_counters();
      counters.page_read = 1;
      counters.page_write = 2;
      counters.block_erase = 3;
    });
    driver.volume()->UpdateWearCount(24);
  });

  trigger_metric_update_op();

  fixture->driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    driver.volume()->SetOnOperation([&driver]() {
      auto& counters = driver.nand_counters();
      counters.page_read = 2;
      counters.page_write = 4;
      counters.block_erase = 5;
    });
    driver.volume()->UpdateWearCount(12345678);
  });

  trigger_metric_update_op();

  expected_counters[ftl::Metrics::GetMaxWearPropertyName()] = 12345678;
  expected_counters["nand.erase_block.max_wear"] = 12345678;

  // Counters
  expected_counters[block_metric_prefix + ".count"] = 2;
  expected_counters[block_metric_prefix + ".issued_nand_operation.count"] = 17;
  expected_counters[block_metric_prefix + ".issued_page_read.count"] = 3;
  expected_counters[block_metric_prefix + ".issued_page_write.count"] = 6;
  expected_counters[block_metric_prefix + ".issued_block_erase.count"] = 8;

  // Rates
  expected_rates[block_metric_prefix + ".issued_nand_operation.average_rate"] = 8.5;
  expected_rates[block_metric_prefix + ".issued_page_read.average_rate"] = 1.5;
  expected_rates[block_metric_prefix + ".issued_page_write.average_rate"] = 3;
  expected_rates[block_metric_prefix + ".issued_block_erase.average_rate"] = 4;

  zx::vmo vmo2;
  fixture->driver_test_.RunInDriverContext([&vmo2](TestFtlBlockDevice& driver) {
    vmo2 = static_cast<ftl::BlockDevice&>(driver).DuplicateInspectVmo();
  });
  counters.clear();
  rates.clear();
  ReadProperties(vmo2, counters, rates);

  for (const auto& counter : counters) {
    EXPECT_EQ(counter.second, expected_counters[counter.first], "Property %s mismatch.",
              counter.first.c_str());
  }

  for (const auto& rate : rates) {
    EXPECT_EQ(rate.second, expected_rates[rate.first], "Property %s mismatch.", rate.first.c_str());
  }
}

TEST_F(BlockDeviceTest, InspectReadMetricsUpdatedCorrectly) {
  VerifyInspectMetrics(
      this, "block.read",
      [&]() {
        Flush();
        return "block.flush.count";
      },
      [&]() { Read(); });
}

TEST_F(BlockDeviceTest, InspectWriteMetricsUpdatedCorrectly) {
  VerifyInspectMetrics(
      this, "block.write",
      [&]() {
        Flush();
        return "block.flush.count";
      },
      [&]() { Write(); });
}

TEST_F(BlockDeviceTest, InspectTrimMetricsUpdatedCorrectly) {
  VerifyInspectMetrics(
      this, "block.trim",
      [&]() {
        Flush();
        return "block.flush.count";
      },
      [&]() { Trim(); });
}

TEST_F(BlockDeviceTest, InspectFlushMetricsUpdatedCorrectly) {
  VerifyInspectMetrics(
      this, "block.flush",
      [&]() {
        Trim();
        return "block.trim.count";
      },
      [&]() { Flush(); });
}

TEST_F(BlockDeviceTest, InspectBadBlockMetricsPopulation) {
  zx::vmo vmo;
  driver_test_.RunInDriverContext([&vmo](TestFtlBlockDevice& driver) {
    vmo = static_cast<ftl::BlockDevice&>(driver).DuplicateInspectVmo();
  });

  std::map<std::string, uint64_t> counters;
  std::map<std::string, double> rates;

  ReadProperties(vmo, counters, rates);
  ASSERT_EQ(counters["nand.initial_bad_blocks"], kInitialBadBlocks);
  ASSERT_EQ(counters["nand.running_bad_blocks"], kRunningBadBlocks);
  ASSERT_EQ(counters["nand.total_bad_blocks"], kInitialBadBlocks + kRunningBadBlocks);
  ASSERT_EQ(counters["nand.worn_blocks_detected"], 0);
  ASSERT_EQ(counters["nand.projected_bad_blocks"], kInitialBadBlocks + kRunningBadBlocks);

  driver_test_.RunInDriverContext([](TestFtlBlockDevice& driver) {
    driver.volume()->UpdateInitialBadBlockCount(7);
    driver.volume()->UpdateRunningBadBlockCount(8);
    driver.volume()->UpdateWornBlocksCount(2);
  });

  // Force a stats update.
  Read();

  zx::vmo vmo2;
  driver_test_.RunInDriverContext([&vmo2](TestFtlBlockDevice& driver) {
    vmo2 = static_cast<ftl::BlockDevice&>(driver).DuplicateInspectVmo();
  });
  counters.clear();
  rates.clear();
  ReadProperties(vmo2, counters, rates);

  ASSERT_EQ(counters["nand.initial_bad_blocks"], 7);
  ASSERT_EQ(counters["nand.running_bad_blocks"], 8);
  ASSERT_EQ(counters["nand.total_bad_blocks"], 15);
  ASSERT_EQ(counters["nand.worn_blocks_detected"], 2);
  ASSERT_EQ(counters["nand.projected_bad_blocks"], 17);
}

TEST_F(BlockDeviceTest, ConcurrentRequests) {
  constexpr int kNumThreads = 4;
  constexpr int kOpsPerThread = 50;

  std::vector<std::thread> threads;
  threads.reserve(kNumThreads);
  std::vector<zx_status_t> results(kNumThreads, ZX_OK);

  for (int i = 0; i < kNumThreads; ++i) {
    threads.emplace_back([this, i, &results]() {
      auto svc_dir = driver_test_.ConnectToDriverSvcDir();
      zx::result service =
          component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
      if (service.is_error()) {
        results[i] = service.error_value();
        return;
      }
      zx::result client = service->connect_volume();
      if (client.is_error()) {
        results[i] = client.error_value();
        return;
      }

      for (int j = 0; j < kOpsPerThread; ++j) {
        char buffer[kPageSize];
        zx_status_t status;
        if (j % 2 == 0) {
          memset(buffer, kMagic, sizeof(buffer));
          status = block_client::SingleWriteBytes(client.value(), buffer, sizeof(buffer),
                                                  (j % kNumPages) * kPageSize);
        } else {
          status = block_client::SingleReadBytes(client.value(), buffer, sizeof(buffer),
                                                 (j % kNumPages) * kPageSize);
        }
        if (status != ZX_OK) {
          results[i] = status;
          return;
        }
      }
    });
  }

  for (auto& t : threads) {
    t.join();
  }

  for (int i = 0; i < kNumThreads; ++i) {
    EXPECT_OK(results[i], "Thread %d failed", i);
  }
}

TEST(BlockDeviceTest, RaceConditionTeardown) {
  fdf_testing::BackgroundDriverTest<TestConfig> driver;
  zx::event node_token;
  ASSERT_OK(zx::event::create(0, &node_token));
  zx::event token_copy;
  ASSERT_OK(node_token.duplicate(ZX_RIGHT_SAME_RIGHTS, &token_copy));
  ASSERT_OK(driver.StartDriverWithCustomStartArgs(
      [&](fdf::DriverStartArgs& args) { args.node_token(std::move(token_copy)); }));
  fidl::ClientEnd<fuchsia_io::Directory> svc_dir = driver.ConnectToDriverSvcDir();
  zx::result service = component::OpenServiceAt<fuchsia_hardware_block_volume::Service>(svc_dir);
  ASSERT_OK(service);
  zx::result client_end = service->connect_volume();
  ASSERT_OK(client_end);
  zx::result client = block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
  ASSERT_OK(client);
  std::atomic<bool> stopped = false;
  std::thread t([client = block_client::ReaderWriter(**std::move(client)), &stopped]() mutable {
    char buffer[kPageSize];
    memset(buffer, kMagic, sizeof(buffer));
    while (!stopped) {
      zx_status_t status = client.Read(0, sizeof(buffer), buffer);
      if (status != ZX_OK) {
        break;
      }
    }
  });

  zx::nanosleep(zx::deadline_after(zx::msec(50)));
  ASSERT_OK(driver.StopDriver());
  stopped = true;

  t.join();
}

}  // namespace
}  // namespace ftl
