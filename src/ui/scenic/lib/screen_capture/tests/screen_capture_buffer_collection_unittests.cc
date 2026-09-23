// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>

#include <cstddef>

#include <gtest/gtest.h>

#include "src/ui/lib/escher/test/common/gtest_vulkan.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/flatland/renderer/tests/common.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

namespace screen_capture::test {

using allocation::BufferCollectionUsage;
using fuchsia_images2::PixelFormat;

class ScreenCaptureBufferCollectionTest : public flatland::RendererTest {
 public:
  void SetUp() {
    RendererTest::SetUp();
    renderer_ = std::make_shared<flatland::VkRenderer>(escher::test::GetEscher()->GetWeakPtr());
    importer_ = std::make_unique<ScreenCaptureBufferCollectionImporter>(
        utils::CreateSysmemAllocatorClient(loop_.dispatcher(), "SCBCTest::Setup"), renderer_);
  }

  fuchsia_sysmem2::BufferCollectionInfo CreateBufferCollectionInfoWithConstraints(
      fuchsia_sysmem2::BufferCollectionConstraints constraints,
      allocation::GlobalBufferCollectionId collection_id) {
    fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator =
        utils::CreateSysmemAllocatorClient(loop_.dispatcher(), "CreateBCInfo2WithConstraints");
    // Create Sysmem tokens.

    auto [local_token, dup_token] = flatland::SysmemTokens::Create(sysmem_allocator);

    // Import into ScreenCaptureBufferCollectionImporter.
    auto promise =
        importer_->ImportBufferCollection(collection_id, sysmem_allocator, std::move(dup_token),
                                          BufferCollectionUsage::kRenderTarget, std::nullopt);
    EXPECT_TRUE(RunPromise(std::move(promise)));

    fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection;
    {
      auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
      fidl::Arena arena;
      fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
          fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
              .token(std::move(local_token))
              .buffer_collection_request(std::move(server_end))
              .Build());
      EXPECT_TRUE(result.ok());
      buffer_collection.Bind(std::move(client_end));
    }

    fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.constraints(std::move(constraints));
    auto set_status = buffer_collection->SetConstraints(std::move(set_constraints_request));
    EXPECT_TRUE(set_status.is_ok());

    // Wait for allocation.
    auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    auto release_status = buffer_collection->Release();
    EXPECT_TRUE(release_status.is_ok());
    return std::move(wait_result.value().buffer_collection_info().value());
  }

 protected:
  // Run promise on this test case's loop.
  // Return true if result is_ok().
  bool RunPromise(fpromise::promise<> promise) {
    bool done = false;
    return integration_tests::RunPromise(
        loop_.dispatcher(),
        [this, &done] {
          while (!done) {
            loop_.RunUntilIdle();
          }
        },
        promise.then([&done](fpromise::result<>& result) {
          done = true;
          return result;
        }));
  }

  async::Loop loop_{&kAsyncLoopConfigAttachToCurrentThread};
  std::shared_ptr<flatland::VkRenderer> renderer_;
  std::shared_ptr<ScreenCaptureBufferCollectionImporter> importer_;
};

class ScreenCaptureBCTestParameterized : public ScreenCaptureBufferCollectionTest,
                                         public testing::WithParamInterface<PixelFormat> {};

// TODO(https://fxbug.dev/42158284): we don't want to "warm up" render passes and pipelines for
// multiple framebuffer formats, so we allow only BGRA framebuffers.  This is supported by all
// current platforms, including the emulator.
INSTANTIATE_TEST_SUITE_P(, ScreenCaptureBCTestParameterized,
                         testing::Values(PixelFormat::kB8G8R8A8));

VK_TEST_F(ScreenCaptureBufferCollectionTest, ImportAndReleaseBufferCollection) {
  // Create Sysmem tokens.
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator =
      utils::CreateSysmemAllocatorClient(loop_.dispatcher(), "SCBCTest-ImportAndReleaseBC");
  // Create Sysmem tokens.

  auto [_, dup_token] = flatland::SysmemTokens::Create(sysmem_allocator);

  // Import into ScreenCaptureBufferCollectionImporter.
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise =
      importer_->ImportBufferCollection(collection_id, sysmem_allocator, std::move(dup_token),
                                        BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

VK_TEST_P(ScreenCaptureBCTestParameterized, ImportBufferImage) {
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  // Set constraints.
  const auto pixel_format = GetParam();
  const uint32_t kWidth = 32;
  const uint32_t kHeight = 32;
  const uint32_t buffer_count = 2;
  fuchsia_sysmem2::BufferCollectionConstraints constraints =
      utils::CreateDefaultConstraints(buffer_count, kWidth, kHeight, pixel_format);

  CreateBufferCollectionInfoWithConstraints(std::move(constraints), collection_id);
  // Extract image into the first Session.
  allocation::ImageMetadata metadata;
  metadata.width = kWidth;
  metadata.height = kHeight;
  metadata.vmo_index = 0;
  metadata.collection_id = collection_id;
  metadata.identifier = display::ImageId(1);

  // Verify image has been imported correctly.
  auto promise = importer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

VK_TEST_P(ScreenCaptureBCTestParameterized, GetBufferCountFromCollectionId) {
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  // Set constraints.
  const auto pixel_format = GetParam();
  const uint32_t kWidth = 32;
  const uint32_t kHeight = 32;
  const uint32_t buffer_count = 2;
  fuchsia_sysmem2::BufferCollectionConstraints constraints =
      utils::CreateDefaultConstraints(buffer_count, kWidth, kHeight, pixel_format);

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(std::move(constraints), collection_id);

  std::optional<uint32_t> info = importer_->GetBufferCollectionBufferCount(collection_id);

  EXPECT_NE(info, std::nullopt);
  EXPECT_EQ(info.value(), buffer_count);

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

VK_TEST_F(ScreenCaptureBufferCollectionTest, ImportBufferCollection_ErrorCases) {
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator =
      utils::CreateSysmemAllocatorClient(loop_.dispatcher(), "SCBCTest-ImportBC_ErrorCases");

  const auto collection_id = allocation::GenerateUniqueBufferCollectionId();

  {
    auto [token_client, token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(token_server))
            .Build());
    EXPECT_TRUE(result.ok());
    auto promise =
        importer_->ImportBufferCollection(collection_id, sysmem_allocator, std::move(token_client),
                                          BufferCollectionUsage::kRenderTarget, std::nullopt);
    EXPECT_TRUE(RunPromise(std::move(promise)));
  }

  // Buffer collection id dup.
  {
    auto [token_client, token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(token_server))
            .Build());
    EXPECT_TRUE(result.ok());
    auto promise =
        importer_->ImportBufferCollection(collection_id, sysmem_allocator, std::move(token_client),
                                          BufferCollectionUsage::kRenderTarget, std::nullopt);
    EXPECT_FALSE(RunPromise(std::move(promise)));
  }
}

VK_TEST_P(ScreenCaptureBCTestParameterized, ImportBufferImage_ErrorCases) {
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  // Set constraints.
  const auto pixel_format = GetParam();
  const uint32_t kWidth = 32;
  const uint32_t kHeight = 32;
  const uint32_t buffer_count = 2;
  fuchsia_sysmem2::BufferCollectionConstraints constraints =
      utils::CreateDefaultConstraints(buffer_count, kWidth, kHeight, pixel_format);

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(std::move(constraints), collection_id);

  // Buffer collection id mismatch.
  {
    allocation::ImageMetadata metadata;
    metadata.collection_id = allocation::GenerateUniqueBufferCollectionId();
    auto promise = importer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
    EXPECT_FALSE(RunPromise(std::move(promise)));
  }

  // Buffer collection id invalid.
  {
    allocation::ImageMetadata metadata;
    metadata.collection_id = 0;
    auto promise = importer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
    EXPECT_FALSE(RunPromise(std::move(promise)));
  }

  // Buffer collection has 0 width and height.
  {
    allocation::ImageMetadata metadata;
    metadata.collection_id = collection_id;
    metadata.width = 0;
    metadata.height = 0;
    auto promise = importer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
    EXPECT_FALSE(RunPromise(std::move(promise)));
  }

  // Buffer count is does not correspond with vmo_index
  {
    allocation::ImageMetadata metadata;
    metadata.collection_id = collection_id;
    metadata.width = 32;
    metadata.height = 32;
    metadata.vmo_index = 3;
    auto promise = importer_->ImportBufferImage(metadata, BufferCollectionUsage::kRenderTarget);
    EXPECT_FALSE(RunPromise(std::move(promise)));
  }

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

VK_TEST_P(ScreenCaptureBCTestParameterized, GetBufferCollectionBufferCount_ErrorCases) {
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  // Set constraints.
  const auto pixel_format = GetParam();
  const uint32_t kWidth = 32;
  const uint32_t kHeight = 32;
  const uint32_t buffer_count = 2;
  fuchsia_sysmem2::BufferCollectionConstraints constraints =
      utils::CreateDefaultConstraints(buffer_count, kWidth, kHeight, pixel_format);

  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info =
      CreateBufferCollectionInfoWithConstraints(std::move(constraints), collection_id);

  // collection_id does not exist
  {
    auto new_collection_id = allocation::GenerateUniqueBufferCollectionId();
    std::optional<uint32_t> info = importer_->GetBufferCollectionBufferCount(new_collection_id);
    EXPECT_EQ(info, std::nullopt);
  }

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

VK_TEST_P(ScreenCaptureBCTestParameterized, GetBufferCollectionBufferCount_BuffersNotAllocated) {
  auto collection_id = allocation::GenerateUniqueBufferCollectionId();
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator =
      utils::CreateSysmemAllocatorClient(loop_.dispatcher(), "GetBCBC_BuffersNotAllocated");
  // Create Sysmem tokens.
  auto [local_token, dup_token] = flatland::SysmemTokens::Create(sysmem_allocator);
  // Import into ScreenCaptureBufferCollectionImporter.
  auto promise =
      importer_->ImportBufferCollection(collection_id, sysmem_allocator, std::move(dup_token),
                                        BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(std::move(promise)));

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(server_end))
          .Build());
  EXPECT_TRUE(result.ok());

  // CheckForBuffersAllocated will return false
  std::optional<uint32_t> info = importer_->GetBufferCollectionBufferCount(collection_id);
  EXPECT_EQ(info, std::nullopt);

  // Cleanup.
  importer_->ReleaseBufferCollection(collection_id, BufferCollectionUsage::kRenderTarget);
}

}  // namespace screen_capture::test
