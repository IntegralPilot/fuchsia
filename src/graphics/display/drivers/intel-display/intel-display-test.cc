// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/intel-display.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/mmio-ptr/fake.h>

#include <gtest/gtest.h>

#include "src/devices/pci/testing/pci_protocol_fake.h"
#include "src/graphics/display/drivers/intel-display/registers.h"
#include "src/graphics/display/drivers/intel-display/testing/fake-buffer-collection.h"
#include "src/graphics/display/drivers/intel-display/testing/mock-allocator.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/predicates/status.h"

namespace {

constexpr uint32_t kBytesPerRowDivisor = 1024;

}  // namespace

namespace intel_display {

namespace {

// Test fixture for tests that only uses fake sysmem but doesn't have any
// other dependency, so that we won't need a fully-fledged device tree.
class FakeSysmemSingleThreadedTest : public testing::Test {
 public:
  FakeSysmemSingleThreadedTest()
      : loop_(&kAsyncLoopConfigAttachToCurrentThread),
        sysmem_(loop_.dispatcher()),
        display_(&engine_events_, inspect::Inspector{}) {}

  void SetUp() override {
    auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    fidl::BindServer(loop_.dispatcher(), std::move(sysmem_server), &sysmem_);

    sysmem_.SetNewBufferCollectionConfig({
        .cpu_domain_supported = false,
        .ram_domain_supported = true,
        .inaccessible_domain_supported = false,
        .bytes_per_row_divisor = kBytesPerRowDivisor,
        .format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear,
    });

    ASSERT_OK(display_.SetAndInitSysmemForTesting(fidl::WireSyncClient(std::move(sysmem_client))));
    EXPECT_OK(loop_.RunUntilIdle());
  }

  void TearDown() override {
    // Shutdown the loop before destroying the FakeSysmem and MockAllocator which
    // may still have pending callbacks.
    loop_.Shutdown();
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  async::Loop loop_;

  MockAllocator sysmem_;

  // Must outlive `display_`.
  display::DisplayEngineEventsFidl engine_events_;
  Controller display_;
};

using ControllerWithFakeSysmemTest = FakeSysmemSingleThreadedTest;

TEST_F(ControllerWithFakeSysmemTest, ImportBufferCollection) {
  const MockAllocator& allocator = sysmem_;

  zx::result token1_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token1_endpoints.is_ok());
  zx::result token2_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token2_endpoints.is_ok());

  // Test ImportBufferCollection().
  constexpr display::DriverBufferCollectionId kValidBufferCollectionId(1);
  EXPECT_OK(display_.ImportBufferCollection(kValidBufferCollectionId,
                                            std::move(token1_endpoints->client)));

  // `collection_id` must be unused.
  EXPECT_STATUS(display_.ImportBufferCollection(kValidBufferCollectionId,
                                                std::move(token2_endpoints->client)),
                zx::error(ZX_ERR_ALREADY_EXISTS));

  loop_.RunUntilIdle();

  // Verify that the current buffer collection token is used.
  {
    auto active_buffer_token_clients = allocator.GetActiveBufferCollectionTokenClients();
    EXPECT_EQ(active_buffer_token_clients.size(), 1u);

    auto inactive_buffer_token_clients = allocator.GetInactiveBufferCollectionTokenClients();
    EXPECT_EQ(inactive_buffer_token_clients.size(), 0u);

    auto [client_koid, client_related_koid] =
        fsl::GetKoids(active_buffer_token_clients[0].channel()->get());
    auto [server_koid, server_related_koid] =
        fsl::GetKoids(token1_endpoints->server.channel().get());

    EXPECT_NE(client_koid, ZX_KOID_INVALID);
    EXPECT_NE(client_related_koid, ZX_KOID_INVALID);
    EXPECT_NE(server_koid, ZX_KOID_INVALID);
    EXPECT_NE(server_related_koid, ZX_KOID_INVALID);

    EXPECT_EQ(client_koid, server_related_koid);
    EXPECT_EQ(server_koid, client_related_koid);
  }

  // Test ReleaseBufferCollection().
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(2);
  EXPECT_STATUS(display_.ReleaseBufferCollection(kInvalidBufferCollectionId),
                zx::error(ZX_ERR_NOT_FOUND));
  EXPECT_OK(display_.ReleaseBufferCollection(kValidBufferCollectionId));

  loop_.RunUntilIdle();

  // Verify that the current buffer collection token is released.
  {
    auto active_buffer_token_clients = allocator.GetActiveBufferCollectionTokenClients();
    EXPECT_EQ(active_buffer_token_clients.size(), 0u);

    auto inactive_buffer_token_clients = allocator.GetInactiveBufferCollectionTokenClients();
    EXPECT_EQ(inactive_buffer_token_clients.size(), 1u);

    auto [client_koid, client_related_koid] =
        fsl::GetKoids(inactive_buffer_token_clients[0].channel()->get());
    auto [server_koid, server_related_koid] =
        fsl::GetKoids(token1_endpoints->server.channel().get());

    EXPECT_NE(client_koid, ZX_KOID_INVALID);
    EXPECT_NE(client_related_koid, ZX_KOID_INVALID);
    EXPECT_NE(server_koid, ZX_KOID_INVALID);
    EXPECT_NE(server_related_koid, ZX_KOID_INVALID);

    EXPECT_EQ(client_koid, server_related_koid);
    EXPECT_EQ(server_koid, client_related_koid);
  }
}

fdf::MmioBuffer MakeMmioBuffer(uint8_t* buffer, size_t size) {
  return fdf::MmioBuffer({
      .vaddr = FakeMmioPtr(buffer),
      .offset = 0,
      .size = size,
      .vmo = ZX_HANDLE_INVALID,
  });
}

TEST(IntelDisplay, ImportImage) {
  fdf_testing::ScopedGlobalLogger logger;
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  loop.StartThread("fidl-loop");

  // Prepare fake sysmem.
  MockAllocator fake_sysmem(loop.dispatcher());
  auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
  fidl::BindServer(loop.dispatcher(), std::move(sysmem_server), &fake_sysmem);

  // Prepare fake PCI.
  pci::FakePciProtocol fake_pci;
  ddk::Pci pci = fake_pci.SetUpFidlServer(loop);

  fake_sysmem.SetNewBufferCollectionConfig({
      .cpu_domain_supported = false,
      .ram_domain_supported = true,
      .inaccessible_domain_supported = false,

      .width_fallback_px = 32,
      .height_fallback_px = 32,
      .bytes_per_row_divisor = kBytesPerRowDivisor,
      .format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear,
  });

  // Initialize display controller and sysmem allocator.

  // Must outlive `display`.
  display::DisplayEngineEventsFidl display_events;
  Controller display(&display_events, inspect::Inspector{});
  ASSERT_OK(display.SetAndInitSysmemForTesting(fidl::WireSyncClient(std::move(sysmem_client))));

  // Initialize the GTT to the smallest allowed size (which is 2MB with the |gtt_size| bits of the
  // graphics control register set to 0x01.
  constexpr size_t kGraphicsTranslationTableSizeBytes = (1 << 21);
  ASSERT_OK(pci.WriteConfig16(registers::GmchGfxControl::kAddr,
                              registers::GmchGfxControl().set_gtt_size(0x01).reg_value()));
  auto buffer = std::make_unique<uint8_t[]>(kGraphicsTranslationTableSizeBytes);
  memset(buffer.get(), 0, kGraphicsTranslationTableSizeBytes);
  fdf::MmioBuffer mmio = MakeMmioBuffer(buffer.get(), kGraphicsTranslationTableSizeBytes);
  ASSERT_OK(display.InitGttForTesting(pci, std::move(mmio), /*fb_offset=*/0));

  // Import buffer collection.
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  zx::result token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token_endpoints.is_ok());
  EXPECT_OK(
      display.ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints->client)));

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display.SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  // Invalid import: bad collection id
  static constexpr display::ImageMetadata kDisplayImageMetadata = {{
      .width = 32,
      .height = 32,
      .tiling_type = display::ImageTilingType::kLinear,
  }};
  static constexpr display::DriverBufferCollectionId kInvalidCollectionId(100);

  zx::result<display::DriverImageId> import_result =
      display.ImportImage(kDisplayImageMetadata, kInvalidCollectionId, 0);
  EXPECT_STATUS(import_result, zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: bad index
  static constexpr uint32_t kInvalidIndex = 100;
  import_result = display.ImportImage(kDisplayImageMetadata, kBufferCollectionId, kInvalidIndex);
  EXPECT_STATUS(import_result, zx::error(ZX_ERR_OUT_OF_RANGE));

  // Valid import
  import_result = display.ImportImage(kDisplayImageMetadata, kBufferCollectionId, 0);
  ASSERT_OK(import_result);

  display::DriverImageId image_id = import_result.value();
  EXPECT_NE(image_id.value(), 0u);

  display.ReleaseImage(image_id);

  // Release buffer collection.
  EXPECT_OK(display.ReleaseBufferCollection(kBufferCollectionId));

  // Shutdown the loop before destroying the FakeSysmem and MockAllocator which
  // may still have pending callbacks.
  loop.Shutdown();
}

TEST_F(ControllerWithFakeSysmemTest, SysmemRequirements) {
  zx::result token_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token_endpoints.is_ok());

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  EXPECT_OK(
      display_.ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints->client)));

  loop_.RunUntilIdle();

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display_.SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  loop_.RunUntilIdle();

  MockAllocator& allocator = sysmem_;
  FakeBufferCollection* collection = allocator.GetMostRecentBufferCollection();
  ASSERT_TRUE(collection);
  EXPECT_TRUE(collection->HasConstraints());
}

}  // namespace

}  // namespace intel_display
