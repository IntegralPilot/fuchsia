// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/devices/bus/drivers/platform/platform-bus.h"

#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.hardware.interrupt/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-bti/bti.h>
#include <lib/zbi-format/partition.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/status.h>

#include <algorithm>

#include <ddk/metadata/test.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/bus/drivers/platform/node-util.h"
#include "src/devices/bus/drivers/platform/platform_bus_config.h"
#include "src/lib/testing/predicates/status.h"

namespace {

class BootItems final : public fidl::WireServer<fuchsia_boot::Items> {
 public:
  fidl::ProtocolHandler<fuchsia_boot::Items> handler(async_dispatcher_t* dispatcher) {
    return bindings_.CreateHandler(this, dispatcher, fidl::kIgnoreBindingClosure);
  }

  void Get(GetRequestView request, GetCompleter::Sync& completer) override;
  void Get2(Get2RequestView request, Get2Completer::Sync& completer) override;
  void GetBootloaderFile(GetBootloaderFileRequestView request,
                         GetBootloaderFileCompleter::Sync& completer) override;

  void SetPartitionMap(const zbi_partition_map_t& map,
                       std::span<const zbi_partition_t> partitions) {
    auto& bytes = partition_map_bytes_.emplace();
    bytes.reserve(sizeof(zbi_partition_map_t) + (partitions.size() * sizeof(zbi_partition_t)));

    const auto* map_bytes = reinterpret_cast<const uint8_t*>(&map);
    bytes.insert(bytes.end(), map_bytes, map_bytes + sizeof(zbi_partition_map_t));

    const auto* partitions_bytes = reinterpret_cast<const uint8_t*>(partitions.data());
    bytes.insert(bytes.end(), partitions_bytes, partitions_bytes + partitions.size_bytes());
  }

 private:
  zx_status_t GetBootItem(const std::vector<board_test::DeviceEntry>& entries, uint32_t type,
                          uint32_t extra, zx::vmo* out, uint32_t* length);

  std::optional<std::vector<uint8_t>> partition_map_bytes_;

  fidl::ServerBindingGroup<fuchsia_boot::Items> bindings_;
};

const zbi_platform_id_t kPlatformId = []() {
  zbi_platform_id_t plat_id = {};
  plat_id.vid = PDEV_VID_TEST;
  plat_id.pid = PDEV_PID_PBUS_TEST;
  strcpy(plat_id.board_name, "pbus-unit-test");
  return plat_id;
}();

#define BOARD_REVISION_TEST 42

const zbi_board_info_t kBoardInfo = []() {
  zbi_board_info_t board_info = {};
  board_info.revision = BOARD_REVISION_TEST;
  return board_info;
}();

zx_status_t BootItems::GetBootItem(const std::vector<board_test::DeviceEntry>& entries,
                                   uint32_t type, uint32_t extra, zx::vmo* out, uint32_t* length) {
  zx::vmo vmo;
  switch (type) {
    case ZBI_TYPE_DRV_PARTITION_MAP: {
      if (!partition_map_bytes_.has_value()) {
        return ZX_ERR_NOT_FOUND;
      }
      const auto& partition_map = partition_map_bytes_.value();
      zx_status_t status = zx::vmo::create(partition_map.size(), 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      status = vmo.write(partition_map.data(), 0, partition_map.size());
      if (status != ZX_OK) {
        return status;
      }
      *length = static_cast<uint32_t>(partition_map.size());
      break;
    }
    case ZBI_TYPE_PLATFORM_ID: {
      zx_status_t status = zx::vmo::create(sizeof(kPlatformId), 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      status = vmo.write(&kPlatformId, 0, sizeof(kPlatformId));
      if (status != ZX_OK) {
        return status;
      }
      *length = sizeof(kPlatformId);
      break;
    }
    case ZBI_TYPE_DRV_BOARD_INFO: {
      zx_status_t status = zx::vmo::create(sizeof(kBoardInfo), 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }
      status = vmo.write(&kBoardInfo, 0, sizeof(kBoardInfo));
      if (status != ZX_OK) {
        return status;
      }
      *length = sizeof(kBoardInfo);
      break;
    }
    case ZBI_TYPE_DRV_BOARD_PRIVATE: {
      size_t list_size = sizeof(board_test::DeviceList);
      size_t entry_size = entries.size() * sizeof(board_test::DeviceEntry);

      size_t metadata_size = 0;
      for (const board_test::DeviceEntry& entry : entries) {
        metadata_size += entry.metadata_size;
      }

      zx_status_t status = zx::vmo::create(list_size + entry_size + metadata_size, 0, &vmo);
      if (status != ZX_OK) {
        return status;
      }

      // Write DeviceList to vmo.
      board_test::DeviceList list{.count = entries.size()};
      status = vmo.write(&list, 0, sizeof(list));
      if (status != ZX_OK) {
        return status;
      }

      // Write DeviceEntries to vmo.
      status = vmo.write(entries.data(), list_size, entry_size);
      if (status != ZX_OK) {
        return status;
      }

      // Write Metadata to vmo.
      size_t write_offset = list_size + entry_size;
      for (const board_test::DeviceEntry& entry : entries) {
        status = vmo.write(entry.metadata, write_offset, entry.metadata_size);
        if (status != ZX_OK) {
          return status;
        }
        write_offset += entry.metadata_size;
      }

      *length = static_cast<uint32_t>(list_size + entry_size + metadata_size);
      break;
    }
    default:
      return ZX_ERR_NOT_FOUND;
  }
  *out = std::move(vmo);
  return ZX_OK;
}

void BootItems::Get(GetRequestView request, GetCompleter::Sync& completer) {
  zx::vmo vmo;
  uint32_t length = 0;
  std::vector<board_test::DeviceEntry> entries = {};
  std::ignore = GetBootItem(entries, request->type, request->extra, &vmo, &length);
  completer.Reply(std::move(vmo), length);
}

void BootItems::Get2(Get2RequestView request, Get2Completer::Sync& completer) {
  std::vector<board_test::DeviceEntry> entries = {};
  zx::vmo vmo;
  uint32_t length = 0;
  uint32_t extra = 0;
  zx_status_t status = GetBootItem(entries, request->type, extra, &vmo, &length);
  if (status != ZX_OK) {
    completer.Reply(zx::error(status));
    return;
  }
  std::vector<fuchsia_boot::wire::RetrievedItems> result;
  fuchsia_boot::wire::RetrievedItems items = {
      .payload = std::move(vmo), .length = length, .extra = extra};
  result.emplace_back(std::move(items));
  completer.ReplySuccess(
      fidl::VectorView<fuchsia_boot::wire::RetrievedItems>::FromExternal(result));
}

void BootItems::GetBootloaderFile(GetBootloaderFileRequestView request,
                                  GetBootloaderFileCompleter::Sync& completer) {
  completer.Reply(zx::vmo());
}

class FakeInterruptController : public fidl::Server<fuchsia_hardware_interrupt::Controller> {
 public:
  struct RegisteredInterrupt {
    uint32_t irq;
    fuchsia_hardware_platform_bus::ZirconInterruptMode mode;
    fuchsia_hardware_interrupt::InterruptOptions options;
    zx::interrupt interrupt;
  };

  void RegisterController(
      fidl::ClientEnd<fuchsia_hardware_interrupt::ControllerRegistry> registry_client_end) {
    auto [controller_client_end, controller_server_end] =
        fidl::Endpoints<fuchsia_hardware_interrupt::Controller>::Create();

    bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                         std::move(controller_server_end), this, fidl::kIgnoreBindingClosure);

    fidl::WireSyncClient<fuchsia_hardware_interrupt::ControllerRegistry> registry(
        std::move(registry_client_end));

    fidl::WireResult result = registry->RegisterController(std::move(controller_client_end));
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  void RegisterInterrupt(RegisterInterruptRequest& request,
                         RegisterInterruptCompleter::Sync& completer) override {
    registered_interrupts.push_back({
        .irq = request.irq(),
        .mode = request.mode(),
        .options = request.options(),
        .interrupt = std::move(request.interrupt()),
    });
    completer.Reply(fit::ok());
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_interrupt::Controller> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    FAIL();
  }

  std::vector<RegisteredInterrupt> take_registered_interrupts() {
    std::vector<RegisteredInterrupt> interrupts = std::move(registered_interrupts);
    registered_interrupts.clear();
    return interrupts;
  }

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_interrupt::Controller> bindings_;
  std::vector<RegisteredInterrupt> registered_interrupts;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    return to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_boot::Items>(
        boot_items_.handler(dispatcher));
  }

  BootItems& boot_items() { return boot_items_; }

  FakeInterruptController& fake_controller() { return fake_controller_; }

 private:
  BootItems boot_items_;
  FakeInterruptController fake_controller_;
};

class TestConfig final {
 public:
  using DriverType = platform_bus::PlatformBus;
  using EnvironmentType = TestEnvironment;
};

class PlatformBusTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test().StartDriverWithCustomStartArgs([](fdf::DriverStartArgs& args) {
      platform_bus_config::Config config;
      args.config(config.ToVmo());
    }));

    zx::result pbus =
        driver_test_.Connect<fuchsia_hardware_platform_bus::Service::PlatformBus>("pt");
    ASSERT_OK(pbus);
    pbus_.Bind(std::move(pbus.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test().StopDriver()); }

 protected:
  void SetPartitionMapBootItem(const zbi_partition_map_t& map,
                               std::span<const zbi_partition_t> partitions) {
    driver_test_.RunInEnvironmentTypeContext(
        [&](auto& env) { env.boot_items().SetPartitionMap(map, partitions); });
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }
  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus() { return pbus_; }

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus> pbus_;
};

// Verify that the platform bus can create a platform device that exposes an empty partition map
// found in boot args as metadata.
TEST_F(PlatformBusTest, EmptyPartitionMapMetadata) {
  const std::array<uint8_t, 16> kGuid = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};

  constexpr std::string_view kNodeName = "test-platform-device";

  const std::vector<fuchsia_hardware_platform_bus::BootMetadata> kBootMetadata{
      {{
          .zbi_type = ZBI_TYPE_DRV_PARTITION_MAP,
          .zbi_extra = 0,
      }},
  };

  const fuchsia_hardware_platform_bus::Node kNode{
      {.name{kNodeName}, .boot_metadata = kBootMetadata}};

  zbi_partition_map_t boot_item_partition_map{
      .block_count = 0,
      .block_size = 0,
      .partition_count = 0,
      .reserved = 0,
  };
  std::ranges::copy(kGuid, boot_item_partition_map.guid);
  SetPartitionMapBootItem(boot_item_partition_map, {});

  // Create a platform device that should serve the partition map as metadata.
  fdf::Arena arena{'PBUS'};
  fdf::WireUnownedResult result = pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kNode));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());

  // Verify that the platform device serves the partition map as metadata.
  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(
      driver_test().ConnectToDriverSvcDir(), kNodeName);
  ASSERT_OK(metadata);
  const auto& partition_map = metadata.value();
  EXPECT_TRUE(partition_map.block_count().has_value());
  EXPECT_EQ(partition_map.block_count().value(), 0u);
  EXPECT_TRUE(partition_map.block_size().has_value());
  EXPECT_EQ(partition_map.block_size().value(), 0u);
  EXPECT_TRUE(partition_map.guid().has_value());
  EXPECT_THAT(partition_map.guid().value(), ::testing::ElementsAreArray(kGuid));
  EXPECT_TRUE(partition_map.partitions().has_value());
  EXPECT_TRUE(partition_map.partitions().value().empty());
}

// Verify that the platform bus can create a platform device that exposes a non-empty partition map
// found in boot args as metadata.
TEST_F(PlatformBusTest, PartitionMapMetadata) {
  const std::array<uint8_t, 16> kGuid = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};

  constexpr std::string_view kNodeName = "test-platform-device";

  const std::vector<fuchsia_hardware_platform_bus::BootMetadata> kBootMetadata{
      {{
          .zbi_type = ZBI_TYPE_DRV_PARTITION_MAP,
          .zbi_extra = 0,
      }},
  };

  const fuchsia_hardware_platform_bus::Node kNode{
      {.name{kNodeName}, .boot_metadata = kBootMetadata}};

  const std::array<uint8_t, 16> kPartition1TypeGuid = {15, 14, 13, 12, 11, 10, 9, 8,
                                                       7,  6,  5,  4,  3,  2,  1, 0};
  const std::array<uint8_t, 16> kPartition1UniqueGuid = {1, 1, 1, 1, 1, 1, 1, 1,
                                                         1, 1, 1, 1, 1, 1, 1, 1};

  const std::array<uint8_t, 16> kPartition2TypeGuid = {1, 2, 3, 4, 1, 2, 3, 4,
                                                       1, 2, 3, 4, 1, 2, 3, 4};
  const std::array<uint8_t, 16> kPartition2UniqueGuid = {4, 3, 2, 1, 4, 3, 2, 1,
                                                         4, 3, 2, 1, 4, 3, 2, 1};

  std::array<zbi_partition_t, 2> boot_item_partitions = {
      zbi_partition_t{.first_block = 1, .last_block = 3, .flags = 3, .name = "partition 1"},
      zbi_partition_t{.first_block = 4, .last_block = 10, .flags = 255, .name = "partition 2"}};

  std::ranges::copy(kPartition1TypeGuid, boot_item_partitions[0].type_guid);
  std::ranges::copy(kPartition1UniqueGuid, boot_item_partitions[0].uniq_guid);
  std::ranges::copy(kPartition2TypeGuid, boot_item_partitions[1].type_guid);
  std::ranges::copy(kPartition2UniqueGuid, boot_item_partitions[1].uniq_guid);

  zbi_partition_map_t boot_item_partition_map{
      .block_count = 10,
      .block_size = 4,
      .partition_count = 2,
      .reserved = 0,
  };
  std::ranges::copy(kGuid, boot_item_partition_map.guid);
  SetPartitionMapBootItem(boot_item_partition_map, boot_item_partitions);

  // Create a platform device that should serve the partition map as metadata.
  fdf::Arena arena{'PBUS'};
  fdf::WireUnownedResult result = pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kNode));
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());

  // Verify that the platform device serves the partition map as metadata.
  zx::result metadata = fdf_metadata::GetMetadata<fuchsia_boot_metadata::PartitionMap>(
      driver_test().ConnectToDriverSvcDir(), kNodeName);
  ASSERT_OK(metadata);
  const auto& partition_map = metadata.value();
  EXPECT_TRUE(partition_map.block_count().has_value());
  EXPECT_EQ(partition_map.block_count().value(), 10u);
  EXPECT_TRUE(partition_map.block_size().has_value());
  EXPECT_EQ(partition_map.block_size().value(), 4u);
  EXPECT_TRUE(partition_map.guid().has_value());
  EXPECT_THAT(partition_map.guid().value(), ::testing::ElementsAreArray(kGuid));
  EXPECT_TRUE(partition_map.partitions().has_value());
  const auto& partitions = partition_map.partitions().value();
  EXPECT_EQ(partitions.size(), 2u);

  const auto& partition1 = partitions[0];
  EXPECT_EQ(partition1.first_block(), 1u);
  EXPECT_EQ(partition1.last_block(), 3u);
  EXPECT_EQ(partition1.flags(), 3u);
  EXPECT_EQ(partition1.name(), "partition 1");
  EXPECT_THAT(partition1.type_guid(), ::testing::ElementsAreArray(kPartition1TypeGuid));
  EXPECT_THAT(partition1.unique_guid(), ::testing::ElementsAreArray(kPartition1UniqueGuid));

  const auto& partition2 = partitions[1];
  EXPECT_EQ(partition2.first_block(), 4u);
  EXPECT_EQ(partition2.last_block(), 10u);
  EXPECT_EQ(partition2.flags(), 255u);
  EXPECT_EQ(partition2.name(), "partition 2");
  EXPECT_THAT(partition2.type_guid(), ::testing::ElementsAreArray(kPartition2TypeGuid));
  EXPECT_THAT(partition2.unique_guid(), ::testing::ElementsAreArray(kPartition2UniqueGuid));
}

TEST_F(PlatformBusTest, UserspaceInterrupts) {
  constexpr std::string_view kControllerName = "fake-interrupt-controller";
  const fuchsia_hardware_platform_bus::Node kControllerNode{{
      .name{kControllerName},
      .interrupt_controller_id = 1,
  }};

  fdf::Arena arena{'PBUS'};
  fdf::WireUnownedResult result =
      pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kControllerNode));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());

  zx::result registry_client_end =
      driver_test().Connect<fuchsia_hardware_interrupt::ControllerRegistryService::Registry>(
          kControllerName);
  ASSERT_TRUE(registry_client_end.is_ok());

  driver_test().RunInEnvironmentTypeContext(
      [registry_client_end = std::move(registry_client_end)](TestEnvironment& env) mutable {
        env.fake_controller().RegisterController(std::move(registry_client_end.value()));
      });

  constexpr std::string_view kDeviceName = "test-device";
  fuchsia_hardware_platform_bus::UserspaceIrq userspace_irq{{
      .irq = 10,
      .controller_id = 1,
  }};

  const fuchsia_hardware_platform_bus::Node kDeviceNode{{
      .name{kDeviceName},
      .irq = std::vector<fuchsia_hardware_platform_bus::Irq>{{
          {
              .irq = fuchsia_hardware_platform_bus::IrqSpec::WithUserspaceIrq(
                  std::move(userspace_irq)),
              .mode = fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh,
          },
      }},
  }};

  result = pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kDeviceNode));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());

  zx::result device_client_end =
      driver_test().Connect<fuchsia_hardware_platform_device::Service::Device>(kDeviceName);
  ASSERT_OK(device_client_end);

  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> device(
      std::move(device_client_end.value()));

  fidl::WireResult get_interrupt_result = device->GetInterruptById(0, ZX_INTERRUPT_TIMESTAMP_MONO);
  ASSERT_TRUE(get_interrupt_result.ok());
  EXPECT_TRUE(get_interrupt_result->is_ok());

  std::vector<FakeInterruptController::RegisteredInterrupt> registered_interrupts;
  // Interrupts are registered on the environment dispatcher, which may not have finished processing
  // the registration request by the time GetInterruptById() returns.
  driver_test().runtime().RunUntil([&]() {
    registered_interrupts =
        driver_test()
            .RunInEnvironmentTypeContext<std::vector<FakeInterruptController::RegisteredInterrupt>>(
                [](TestEnvironment& env) {
                  return env.fake_controller().take_registered_interrupts();
                });
    return !registered_interrupts.empty();
  });

  ASSERT_EQ(registered_interrupts.size(), 1u);
  const FakeInterruptController::RegisteredInterrupt& interrupt = registered_interrupts[0];

  EXPECT_EQ(interrupt.irq, 10u);
  EXPECT_EQ(interrupt.mode, fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh);
  EXPECT_EQ(interrupt.options, fuchsia_hardware_interrupt::InterruptOptions::kTimestampMono);

  // Verify that the interrupt controller and interrupt consumer got handles to the same interrupt
  // object.
  zx_info_handle_basic_t info{};
  EXPECT_OK(get_interrupt_result->value()->irq.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info),
                                                        nullptr, nullptr));
  const zx_koid_t expected_koid = info.koid;

  EXPECT_OK(
      interrupt.interrupt.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.koid, expected_koid);
}

TEST_F(PlatformBusTest, GetUserspaceInterruptDeferred) {
  constexpr std::string_view kDeviceName = "test-device";
  fuchsia_hardware_platform_bus::UserspaceIrq userspace_irq{{
      .irq = 10,
      .controller_id = 1,
  }};

  const fuchsia_hardware_platform_bus::Node kDeviceNode{{
      .name{kDeviceName},
      .irq = std::vector<fuchsia_hardware_platform_bus::Irq>{{
          {
              .irq = fuchsia_hardware_platform_bus::IrqSpec::WithUserspaceIrq(
                  std::move(userspace_irq)),
              .mode = fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh,
          },
      }},
  }};

  // This time, add the interrupt consumer before adding the controller.
  fdf::Arena arena{'PBUS'};
  fdf::WireUnownedResult result = pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kDeviceNode));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());

  zx::result device_client_end =
      driver_test().Connect<fuchsia_hardware_platform_device::Service::Device>(kDeviceName);
  ASSERT_OK(device_client_end);

  fidl::WireClient<fuchsia_hardware_platform_device::Device> device(
      std::move(device_client_end.value()), fdf::Dispatcher::GetCurrent()->async_dispatcher());

  zx::interrupt client_interrupt;
  device->GetInterruptById(0, ZX_INTERRUPT_TIMESTAMP_MONO)
      .ThenExactlyOnce(
          [&](fidl::WireUnownedResult<fuchsia_hardware_platform_device::Device::GetInterruptById>&
                  result) {
            ASSERT_TRUE(result.ok());
            EXPECT_TRUE(result->is_ok());
            client_interrupt = std::move(result->value()->irq);
          });

  constexpr std::string_view kControllerName = "fake-interrupt-controller";
  const fuchsia_hardware_platform_bus::Node kControllerNode{{
      .name{kControllerName},
      .interrupt_controller_id = 1,
  }};

  result = pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kControllerNode));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_ok());

  zx::result registry_client_end =
      driver_test().Connect<fuchsia_hardware_interrupt::ControllerRegistryService::Registry>(
          kControllerName);
  ASSERT_TRUE(registry_client_end.is_ok());

  driver_test().RunInEnvironmentTypeContext(
      [registry_client_end = std::move(registry_client_end)](TestEnvironment& env) mutable {
        env.fake_controller().RegisterController(std::move(registry_client_end.value()));
      });

  // GetInterruptById() should have been completed by platform-bus after registering the controller.
  driver_test().runtime().RunUntil([&]() { return client_interrupt.is_valid(); });

  std::vector<FakeInterruptController::RegisteredInterrupt> registered_interrupts;
  driver_test().runtime().RunUntil([&]() {
    registered_interrupts =
        driver_test()
            .RunInEnvironmentTypeContext<std::vector<FakeInterruptController::RegisteredInterrupt>>(
                [](TestEnvironment& env) {
                  return env.fake_controller().take_registered_interrupts();
                });
    return !registered_interrupts.empty();
  });

  ASSERT_EQ(registered_interrupts.size(), 1u);
  const FakeInterruptController::RegisteredInterrupt& interrupt = registered_interrupts[0];

  EXPECT_EQ(interrupt.irq, 10u);
  EXPECT_EQ(interrupt.mode, fuchsia_hardware_platform_bus::ZirconInterruptMode::kEdgeHigh);
  EXPECT_EQ(interrupt.options, fuchsia_hardware_interrupt::InterruptOptions::kTimestampMono);

  zx_info_handle_basic_t info{};
  EXPECT_OK(client_interrupt.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  const zx_koid_t expected_koid = info.koid;

  EXPECT_OK(
      interrupt.interrupt.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.koid, expected_koid);
}

TEST_F(PlatformBusTest, InterruptControllerDisconnect) {
  constexpr std::string_view kControllerName = "fake-interrupt-controller";
  const fuchsia_hardware_platform_bus::Node kControllerNode{{
      .name{kControllerName},
      .interrupt_controller_id = 1,
  }};

  fdf::Arena arena{'PBUS'};
  fdf::WireUnownedResult add_result =
      pbus().buffer(arena)->NodeAdd(fidl::ToWire(arena, kControllerNode));
  ASSERT_TRUE(add_result.ok());
  EXPECT_TRUE(add_result->is_ok());

  zx::result registry_client_end =
      driver_test().Connect<fuchsia_hardware_interrupt::ControllerRegistryService::Registry>(
          kControllerName);
  ASSERT_TRUE(registry_client_end.is_ok());

  fidl::WireSyncClient<fuchsia_hardware_interrupt::ControllerRegistry> registry(
      std::move(registry_client_end.value()));

  auto [client_end1, server_end1] =
      fidl::Endpoints<fuchsia_hardware_interrupt::Controller>::Create();

  // Register first controller.
  {
    auto result = registry->RegisterController(std::move(client_end1));
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  }

  auto [client_end2, server_end2] =
      fidl::Endpoints<fuchsia_hardware_interrupt::Controller>::Create();

  // Trying to register another one with same ID should fail.
  {
    auto result = registry->RegisterController(std::move(client_end2));
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_error());
    EXPECT_EQ(result->error_value(), ZX_ERR_ALREADY_BOUND);
  }

  // Now disconnect the first one.
  server_end1.reset();

  // Wait for the disconnect to propagate and register again.
  driver_test().runtime().RunUntil([&registry]() {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_hardware_interrupt::Controller>::Create();
    auto result = registry->RegisterController(std::move(client_end));
    return result.ok() && result->is_ok();
  });
}

TEST(PlatformBusTest2, GetMmioIndex) {
  const std::vector<fuchsia_hardware_platform_bus::Mmio> mmios{
      {{
          .base = 1,
          .length = 2,
          .name = "first",
      }},
      {{
          .base = 3,
          .length = 4,
          .name = "second",
      }},
  };

  fuchsia_hardware_platform_bus::Node node = {};
  node.mmio() = mmios;

  {
    auto result = platform_bus::GetMmioIndex(node, "first");
    ASSERT_TRUE(result.has_value());
    ASSERT_EQ(result.value(), 0u);
  }
  {
    auto result = platform_bus::GetMmioIndex(node, "second");
    ASSERT_TRUE(result.has_value());
    ASSERT_EQ(result.value(), 1u);
  }
  {
    auto result = platform_bus::GetMmioIndex(node, "none");
    ASSERT_FALSE(result.has_value());
  }
}

TEST(PlatformBusTest2, GetMmioIndexNoMmios) {
  fuchsia_hardware_platform_bus::Node node = {};
  {
    auto result = platform_bus::GetMmioIndex(node, "none");
    ASSERT_FALSE(result.has_value());
  }
}

}  // namespace

__EXPORT
zx_status_t zx_bti_create(zx_handle_t handle, uint32_t options, uint64_t bti_id, zx_handle_t* out) {
  return fake_bti_create(out);
}
