// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/nand/drivers/nandpart/nandpart.h"

#include <fidl/fuchsia.boot.metadata/cpp/fidl.h>
#include <fidl/fuchsia.driver.compat/cpp/markers.h>
#include <fidl/fuchsia.driver.compat/cpp/wire_messaging.h>
#include <fidl/fuchsia.hardware.nand/cpp/common_types.h>
#include <fidl/fuchsia.hardware.nand/cpp/natural_types.h>
#include <fuchsia/hardware/badblock/cpp/banjo.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <fuchsia/hardware/nandinfo/c/banjo.h>
#include <lib/async/dispatcher.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/compat/cpp/banjo_client.h>
#include <lib/driver/compat/cpp/banjo_server.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/outgoing/cpp/outgoing_directory.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fit/internal/result.h>
#include <lib/sync/completion.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string_view>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace nand::testing {

class FakeNand : public ddk::NandProtocol<FakeNand> {
 public:
  compat::DeviceServer::BanjoConfig GetBanjoConfig() {
    compat::DeviceServer::BanjoConfig config{ZX_PROTOCOL_NAND};
    config.callbacks[ZX_PROTOCOL_NAND] = banjo_server_.callback();
    return config;
  }

  // Nand protocol implementation.
  void NandQuery(nand_info_t* info_out, size_t* nand_op_size_out) {
    *info_out = nand_info_t{
        .page_size = 1,
        .pages_per_block = 1,
        .num_blocks = 100,
        .ecc_bits = 0,
        .oob_size = 0,
    };
    *nand_op_size_out = sizeof(nand_operation_t);
  }

  void NandQueue(nand_operation_t* op, nand_queue_callback completion_cb, void* cookie) {
    completion_cb(cookie, ZX_OK, op);
  }

  zx_status_t NandGetFactoryBadBlockList(uint32_t* bad_blocks, size_t bad_block_len,
                                         size_t* num_bad_blocks) {
    return ZX_ERR_NOT_SUPPORTED;
  }

 private:
  compat::BanjoServer banjo_server_{ZX_PROTOCOL_NAND, this, &nand_protocol_ops_};
};

class NandpartTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(const fuchsia_hardware_nand::Config& nand_config,
            fuchsia_boot_metadata::PartitionMap partition_map) {
    device_server_.Initialize("default", std::nullopt, nand_.GetBanjoConfig());

    {
      fit::result persisted = fidl::Persist(nand_config);
      ASSERT_TRUE(persisted.is_ok());
      device_server_.AddMetadata(DEVICE_METADATA_PRIVATE, persisted.value().data(),
                                 persisted.value().size());
    }

    partition_map_ = std::move(partition_map);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    EXPECT_OK(device_server_.Serve(dispatcher, &to_driver_vfs));
    EXPECT_OK(partition_map_metadata_server_.Serve(to_driver_vfs, dispatcher, partition_map_));

    return zx::ok();
  }

 private:
  FakeNand nand_;
  compat::DeviceServer device_server_;
  fdf_metadata::MetadataServer<fuchsia_boot_metadata::PartitionMap> partition_map_metadata_server_;
  fuchsia_boot_metadata::PartitionMap partition_map_;
};

class FixtureConfig final {
 public:
  using DriverType = Driver;
  using EnvironmentType = NandpartTestEnvironment;
};

class NandpartTest : public ::testing::Test {
 public:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void StartDriver(const fuchsia_hardware_nand::Config& nand_config,
                   const fuchsia_boot_metadata::PartitionMap& partition_map) {
    driver_test_.RunInEnvironmentTypeContext(
        [&](NandpartTestEnvironment& env) { env.Init(nand_config, partition_map); });
    ASSERT_OK(driver_test_.StartDriver());
  }

  template <typename BanjoClient>
  BanjoClient ConnectToBanjo(std::string_view partition_name) {
    static const uint64_t kProcessKoid = compat::internal::GetKoid();

    zx::result compat_client_end =
        driver_test_.Connect<fuchsia_driver_compat::Service::Device>(partition_name);
    EXPECT_OK(compat_client_end);
    fidl::WireClient<fuchsia_driver_compat::Device> compat(
        std::move(compat_client_end.value()),
        driver_test_.runtime().GetForegroundDispatcher()->async_dispatcher());

    zx::result<BanjoClient> banjo_client;
    compat->GetBanjoProtocol(BanjoClient::kProtocolId, kProcessKoid)
        .ThenExactlyOnce(
            [&](fidl::WireUnownedResult<fuchsia_driver_compat::Device::GetBanjoProtocol>& result) {
              ASSERT_OK(result.status());
              banjo_client = compat::internal::OnResult<BanjoClient>(result);
              driver_test_.runtime().Quit();
            });

    driver_test_.runtime().Run();
    EXPECT_OK(banjo_client);
    EXPECT_TRUE(banjo_client.value().is_valid());
    return banjo_client.value();
  }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

// Verify that the nandpart driver creates a nandpart device when given a single partition.
TEST_F(NandpartTest, OnePartition) {
  static const fuchsia_hardware_nand::Config kNandConfig(
      {.bad_block_config = fuchsia_hardware_nand::BadBlockConfig({
           .type = fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot,
           .table_start_block = 0,
           .table_end_block = 0,
       })});

  static const fuchsia_boot_metadata::PartitionMap kPartitionMap(
      {.block_count = 1,
       .block_size = 1,
       .partitions = std::vector{{fuchsia_boot_metadata::Partition({
           .first_block = 0,
           .last_block = 0,
           .name = "partition 1",
       })}}});

  StartDriver(kNandConfig, kPartitionMap);

  // Verify that the nandpart driver created a new nandpart device that serves the nand and bad
  // block banjo protocols.
  ConnectToBanjo<ddk::NandProtocolClient>("partition 1");
  ConnectToBanjo<ddk::BadBlockProtocolClient>("partition 1");
}

TEST_F(NandpartTest, BoundsCheck) {
  static const fuchsia_hardware_nand::Config kNandConfig(
      {.bad_block_config = fuchsia_hardware_nand::BadBlockConfig({
           .type = fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot,
           .table_start_block = 0,
           .table_end_block = 0,
       })});

  // These settings are strange, but are built so that size_bytes==num_pages==num_blocks.
  static const fuchsia_boot_metadata::PartitionMap kPartitionMap(
      {.block_count = 10,
       .block_size = 1,
       .partitions = std::vector{{fuchsia_boot_metadata::Partition({
           .first_block = 0,
           .last_block = 9,
           .name = "partition 1",
       })}}});

  StartDriver(kNandConfig, kPartitionMap);

  auto client = ConnectToBanjo<ddk::NandProtocolClient>("partition 1");

  nand_info_t info;
  size_t op_size;
  client.Query(&info, &op_size);

  auto TestOpRange = [&](uint32_t command, uint32_t offset, uint32_t length,
                         zx_status_t expected_status) {
    std::vector<uint8_t> mem(op_size);
    nand_operation_t* op = reinterpret_cast<nand_operation_t*>(mem.data());
    op->command = command;
    if (command == NAND_OP_ERASE) {
      op->erase.first_block = offset;
      op->erase.num_blocks = length;
    } else if (command == NAND_OP_READ_BYTES || command == NAND_OP_WRITE_BYTES) {
      op->rw_bytes.offset_nand = offset;
      op->rw_bytes.length = length;
    } else {
      op->rw.offset_nand = offset;
      op->rw.length = length;
    }

    struct CompletionInfo {
      sync_completion_t completion;
      zx_status_t status;
    } comp_info;
    sync_completion_reset(&comp_info.completion);

    auto callback = [](void* cookie, zx_status_t status, nand_operation_t* op) {
      auto* info = static_cast<CompletionInfo*>(cookie);
      info->status = status;
      sync_completion_signal(&info->completion);
    };

    client.Queue(op, callback, &comp_info);
    ASSERT_OK(sync_completion_wait(&comp_info.completion, ZX_SEC(5)));
    EXPECT_STATUS(comp_info.status, expected_status);
  };

  for (uint32_t op_type :
       {NAND_OP_READ, NAND_OP_WRITE, NAND_OP_ERASE, NAND_OP_READ_BYTES, NAND_OP_WRITE_BYTES}) {
    // In bounds:
    TestOpRange(op_type, 0, 1, ZX_OK);
    TestOpRange(op_type, 9, 1, ZX_OK);
    TestOpRange(op_type, 0, 10, ZX_OK);

    // Out of bounds:
    TestOpRange(op_type, 10, 1, ZX_ERR_OUT_OF_RANGE);
    TestOpRange(op_type, 9, 2, ZX_ERR_OUT_OF_RANGE);
    TestOpRange(op_type, 0, 11, ZX_ERR_OUT_OF_RANGE);

    // Overflow:
    TestOpRange(op_type, 0xFFFFFFFF, 1, ZX_ERR_OUT_OF_RANGE);
    TestOpRange(op_type, 1, 0xFFFFFFFF, ZX_ERR_OUT_OF_RANGE);
  }
}

}  // namespace nand::testing
