// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fit/defer.h>
#include <lib/zx/object.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <list>
#include <memory>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/predicates/status.h"

namespace framebuffer_display {

namespace {

constexpr DisplayProperties kDisplayProperties{
    .width_px = 800,
    .height_px = 600,
    .row_stride_px = 800,
    .pixel_format = display::PixelFormat::kB8G8R8A8,
};
constexpr size_t kBytesPerPixel = 4;
constexpr size_t kImageByteSize = uint64_t{kDisplayProperties.row_stride_px} *
                                  static_cast<uint64_t>(kDisplayProperties.height_px) *
                                  kBytesPerPixel;

// TODO(https://fxbug.dev/42072949): Consider creating and using a unified set of sysmem
// testing doubles instead of writing mocks for each display driver test.
class FakeBufferCollection : public fidl::testing::WireTestBase<fuchsia_sysmem2::BufferCollection> {
 public:
  explicit FakeBufferCollection(zx::unowned_vmo framebuffer_vmo, size_t image_byte_size,
                                uint32_t num_buffers = 2)
      : framebuffer_vmo_(std::move(framebuffer_vmo)),
        image_byte_size_(image_byte_size),
        num_buffers_(num_buffers) {}

  void SetConstraints(::fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest* request,
                      SetConstraintsCompleter::Sync& completer) override {}
  void CheckAllBuffersAllocated(CheckAllBuffersAllocatedCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void WaitForAllBuffersAllocated(WaitForAllBuffersAllocatedCompleter::Sync& completer) override {
    fidl::Arena arena;
    auto response =
        fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse::Builder(arena);
    auto collection_info = fuchsia_sysmem2::wire::BufferCollectionInfo::Builder(arena);
    auto single_buffer_settings = fuchsia_sysmem2::wire::SingleBufferSettings::Builder(arena);
    auto buffer_memory_settings = fuchsia_sysmem2::wire::BufferMemorySettings::Builder(arena);
    single_buffer_settings.buffer_settings(buffer_memory_settings.Build());
    auto image_format_constraints = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
    image_format_constraints.pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
    image_format_constraints.pixel_format_modifier(
        fuchsia_images2::wire::PixelFormatModifier::kLinear);
    single_buffer_settings.image_format_constraints(image_format_constraints.Build());
    collection_info.settings(single_buffer_settings.Build());

    fidl::VectorView<fuchsia_sysmem2::wire::VmoBuffer> buffers(arena, num_buffers_);
    for (uint32_t i = 0; i < num_buffers_; i++) {
      zx::vmo vmo;
      if (i == 0 && framebuffer_vmo_->is_valid()) {
        EXPECT_OK(framebuffer_vmo_->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo));
      } else {
        zx::vmo::create(image_byte_size_, 0, &vmo);
      }
      auto vmo_buffer = fuchsia_sysmem2::wire::VmoBuffer::Builder(arena);
      vmo_buffer.vmo(std::move(vmo));
      vmo_buffer.vmo_usable_start(0);
      buffers[i] = vmo_buffer.Build();
    }
    collection_info.buffers(buffers);
    response.buffer_collection_info(collection_info.Build());

    completer.ReplySuccess(response.Build());
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {}

 private:
  zx::unowned_vmo framebuffer_vmo_;
  size_t image_byte_size_;
  uint32_t num_buffers_;
};

using BufferCollectionId = uint64_t;

class FakeSysmemBase {
 public:
  virtual BufferCollectionId AllocBufferCollectionId() = 0;
};

class MockAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  explicit MockAllocator(FakeSysmemBase& parent, async_dispatcher_t* dispatcher,
                         zx::unowned_vmo framebuffer_vmo, size_t image_byte_size)
      : parent_(parent),
        dispatcher_(dispatcher),
        framebuffer_vmo_(std::move(framebuffer_vmo)),
        image_byte_size_(image_byte_size) {
    ZX_ASSERT(dispatcher_);
  }

  void BindSharedCollection(BindSharedCollectionRequestView request,
                            BindSharedCollectionCompleter::Sync& completer) override {
    auto buffer_collection_id = parent_.AllocBufferCollectionId();
    active_buffer_collections_.emplace(
        buffer_collection_id,
        BufferCollection{.token_client = std::move(request->token()),
                         .unowned_collection_server = request->buffer_collection_request(),
                         .fake_buffer_collection =
                             FakeBufferCollection(framebuffer_vmo_->borrow(), image_byte_size_)});
    fidl::BindServer(
        dispatcher_, std::move(request->buffer_collection_request()),
        &active_buffer_collections_.at(buffer_collection_id).fake_buffer_collection,
        [this, buffer_collection_id](FakeBufferCollection*, fidl::UnbindInfo,
                                     fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>) {
          inactive_buffer_collection_tokens_.push_back(
              std::move(active_buffer_collections_.at(buffer_collection_id).token_client));
          active_buffer_collections_.erase(buffer_collection_id);
        });
  }

  std::vector<std::pair<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>,
                        fidl::UnownedServerEnd<fuchsia_sysmem2::BufferCollection>>>
  GetBufferCollectionConnections() {
    if (active_buffer_collections_.empty()) {
      return {};
    }

    std::vector<std::pair<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>,
                          fidl::UnownedServerEnd<fuchsia_sysmem2::BufferCollection>>>
        result;
    for (const auto& kv : active_buffer_collections_) {
      result.emplace_back(kv.second.token_client, kv.second.unowned_collection_server);
    }
    return result;
  }

  void SetDebugClientInfo(SetDebugClientInfoRequestView request,
                          SetDebugClientInfoCompleter::Sync& completer) override {
    EXPECT_EQ(request->name().get().find("framebuffer-display"), 0u);
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    fdf::error("{} not implemented", name);
    EXPECT_TRUE(false);
  }

 private:
  struct BufferCollection {
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_client;
    fidl::UnownedServerEnd<fuchsia_sysmem2::BufferCollection> unowned_collection_server;
    FakeBufferCollection fake_buffer_collection;
  };

  FakeSysmemBase& parent_;
  std::unordered_map<BufferCollectionId, BufferCollection> active_buffer_collections_;
  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
      inactive_buffer_collection_tokens_;

  async_dispatcher_t* dispatcher_ = nullptr;
  zx::unowned_vmo framebuffer_vmo_;
  size_t image_byte_size_ = 0;
};

class FakeSysmem : public FakeSysmemBase {
 public:
  explicit FakeSysmem(async_dispatcher_t* dispatcher, zx::unowned_vmo framebuffer_vmo,
                      uint64_t first_buffer_collection_id, size_t collection_image_byte_size)
      : dispatcher_(dispatcher),
        framebuffer_vmo_(std::move(framebuffer_vmo)),
        next_buffer_collection_id_(first_buffer_collection_id),
        collection_image_byte_size_(collection_image_byte_size) {
    EXPECT_TRUE(dispatcher_);
  }

  fit::result<zx_status_t, fidl::WireSyncClient<fuchsia_sysmem2::Allocator>>
  MakeFakeSysmemAllocator() {
    auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();

    mock_allocators_.emplace_front(*this, dispatcher_, framebuffer_vmo_->borrow(),
                                   collection_image_byte_size_);
    auto it = mock_allocators_.begin();
    fidl::BindServer(dispatcher_, std::move(sysmem_server), &*it);

    return fit::ok(fidl::WireSyncClient(std::move(sysmem_client)));
  }

  std::list<MockAllocator>& mock_allocators() { return mock_allocators_; }

  BufferCollectionId AllocBufferCollectionId() override { return next_buffer_collection_id_++; }

 private:
  friend class MockAllocator;
  std::list<MockAllocator> mock_allocators_;
  async_dispatcher_t* dispatcher_ = nullptr;
  zx::unowned_vmo framebuffer_vmo_ = {};
  BufferCollectionId next_buffer_collection_id_ = 0;
  size_t collection_image_byte_size_ = 0;
};

class FakeMmio {
 public:
  FakeMmio() {
    mmio_ = std::make_unique<ddk_fake::FakeMmioRegRegion>(sizeof(uint32_t), kRegArrayLength);
  }

  fdf::MmioBuffer MmioBuffer() { return mmio_->GetMmioBuffer(); }

  ddk_fake::FakeMmioReg& FakeRegister(size_t address) { return (*mmio_)[address]; }

 private:
  static constexpr size_t kMmioBufferSize = 0x5000;
  static constexpr size_t kRegArrayLength = kMmioBufferSize / sizeof(uint32_t);
  std::unique_ptr<ddk_fake::FakeMmioRegRegion> mmio_;
};

class FramebufferDisplayTest : public ::testing::Test {
 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  display::DisplayEngineEventsFidl engine_events_;
};

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

TEST_F(FramebufferDisplayTest, ImportBufferCollection) {
  async::Loop env_loop(&kAsyncLoopConfigAttachToCurrentThread);
  FakeSysmem fake_sysmem(env_loop.dispatcher(), /*framebuffer_vmo=*/{}, 0, kImageByteSize);
  FakeMmio fake_mmio;

  auto sysmem_client_result = fake_sysmem.MakeFakeSysmemAllocator();
  ASSERT_TRUE(sysmem_client_result.is_ok());
  auto& sysmem_client = sysmem_client_result.value();

  async::Loop display_loop(&kAsyncLoopConfigNeverAttachToThread);
  display_loop.StartThread("framebuffer-display-loop");
  FramebufferDisplay display(&engine_events_, std::move(sysmem_client), fake_mmio.MmioBuffer(),
                             kDisplayProperties, display_loop.dispatcher());

  auto token1_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  auto token2_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  // Test ImportBufferCollection().
  const display::DriverBufferCollectionId kValidCollectionId(1);
  EXPECT_OK(display.ImportBufferCollection(kValidCollectionId, std::move(token1_endpoints.client)));

  // `collection_id` must be unused.
  EXPECT_STATUS(
      display.ImportBufferCollection(kValidCollectionId, std::move(token2_endpoints.client)),
      zx::error(ZX_ERR_ALREADY_EXISTS));

  env_loop.RunUntilIdle();

  EXPECT_EQ(fake_sysmem.mock_allocators().size(), 1u);
  auto& allocator = fake_sysmem.mock_allocators().front();

  // Verify that the current buffer collection token is used.
  {
    const std::vector buffer_collection_connections = allocator.GetBufferCollectionConnections();
    ASSERT_EQ(buffer_collection_connections.size(), 1u);

    const auto& buffer_collection_server = buffer_collection_connections[0].second;
    const auto& buffer_collection_client =
        display.GetBufferCollectionsForTesting().at(kValidCollectionId).client_end();
    ExpectObjectsArePaired(buffer_collection_server.handle(), buffer_collection_client.handle());

    const auto& buffer_collection_token_server = token1_endpoints.server;
    const auto& buffer_collection_token_client = buffer_collection_connections[0].first;
    ExpectObjectsArePaired(buffer_collection_token_server.handle(),
                           buffer_collection_token_client.handle());
  }

  // Test ReleaseBufferCollection().
  const display::DriverBufferCollectionId kInvalidCollectionId(2);
  EXPECT_STATUS(display.ReleaseBufferCollection(kInvalidCollectionId), zx::error(ZX_ERR_NOT_FOUND));
  EXPECT_OK(display.ReleaseBufferCollection(kValidCollectionId));

  env_loop.RunUntilIdle();

  // Verify that the current buffer collection token is released.
  {
    const std::vector buffer_collection_connections = allocator.GetBufferCollectionConnections();
    ASSERT_EQ(buffer_collection_connections.size(), 0u);
  }

  // Shutdown the loop before destroying the FakeSysmem and MockAllocator which
  // may still have pending callbacks.
  env_loop.Shutdown();
  display_loop.Shutdown();
}

TEST_F(FramebufferDisplayTest, ImportImage) {
  const display::DriverBufferCollectionId kBufferCollectionId(1);

  // `framebuffer_vmo` must outlive `fake_sysmem`.
  zx::vmo framebuffer_vmo;
  EXPECT_OK(zx::vmo::create(kImageByteSize, 0, &framebuffer_vmo));

  async::Loop env_loop(&kAsyncLoopConfigNeverAttachToThread);
  FakeSysmem fake_sysmem(env_loop.dispatcher(), framebuffer_vmo.borrow(),
                         kBufferCollectionId.value(), kImageByteSize);
  FakeMmio fake_mmio;

  env_loop.StartThread("env-loop");

  auto sysmem_client_result = fake_sysmem.MakeFakeSysmemAllocator();
  ASSERT_TRUE(sysmem_client_result.is_ok());
  auto& sysmem_client = sysmem_client_result.value();

  async::Loop display_loop(&kAsyncLoopConfigNeverAttachToThread);
  display_loop.StartThread("framebuffer-display-loop");
  FramebufferDisplay display(&engine_events_, std::move(sysmem_client), fake_mmio.MmioBuffer(),
                             kDisplayProperties, display_loop.dispatcher());
  auto token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  // Import BufferCollection.
  EXPECT_OK(display.ImportBufferCollection(kBufferCollectionId, std::move(token_endpoints.client)));

  // Set Buffer collection constraints.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(display.SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  static constexpr display::ImageMetadata kDisplayImageMetadata({
      .width = kDisplayProperties.width_px,
      .height = kDisplayProperties.height_px,
      .tiling_type = display::ImageTilingType::kLinear,
  });

  // Invalid import: bad collection id.
  display::DriverBufferCollectionId kInvalidCollectionId(100);
  EXPECT_STATUS(display.ImportImage(kDisplayImageMetadata, kInvalidCollectionId, 0),
                zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: bad index.
  EXPECT_STATUS(display.ImportImage(kDisplayImageMetadata, kBufferCollectionId, 100),
                zx::error(ZX_ERR_OUT_OF_RANGE));

  // Invalid import: bad width.
  static constexpr display::ImageMetadata kImageMetadataWithIncorrectWidth({
      .width = kDisplayProperties.width_px * 2,
      .height = kDisplayProperties.height_px,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_STATUS(display.ImportImage(kImageMetadataWithIncorrectWidth, kBufferCollectionId, 0),
                zx::error(ZX_ERR_INVALID_ARGS));

  // Invalid import: bad height.
  static constexpr display::ImageMetadata kImageMetadataWithIncorrectHeight({
      .width = kDisplayProperties.width_px,
      .height = kDisplayProperties.height_px * 2,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_STATUS(display.ImportImage(kImageMetadataWithIncorrectHeight, kBufferCollectionId, 0),
                zx::error(ZX_ERR_INVALID_ARGS));

  // Valid import: buffer_index 0.
  zx::result<display::DriverImageId> import0_result =
      display.ImportImage(kDisplayImageMetadata, kBufferCollectionId, 0);
  ASSERT_OK(import0_result);
  EXPECT_NE(display::kInvalidDriverImageId, import0_result.value());

  // Valid import: buffer_index 1.
  zx::result<display::DriverImageId> import1_result =
      display.ImportImage(kDisplayImageMetadata, kBufferCollectionId, 1);
  ASSERT_OK(import1_result);
  EXPECT_NE(display::kInvalidDriverImageId, import1_result.value());

  // Each import gets a unique image ID.
  EXPECT_NE(import0_result.value(), import1_result.value());

  display.ReleaseImage(import0_result.value());
  display.ReleaseImage(import1_result.value());

  // Release buffer collection.
  EXPECT_OK(display.ReleaseBufferCollection(kBufferCollectionId));

  env_loop.RunUntilIdle();

  // Shutdown the loop before destroying the FakeSysmem and MockAllocator which
  // may still have pending callbacks.
  env_loop.Shutdown();
  display_loop.Shutdown();
}

TEST_F(FramebufferDisplayTest, DestructionCancelsVsyncTask) {
  FakeMmio fake_mmio;
  async::Loop display_loop(&kAsyncLoopConfigNeverAttachToThread);

  {
    FramebufferDisplay display(&engine_events_, /*sysmem_client=*/{}, fake_mmio.MmioBuffer(),
                               kDisplayProperties, display_loop.dispatcher());
    EXPECT_OK(display.Initialize());
  }

  // Running the loop after display is destroyed should not invoke callbacks
  // on the destroyed object or crash.
  display_loop.RunUntilIdle();
  display_loop.Shutdown();
}

TEST_F(FramebufferDisplayTest, DeinitializeWithoutInitializeIsNoOp) {
  FakeMmio fake_mmio;
  async::Loop display_loop(&kAsyncLoopConfigNeverAttachToThread);

  FramebufferDisplay display(&engine_events_, /*sysmem_client=*/{}, fake_mmio.MmioBuffer(),
                             kDisplayProperties, display_loop.dispatcher());
  // Calling Deinitialize() on an uninitialized display is safe and is a no-op.
  display.Deinitialize();
}

TEST_F(FramebufferDisplayTest, DeinitializeMultipleTimesIsSafe) {
  FakeMmio fake_mmio;
  async::Loop display_loop(&kAsyncLoopConfigNeverAttachToThread);

  FramebufferDisplay display(&engine_events_, /*sysmem_client=*/{}, fake_mmio.MmioBuffer(),
                             kDisplayProperties, display_loop.dispatcher());
  EXPECT_OK(display.Initialize());

  // Calling Deinitialize() multiple times is safe and subsequent calls are no-ops.
  display.Deinitialize();
  display.Deinitialize();

  display_loop.RunUntilIdle();
  display_loop.Shutdown();
}

}  // namespace

}  // namespace framebuffer_display
