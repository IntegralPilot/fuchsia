// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/intel-display-driver.h"

#include <fidl/fuchsia.kernel/cpp/test_base.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/test_base.h>
#include <fuchsia/hardware/intelgpucore/c/banjo.h>
#include <fuchsia/hardware/intelgpucore/cpp/banjo.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/ddk/driver.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-resource/resource.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zircon-internal/align.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <limits>
#include <string>
#include <utility>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/intel/platform/gpucore/cpp/bind.h>
#include <ddktl/device.h>
#include <gtest/gtest.h>

#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/intel-display.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"
#include "src/graphics/display/drivers/intel-display/registers.h"
#include "src/graphics/display/drivers/intel-display/testing/fake-buffer-collection.h"
#include "src/graphics/display/drivers/intel-display/testing/fake-framebuffer.h"
#include "src/graphics/display/drivers/intel-display/testing/mock-allocator.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/lib/testing/predicates/status.h"

namespace {

constexpr uint32_t kBytesPerRowDivisor = 1024;

}  // namespace

namespace intel_display {

namespace {

zx::resource CreateFakeMmioRootResource() {
  zx::resource root;
  zx_status_t status = fake_resource_create(ZX_RSRC_KIND_MMIO, root.reset_and_get_address());
  ZX_ASSERT(status == ZX_OK);
  return root;
}

zx::resource CreateFakeIoportRootResource() {
  zx::resource root;
  zx_status_t status = fake_resource_create(ZX_RSRC_KIND_IOPORT, root.reset_and_get_address());
  ZX_ASSERT(status == ZX_OK);
  return root;
}

class FakeSystemStateTransition
    : public fidl::testing::TestBase<fuchsia_system_state::SystemStateTransition> {
 public:
  FakeSystemStateTransition() = default;
  ~FakeSystemStateTransition() override = default;

  // fidl::testing::TestBase:
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("Not implemented: %s", name.c_str());
  }

  // fuchsia_system_state::SystemStateTransition:
  void GetTerminationSystemState(GetTerminationSystemStateCompleter::Sync& completer) override {
    completer.Reply({termination_system_state_});
  }

  void SetTerminationSystemState(fuchsia_system_state::SystemPowerState termination_system_state) {
    termination_system_state_ = termination_system_state;
  }

 private:
  fuchsia_system_state::SystemPowerState termination_system_state_ =
      fuchsia_system_state::SystemPowerState::kFullyOn;
};

class FakeMmioResource : public fidl::testing::TestBase<fuchsia_kernel::MmioResource> {
 public:
  explicit FakeMmioResource(zx::unowned_resource root_resource)
      : root_resource_(root_resource->borrow()) {}
  ~FakeMmioResource() override = default;

  // fidl::testing::TestBase:
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("Not implemented: %s", name.c_str());
  }

  void Get(GetCompleter::Sync& completer) override {
    zx::resource mmio_child;
    std::array<char, ZX_MAX_NAME_LEN> child_name = {"child"};
    zx_status_t status =
        zx_resource_create(root_resource_->get(), ZX_RSRC_KIND_MMIO, 16, 32, child_name.data(),
                           child_name.size(), mmio_child.reset_and_get_address());
    ZX_ASSERT(status == ZX_OK);
    completer.Reply(std::move(mmio_child));
  }

 private:
  zx::unowned_resource root_resource_;
};

class FakeIoportResource : public fidl::testing::TestBase<fuchsia_kernel::IoportResource> {
 public:
  explicit FakeIoportResource(zx::unowned_resource root_resource)
      : root_resource_(root_resource->borrow()) {}
  ~FakeIoportResource() override = default;

  // fidl::testing::TestBase:
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ZX_PANIC("Not implemented: %s", name.c_str());
  }

  void Get(GetCompleter::Sync& completer) override {
    zx::resource ioport_child;
    std::array<char, ZX_MAX_NAME_LEN> child_name = {"child"};
    zx_status_t status =
        zx_resource_create(root_resource_->get(), ZX_RSRC_KIND_IOPORT, 32, 64, child_name.data(),
                           child_name.size(), ioport_child.reset_and_get_address());
    ZX_ASSERT(status == ZX_OK);
    completer.Reply(std::move(ioport_child));
  }

 private:
  zx::unowned_resource root_resource_;
};

class IntelDisplayTestEnvironment : public fdf_testing::Environment {
 public:
  IntelDisplayTestEnvironment()
      : fake_mmio_root_resource_(CreateFakeMmioRootResource()),
        fake_ioport_root_resource_(CreateFakeIoportRootResource()),
        fake_mmio_resource_(fake_mmio_root_resource_.borrow()),
        fake_ioport_resource_(fake_ioport_root_resource_.borrow()),
        sysmem_(fdf::Dispatcher::GetCurrent()->async_dispatcher()) {
    sysmem_.SetNewBufferCollectionConfig({
        .cpu_domain_supported = false,
        .ram_domain_supported = true,
        .inaccessible_domain_supported = false,
        .bytes_per_row_divisor = kBytesPerRowDivisor,
        .format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear,
    });
    boot_items_.SetFramebuffer({});
    SetUpFakePci();
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    zx::result<> add_sysmem_result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_sysmem2::Allocator>(
            sysmem_.bind_handler(dispatcher));
    if (add_sysmem_result.is_error()) {
      return add_sysmem_result.take_error();
    }

    zx::result<> add_boot_items_result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_boot::Items>(
            boot_items_.CreateHandler(dispatcher));
    if (add_boot_items_result.is_error()) {
      return add_boot_items_result.take_error();
    }

    zx::result<> add_pci_result = to_driver_vfs.AddService<fuchsia_hardware_pci::Service>(
        fuchsia_hardware_pci::Service::InstanceHandler({.device = pci_.bind_handler(dispatcher)}),
        "pci");
    if (add_pci_result.is_error()) {
      return add_pci_result.take_error();
    }

    zx::result<> add_mmio_resource_result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_kernel::MmioResource>(
            fake_mmio_resource_.bind_handler(dispatcher));
    if (add_mmio_resource_result.is_error()) {
      return add_mmio_resource_result.take_error();
    }

    zx::result<> add_ioport_resource_result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_kernel::IoportResource>(
            fake_ioport_resource_.bind_handler(dispatcher));
    if (add_ioport_resource_result.is_error()) {
      return add_ioport_resource_result.take_error();
    }

    zx::result<> add_system_state_transition_result =
        to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_system_state::SystemStateTransition>(
            fake_system_state_transition_.bind_handler(dispatcher));
    if (add_system_state_transition_result.is_error()) {
      return add_system_state_transition_result.take_error();
    }

    return zx::ok();
  }

  void SetUpFakePci() {
    pci_.CreateBar(0u, std::numeric_limits<uint32_t>::max(), /*is_mmio=*/true);
    pci_.AddLegacyInterrupt();
    // This configures the "GMCH Graphics Control" register to report 2MB for the available GTT
    // Graphics Memory. All other bits of this register are set to zero and should get populated as
    // required for the tests below.
    pci_.PciWriteConfig16(registers::GmchGfxControl::kAddr, 0x40);
    constexpr uint16_t kIntelVendorId = 0x8086;
    pci_.SetDeviceInfo({
        .vendor_id = kIntelVendorId,
        .device_id = kTestDeviceDid,
    });
  }

  MockAllocator& sysmem() { return sysmem_; }
  fake_framebuffer::FakeBootItems& boot_items() { return boot_items_; }
  pci::FakePciProtocol& pci() { return pci_; }

 private:
  zx::resource fake_mmio_root_resource_;
  zx::resource fake_ioport_root_resource_;
  FakeMmioResource fake_mmio_resource_;
  FakeIoportResource fake_ioport_resource_;
  FakeSystemStateTransition fake_system_state_transition_;

  MockAllocator sysmem_;
  fake_framebuffer::FakeBootItems boot_items_;
  pci::FakePciProtocol pci_;
};

struct IntelDisplayTestConfig {
  using DriverType = IntelDisplayDriver;
  using EnvironmentType = IntelDisplayTestEnvironment;
};

class IntegrationTest : public ::testing::Test {
 public:
  void SetUp() override {}
  void TearDown() override {}

 protected:
  fdf_testing::BackgroundDriverTest<IntelDisplayTestConfig> driver_test_;
};

struct DeviceNode {
  std::string name;
  std::vector<fuchsia_driver_framework::NodeProperty2> properties;
};

bool IsDisplayControllerNode(const DeviceNode& node) {
  return node.name == "intel-display-controller";
}

bool IsIntelGpuCoreNode(const DeviceNode& node) {
  const std::vector<fuchsia_driver_framework::NodeProperty2>& properties = node.properties;
  return properties.end() !=
         std::find_if(properties.begin(), properties.end(),
                      [](const fuchsia_driver_framework::NodeProperty2& property) {
                        if (!property.value().int_value().has_value())
                          return false;
                        return property.key() == bind_fuchsia::PROTOCOL &&
                               property.value().int_value().value() ==
                                   bind_fuchsia_intel_platform_gpucore::BIND_PROTOCOL_DEVICE;
                      });
}

TEST_F(IntegrationTest, BindAndInit) {
  zx::result<> start_result = driver_test_.StartDriver();
  ASSERT_OK(start_result);

  std::vector<DeviceNode> nodes =
      driver_test_.RunInNodeContext<std::vector<DeviceNode>>([](fdf_testing::TestNode& root) {
        std::vector<DeviceNode> nodes;
        for (auto& [name, node] : root.children()) {
          nodes.push_back({
              .name = name,
              .properties = node.GetProperties(),
          });
        }
        return nodes;
      });

  // There should be two published node: one "intel-display-controller" node
  // and a child "intel-gpu-core" node.
  ASSERT_EQ(nodes.size(), 2u);

  auto display_controller_node_it =
      std::find_if(nodes.begin(), nodes.end(), IsDisplayControllerNode);
  ASSERT_NE(display_controller_node_it, nodes.end());
  fdf::info("Display controller node is: {}", display_controller_node_it->name);

  auto intel_gpu_core_node_it = std::find_if(nodes.begin(), nodes.end(), IsIntelGpuCoreNode);
  ASSERT_NE(intel_gpu_core_node_it, nodes.end());
  fdf::info("Intel GPU node is: {}", intel_gpu_core_node_it->name);

  ASSERT_NE(display_controller_node_it, intel_gpu_core_node_it);

  zx::result<> stop_result = driver_test_.StopDriver();
  EXPECT_OK(stop_result);
}

// Tests that the device can initialize even if bootloader framebuffer information is not available
// and global GTT allocations start at offset 0.
TEST_F(IntegrationTest, InitFailsIfBootloaderGetInfoFails) {
  driver_test_.RunInEnvironmentTypeContext([](IntelDisplayTestEnvironment& env) {
    env.boot_items().SetFramebuffer({.status = ZX_ERR_INVALID_ARGS});
  });

  zx::result<> start_result = driver_test_.StartDriver();
  ASSERT_OK(start_result);

  zx::result<ddk::AnyProtocol> gpu_protocol_result =
      driver_test_.RunInDriverContext<zx::result<ddk::AnyProtocol>>([](IntelDisplayDriver& driver) {
        return driver.GetProtocol(ZX_PROTOCOL_INTEL_GPU_CORE);
      });
  ASSERT_OK(gpu_protocol_result);
  ddk::AnyProtocol gpu_protocol = std::move(gpu_protocol_result).value();
  ddk::IntelGpuCoreProtocolClient gpu(
      reinterpret_cast<const intel_gpu_core_protocol_t*>(&gpu_protocol));

  uint64_t addr;

  zx_status_t alloc_status = gpu.GttAlloc(1, &addr);
  EXPECT_OK(alloc_status);
  EXPECT_EQ(0u, addr);

  zx::result<> stop_result = driver_test_.StopDriver();
  EXPECT_OK(stop_result);
}

// TODO(https://fxbug.dev/42166779): Add tests for DisplayPort display enumeration,
// covering the following cases:
//   - Display found during start up but not already powered.
//   - Display found during start up but already powered up.
//   - Display added and removed in a hotplug event.
// TODO(https://fxbug.dev/42167311): Add test for HDMI display enumeration.
// TODO(https://fxbug.dev/42167312): Add test for DVI display enumeration.

TEST_F(IntegrationTest, GttAllocationDoesNotOverlapBootloaderFramebuffer) {
  constexpr uint32_t kStride = 1920;
  constexpr uint32_t kHeight = 1080;
  driver_test_.RunInEnvironmentTypeContext([&](IntelDisplayTestEnvironment& env) {
    env.boot_items().SetFramebuffer({
        .format = ZBI_PIXEL_FORMAT_RGB_888,
        .width = kStride,
        .height = kHeight,
        .stride = kStride,
    });
  });

  zx::result<> start_result = driver_test_.StartDriver();
  ASSERT_OK(start_result);

  zx::result<ddk::AnyProtocol> gpu_protocol_result =
      driver_test_.RunInDriverContext<zx::result<ddk::AnyProtocol>>([](IntelDisplayDriver& driver) {
        return driver.GetProtocol(ZX_PROTOCOL_INTEL_GPU_CORE);
      });
  ASSERT_OK(gpu_protocol_result);
  ddk::AnyProtocol gpu_protocol = std::move(gpu_protocol_result).value();
  ddk::IntelGpuCoreProtocolClient gpu(
      reinterpret_cast<const intel_gpu_core_protocol_t*>(&gpu_protocol));

  uint64_t addr;
  zx_status_t alloc_status = gpu.GttAlloc(1, &addr);
  EXPECT_OK(alloc_status);
  EXPECT_EQ(ZX_ROUNDUP(kHeight * kStride * 3, zx_system_get_page_size()), addr);

  zx::result<> stop_result = driver_test_.StopDriver();
  EXPECT_OK(stop_result);
}

TEST_F(IntegrationTest, SysmemImport) {
  static constexpr int kImageWidth = 128;
  static constexpr int kImageHeight = 32;
  driver_test_.RunInEnvironmentTypeContext([&](IntelDisplayTestEnvironment& env) {
    env.sysmem().SetNewBufferCollectionConfig({
        .cpu_domain_supported = false,
        .ram_domain_supported = true,
        .inaccessible_domain_supported = false,

        .width_fallback_px = kImageWidth,
        .height_fallback_px = kImageHeight,
        .bytes_per_row_divisor = kBytesPerRowDivisor,
        .format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear,
    });
  });

  zx::result<> start_result = driver_test_.StartDriver();
  ASSERT_OK(start_result);

  zx::result<fdf::ClientEnd<fuchsia_hardware_display_engine::Engine>> connect_engine_result =
      driver_test_.Connect<fuchsia_hardware_display_engine::Service::Engine>();
  ASSERT_OK(connect_engine_result);

  fdf::WireSyncClient display(std::move(connect_engine_result).value());

  // Import buffer collection.
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  auto [token_client, token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportBufferCollection>
      import_fidl_transport_result = display.buffer(arena)->ImportBufferCollection(
          kBufferCollectionId.ToFidl(), std::move(token_client));
  ASSERT_TRUE(import_fidl_transport_result.ok());

  fit::result<zx_status_t> import_fidl_domain_result = import_fidl_transport_result.value();
  ASSERT_TRUE(import_fidl_domain_result.is_ok());

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });

  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetBufferCollectionConstraints>
      set_constraints_fidl_transport_result = display.buffer(arena)->SetBufferCollectionConstraints(
          kDisplayUsage.ToFidl(), kBufferCollectionId.ToFidl());
  ASSERT_TRUE(set_constraints_fidl_transport_result.ok());

  fit::result<zx_status_t> set_constraints_fidl_domain_result =
      set_constraints_fidl_transport_result.value();
  ASSERT_TRUE(set_constraints_fidl_domain_result.is_ok());

  driver_test_.runtime().RunUntil([&] {
    return driver_test_.RunInEnvironmentTypeContext<bool>([](IntelDisplayTestEnvironment& env) {
      FakeBufferCollection* collection = env.sysmem().GetMostRecentBufferCollection();
      return collection && collection->HasConstraints();
    });
  });

  static constexpr display::ImageMetadata kDisplayImageMetadata({
      .width = kImageWidth,
      .height = kImageHeight,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImage>
      import_image_fidl_transport_result = display.buffer(arena)->ImportImage(
          kDisplayImageMetadata.ToFidl(), kBufferCollectionId.ToFidl(), /*index=*/0);
  ASSERT_TRUE(import_image_fidl_transport_result.ok());

  fit::result import_image_fidl_domain_result = import_image_fidl_transport_result.value();
  ASSERT_TRUE(import_image_fidl_domain_result.is_ok());
  display::DriverImageId image_id(import_image_fidl_domain_result->image_id);

  uint64_t bytes_per_row =
      driver_test_.RunInDriverContext<uint64_t>([&](IntelDisplayDriver& driver) {
        const GttRegion& region = driver.controller()->SetupGttImage(
            kDisplayImageMetadata, image_id, display::CoordinateTransformation::kIdentity);
        return region.bytes_per_row();
      });
  EXPECT_LT(kDisplayImageMetadata.dimensions().width() * 4, int32_t{kBytesPerRowDivisor});
  EXPECT_EQ(kBytesPerRowDivisor, bytes_per_row);

  fidl::OneWayStatus release_image_fidl_transport_result =
      display.buffer(arena)->ReleaseImage(image_id.ToFidl());
  ASSERT_TRUE(release_image_fidl_transport_result.ok());

  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseBufferCollection>
      release_collection_fidl_transport_result =
          display.buffer(arena)->ReleaseBufferCollection(kBufferCollectionId.ToFidl());
  ASSERT_TRUE(release_collection_fidl_transport_result.ok());

  fit::result<zx_status_t> release_collection_fidl_domain_result =
      release_collection_fidl_transport_result.value();
  ASSERT_TRUE(release_collection_fidl_domain_result.is_ok());

  zx::result<> stop_result = driver_test_.StopDriver();
  EXPECT_OK(stop_result);
}

}  // namespace

}  // namespace intel_display
