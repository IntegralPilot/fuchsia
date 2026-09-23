// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/display-engine.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fake-bti/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/compiler.h>

#include <cstdint>
#include <memory>
#include <utility>

#include <virtio/virtio.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-pci-device.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

#define USE_GTEST
#include <lib/virtio/backends/fake.h>
#undef USE_GTEST

#include <fbl/auto_lock.h>
#include <gtest/gtest.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/predicates/status.h"

namespace virtio_display {

namespace {
// Use a stub buffer collection instead of the real sysmem since some tests may
// require things that aren't available on the current system.
//
// TODO(https://fxbug.dev/42072949): Consider creating and using a unified set of sysmem
// testing doubles instead of writing mocks for each display driver test.
class StubBufferCollection : public fidl::testing::WireTestBase<fuchsia_sysmem2::BufferCollection> {
 public:
  void SetConstraints(SetConstraintsRequestView request,
                      SetConstraintsCompleter::Sync& _completer) override {
    if (!request->has_constraints()) {
      return;
    }
    auto& image_constraints = request->constraints().image_format_constraints()[0];
    EXPECT_EQ(fuchsia_images2::wire::PixelFormat::kB8G8R8A8, image_constraints.pixel_format());
    EXPECT_EQ(4u, image_constraints.bytes_per_row_divisor());
  }

  void CheckAllBuffersAllocated(CheckAllBuffersAllocatedCompleter::Sync& completer) override {
    completer.Reply(fit::ok());
  }

  void WaitForAllBuffersAllocated(WaitForAllBuffersAllocatedCompleter::Sync& completer) override {
    static constexpr uint32_t kPixelSize = 4;
    static constexpr uint32_t kRowSize = 640 * kPixelSize;
    static constexpr uint32_t kImageSize = kRowSize * 480;

    zx::vmo buffer_vmo;
    ASSERT_OK(zx::vmo::create(kImageSize, 0, &buffer_vmo));

    fuchsia_sysmem2::wire::VmoBuffer vmo_buffers[] = {
        fuchsia_sysmem2::wire::VmoBuffer::Builder(arena_)
            .vmo(std::move(buffer_vmo))
            .vmo_usable_start(0)
            .Build(),
    };

    auto buffer_collection_info_builder =
        fuchsia_sysmem2::wire::BufferCollectionInfo::Builder(arena_);
    buffer_collection_info_builder.settings(
        fuchsia_sysmem2::wire::SingleBufferSettings::Builder(arena_)
            .buffer_settings(fuchsia_sysmem2::wire::BufferMemorySettings::Builder(arena_)
                                 .coherency_domain(fuchsia_sysmem2::wire::CoherencyDomain::kRam)
                                 .is_physically_contiguous(true)
                                 .is_secure(false)
                                 .size_bytes(kImageSize)
                                 .Build())
            .image_format_constraints(
                fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena_)
                    .pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8)
                    .pixel_format_modifier(fuchsia_images2::wire::PixelFormatModifier::kLinear)
                    .min_size(fuchsia_math::wire::SizeU{.width = 640, .height = 480})
                    .min_bytes_per_row(kRowSize)
                    .bytes_per_row_divisor(4)
                    .max_size(fuchsia_math::wire::SizeU{.width = 1000, .height = 0xffffffff})
                    .max_bytes_per_row(1000 * kPixelSize)
                    .Build())
            .Build());
    buffer_collection_info_builder.buffers(vmo_buffers);

    auto response =
        fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse::Builder(arena_)
            .buffer_collection_info(buffer_collection_info_builder.Build())
            .Build();
    completer.Reply(fit::ok(&response));
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    EXPECT_TRUE(false);
  }

 private:
  fidl::Arena<fidl::kDefaultArenaInitialCapacity> arena_;
};

class MockAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  explicit MockAllocator(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
    EXPECT_TRUE(dispatcher_);
  }

  void BindSharedCollection(BindSharedCollectionRequestView request,
                            BindSharedCollectionCompleter::Sync& completer) override {
    display::DriverBufferCollectionId buffer_collection_id = next_buffer_collection_id_++;
    fbl::AutoLock lock(&lock_);
    active_buffer_collections_.emplace(
        buffer_collection_id,
        BufferCollection{.token_client = std::move(request->token()),
                         .unowned_collection_server = request->buffer_collection_request(),
                         .mock_buffer_collection = std::make_unique<StubBufferCollection>()});

    auto ref = fidl::BindServer(
        dispatcher_, std::move(request->buffer_collection_request()),
        active_buffer_collections_.at(buffer_collection_id).mock_buffer_collection.get(),
        [this, buffer_collection_id](StubBufferCollection*, fidl::UnbindInfo,
                                     fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>) {
          fbl::AutoLock lock(&lock_);
          inactive_buffer_collection_tokens_.push_back(
              std::move(active_buffer_collections_.at(buffer_collection_id).token_client));
          active_buffer_collections_.erase(buffer_collection_id);
        });
  }

  void SetDebugClientInfo(SetDebugClientInfoRequestView request,
                          SetDebugClientInfoCompleter::Sync& completer) override {
    EXPECT_EQ(request->name().get().find("virtio-gpu-display"), 0u);
  }

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetActiveBufferCollectionTokenClients() const {
    fbl::AutoLock lock(&lock_);
    std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
        unowned_token_clients;
    unowned_token_clients.reserve(active_buffer_collections_.size());

    for (const auto& kv : active_buffer_collections_) {
      unowned_token_clients.push_back(kv.second.token_client);
    }
    return unowned_token_clients;
  }

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetInactiveBufferCollectionTokenClients() const {
    fbl::AutoLock lock(&lock_);
    std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
        unowned_token_clients;
    unowned_token_clients.reserve(inactive_buffer_collection_tokens_.size());

    for (const auto& token : inactive_buffer_collection_tokens_) {
      unowned_token_clients.push_back(token);
    }
    return unowned_token_clients;
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    EXPECT_TRUE(false);
  }

 private:
  struct BufferCollection {
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_client;
    fidl::UnownedServerEnd<fuchsia_sysmem2::BufferCollection> unowned_collection_server;
    std::unique_ptr<StubBufferCollection> mock_buffer_collection;
  };

  mutable fbl::Mutex lock_;
  std::unordered_map<display::DriverBufferCollectionId, BufferCollection> active_buffer_collections_
      __TA_GUARDED(lock_);
  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
      inactive_buffer_collection_tokens_ __TA_GUARDED(lock_);

  display::DriverBufferCollectionId next_buffer_collection_id_ =
      display::DriverBufferCollectionId(0);
  async_dispatcher_t* dispatcher_ = nullptr;
};

class FakeGpuBackend : public virtio::FakeBackend {
 public:
  FakeGpuBackend() : FakeBackend({{0, 1024}, {1, 16}}) {}

  uint64_t ReadFeatures() override { return VIRTIO_F_VERSION_1; }

  void SetDeviceConfig(const virtio_abi::GpuDeviceConfig& device_config) {
    for (size_t i = 0; i < sizeof(device_config); ++i) {
      WriteDeviceConfig(static_cast<uint16_t>(i),
                        *(reinterpret_cast<const uint8_t*>(&device_config) + i));
    }
  }
};

class VirtioGpuTest : public testing::Test, public loop_fixture::RealLoop {
 public:
  VirtioGpuTest()
      : virtio_control_queue_buffer_pool_(zx_system_get_page_size()),
        virtio_cursor_queue_buffer_pool_(zx_system_get_page_size()) {}
  ~VirtioGpuTest() override = default;

  void SetUp() override {
    zx::bti bti;
    fake_bti_create(bti.reset_and_get_address());

    auto backend = std::make_unique<FakeGpuBackend>();
    backend->SetDeviceConfig({
        .pending_events = 0,
        .clear_events = 0,
        .scanout_limit = 1,
        .capability_set_limit = 1,
    });

    zx::result<std::unique_ptr<VirtioPciDevice>> virtio_device_result =
        VirtioPciDevice::Create(std::move(bti), std::move(backend));
    ASSERT_OK(virtio_device_result);

    std::unique_ptr<VirtioGpuDevice> gpu_device =
        std::make_unique<VirtioGpuDevice>(std::move(virtio_device_result).value());

    fake_sysmem_ = std::make_unique<MockAllocator>(dispatcher());
    auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    fidl::BindServer(dispatcher(), std::move(sysmem_server), fake_sysmem_.get());

    engine_ = std::make_unique<DisplayEngine>(&engine_events_, std::move(sysmem_client),
                                              std::move(gpu_device));
    ASSERT_OK(engine_->Init());

    RunLoopUntilIdle();
  }

  void TearDown() override {
    // Ensure the loop processes all queued FIDL messages.
    loop().Shutdown();

    engine_.reset();
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  std::vector<uint8_t> virtio_control_queue_buffer_pool_;
  std::vector<uint8_t> virtio_cursor_queue_buffer_pool_;
  std::unique_ptr<MockAllocator> fake_sysmem_;

  display::DisplayEngineEventsFidl engine_events_;
  std::unique_ptr<DisplayEngine> engine_;
};

TEST_F(VirtioGpuTest, ImportVmo) {
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  EXPECT_OK(
      engine_->ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints.client)));

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));
  RunLoopUntilIdle();

  PerformBlockingWork([&] {
    ImportedBufferCollection* imported_buffer_collection =
        engine_->imported_images_for_testing()->FindBufferCollectionById(kBufferCollectionId);
    ASSERT_TRUE(imported_buffer_collection != nullptr);

    zx::result<SysmemBufferInfo> sysmem_buffer_info_result =
        imported_buffer_collection->GetSysmemMetadata(/*buffer_index=*/0);
    ASSERT_OK(sysmem_buffer_info_result);

    const SysmemBufferInfo& sysmem_buffer_info = sysmem_buffer_info_result.value();
    EXPECT_EQ(uint32_t{640 * 4}, sysmem_buffer_info.minimum_bytes_per_row);
  });
}

TEST_F(VirtioGpuTest, SetBufferCollectionConstraints) {
  // Import buffer collection.
  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  EXPECT_OK(
      engine_->ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints.client)));
  RunLoopUntilIdle();

  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));
  RunLoopUntilIdle();
}

void ExpectHandlesArePaired(zx_handle_t lhs, zx_handle_t rhs) {
  auto [lhs_koid, lhs_related_koid] = fsl::GetKoids(lhs);
  auto [rhs_koid, rhs_related_koid] = fsl::GetKoids(rhs);

  EXPECT_NE(lhs_koid, ZX_KOID_INVALID);
  EXPECT_NE(lhs_related_koid, ZX_KOID_INVALID);
  EXPECT_NE(rhs_koid, ZX_KOID_INVALID);
  EXPECT_NE(rhs_related_koid, ZX_KOID_INVALID);

  EXPECT_EQ(lhs_koid, rhs_related_koid);
  EXPECT_EQ(rhs_koid, lhs_related_koid);
}

template <typename T>
void ExpectObjectsArePaired(zx::unowned<T> lhs, zx::unowned<T> rhs) {
  return ExpectHandlesArePaired(lhs->get(), rhs->get());
}

TEST_F(VirtioGpuTest, ImportBufferCollection) {
  // This allocator is expected to be alive as long as the `device_` is
  // available, so it should outlive the test body.
  const MockAllocator* allocator = fake_sysmem_.get();

  auto token1_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  constexpr display::DriverBufferCollectionId kValidBufferCollectionId(1);
  EXPECT_OK(engine_->ImportBufferCollection(kValidBufferCollectionId,
                                            std::move(token1_endpoints.client)));

  auto token2_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  zx::result<> import2_result =
      engine_->ImportBufferCollection(kValidBufferCollectionId, std::move(token2_endpoints.client));
  EXPECT_EQ(ZX_ERR_ALREADY_EXISTS, import2_result.status_value()) << import2_result.status_string();

  RunLoopUntilIdle();
  EXPECT_TRUE(!allocator->GetActiveBufferCollectionTokenClients().empty());

  // Verify that the current buffer collection token is used.
  {
    auto active_buffer_token_clients = allocator->GetActiveBufferCollectionTokenClients();
    EXPECT_EQ(active_buffer_token_clients.size(), 1u);

    auto inactive_buffer_token_clients = allocator->GetInactiveBufferCollectionTokenClients();
    EXPECT_EQ(inactive_buffer_token_clients.size(), 0u);

    ExpectObjectsArePaired(active_buffer_token_clients[0].channel(),
                           token1_endpoints.server.channel().borrow());
  }

  // Test ReleaseBufferCollection().
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(2);
  zx::result<> release_result = engine_->ReleaseBufferCollection(kInvalidBufferCollectionId);
  EXPECT_EQ(ZX_ERR_NOT_FOUND, release_result.status_value()) << release_result.status_string();
  EXPECT_OK(engine_->ReleaseBufferCollection(kValidBufferCollectionId));

  RunLoopUntilIdle();
  EXPECT_TRUE(allocator->GetActiveBufferCollectionTokenClients().empty());

  // Verify that the current buffer collection token is released.
  {
    auto active_buffer_token_clients = allocator->GetActiveBufferCollectionTokenClients();
    EXPECT_EQ(active_buffer_token_clients.size(), 0u);

    auto inactive_buffer_token_clients = allocator->GetInactiveBufferCollectionTokenClients();
    EXPECT_EQ(inactive_buffer_token_clients.size(), 1u);

    ExpectObjectsArePaired(inactive_buffer_token_clients[0].channel(),
                           token1_endpoints.server.channel().borrow());
  }
}

TEST_F(VirtioGpuTest, ImportImage) {
  // This allocator is expected to be alive as long as the `device_` is
  // available, so it should outlive the test body.
  const MockAllocator* allocator = fake_sysmem_.get();

  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  EXPECT_OK(
      engine_->ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints.client)));

  RunLoopUntilIdle();
  EXPECT_TRUE(!allocator->GetActiveBufferCollectionTokenClients().empty());

  // Set buffer collection constraints.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(engine_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));
  RunLoopUntilIdle();

  // Invalid import: bad collection id
  static constexpr display::ImageMetadata kDefaultImageMetadata(
      {.width = 800, .height = 600, .tiling_type = display::ImageTilingType::kLinear});
  static constexpr display::DriverBufferCollectionId kInvalidCollectionId(100);
  static constexpr uint32_t kBufferCollectionIndex = 0;
  PerformBlockingWork([&] {
    zx::result<display::DriverImageId> import_result =
        engine_->ImportImage(kDefaultImageMetadata, kInvalidCollectionId, kBufferCollectionIndex);
    EXPECT_EQ(ZX_ERR_NOT_FOUND, import_result.status_value()) << import_result.status_string();
  });

  // Invalid import: bad index
  static constexpr uint32_t kInvalidBufferCollectionIndex = 100;
  PerformBlockingWork([&] {
    zx::result<display::DriverImageId> import_result = engine_->ImportImage(
        kDefaultImageMetadata, kBufferCollectionId, kInvalidBufferCollectionIndex);
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, import_result.status_value()) << import_result.status_string();
  });

  // TODO(https://fxbug.dev/42073709): Implement fake ring-buffer based tests so that we
  // can test the valid import case.

  EXPECT_OK(engine_->ReleaseBufferCollection(kBufferCollectionId));

  RunLoopUntilIdle();
  EXPECT_TRUE(allocator->GetActiveBufferCollectionTokenClients().empty());
}

}  // namespace

}  // namespace virtio_display
