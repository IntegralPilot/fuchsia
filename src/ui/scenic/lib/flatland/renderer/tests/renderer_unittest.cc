// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>

#include <cstdint>
#include <thread>

#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"
#include "src/ui/scenic/lib/flatland/renderer/null_renderer.h"
#include "src/ui/scenic/lib/flatland/renderer/tests/common.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"
// TODO(https://fxbug.dev/42179449): Remove dependency on screen_capture.
#include "gmock/gmock.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

#include <glm/glm.hpp>
#include <glm/gtc/constants.hpp>
#include <glm/gtc/type_ptr.hpp>
#include <glm/gtx/matrix_transform_2d.hpp>

namespace flatland {

fuchsia_sysmem2::BufferUsage get_cpu_usage_read() {
  static const fuchsia_sysmem2::BufferUsage usage = [] {
    fuchsia_sysmem2::BufferUsage usage;
    usage.cpu(fuchsia_sysmem2::kCpuUsageRead);
    return usage;
  }();
  return usage;
}

using allocation::BufferCollectionUsage;
using allocation::ImageMetadata;
using fuchsia_ui_composition::ImageFlip;
using fuchsia_ui_composition::Orientation;

static bool RunPromise(async::TestLoop& loop, fpromise::promise<> promise) {
  bool done = false;
  return integration_tests::RunPromise(
      loop.dispatcher(),
      [&loop, &done] {
        while (!done) {
          loop.RunUntilIdle();
        }
      },
      promise.then([&done](fpromise::result<>& result) {
        done = true;
        return result;
      }));
}

// TODO(https://fxbug.dev/42129956): Move common functions to testing::WithParamInterface instead of
// function calls.
using NullRendererTest = RendererTest;
using VulkanRendererTest = RendererTest;

// We need this function for several tests because directly reading the vmo values for sysmem-backed
// images does not unmap the sRGB image values back into a linear space. So we have to do that
// conversion here before we do any value comparisons. This conversion could be done automatically
// if we were doing a Vulkan read on the vk::Image directly and not a sysmem read of the vmo,
// but we don't have direct access to the images in the Renderer.
static void sRGBtoLinear(const uint8_t* in_sRGB, uint8_t* out_linear, uint32_t num_bytes) {
  for (uint32_t i = 0; i < num_bytes; i++) {
    // Do not de-encode the alpha value.
    if ((i + 1) % 4 == 0) {
      out_linear[i] = in_sRGB[i];
      continue;
    }

    // Function to convert from sRGB to linear RGB.
    float s_val = (float(in_sRGB[i]) / float(0xFF));
    if (0.f <= s_val && s_val <= 0.04045f) {
      out_linear[i] = static_cast<uint8_t>((s_val / 12.92f) * 255.f);
    } else {
      out_linear[i] = static_cast<uint8_t>(std::powf(((s_val + 0.055f) / 1.055f), 2.4f) * 255.f);
    }
  }
}

namespace {
static constexpr float kDegreesToRadians = glm::pi<float>() / 180.f;

const size_t kBytesPerRGBAPixel = 4;

glm::ivec4 GetPixel(const uint8_t* vmo_host, uint32_t width, uint32_t x, uint32_t y) {
  uint32_t r = vmo_host[y * width * 4 + x * 4];
  uint32_t g = vmo_host[y * width * 4 + x * 4 + 1];
  uint32_t b = vmo_host[y * width * 4 + x * 4 + 2];
  uint32_t a = vmo_host[y * width * 4 + x * 4 + 3];
  return glm::ivec4(r, g, b, a);
}

// When checking the output of a render target, we want to make sure that not only
// are the renderables rendered correctly, but that the rest of the image is black,
// without any errantly colored pixels.
#define CHECK_BLACK_PIXELS(bytes, kTargetWidth, kTargetHeight, color_count) \
  {                                                                         \
    uint32_t black_pixels = 0;                                              \
    for (uint32_t y = 0; y < kTargetHeight; y++) {                          \
      for (uint32_t x = 0; x < kTargetWidth; x++) {                         \
        auto col = GetPixel(bytes, kTargetWidth, x, y);                     \
        if (col == glm::ivec4(0, 0, 0, 0)) {                                \
          black_pixels++;                                                   \
        }                                                                   \
      }                                                                     \
    }                                                                       \
    EXPECT_EQ(black_pixels, kTargetWidth * kTargetHeight - color_count);    \
  }

// Utility function to simplify tests, since setting up a buffer collection is a process that
// requires a lot of boilerplate code. The |collection_info| and |collection_ptr| need to be
// kept alive in the test body, so they are passed in as parameters.
allocation::GlobalBufferCollectionId SetupBufferCollection(
    async::TestLoop& loop, const uint32_t& num_buffers, const uint32_t& image_width,
    const uint32_t& image_height, allocation::BufferCollectionUsage usage, Renderer* renderer,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fuchsia_sysmem2::BufferCollectionInfo* collection_info,
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& collection_ptr,
    allocation::GlobalBufferCollectionId collection_id =
        allocation::GenerateUniqueBufferCollectionId()) {
  // First create the pair of sysmem tokens, one for the client, one for the renderer.
  auto [dup_token, local_token] = flatland::SysmemTokens::Create(sysmem_allocator);
  EXPECT_TRUE(dup_token.is_valid());
  EXPECT_TRUE(local_token.is_valid());

  auto promise = renderer->ImportBufferCollection(collection_id, sysmem_allocator,
                                                  std::move(dup_token), usage, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise)));

  // Create a client-side handle to the buffer collection and set the client constraints.
  auto [buffer_usage, memory_constraints] = GetUsageAndMemoryConstraintsForCpuWriteOften();
  collection_ptr = CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator, std::move(local_token),
      /*image_count*/ num_buffers,
      /*width*/ image_width,
      /*height*/ image_height, std::move(buffer_usage), fuchsia_images2::PixelFormat::kR8G8B8A8,
      std::make_optional(std::move(memory_constraints)),
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));
  EXPECT_TRUE(collection_ptr.is_valid());

  // Have the client wait for buffers allocated so it can populate its information
  // struct with the vmo data.
  {
    auto wait_result = collection_ptr->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    *collection_info = std::move(wait_result->buffer_collection_info().value());
  }

  return collection_id;
}

// Test-local bundle of the per-layer visual params that ImageMetadata used to carry,
// which MakeLayers() maps onto ResolvedLayer.
struct TestLayerInfo {
  allocation::ImageMetadata metadata;
  BlendMode blend_mode = BlendMode::kReplace();
  std::array<float, 4> multiply_color = {1.f, 1.f, 1.f, 1.f};
};

std::vector<ResolvedLayer> MakeLayers(const std::vector<SrcToDest>& rects,
                                      const std::vector<TestLayerInfo>& images = {}) {
  FX_CHECK(rects.size() == images.size() || images.empty());
  std::vector<ResolvedLayer> output;
  output.reserve(rects.size());
  for (size_t i = 0; i < rects.size(); ++i) {
    const auto& rect = rects[i];
    const auto& meta = images.empty() ? TestLayerInfo{} : images[i];
    ResolvedLayer layer;
    layer.geometry = rect;
    layer.blend_mode = meta.blend_mode;
    layer.topology_index = ResolvedLayer::kInvalidTopologyIndex;

    if (meta.metadata.identifier == allocation::kInvalidImageId) {
      layer.multiply_color = {1.f, 1.f, 1.f, 1.f};
      layer.content = ResolvedLayer::SolidColorContent{.color = meta.multiply_color};
    } else {
      layer.multiply_color = meta.multiply_color;
      layer.content = ResolvedLayer::ImageContent{
          .image_id = meta.metadata.identifier,
          .width = meta.metadata.width,
          .height = meta.metadata.height,
      };
    }
    output.push_back(layer);
  }
  return output;
}

std::vector<ResolvedLayer> MakeLayers(const std::vector<SrcToDest>& rects,
                                      const std::vector<allocation::ImageMetadata>& images) {
  std::vector<TestLayerInfo> info_list;
  info_list.reserve(images.size());
  for (const auto& img : images) {
    info_list.push_back(TestLayerInfo{.metadata = img});
  }
  return MakeLayers(rects, info_list);
}

}  // anonymous namespace

// Make sure a valid token can be used to import a buffer collection.
void ImportCollectionTest(async::TestLoop& loop, Renderer* renderer,
                          fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator) {
  auto [token_client, token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  auto result = sysmem_allocator->AllocateSharedCollection(
      fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
          .token_request(std::move(token_server))
          .Build());
  EXPECT_TRUE(result.ok());

  auto sync_result = fidl::WireCall(token_client)->Sync();
  EXPECT_TRUE(sync_result.ok());

  // First id should be valid.
  auto bcid = allocation::GenerateUniqueBufferCollectionId();
  auto promise =
      renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(token_client),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise)));
}

// Multiple clients may need to reference the same buffer collection in the renderer
// (for example if they both need access to a global camera feed). In this case, both
// clients will be passing their own duped tokens to the same collection to the renderer,
// and will each get back a different ID. The collection itself (which is just a pointer)
// will be in the renderer's map twice. So if all tokens are set, both server-side
// importer collections should be allocated (since they are just pointers that refer
// to the same collection).
void SameTokenTwiceTest(async::TestLoop& loop, Renderer* renderer,
                        fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                        bool use_vulkan) {
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator);

  // Create a client token to represent a single client.
  fidl::Arena arena;
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::OneWayStatus result =
      fidl::WireCall(local_token)
          ->Duplicate(fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateRequest::Builder(arena)
                          .rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                          .token_request(std::move(server_end))
                          .Build());
  EXPECT_TRUE(result.ok());

  auto sync_result = fidl::WireCall(local_token)->Sync();
  EXPECT_TRUE(sync_result.ok());

  // First id should be valid.
  auto bcid = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 =
      renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(local_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  // Second id should be valid.
  auto bcid2 = allocation::GenerateUniqueBufferCollectionId();
  auto promise2 =
      renderer->ImportBufferCollection(bcid2, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  // Set the client constraints.
  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(client_end),
                                          /* image_count */ 1, /* width */ 64, /* height */ 32,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // Now check that both server ids are allocated.
  auto import_promise1 =
      renderer->ImportBufferImage({.collection_id = bcid,
                                   .identifier = allocation::GenerateUniqueImageId(),
                                   .vmo_index = 0,
                                   .width = 1,
                                   .height = 1},
                                  BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(import_promise1)));

  auto import_promise2 =
      renderer->ImportBufferImage({.collection_id = bcid2,
                                   .identifier = allocation::GenerateUniqueImageId(),
                                   .vmo_index = 0,
                                   .width = 1,
                                   .height = 1},
                                  BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(import_promise2)));
}

void BadImageInputTest(async::TestLoop& loop, Renderer* renderer,
                       fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                       bool use_vulkan) {
  const uint32_t kNumImages = 1;
  auto [dup_token, local_token] = SysmemTokens::Create(sysmem_allocator);

  auto bcid = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 =
      renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(local_token),
                                          /* image_count */ kNumImages, /* width */ 64,
                                          /* height */ 32,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // Using an invalid buffer collection id.
  auto image_id = allocation::GenerateUniqueImageId();
  auto promise3 = renderer->ImportBufferImage({.collection_id = allocation::kInvalidId,
                                               .identifier = image_id,
                                               .vmo_index = kNumImages,
                                               .width = 1,
                                               .height = 1},
                                              BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise3)));

  // Using an invalid image identifier.
  auto promise4 = renderer->ImportBufferImage({.collection_id = bcid,
                                               .identifier = allocation::kInvalidImageId,
                                               .vmo_index = kNumImages,
                                               .width = 1,
                                               .height = 1},
                                              BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise4)));

  // VMO index is out of bounds.
  auto promise5 = renderer->ImportBufferImage({.collection_id = bcid,
                                               .identifier = image_id,
                                               .vmo_index = kNumImages,
                                               .width = 1,
                                               .height = 1},
                                              BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise5)));
}

// Test the ImportBufferImage() function. First call ImportBufferImage() without setting the client
// constraints, which should return false, and then set the client constraints which
// should cause it to return true.
void ImportImageTest(async::TestLoop& loop, Renderer* renderer,
                     fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                     bool use_vulkan) {
  auto [dup_token, local_token] = SysmemTokens::Create(sysmem_allocator);

  auto bcid = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 =
      renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  // The buffer collection should not be valid here.
  auto image_id = allocation::GenerateUniqueImageId();
  auto promise2 = renderer->ImportBufferImage(
      {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
      BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise2)));

  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(local_token),
                                          /* image_count */ 1, /* width */ 64, /* height */ 32,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // The buffer collection *should* be valid here.
  auto promise3 = renderer->ImportBufferImage(
      {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
      BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));
}

// Simple release test that calls ReleaseBufferCollection() directly without
// any zx::events just to make sure that the method's functionality itself is
// working as intended.
void DeregistrationTest(async::TestLoop& loop, Renderer* renderer,
                        fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                        bool use_vulkan) {
  auto [dup_token, local_token] = SysmemTokens::Create(sysmem_allocator);

  auto bcid = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 =
      renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  // The buffer collection should not be valid here.
  auto image_id = allocation::GenerateUniqueImageId();
  auto promise2 = renderer->ImportBufferImage(
      {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
      BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise2)));

  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(local_token),
                                          /* image_count */ 1, /* width */ 64, /* height */ 32,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // The buffer collection *should* be valid here.
  auto promise3 = renderer->ImportBufferImage(
      {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
      BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Now release the collection.
  renderer->ReleaseBufferCollection(bcid, BufferCollectionUsage::kRenderTarget);

  // After deregistration, calling ImportBufferImage() should return false.
  auto promise4 = renderer->ImportBufferImage(
      {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
      BufferCollectionUsage::kRenderTarget);
  EXPECT_FALSE(RunPromise(loop, std::move(promise4)));
}

// Test that calls ReleaseBufferCollection() before ReleaseBufferImage() and makes sure that
// imported Image can still be rendered.
void RenderImageAfterBufferCollectionReleasedTest(
    async::TestLoop& loop, Renderer* renderer,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator, bool use_vulkan) {
  auto texture_tokens = SysmemTokens::Create(sysmem_allocator);
  auto target_tokens = SysmemTokens::Create(sysmem_allocator);

  auto texture_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto target_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 = renderer->ImportBufferCollection(
      texture_collection_id, sysmem_allocator, std::move(texture_tokens.dup_token),
      BufferCollectionUsage::kClientImage, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  auto promise2 = renderer->ImportBufferCollection(
      target_collection_id, sysmem_allocator, std::move(target_tokens.dup_token),
      BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  const uint32_t kWidth = 64, kHeight = 32;
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(texture_tokens.local_token),
                                          /* image_count */ 1, /* width */ kWidth,
                                          /* height */ kHeight,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(target_tokens.local_token),
                                          /* image_count */ 1, /* width */ kWidth,
                                          /* height */ kHeight,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // Import render target.
  ImageMetadata render_target = {.collection_id = target_collection_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kWidth,
                                 .height = kHeight};
  auto promise3 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Import image.
  ImageMetadata image = {.collection_id = texture_collection_id,
                         .identifier = allocation::GenerateUniqueImageId(),
                         .vmo_index = 0,
                         .width = kWidth,
                         .height = kHeight};
  auto promise4 = renderer->ImportBufferImage(image, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise4)));

  // Now release the collection.
  renderer->ReleaseBufferCollection(texture_collection_id, BufferCollectionUsage::kClientImage);
  renderer->ReleaseBufferCollection(target_collection_id, BufferCollectionUsage::kRenderTarget);

  // We should still be able to render this image.
  renderer->Render(
      render_target,
      MakeLayers({SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(kWidth),
                                               .height = static_cast<float>(kHeight)}))},
                 {image}),
      {});
  if (use_vulkan) {
    auto vk_renderer = static_cast<VkRenderer*>(renderer);
    vk_renderer->WaitIdle();
  }
}

void RenderAfterImageReleasedTest(async::TestLoop& loop, Renderer* renderer,
                                  fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                                  bool use_vulkan) {
  auto texture_tokens = SysmemTokens::Create(sysmem_allocator);
  auto target_tokens = SysmemTokens::Create(sysmem_allocator);

  auto texture_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto target_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 = renderer->ImportBufferCollection(
      texture_collection_id, sysmem_allocator, std::move(texture_tokens.dup_token),
      BufferCollectionUsage::kClientImage, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  auto promise2 = renderer->ImportBufferCollection(
      target_collection_id, sysmem_allocator, std::move(target_tokens.dup_token),
      BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
  if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
    additional_format_modifiers.push_back(
        fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
  }
  const uint32_t kWidth = 64, kHeight = 32;
  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(texture_tokens.local_token),
                                          /* image_count */ 1, /* width */ kWidth,
                                          /* height */ kHeight,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(target_tokens.local_token),
                                          /* image_count */ 1, /* width */ kWidth,
                                          /* height */ kHeight,
                                          use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                          additional_format_modifiers);

  // Import render target.
  ImageMetadata render_target = {.collection_id = target_collection_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kWidth,
                                 .height = kHeight};
  auto promise3 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Import image.
  ImageMetadata image = {.collection_id = texture_collection_id,
                         .identifier = allocation::GenerateUniqueImageId(),
                         .vmo_index = 0,
                         .width = kWidth,
                         .height = kHeight};
  auto promise4 = renderer->ImportBufferImage(image, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise4)));

  // Now release the collection.
  renderer->ReleaseBufferImage(image.identifier);

  // Send an empty render.
  renderer->Render(render_target, {}, {});
}

// Test to make sure we can call the functions import kRenderTarget and kClientImage collections
// and ImportBufferImage() simultaneously from multiple threads and have it work.
void MultithreadingTest(Renderer* renderer, bool use_vulkan) {
  const uint32_t kNumThreads = 50;

  std::set<allocation::GlobalBufferCollectionId> bcid_set;
  std::mutex lock;

  auto register_and_import_function = [&renderer, &bcid_set, &lock, use_vulkan]() {
    // Make a test loop.
    async::TestLoop loop;

    // Make an extra sysmem allocator for tokens.
    fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator =
        utils::CreateSysmemAllocatorClient(loop.dispatcher(), "MultithreadingTest");

    auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator);
    auto bcid = allocation::GenerateUniqueBufferCollectionId();
    auto image_id = allocation::GenerateUniqueImageId();
    auto promise1 =
        renderer->ImportBufferCollection(bcid, sysmem_allocator, std::move(dup_token),
                                         BufferCollectionUsage::kRenderTarget, std::nullopt);
    ASSERT_TRUE(RunPromise(loop, std::move(promise1)));

    std::vector<fuchsia_images2::PixelFormatModifier> additional_format_modifiers;
    if (escher::VulkanIsSupported() && escher::test::GlobalEscherUsesVirtualGpu()) {
      additional_format_modifiers.push_back(
          fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal);
    }
    SetClientConstraintsAndWaitForAllocated(sysmem_allocator, std::move(local_token),
                                            /* image_count */ 1, /* width */ 64,
                                            /* height */ 32,
                                            use_vulkan ? get_none_usage() : get_cpu_usage_read(),
                                            additional_format_modifiers);

    // Add the bcid to the global vector in a thread-safe manner.
    {
      std::unique_lock<std::mutex> unique_lock(lock);
      bcid_set.insert(bcid);
    }

    // The buffer collection *should* be valid here.
    auto promise2 = renderer->ImportBufferImage(
        {.collection_id = bcid, .identifier = image_id, .vmo_index = 0, .width = 1, .height = 1},
        BufferCollectionUsage::kRenderTarget);
    EXPECT_TRUE(RunPromise(loop, std::move(promise2)));
  };

  // Run a bunch of threads, alternating between threads that import texture collections
  // and threads that import render target collections.
  std::vector<std::thread> threads;
  for (uint32_t i = 0; i < kNumThreads; i++) {
    threads.push_back(std::thread(register_and_import_function));
  }

  for (auto&& thread : threads) {
    thread.join();
  }

  // Import the ids here one more time to make sure the renderer's internal
  // state hasn't been corrupted. We use the values gathered in the bcid_vec
  // to test with.
  EXPECT_EQ(bcid_set.size(), kNumThreads);
  async::TestLoop loop;
  for (const auto& bcid : bcid_set) {
    // The buffer collection *should* be valid here.
    auto promise = renderer->ImportBufferImage({.collection_id = bcid,
                                                .identifier = allocation::GenerateUniqueImageId(),
                                                .vmo_index = 0,
                                                .width = 1,
                                                .height = 1},
                                               BufferCollectionUsage::kRenderTarget);
    EXPECT_TRUE(RunPromise(loop, std::move(promise)));
  }
}

// This test checks to make sure that the Render() function properly signals
// a zx::event which can be used by an async::Wait object to asynchronously
// call a custom function.
void AsyncEventSignalTest(async::TestLoop& loop, Renderer* renderer,
                          fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                          bool use_vulkan) {
  // Setup the render target collection.
  const uint32_t kWidth = 64, kHeight = 32;
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, kWidth, kHeight, BufferCollectionUsage::kRenderTarget,
                            renderer, sysmem_allocator, &client_target_info, target_ptr);

  // Now that the renderer and client have set their contraints, we can import the render target.
  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kWidth,
                                 .height = kHeight};
  auto promise = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise)));

  // Create the release fence that will be passed along to the Render()
  // function and be used to signal when we should release the collection.
  zx::event release_fence;
  auto status = zx::event::create(0, &release_fence);
  EXPECT_EQ(status, ZX_OK);

  // Set up the async::Wait object to wait until the release_fence signals
  // ZX_EVENT_SIGNALED. We make use of a test loop to access an async dispatcher.
  bool signaled = false;
  auto wait = std::make_unique<async::Wait>(release_fence.get(), ZX_EVENT_SIGNALED);
  wait->set_handler([&signaled](async_dispatcher_t*, async::Wait*, zx_status_t /*status*/,
                                const zx_packet_signal_t* /*signal*/) mutable { signaled = true; });
  wait->Begin(loop.dispatcher());

  // The call to Render() will signal the release fence, triggering the wait object to
  // call its handler function.
  std::vector<zx::event> fences;
  fences.push_back(std::move(release_fence));
  renderer->Render(render_target, {}, {.release_fences = fences});

  if (use_vulkan) {
    auto vk_renderer = static_cast<VkRenderer*>(renderer);
    vk_renderer->WaitIdle();
  }

  // Close the test loop and test that our handler was called.
  loop.RunUntilIdle();
  EXPECT_TRUE(signaled);
}

TEST_F(NullRendererTest, ImportCollectionTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  ImportCollectionTest(loop, &renderer, sysmem_allocator);
}

TEST_F(NullRendererTest, SameTokenTwiceTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  SameTokenTwiceTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, BadImageInputTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  BadImageInputTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, ImportImageTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  ImportImageTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, DeregistrationTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  DeregistrationTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, RenderImageAfterBufferCollectionReleasedTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  RenderImageAfterBufferCollectionReleasedTest(loop, &renderer, sysmem_allocator,
                                               /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, RenderAfterImageReleasedTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  RenderAfterImageReleasedTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, DISABLED_MultithreadingTest) {
  NullRenderer renderer;
  MultithreadingTest(&renderer, /*use_vulkan*/ false);
}

TEST_F(NullRendererTest, AsyncEventSignalTest) {
  async::TestLoop loop;
  NullRenderer renderer;
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  AsyncEventSignalTest(loop, &renderer, sysmem_allocator, /*use_vulkan*/ false);
}

VK_TEST_F(VulkanRendererTest, ImportCollectionTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  ImportCollectionTest(loop, renderer.get(), sysmem_allocator);
}

VK_TEST_F(VulkanRendererTest, SameTokenTwiceTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  SameTokenTwiceTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, BadImageInputTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  BadImageInputTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, ImportImageTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  ImportImageTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, DeregistrationTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  DeregistrationTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

// TODO(https://fxbug.dev/42144999) This test is flaking on FEMU.
VK_TEST_F(VulkanRendererTest, DISABLED_RenderImageAfterBufferCollectionReleasedTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  RenderImageAfterBufferCollectionReleasedTest(loop, renderer.get(), sysmem_allocator,
                                               /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, RenderAfterImageReleasedTest) {
  // TODO(https://fxbug.dev/42178670): Re-enable on FEMU once it doesn't flake.
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  RenderAfterImageReleasedTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, DISABLED_MultithreadingTest) {
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  MultithreadingTest(renderer.get(), /*use_vulkan*/ true);
}

VK_TEST_F(VulkanRendererTest, AsyncEventSignalTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());
  AsyncEventSignalTest(loop, renderer.get(), sysmem_allocator, /*use_vulkan*/ true);
}

// This test actually renders a rectangle using the VKRenderer. We create a single rectangle,
// with a half-red, half-green texture, and translate it. The render target is 16x8
// and the rectangle is 4x2. So in the end the result should look like this:
//
// ----------------
// ----------------
// ----------------
// ------RRGG------
// ------RRGG------
// ----------------
// ----------------
// ----------------
//
// It then renders the renderable a second time, this time with modified UVs so that only
// the green portion of the texture covers the rect, resulting in a fully green view despite
// the texture also having red pixels:
//
// ----------------
// ----------------
// ----------------
// ------GGGG------
// ------GGGG------
// ----------------
// ----------------
// ----------------
//
VK_TEST_F(VulkanRendererTest, RenderTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Setup renderable texture collection.
  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kClientImage, renderer.get(),
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // The texture width and height, also used for unnormalized texture coordinates.
  const uint32_t kTextureWidth = 4;
  const uint32_t kTextureHeight = 2;

  // Create the image meta data for the renderable.
  ImageMetadata renderable_texture = {.collection_id = collection_id,
                                      .identifier = allocation::GenerateUniqueImageId(),
                                      .vmo_index = 0,
                                      .width = static_cast<uint32_t>(kTextureWidth),
                                      .height = static_cast<uint32_t>(kTextureHeight)};

  auto promise1 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  auto promise2 =
      renderer->ImportBufferImage(renderable_texture, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  // Create a renderable where the upper-left hand corner should be at position (6,3) with a
  // width/height of (4,2).
  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RotateFlip::kIdentity());

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(
      client_collection_info, renderable_texture.vmo_index, HostPointerAccessMode::kWriteOnly,
      [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
        EXPECT_EQ(kBytesPerRGBAPixel,
                  utils::GetBytesPerPixel(client_collection_info.settings().value()));
        const uint32_t pixels_per_row =
            utils::GetPixelsPerRow(client_collection_info.settings().value(), kTextureWidth);

        // The texture only has 8 pixels, so it needs 32 write values for 4 channels. We
        // set the left half of pixels to red and the right half to green.
        const uint8_t kWriteRed[] = {/*red*/ 255U, 0, 0, 255U};
        const uint8_t kWriteGreen[] = {/*green*/ 0, 255U, 0, 255U};
        for (size_t y = 0; y < kTextureHeight; ++y) {
          for (size_t x = 0; x < kTextureWidth; ++x) {
            memcpy(&vmo_host[(y * pixels_per_row + x) * kBytesPerRGBAPixel],
                   x < kTextureWidth / 2 ? kWriteRed : kWriteGreen, kBytesPerRGBAPixel);
          }
        }
      });

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable}, {renderable_texture}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 3), glm::ivec4(255, 0, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 3), glm::ivec4(255, 0, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 3), glm::ivec4(0, 255, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 3), glm::ivec4(0, 255, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 4), glm::ivec4(255, 0, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 4), glm::ivec4(255, 0, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 4), glm::ivec4(0, 255, 0, 255));
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 4), glm::ivec4(0, 255, 0, 255));

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kTextureWidth * kTextureHeight);
                 });

  // Now let's update the src of the renderable so only the green portion of the image maps
  // onto the rect. Take the rightmost column of the image, which is green, to eliminate any linear
  // filtering artifacts.
  auto renderable2 = SrcToDest(types::RectangleF({.x = static_cast<float>(kTextureWidth - 1),
                                                  .y = 0,
                                                  .width = 1,
                                                  .height = static_cast<float>(kTextureHeight)}),
                               types::RectangleF({.x = 6,
                                                  .y = 3,
                                                  .width = static_cast<float>(kTextureWidth),
                                                  .height = static_cast<float>(kTextureHeight)}),
                               types::RotateFlip::kIdentity());

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable2}, {renderable_texture}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        // All of the renderable's pixels should be green.
        for (uint32_t i = 6; i < 6 + kTextureWidth; i++) {
          for (uint32_t j = 3; j < 3 + kTextureHeight; j++) {
            EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, i, j), glm::ivec4(0, 255, 0, 255));
          }
        }

        // Make sure the remaining pixels are black.
        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, kTextureWidth * kTextureHeight);
      });
}

// This test actually renders a rectangle using the VKRenderer. We create a single rectangle,
// with a half-red, half-green texture. The render target is 128x128 and the rectangle is 128x128.
// So in the end the result should look like this:
//
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
// RRRRRRRRGGGGGGGG
//
VK_TEST_F(VulkanRendererTest, FullScreenRenderTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  const uint32_t kWidth = 128;
  const uint32_t kHeight = 128;

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, kWidth, kHeight, BufferCollectionUsage::kRenderTarget,
                            renderer.get(), sysmem_allocator, &client_target_info, target_ptr);

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kWidth,
                                 .height = kHeight};
  auto promise1 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  // Setup renderable texture collection.
  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id = SetupBufferCollection(
      loop, 1, kWidth, kHeight, BufferCollectionUsage::kClientImage, renderer.get(),
      sysmem_allocator, &client_collection_info, collection_ptr);

  // Create the image meta data for the renderable.
  ImageMetadata renderable_texture = {.collection_id = collection_id,
                                      .identifier = allocation::GenerateUniqueImageId(),
                                      .vmo_index = 0,
                                      .width = static_cast<uint32_t>(kWidth),
                                      .height = static_cast<uint32_t>(kHeight)};
  auto promise2 =
      renderer->ImportBufferImage(renderable_texture, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kWidth),
                                          .height = static_cast<float>(kHeight)}),
                       types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kWidth),
                                          .height = static_cast<float>(kHeight)}),
                       types::RotateFlip::kIdentity());

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(
      client_collection_info, renderable_texture.vmo_index, HostPointerAccessMode::kWriteOnly,
      [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
        EXPECT_EQ(kBytesPerRGBAPixel,
                  utils::GetBytesPerPixel(client_collection_info.settings().value()));
        const uint32_t pixels_per_row =
            utils::GetPixelsPerRow(client_collection_info.settings().value(), kWidth);

        const uint8_t kWriteRed[] = {/*red*/ 255U, 0, 0, 255U};
        const uint8_t kWriteGreen[] = {/*green*/ 0, 255U, 0, 255U};
        for (size_t y = 0; y < kHeight; ++y) {
          for (size_t x = 0; x < kWidth; ++x) {
            memcpy(&vmo_host[(y * pixels_per_row + x) * kBytesPerRGBAPixel],
                   x < kWidth / 2 ? kWriteRed : kWriteGreen, kBytesPerRGBAPixel);
          }
        }

        // Flush the cache after writing to host VMO.
        EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_host, pixels_per_row * kHeight * kBytesPerRGBAPixel,
                                        ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
      });

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable}, {renderable_texture}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   EXPECT_EQ(kBytesPerRGBAPixel,
                             utils::GetBytesPerPixel(client_target_info.settings().value()));
                   const uint32_t pixels_per_row =
                       utils::GetPixelsPerRow(client_target_info.settings().value(), kWidth);

                   // Flush the cache before reading back target image.
                   EXPECT_EQ(ZX_OK,
                             zx_cache_flush(vmo_host, pixels_per_row * kHeight * kBytesPerRGBAPixel,
                                            ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));

                   const auto kReadRed = glm::ivec4(255, 0, 0, 255);
                   const auto kReadGreen = glm::ivec4(0, 255, 0, 255);
                   int num_other_pixels = 0;
                   // Make sure the pixels are in the right order.
                   for (uint32_t y = 0; y < kHeight; ++y) {
                     for (uint32_t x = 0; x < kWidth; ++x) {
                       const auto pixel = GetPixel(vmo_host, pixels_per_row, x, y);
                       if (pixel != (x < kWidth / 2 ? kReadRed : kReadGreen)) {
                         if (!num_other_pixels) {
                           FX_LOGS(ERROR) << "Unexpected pixel: " << pixel.r << "," << pixel.g
                                          << "," << pixel.b << "," << pixel.a;
                         }
                         ++num_other_pixels;
                       }
                     }
                   }
                   EXPECT_EQ(num_other_pixels, 0);
                 });
}

// This test actually renders a rectangle using the VKRenderer. We create a single rectangle,
// with a half-red, half-green texture, and translate it. The render target is 32x16
// and the rectangle is 6x2. So in the end the result should look like this:
//
// ----------------
// ----------------
// ----------------
// ------RRGG------
// ------RRGG------
// ----------------
// ----------------
// ----------------
//
// It then renders the renderable more times, rotating it 90* clockwise each time. This results in
// the following images:
//
// ----------------
// ----------------
// -------RR-------
// -------RR-------
// -------GG-------
// -------GG-------
// ----------------
// ----------------
//
// ----------------
// ----------------
// ----------------
// ------GGRR------
// ------GGRR------
// ----------------
// ----------------
// ----------------
//
// ----------------
// ----------------
// -------GG-------
// -------GG-------
// -------RR-------
// -------RR-------
// ----------------
// ----------------
//
VK_TEST_F(VulkanRendererTest, RotationRenderTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kClientImage, renderer.get(),
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 2, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  const uint32_t kTargetWidth = 32;
  const uint32_t kTargetHeight = 16;

  const uint32_t kTargetWidthFlipped = kTargetHeight;
  const uint32_t kTargetHeightFlipped = kTargetWidth;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create another render target with dimensions flipped.
  ImageMetadata render_target_flipped = {.collection_id = target_id,
                                         .identifier = allocation::GenerateUniqueImageId(),
                                         .vmo_index = 1,
                                         .width = kTargetWidthFlipped,
                                         .height = kTargetHeightFlipped};

  // The texture width and height, also used for unnormalized texture coordinates.
  const uint32_t kTextureWidth = 6;
  const uint32_t kTextureHeight = 2;

  // Create the image meta data for the renderable.
  ImageMetadata renderable_texture = {.collection_id = collection_id,
                                      .identifier = allocation::GenerateUniqueImageId(),
                                      .vmo_index = 0,
                                      .width = static_cast<uint32_t>(kTextureWidth),
                                      .height = static_cast<uint32_t>(kTextureHeight)};

  auto promise1 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 =
      renderer->ImportBufferImage(render_target_flipped, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 =
      renderer->ImportBufferImage(renderable_texture, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create a renderable where the upper-left hand corner should be at position (5,3)
  // with a width/height of (6,2).
  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RectangleF({.x = 5,
                                          .y = 3,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RotateFlip::kIdentity());

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(
      client_collection_info, renderable_texture.vmo_index, HostPointerAccessMode::kWriteOnly,
      [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
        EXPECT_EQ(kBytesPerRGBAPixel,
                  utils::GetBytesPerPixel(client_collection_info.settings().value()));
        const uint32_t pixels_per_row =
            utils::GetPixelsPerRow(client_collection_info.settings().value(), kTextureWidth);

        // The texture only has 8 pixels, so it needs 32 write values for 4 channels. We
        // set the left half of pixels to red and the right half to green.
        const uint8_t kWriteRed[] = {/*red*/ 255U, 0, 0, 255U};
        const uint8_t kWriteGreen[] = {/*green*/ 0, 255U, 0, 255U};
        for (size_t y = 0; y < kTextureHeight; ++y) {
          for (size_t x = 0; x < kTextureWidth; ++x) {
            memcpy(&vmo_host[(y * pixels_per_row + x) * kBytesPerRGBAPixel],
                   x < kTextureWidth / 2 ? kWriteRed : kWriteGreen, kBytesPerRGBAPixel);
          }
        }
      });

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable}, {renderable_texture}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   const auto red = glm::ivec4(255, 0, 0, 255);
                   const auto green = glm::ivec4(0, 255, 0, 255);

                   // Reds (left)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 4), red);

                   // Greens (right)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 4), green);

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kTextureWidth * kTextureHeight);
                 });

  // Now let's update the renderable so it is rotated 90 deg.
  auto layers_90deg = screen_capture::ScreenCapture::RotateRenderables(
      MakeLayers({renderable}, {renderable_texture}),
      fuchsia_ui_composition::Rotation::kCw90Degrees, kTargetWidthFlipped, kTargetHeightFlipped);
  // Render the renderable to the render target.
  renderer->Render(render_target_flipped, layers_90deg, {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target_flipped.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        // Make sure the pixels are in the right order.
        const auto red = glm::ivec4(255, 0, 0, 255);
        const auto green = glm::ivec4(0, 255, 0, 255);

        // Reds (top)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 5), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 5), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 6), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 6), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 7), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 7), red);

        // Greens (bottom)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 8), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 8), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 9), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 9), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 10), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 10), green);

        // Make sure the remaining pixels are black.

        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, kTextureWidth * kTextureHeight);
      });

  // Now let's update the renderable so it is rotated 180 deg.
  auto layers_180deg = screen_capture::ScreenCapture::RotateRenderables(
      MakeLayers({renderable}, {renderable_texture}),
      fuchsia_ui_composition::Rotation::kCw180Degrees, 16, 8);
  // Render the renderable to the render target.
  renderer->Render(render_target, layers_180deg, {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   const auto red = glm::ivec4(255, 0, 0, 255);
                   const auto green = glm::ivec4(0, 255, 0, 255);

                   // Greens (left)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 4), green);

                   // Reds (right)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 4), red);

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kTextureWidth * kTextureHeight);
                 });

  // Now let's update the renderable so it is rotated 270 deg.
  auto layers_270deg = screen_capture::ScreenCapture::RotateRenderables(
      MakeLayers({renderable}, {renderable_texture}),
      fuchsia_ui_composition::Rotation::kCw270Degrees, kTargetWidthFlipped, kTargetHeightFlipped);
  // Render the renderable to the render target.
  renderer->Render(render_target_flipped, layers_270deg, {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target_flipped.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        // Make sure the pixels are in the right order.
        const auto red = glm::ivec4(255, 0, 0, 255);
        const auto green = glm::ivec4(0, 255, 0, 255);

        // Greens (top)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 21), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 21), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 22), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 22), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 23), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 23), green);

        // Reds (bottom)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 24), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 24), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 25), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 25), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 3, 26), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 4, 26), red);

        // Make sure the remaining pixels are black.
        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, kTextureWidth * kTextureHeight);
      });
}

// This test actually renders a rectangle using the VKRenderer. We create a single rectangle,
// with a half-red, half-green texture (with red first). We translate it and flip the texture on the
// left-right axis. The render target is 32x16 and the rectangle is 6x2. So in the end the result
// should look like this (note that green is now first):
//
// ----------------
// ----------------
// ----------------
// ------GGGRRR----
// ------GGGRRR----
// ----------------
// ----------------
// ----------------
//
// It then renders the renderable more one more time, rotating it 90* clockwise. This results in the
// following image:
//
// ----------------
// ----------------
// -------GG-------
// -------GG-------
// -------GG-------
// -------RR-------
// -------RR-------
// -------RR-------
// ----------------
// ----------------
//
VK_TEST_F(VulkanRendererTest, FlipLeftRightAndRotate90RenderTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();
  auto unique_escher = std::make_unique<escher::Escher>(
      env->GetVulkanDevice(), env->GetFilesystem(), /*gpu_allocator*/ nullptr);
  VkRenderer renderer(unique_escher->GetWeakPtr());
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kClientImage, &renderer,
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 2, 60, 40, BufferCollectionUsage::kRenderTarget, &renderer,
                            sysmem_allocator, &client_target_info, target_ptr);

  const uint32_t kTargetWidth = 32;
  const uint32_t kTargetHeight = 16;

  const uint32_t kTargetWidthFlipped = kTargetHeight;
  const uint32_t kTargetHeightFlipped = kTargetWidth;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create another render target with dimensions flipped.
  ImageMetadata render_target_flipped = {.collection_id = target_id,
                                         .identifier = allocation::GenerateUniqueImageId(),
                                         .vmo_index = 1,
                                         .width = kTargetWidthFlipped,
                                         .height = kTargetHeightFlipped};

  // The texture width and height, also used for unnormalized texture coordinates.
  const uint32_t kTextureWidth = 6;
  const uint32_t kTextureHeight = 2;

  // Create the image meta data for the renderable.
  TestLayerInfo renderable_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = static_cast<uint32_t>(kTextureWidth),
                   .height = static_cast<uint32_t>(kTextureHeight)},
  };

  auto promise1 = renderer.ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 =
      renderer.ImportBufferImage(render_target_flipped, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 =
      renderer.ImportBufferImage(renderable_texture.metadata, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create a renderable where the upper-left hand corner should be at position (5,3)
  // with a width/height of (6,2).
  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RectangleF({.x = 5,
                                          .y = 3,
                                          .width = static_cast<float>(kTextureWidth),
                                          .height = static_cast<float>(kTextureHeight)}),
                       types::RotateFlip::From(fuchsia_ui_composition::Orientation::kCcw0Degrees,
                                               fuchsia_ui_composition::ImageFlip::kLeftRight));

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(
      client_collection_info, renderable_texture.metadata.vmo_index,
      HostPointerAccessMode::kWriteOnly, [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
        EXPECT_EQ(kBytesPerRGBAPixel,
                  utils::GetBytesPerPixel(client_collection_info.settings().value()));
        const uint32_t pixels_per_row =
            utils::GetPixelsPerRow(client_collection_info.settings().value(), kTextureWidth);

        // The texture only has 8 pixels, so it needs 32 write values for 4 channels. We
        // set the left half of pixels to red and the right half to green.
        const uint8_t kWriteRed[] = {/*red*/ 255U, 0, 0, 255U};
        const uint8_t kWriteGreen[] = {/*green*/ 0, 255U, 0, 255U};
        for (size_t y = 0; y < kTextureHeight; ++y) {
          for (size_t x = 0; x < kTextureWidth; ++x) {
            memcpy(&vmo_host[(y * pixels_per_row + x) * kBytesPerRGBAPixel],
                   x < kTextureWidth / 2 ? kWriteRed : kWriteGreen, kBytesPerRGBAPixel);
          }
        }
      });

  // Render the renderable to the render target.
  renderer.Render(render_target, MakeLayers({renderable}, {renderable_texture}), {});
  renderer.WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   const auto red = glm::ivec4(255, 0, 0, 255);
                   const auto green = glm::ivec4(0, 255, 0, 255);

                   // Greens (left)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 5, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 6, 4), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 3), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 7, 4), green);

                   // Reds (right)
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 8, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 9, 4), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 3), red);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 10, 4), red);

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kTextureWidth * kTextureHeight);
                 });

  // Now let's update the renderable so it is rotated 90 deg.
  auto layers_90deg = screen_capture::ScreenCapture::RotateRenderables(
      MakeLayers({renderable}, {renderable_texture}),
      fuchsia_ui_composition::Rotation::kCw90Degrees, kTargetWidthFlipped, kTargetHeightFlipped);
  // Render the renderable to the render target.
  renderer.Render(render_target_flipped, layers_90deg, {});
  renderer.WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target_flipped.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        // Make sure the pixels are in the right order.
        const auto red = glm::ivec4(255, 0, 0, 255);
        const auto green = glm::ivec4(0, 255, 0, 255);

        // Greens (top)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 5), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 5), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 6), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 6), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 7), green);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 7), green);

        // Reds (bottom)
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 8), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 8), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 9), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 9), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 11, 10), red);
        EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 12, 10), red);

        // Make sure the remaining pixels are black.

        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, kTextureWidth * kTextureHeight);
      });
}

// This test actually renders a rectangle using the VKRenderer. We create a single rectangle,
// with a half-red, half-green texture (with red on top). We translate it and flip the texture on
// the up-down axis. The render target is 32x16 and the rectangle is 1x2. So in the end the result
// should look like this (note that green is now first):
//
// G---------
// R---------
// ----------
//
// It then renders the renderable more one more time, rotating it 90* clockwise. This results in the
// following image:
//
// --------RG
// ----------
// ----------
//
VK_TEST_F(VulkanRendererTest, FlipUpDownAndRotate90RenderTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();
  auto unique_escher = std::make_unique<escher::Escher>(
      env->GetVulkanDevice(), env->GetFilesystem(), /*gpu_allocator*/ nullptr);
  VkRenderer renderer(unique_escher->GetWeakPtr());
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kClientImage, &renderer,
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 2, 60, 40, BufferCollectionUsage::kRenderTarget, &renderer,
                            sysmem_allocator, &client_target_info, target_ptr);

  const uint32_t kTargetWidth = 32;
  const uint32_t kTargetHeight = 16;

  const uint32_t kTargetWidthFlipped = kTargetHeight;
  const uint32_t kTargetHeightFlipped = kTargetWidth;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create another render target with dimensions flipped.
  ImageMetadata render_target_flipped = {.collection_id = target_id,
                                         .identifier = allocation::GenerateUniqueImageId(),
                                         .vmo_index = 1,
                                         .width = kTargetWidthFlipped,
                                         .height = kTargetHeightFlipped};

  // The texture width and height, also used for unnormalized texture coordinates.
  const float w = 1;
  const float h = 2;

  // Create the image meta data for the renderable.
  TestLayerInfo renderable_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = static_cast<uint32_t>(w),
                   .height = static_cast<uint32_t>(h)},
  };

  auto promise1 = renderer.ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 =
      renderer.ImportBufferImage(render_target_flipped, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 =
      renderer.ImportBufferImage(renderable_texture.metadata, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create a renderable where the upper-left hand corner should be at position (0, 0)
  // with a width/height of (2,6).
  const uint32_t kRenderableWidth = 1;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 0, .y = 0, .width = w, .height = h}),
                       types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}),
                       types::RotateFlip::From(fuchsia_ui_composition::Orientation::kCcw0Degrees,
                                               fuchsia_ui_composition::ImageFlip::kUpDown));

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(
      client_collection_info, renderable_texture.metadata.vmo_index,
      HostPointerAccessMode::kWriteOnly, [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
        EXPECT_EQ(4u, utils::GetBytesPerPixel(client_collection_info.settings().value()));
        uint32_t pixels_per_row =
            utils::GetPixelsPerRow(client_collection_info.settings().value(), 1U);

        const uint8_t kNumWrites = static_cast<uint8_t>((pixels_per_row * 4) + 4);

        const uint8_t kWriteRed[] = {/*red*/ 255U, 0, 0, 255U};
        const uint8_t kWriteGreen[] = {/*green*/ 0, 255U, 0, 255U};

        memcpy(vmo_host, kWriteRed, sizeof(kWriteRed));
        memcpy(&vmo_host[pixels_per_row * 4], kWriteGreen, sizeof(kWriteGreen));
      });

  // Render the renderable to the render target.
  renderer.Render(render_target, MakeLayers({renderable}, {renderable_texture}), {});
  renderer.WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   const auto red = glm::ivec4(255, 0, 0, 255);
                   const auto green = glm::ivec4(0, 255, 0, 255);

                   // Green is now on top after the texture is flipped up-down.
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 0, 0), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, 0, 1), red);

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kRenderableWidth * kRenderableHeight);
                 });

  // Now let's update the renderable so it is rotated 90 deg.
  auto layers_90deg = screen_capture::ScreenCapture::RotateRenderables(
      MakeLayers({renderable}, {renderable_texture}),
      fuchsia_ui_composition::Rotation::kCw90Degrees, kTargetWidthFlipped, kTargetHeightFlipped);
  // Render the renderable to the render target.
  renderer.Render(render_target_flipped, layers_90deg, {});
  renderer.WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target_flipped.vmo_index,
                 HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are in the right order.
                   const auto red = glm::ivec4(255, 0, 0, 255);
                   const auto green = glm::ivec4(0, 255, 0, 255);

                   // Green is on the right, as rotation is applied after flip.
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 15, 0), green);
                   EXPECT_EQ(GetPixel(vmo_host, kTargetWidthFlipped, 14, 0), red);

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      kRenderableWidth * kRenderableHeight);
                 });
}

class VulkanRendererColorTest : public VulkanRendererTest {};

// Tests if the VK renderer can handle rendering an image without a provided image
// and only a multiply color (which means that we do not allocate an image for the
// renderable in this test, only the render target).
VK_TEST_F(VulkanRendererColorTest, SolidColorTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  // Create the render_target image metadata.
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  TestLayerInfo renderable_image_data = {
      .metadata = {.identifier = allocation::kInvalidImageId},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .multiply_color = {1.f, 0.4f, 0.f, 1.f},
  };

  auto promise = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise)));

  // Create the two renderables.
  const uint32_t kRenderableWidth = 4;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable}, {renderable_image_data}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t linear_vals[num_bytes];
                   sRGBtoLinear(vmo_host, linear_vals, num_bytes);

                   // Make sure the pixels are in the right order give that we rotated
                   // the rectangle. Values are BGRA.
                   for (uint32_t i = 6; i < 6 + kRenderableWidth; i++) {
                     for (uint32_t j = 3; j < 3 + kRenderableHeight; j++) {
                       auto pixel = GetPixel(linear_vals, kTargetWidth, i, j);
                       // The sRGB conversion function provides slightly different results depending
                       // on the platform.
                       EXPECT_TRUE(pixel == glm::ivec4(255, 101, 0, 255) ||
                                   pixel == glm::ivec4(255, 102, 0, 255));
                     }
                   }

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, 8U);
                 });
}

// Test that colors change properly when we apply a color correction matrix.
VK_TEST_F(VulkanRendererColorTest, ColorCorrectionTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Set the color correction data on the renderer.
  static const fidl::Array<float, 3> preoffsets = {0, 0, 0};
  static const fidl::Array<float, 9> matrix = {0.288299f, 0.052709f,  -0.257912f,
                                               0.711701f, 0.947291f,  0.257912f,
                                               0.000000f, -0.000000f, 1.000000f};
  static const fidl::Array<float, 3> postoffsets = {0, 0, 0};
  renderer->SetColorConversionValues(matrix, preoffsets, postoffsets);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  // Create the render_target image metadata.
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  TestLayerInfo renderable_image_data = {
      .metadata = {.identifier = allocation::kInvalidImageId},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .multiply_color = {1, 0, 0, 1},
  };

  auto promise = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise)));

  // Create the two renderables.
  const uint32_t kRenderableWidth = 4;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));

  // Render the renderable to the render target.
  renderer->Render(render_target, MakeLayers({renderable}, {renderable_image_data}),
                   {.apply_color_conversion = true});
  renderer->WaitIdle();

  // Calculate expected color.
  float values[16] = {matrix[0], matrix[3], matrix[6], 0, matrix[1], matrix[4], matrix[7], 0,
                      matrix[2], matrix[5], matrix[8], 0, 0,         0,         0,         1};
  glm::mat4 glm_matrix = glm::make_mat4(values);
  auto expected_color_float = glm_matrix * glm::vec4(1, 0, 0, 1);

  // Order needs to be RGBA.
  glm::ivec4 expected_color = {
      static_cast<uint8_t>(std::max(expected_color_float.x * 255, 0.f)),
      static_cast<uint8_t>(std::max(expected_color_float.y * 255, 0.f)),
      static_cast<uint8_t>(std::max(expected_color_float.z * 255, 0.f)),
      static_cast<uint8_t>(std::max(expected_color_float.w * 255, 0.f)),
  };

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        uint8_t linear_vals[num_bytes];
        sRGBtoLinear(vmo_host, linear_vals, num_bytes);

        // Make sure the pixels are in the right order give that we rotated
        // the rectangle. Values are RGBA.
        for (uint32_t i = 6; i < 6 + kRenderableWidth; i++) {
          for (uint32_t j = 3; j < 3 + kRenderableHeight; j++) {
            auto pixel = GetPixel(linear_vals, kTargetWidth, i, j);
            for (uint32_t k = 0; k < 4; k++) {
              // Due to different GPU floating point implementations, and other rounding
              // issues with converting between linear and sRGB, the pixel values may be
              // off by 1.
              EXPECT_TRUE(pixel[k] == expected_color[k] || pixel[k] == expected_color[k] - 1);
            }
          }
        }

        // Make sure the remaining pixels are black.
        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, 8U);
      });
}

// Tests if the VK renderer can handle rendering 2 solid color images. Since solid
// color images make use of a shared default 1x1 white texture within the vk renderer,
// this tests to make sure that there aren't any problems that arise from this sharing.
VK_TEST_F(VulkanRendererColorTest, MultipleSolidColorTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  // Create the render_target image metadata.
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create the image meta data for the solid color renderable - red.
  TestLayerInfo renderable_image_data = {
      .metadata = {.identifier = allocation::kInvalidImageId},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .multiply_color = {1, 0, 0, 1},
  };

  // Create the image meta data for the other solid color renderable - blue.
  TestLayerInfo renderable_image_data_2 = {
      .metadata = {.identifier = allocation::kInvalidImageId},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .multiply_color = {0, 0, 1, 1},
  };

  auto promise = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise)));

  // Create the two renderables.
  const uint32_t kRenderableWidth = 4;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));
  SrcToDest renderable_2(types::RectangleF({.x = 6,
                                            .y = 5,
                                            .width = static_cast<float>(kRenderableWidth),
                                            .height = static_cast<float>(kRenderableHeight)}));

  // Render the renderable to the render target.
  renderer->Render(
      render_target,
      MakeLayers({renderable, renderable_2}, {renderable_image_data, renderable_image_data_2}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        uint8_t linear_vals[num_bytes];
        sRGBtoLinear(vmo_host, linear_vals, num_bytes);

        // Make sure the pixels are in the right order give that we rotated
        // the rectangle.
        for (uint32_t i = 6; i < 6 + kRenderableWidth; i++) {
          for (uint32_t j = 3; j < 3 + kRenderableHeight; j++) {
            EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, i, j), glm::ivec4(255, 0, 0, 255));
          }
        }

        for (uint32_t i = 6; i < 6 + kRenderableWidth; i++) {
          for (uint32_t j = 5; j < 5 + kRenderableHeight; j++) {
            EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, i, j), glm::ivec4(0, 0, 255, 255));
          }
        }

        // Make sure the remaining pixels are black.
        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, 16U);
      });
}

// Tests if the VK renderer can handle rendering a solid color rectangle as well as
// an image-backed rectangle. Make sure that the two rectangles, if given the same
// dimensions, occupy the exact same number of pixels.
VK_TEST_F(VulkanRendererColorTest, MixSolidColorAndImageTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Both renderables should be the same size.
  const uint32_t kRenderableWidth = 93;
  const uint32_t kRenderableHeight = 78;
  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 100, 100, BufferCollectionUsage::kClientImage, renderer.get(),
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  const uint32_t kTargetWidth = 200;
  const uint32_t kTargetHeight = 100;
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 200, 100, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create the image meta data for the solid color renderable - green.
  TestLayerInfo renderable_image_data = {
      .metadata = {.identifier = allocation::kInvalidImageId},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .multiply_color = {0, 1, 0, 1},
  };

  // Create the image meta data for the image backed renderable - red.
  TestLayerInfo renderable_image_data_2 = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = kRenderableWidth,
                   .height = kRenderableHeight},
  };

  // Have the client write pixel values to the renderable's texture. They should all be red.
  MapHostPointer(client_collection_info, renderable_image_data_2.metadata.vmo_index,
                 HostPointerAccessMode::kWriteOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t writeValues[num_bytes];
                   for (uint32_t i = 0; i < num_bytes; i += 4) {
                     writeValues[i] = 255U;
                     writeValues[i + 1] = 0;
                     writeValues[i + 2] = 0;
                     writeValues[i + 3] = 255U;
                   }

                   memcpy(vmo_host, writeValues, sizeof(writeValues));
                 });

  auto promise1 = renderer->ImportBufferImage(renderable_image_data_2.metadata,
                                              BufferCollectionUsage::kClientImage);
  ASSERT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise2)));

  // Create the two renderables.
  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));
  SrcToDest renderable_2(types::RectangleF({.x = static_cast<float>(kRenderableWidth + 1),
                                            .y = 0,
                                            .width = static_cast<float>(kRenderableWidth),
                                            .height = static_cast<float>(kRenderableHeight)}));

  // Render the renderable to the render target.
  renderer->Render(
      render_target,
      MakeLayers({renderable, renderable_2}, {renderable_image_data, renderable_image_data_2}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the two renderables were rendered side by
  // side at the same size.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t linear_vals[num_bytes];
                   sRGBtoLinear(vmo_host, linear_vals, num_bytes);

                   uint32_t num_red = 0, num_green = 0;
                   for (uint32_t i = 0; i < kTargetWidth; i++) {
                     for (uint32_t j = 0; j < kTargetHeight; j++) {
                       auto pixel = GetPixel(linear_vals, kTargetWidth, i, j);
                       if (pixel == glm::ivec4(0, 255, 0, 255)) {
                         num_green++;
                       } else if (pixel == glm::ivec4(255, 0, 0, 255)) {
                         num_red++;
                       }
                     }
                   }

                   EXPECT_EQ(num_green, num_red);
                   EXPECT_EQ(num_green, kRenderableWidth * kRenderableHeight);
                   EXPECT_EQ(num_red, kRenderableWidth * kRenderableHeight);
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight,
                                      2 * (kRenderableWidth * kRenderableHeight));
                 });
}

// Tests transparency. Render two overlapping rectangles, a red opaque one covered slightly by
// a green transparent one with an alpha of 0.5. The result should look like this:
//
// ----------------
// ----------------
// ----------------
// ------RYYYG----
// ------RYYYG----
// ----------------
// ----------------
// ----------------
VK_TEST_F(VulkanRendererColorTest, TransparencyTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 2, 60, 40, BufferCollectionUsage::kClientImage, renderer.get(),
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create the image meta data for the renderable.
  TestLayerInfo renderable_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = 1,
                   .height = 1},
  };

  // Create the texture that will go on the transparent renderable.
  TestLayerInfo transparent_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 1,
                   .width = 1,
                   .height = 1},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
  };

  // Import all the images.
  auto promise1 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 =
      renderer->ImportBufferImage(renderable_texture.metadata, BufferCollectionUsage::kClientImage);
  ASSERT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 = renderer->ImportBufferImage(transparent_texture.metadata,
                                              BufferCollectionUsage::kClientImage);
  ASSERT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create the two renderables.
  const uint32_t kRenderableWidth = 4;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));
  SrcToDest transparent_renderable(
      types::RectangleF({.x = 7,
                         .y = 3,
                         .width = static_cast<float>(kRenderableWidth),
                         .height = static_cast<float>(kRenderableHeight)}));

  // Have the client write pixel values to the renderable's texture.
  MapHostPointer(client_collection_info, renderable_texture.metadata.vmo_index,
                 HostPointerAccessMode::kWriteOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Create a red opaque pixel.
                   const uint8_t kNumWrites = 4;
                   const uint8_t kWriteValues[] = {/*red*/ 255U, 0, 0, 255U};
                   memcpy(vmo_host, kWriteValues, sizeof(kWriteValues));
                 });

  MapHostPointer(client_collection_info, transparent_texture.metadata.vmo_index,
                 HostPointerAccessMode::kWriteOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Create a green pixel with an alpha of 0.5.
                   const uint8_t kNumWrites = 4;
                   const uint8_t kWriteValues[] = {/*red*/ 0, 255, 0, 128U};
                   memcpy(vmo_host, kWriteValues, sizeof(kWriteValues));
                 });

  // Render the renderable to the render target.
  renderer->Render(
      render_target,
      MakeLayers({renderable, transparent_renderable}, {renderable_texture, transparent_texture}),
      {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(
      client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
      [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
        uint8_t linear_vals[num_bytes];
        sRGBtoLinear(vmo_host, linear_vals, num_bytes);

        // Make sure the pixels are in the right order give that we rotated
        // the rectangle.
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 6, 3), glm::ivec4(255, 0, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 6, 4), glm::ivec4(255, 0, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 7, 3), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 7, 4), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 8, 3), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 8, 4), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 9, 3), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 9, 4), glm::ivec4(126, 255, 0, 255));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 10, 3), glm::ivec4(0, 255, 0, 128));
        EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 10, 4), glm::ivec4(0, 255, 0, 128));

        // Make sure the remaining pixels are black.
        CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, 10U);
      });
}

// Partial ordering, true if it's true for all components.
bool operator<=(const glm::tvec4<int, glm::packed_highp>& a,
                const glm::tvec4<int, glm::packed_highp>& b) {
  return a.x <= b.x && a.y <= b.y && a.z <= b.z && a.w <= b.w;
}

MATCHER_P2(InRange, low, high, "") { return low <= arg && arg <= high; }

class VulkanRendererParameterizedMultiplyColorTest
    : public VulkanRendererTest,
      public ::testing::WithParamInterface<BlendMode> {};

// Tests the multiply color for images, which can also affect transparency.
// Render two overlapping rectangles, a red opaque one covered slightly by
// a green transparent one with an alpha of 0.5. These values are set not
// on the pixel values of the images which should be all white and opaque
// (1,1,1,1) but instead via the multiply_color value on the ImageMetadata.
// ----------------
// ----------------
// ----------------
// ------RYYYG----
// ------RYYYG----
// ----------------
// ----------------
// ----------------
VK_TEST_P(VulkanRendererParameterizedMultiplyColorTest, MultiplyColorTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  fuchsia_sysmem2::BufferCollectionInfo client_collection_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection_ptr;
  auto collection_id =
      SetupBufferCollection(loop, 1, 1, 1, BufferCollectionUsage::kClientImage, renderer.get(),
                            sysmem_allocator, &client_collection_info, collection_ptr);

  // Setup the render target collection.
  fuchsia_sysmem2::BufferCollectionInfo client_target_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr;
  auto target_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kRenderTarget, renderer.get(),
                            sysmem_allocator, &client_target_info, target_ptr);

  const BlendMode blend_mode = GetParam();
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;

  // Create the render_target image metadata.
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};

  // Create the image meta data for the renderable.
  TestLayerInfo renderable_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = 1,
                   .height = 1},
      .blend_mode = blend_mode,
      .multiply_color = {1, 0, 0, 1},
  };

  // Create the texture that will go on the transparent renderable.
  std::array<float, 4> transparent_color;
  switch (blend_mode.enum_value()) {
    case BlendMode::Enum::kPremultipliedAlpha:
      transparent_color = {0, 0.5, 0, 0.5};
      break;
    case BlendMode::Enum::kStraightAlpha:
      transparent_color = {0, 1, 0, 0.5};
      break;
    default:
      GTEST_FAIL() << "Unsupported blend mode";
      break;
  }
  TestLayerInfo transparent_texture = {
      .metadata = {.collection_id = collection_id,
                   .identifier = allocation::GenerateUniqueImageId(),
                   .vmo_index = 0,
                   .width = 1,
                   .height = 1},
      .blend_mode = blend_mode,
      .multiply_color = transparent_color,
  };

  // Import all the images.
  auto promise1 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise1)));
  auto promise2 =
      renderer->ImportBufferImage(renderable_texture.metadata, BufferCollectionUsage::kClientImage);
  ASSERT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 = renderer->ImportBufferImage(transparent_texture.metadata,
                                              BufferCollectionUsage::kClientImage);
  ASSERT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create the two renderables.
  const uint32_t kRenderableWidth = 4;
  const uint32_t kRenderableHeight = 2;
  SrcToDest renderable(types::RectangleF({.x = 6,
                                          .y = 3,
                                          .width = static_cast<float>(kRenderableWidth),
                                          .height = static_cast<float>(kRenderableHeight)}));
  SrcToDest transparent_renderable(
      types::RectangleF({.x = 7,
                         .y = 3,
                         .width = static_cast<float>(kRenderableWidth),
                         .height = static_cast<float>(kRenderableHeight)}));

  // Have the client write white pixel values to image backing the above two renderables.
  MapHostPointer(client_collection_info, renderable_texture.metadata.vmo_index,
                 HostPointerAccessMode::kWriteOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Create a red opaque pixel.
                   const uint8_t kNumWrites = 4;
                   const uint8_t kWriteValues[] = {/*red*/ 255U, /*green*/ 255U, /*blue*/ 255U,
                                                   /*alpha*/ 255U};
                   memcpy(vmo_host, kWriteValues, sizeof(kWriteValues));
                 });

  // Render the renderable to the render target.
  renderer->Render(
      render_target,
      MakeLayers({renderable, transparent_renderable}, {renderable_texture, transparent_texture}),
      {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target
  // and read its values. This should show that the renderable was rendered to the center
  // of the render target, with its associated texture.
  MapHostPointer(client_target_info, render_target.vmo_index, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t linear_vals[num_bytes];
                   sRGBtoLinear(vmo_host, linear_vals, num_bytes);

                   // Different platforms have slightly different sRGB<->linear conversions, so use
                   // fuzzy matching. Offset for Intel Gen:
                   constexpr uint32_t kLoOfs = -1;
                   // Offset for ARM Mali:
                   constexpr uint32_t kHiOfs = +1;
                   auto kMultiLowVal = glm::ivec4(127 + kLoOfs, 127 + kLoOfs, 0, 255);
                   auto kMultiHighVal = glm::ivec4(127 + kHiOfs, 127 + kHiOfs, 0, 255);
                   auto kGreenLowVal = glm::ivec4(0, 127 + kLoOfs, 0, 128);
                   auto kGreenHighVal = glm::ivec4(0, 127 + kHiOfs, 0, 128);

                   // Make sure the pixels are in the right order give that we rotated
                   // the rectangle.
                   EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 6, 3), glm::ivec4(255, 0, 0, 255));
                   EXPECT_EQ(GetPixel(linear_vals, kTargetWidth, 6, 4), glm::ivec4(255, 0, 0, 255));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 7, 3),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 7, 4),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 8, 3),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 8, 4),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 9, 3),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 9, 4),
                               InRange(kMultiLowVal, kMultiHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 10, 3),
                               InRange(kGreenLowVal, kGreenHighVal));
                   EXPECT_THAT(GetPixel(linear_vals, kTargetWidth, 10, 4),
                               InRange(kGreenLowVal, kGreenHighVal));

                   // Make sure the remaining pixels are black.
                   CHECK_BLACK_PIXELS(vmo_host, kTargetWidth, kTargetHeight, 10U);
                 });
}

INSTANTIATE_TEST_SUITE_P(BlendModes, VulkanRendererParameterizedMultiplyColorTest,
                         ::testing::Values(BlendMode::kPremultipliedAlpha(),
                                           BlendMode::kStraightAlpha()));

class VulkanRendererParameterizedYuvTest
    : public VulkanRendererTest,
      public ::testing::WithParamInterface<fuchsia_images2::PixelFormat> {};

// This test actually renders a YUV format texture using the VKRenderer. We create a single
// rectangle, with a fuchsia texture. The render target and the rectangle are 32x32.
VK_TEST_P(VulkanRendererParameterizedYuvTest, YuvTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);
  // TODO(https://fxbug.dev/321072153)
  SKIP_TEST_IF_ESCHER_USES_DEVICE(SoftwareGpu);

  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Create a pair of tokens for the Image allocation.
  auto image_tokens = SysmemTokens::Create(sysmem_allocator);

  // Register the Image token with the renderer.
  auto image_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 = renderer->ImportBufferCollection(
      image_collection_id, sysmem_allocator, std::move(image_tokens.dup_token),
      BufferCollectionUsage::kClientImage, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  const uint32_t kTargetWidth = 32;
  const uint32_t kTargetHeight = 32;

  // Set the local constraints for the Image.
  fuchsia_images2::PixelFormat pixel_format = GetParam();
  auto [buffer_usage, memory_constraints] = GetUsageAndMemoryConstraintsForCpuWriteOften();
  auto image_collection = CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator, std::move(image_tokens.local_token),
      /*image_count*/ 1,
      /*width*/ kTargetWidth,
      /*height*/ kTargetHeight, buffer_usage, pixel_format, std::make_optional(memory_constraints),
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

  // Wait for buffers allocated so it can populate its information struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo image_collection_info;
  {
    auto wait_result = image_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    image_collection_info = std::move(wait_result->buffer_collection_info().value());
    EXPECT_EQ(image_collection_info.settings()
                  .value()
                  .image_format_constraints()
                  .value()
                  .pixel_format()
                  .value(),
              pixel_format);
  }

  // Create the image meta data for the Image and import.
  ImageMetadata image_metadata = {.collection_id = image_collection_id,
                                  .identifier = allocation::GenerateUniqueImageId(),
                                  .vmo_index = 0,
                                  .width = kTargetWidth,
                                  .height = kTargetHeight};
  auto promise2 = renderer->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  // Create a pair of tokens for the render target allocation.
  auto render_target_tokens = SysmemTokens::Create(sysmem_allocator);

  // Register the render target tokens with the renderer.
  auto render_target_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise3 = renderer->ImportBufferCollection(
      render_target_collection_id, sysmem_allocator, std::move(render_target_tokens.dup_token),
      BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create a client-side handle to the render target's buffer collection and set the client
  // constraints.
  auto render_target_collection = CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator, std::move(render_target_tokens.local_token),
      /*image_count*/ 1,
      /*width*/ kTargetWidth,
      /*height*/ kTargetHeight, buffer_usage, fuchsia_images2::PixelFormat::kR8G8B8A8,
      std::make_optional(memory_constraints),
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

  // Wait for buffers allocated so it can populate its information struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo render_target_collection_info;
  {
    auto wait_result = render_target_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    render_target_collection_info = std::move(wait_result->buffer_collection_info().value());
  }

  // Create the render_target image metadata and import.
  ImageMetadata render_target_metadata = {.collection_id = render_target_collection_id,
                                          .identifier = allocation::GenerateUniqueImageId(),
                                          .vmo_index = 0,
                                          .width = kTargetWidth,
                                          .height = kTargetHeight};
  auto promise4 =
      renderer->ImportBufferImage(render_target_metadata, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise4)));

  // Create a renderable where the upper-left hand corner should be at position (0,0) with a
  // width/height of (32,32).
  SrcToDest image_renderable(types::RectangleF({.x = 0,
                                                .y = 0,
                                                .width = static_cast<float>(kTargetWidth),
                                                .height = static_cast<float>(kTargetHeight)}));

  const uint32_t num_pixels = kTargetWidth * kTargetHeight;
  const uint8_t kFuchsiaYuvValues[] = {110U, 192U, 192U};
  const uint8_t kFuchsiaRgbaValues[] = {228U, 68U, 246U, 255U};
  // Have the client write pixel values to the renderable Image's texture.
  MapHostPointer(image_collection_info, image_metadata.vmo_index, HostPointerAccessMode::kWriteOnly,
                 [&](uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   for (uint32_t i = 0; i < num_pixels; ++i) {
                     vmo_host[i] = kFuchsiaYuvValues[0];
                   }
                   switch (GetParam()) {
                     case fuchsia_images2::PixelFormat::kNv12:
                       for (uint32_t i = num_pixels; i < num_pixels + num_pixels / 2; i += 2) {
                         vmo_host[i] = kFuchsiaYuvValues[1];
                         vmo_host[i + 1] = kFuchsiaYuvValues[2];
                       }
                       break;
                       break;
                     case fuchsia_images2::PixelFormat::kI420:
                       for (uint32_t i = num_pixels; i < num_pixels + num_pixels / 4; ++i) {
                         vmo_host[i] = kFuchsiaYuvValues[1];
                       }
                       for (uint32_t i = num_pixels + num_pixels / 4;
                            i < num_pixels + num_pixels / 2; ++i) {
                         vmo_host[i] = kFuchsiaYuvValues[2];
                       }
                       break;
                     default:
                       FX_NOTREACHED();
                   }
                 });

  // Render the renderable to the render target.
  renderer->Render(render_target_metadata, MakeLayers({image_renderable}, {image_metadata}), {});
  renderer->WaitIdle();

  // Get a raw pointer from the client collection's vmo that represents the render target and read
  // its values. This should show that the renderable was rendered with expected BGRA colors.
  MapHostPointer(render_target_collection_info, render_target_metadata.vmo_index,
                 HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   // Make sure the pixels are fuchsia.
                   for (uint32_t y = 0; y < kTargetHeight; y++) {
                     for (uint32_t x = 0; x < kTargetWidth; x++) {
                       EXPECT_EQ(GetPixel(vmo_host, kTargetWidth, x, y),
                                 glm::ivec4(kFuchsiaRgbaValues[0], kFuchsiaRgbaValues[1],
                                            kFuchsiaRgbaValues[2], kFuchsiaRgbaValues[3]));
                     }
                   }
                 });
}

INSTANTIATE_TEST_SUITE_P(YuvPixelFormats, VulkanRendererParameterizedYuvTest,
                         ::testing::Values(fuchsia_images2::PixelFormat::kNv12,
                                           fuchsia_images2::PixelFormat::kI420));

// This test actually renders a protected memory backed image using the VKRenderer.
VK_TEST_F(VulkanRendererTest, ProtectedMemoryTest) {
  SKIP_TEST_IF_ESCHER_USES_DEVICE(VirtualGpu);

  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer(/*use_protected_memory=*/true);
  if (!escher) {
    FX_LOGS(WARNING) << "Protected memory not supported. Test skipped.";
    GTEST_SKIP();
  }
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Create a pair of tokens for the Image allocation.
  auto image_tokens = SysmemTokens::Create(sysmem_allocator);

  // Register the Image token with the renderer.
  auto image_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise1 = renderer->ImportBufferCollection(
      image_collection_id, sysmem_allocator, std::move(image_tokens.dup_token),
      BufferCollectionUsage::kClientImage, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise1)));

  const uint32_t kTargetWidth = 32;
  const uint32_t kTargetHeight = 32;

  // Set the local constraints for the Image.
  fuchsia_images2::PixelFormat pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  const fuchsia_sysmem2::BufferMemoryConstraints memory_constraints = [] {
    fuchsia_sysmem2::BufferMemoryConstraints memory_constraints;
    memory_constraints.secure_required(true);
    memory_constraints.cpu_domain_supported(false);
    memory_constraints.ram_domain_supported(false);
    memory_constraints.inaccessible_domain_supported(true);
    return memory_constraints;
  }();
  const fuchsia_sysmem2::BufferUsage buffer_usage = [] {
    fuchsia_sysmem2::BufferUsage usage;
    usage.vulkan(fuchsia_sysmem2::kVulkanImageUsageTransferSrc);
    return usage;
  }();
  auto image_collection = CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator, std::move(image_tokens.local_token),
      /*image_count*/ 1,
      /*width*/ kTargetWidth,
      /*height*/ kTargetHeight, buffer_usage, pixel_format, std::make_optional(memory_constraints),
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

  // Wait for buffers allocated so it can populate its information struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo image_collection_info;
  {
    auto wait_result = image_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    image_collection_info = std::move(wait_result->buffer_collection_info().value());
    EXPECT_EQ(image_collection_info.settings()
                  .value()
                  .image_format_constraints()
                  .value()
                  .pixel_format()
                  .value(),
              pixel_format);
    EXPECT_TRUE(
        image_collection_info.settings().value().buffer_settings().value().is_secure().value());
  }

  // Create the image meta data for the Image and import.
  ImageMetadata image_metadata = {.collection_id = image_collection_id,
                                  .identifier = allocation::GenerateUniqueImageId(),
                                  .vmo_index = 0,
                                  .width = kTargetWidth,
                                  .height = kTargetHeight};
  auto promise2 = renderer->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage);
  EXPECT_TRUE(RunPromise(loop, std::move(promise2)));

  // Create a pair of tokens for the render target allocation.
  auto render_target_tokens = SysmemTokens::Create(sysmem_allocator);

  // Register the render target tokens with the renderer.
  auto render_target_collection_id = allocation::GenerateUniqueBufferCollectionId();
  auto promise3 = renderer->ImportBufferCollection(
      render_target_collection_id, sysmem_allocator, std::move(render_target_tokens.dup_token),
      BufferCollectionUsage::kRenderTarget, std::nullopt);
  EXPECT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create a client-side handle to the render target's buffer collection and set the client
  // constraints.
  auto render_target_collection = CreateBufferCollectionSyncPtrAndSetConstraints(
      sysmem_allocator, std::move(render_target_tokens.local_token),
      /*image_count*/ 1,
      /*width*/ kTargetWidth,
      /*height*/ kTargetHeight, buffer_usage, fuchsia_images2::PixelFormat::kR8G8B8A8,
      std::make_optional(memory_constraints),
      std::make_optional(fuchsia_images2::PixelFormatModifier::kLinear));

  // Wait for buffers allocated so it can populate its information struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo render_target_collection_info;
  {
    auto wait_result = render_target_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    render_target_collection_info = std::move(wait_result->buffer_collection_info().value());
    EXPECT_TRUE(render_target_collection_info.settings()
                    .value()
                    .buffer_settings()
                    .value()
                    .is_secure()
                    .value());
  }

  // Create the render_target image metadata and import.
  ImageMetadata render_target_metadata = {.collection_id = render_target_collection_id,
                                          .identifier = allocation::GenerateUniqueImageId(),
                                          .vmo_index = 0,
                                          .width = kTargetWidth,
                                          .height = kTargetHeight};
  auto promise4 =
      renderer->ImportBufferImage(render_target_metadata, BufferCollectionUsage::kRenderTarget);
  EXPECT_TRUE(RunPromise(loop, std::move(promise4)));

  // Create a renderable where the upper-left hand corner should be at position (0,0) with a
  // width/height of (32,32).
  SrcToDest image_renderable(types::RectangleF({.x = 0,
                                                .y = 0,
                                                .width = static_cast<float>(kTargetWidth),
                                                .height = static_cast<float>(kTargetHeight)}));
  // Render the renderable to the render target.
  renderer->Render(render_target_metadata, MakeLayers({image_renderable}, {image_metadata}), {});
  renderer->WaitIdle();

  // Note that we cannot read pixel values from either buffer because protected memory does not
  // allow that.
}

// Tests VkRenderer's readback path. This test is enabled on virtual gpu.
VK_TEST_F(VulkanRendererTest, ReadbackTest) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Setup the render target collection.
  allocation::GlobalBufferCollectionId target_id = allocation::GenerateUniqueBufferCollectionId();
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator);
  auto promise1 =
      renderer->ImportBufferCollection(target_id, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  ASSERT_TRUE(RunPromise(loop, std::move(promise1)));
  auto [target_client_end, target_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(target_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr(std::move(target_client_end));

  auto set_constraints_result =
      target_ptr->SetConstraints(fuchsia_sysmem2::BufferCollectionSetConstraintsRequest{});
  ASSERT_TRUE(set_constraints_result.is_ok());
  {
    auto wait_result = target_ptr->WaitForAllBuffersAllocated();
    ASSERT_TRUE(wait_result.is_ok());
  }
  auto release_result = target_ptr->Release();
  EXPECT_TRUE(release_result.is_ok());

  // Setup the readback collection.
  fuchsia_sysmem2::BufferCollectionInfo readback_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> readback_ptr;
  auto readback_id =
      SetupBufferCollection(loop, 1, 60, 40, BufferCollectionUsage::kReadback, renderer.get(),
                            sysmem_allocator, &readback_info, readback_ptr, target_id);
  EXPECT_EQ(target_id, readback_id);

  // Create the render_target image metadata and import.
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;
  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};
  auto promise2 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kReadback);
  ASSERT_TRUE(RunPromise(loop, std::move(promise3)));

  // Create the image metadata for the solid color renderable.
  const auto blend_mode = BlendMode::kPremultipliedAlpha();
  const std::array<float, 4> multiply_color = {1.f, 0.4f, 0.f, 1.f};
  SrcToDest renderable(types::RectangleF({.x = 0,
                                          .y = 0,
                                          .width = static_cast<float>(kTargetWidth),
                                          .height = static_cast<float>(kTargetHeight)}));

  // Render the renderable to the render target.
  ResolvedLayer layer = {
      .geometry = renderable,
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .content = ResolvedLayer::SolidColorContent{.color = {1.f, 0.4f, 0.f, 1.f}},
  };
  renderer->Render(render_target, std::span<const ResolvedLayer>(&layer, 1), {});
  renderer->WaitIdle();

  // Get a raw pointer from the readback collection's vmo that represents the copied render target
  // and read its values.
  MapHostPointer(readback_info, 0, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t linear_vals[num_bytes];
                   sRGBtoLinear(vmo_host, linear_vals, num_bytes);

                   // Make sure the pixels are in the right order give that we rotated
                   // the rectangle. Values are BGRA.
                   for (uint32_t i = 0; i < kTargetWidth; i++) {
                     for (uint32_t j = 0; j < kTargetHeight; j++) {
                       auto pixel = GetPixel(linear_vals, kTargetWidth, i, j);
                       // The sRGB conversion function provides slightly different results depending
                       // on the platform.
                       EXPECT_TRUE(pixel == glm::ivec4(255, 101, 0, 255) ||
                                   pixel == glm::ivec4(255, 102, 0, 255));
                     }
                   }
                 });
}

// Check that rendering a list of layers that includes an unresolvable / missing image ID
// does not cause vector size mismatch crashes in RectangleCompositor::DrawBatch and
// filters out the missing layer without drawing it.
VK_TEST_F(VulkanRendererTest, UnresolvableImageLayerFiltered) {
  async::TestLoop loop;
  auto [escher, renderer] = CreateEscherAndPrewarmedRenderer();
  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Setup the render target collection.
  allocation::GlobalBufferCollectionId target_id = allocation::GenerateUniqueBufferCollectionId();
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator);
  auto promise1 =
      renderer->ImportBufferCollection(target_id, sysmem_allocator, std::move(dup_token),
                                       BufferCollectionUsage::kRenderTarget, std::nullopt);
  ASSERT_TRUE(RunPromise(loop, std::move(promise1)));
  auto [target_client_end, target_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(target_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> target_ptr(std::move(target_client_end));

  auto set_constraints_result =
      target_ptr->SetConstraints(fuchsia_sysmem2::BufferCollectionSetConstraintsRequest{});
  ASSERT_TRUE(set_constraints_result.is_ok());
  {
    auto wait_result = target_ptr->WaitForAllBuffersAllocated();
    ASSERT_TRUE(wait_result.is_ok());
  }
  auto release_result = target_ptr->Release();
  EXPECT_TRUE(release_result.is_ok());

  // Setup the readback collection to verify that layer2 was filtered out.
  const uint32_t kTargetWidth = 16;
  const uint32_t kTargetHeight = 8;
  fuchsia_sysmem2::BufferCollectionInfo readback_info;
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> readback_ptr;
  auto readback_id = SetupBufferCollection(
      loop, 1, kTargetWidth, kTargetHeight, BufferCollectionUsage::kReadback, renderer.get(),
      sysmem_allocator, &readback_info, readback_ptr, target_id);
  EXPECT_EQ(target_id, readback_id);

  ImageMetadata render_target = {.collection_id = target_id,
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .vmo_index = 0,
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};
  auto promise2 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kRenderTarget);
  ASSERT_TRUE(RunPromise(loop, std::move(promise2)));
  auto promise3 = renderer->ImportBufferImage(render_target, BufferCollectionUsage::kReadback);
  ASSERT_TRUE(RunPromise(loop, std::move(promise3)));

  // Layer 1: Solid red layer (valid).
  SrcToDest rect1(types::RectangleF({.x = 0,
                                     .y = 0,
                                     .width = static_cast<float>(kTargetWidth),
                                     .height = static_cast<float>(kTargetHeight)}));
  ResolvedLayer layer1 = {
      .geometry = rect1,
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .content = ResolvedLayer::SolidColorContent{.color = {1.f, 0.f, 0.f, 1.f}},
  };

  // Layer 2: Image content layer pointing to an unknown/missing image ID, covering full screen.
  SrcToDest rect2(types::RectangleF({.x = 0,
                                     .y = 0,
                                     .width = static_cast<float>(kTargetWidth),
                                     .height = static_cast<float>(kTargetHeight)}));
  ImageMetadata missing_image = {.collection_id = allocation::GenerateUniqueBufferCollectionId(),
                                 .identifier = allocation::GenerateUniqueImageId(),
                                 .width = kTargetWidth,
                                 .height = kTargetHeight};
  ResolvedLayer layer2 = {
      .geometry = rect2,
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kPremultipliedAlpha(),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = missing_image.identifier,
              .width = missing_image.width,
              .height = missing_image.height,
          },
  };

  ResolvedLayer layers[] = {layer1, layer2};
  // Rendering should filter out layer2 gracefully without crashing in DrawBatch.
  renderer->Render(render_target, std::span<const ResolvedLayer>(layers, 2), {});
  renderer->WaitIdle();

  // Read back pixels and verify layer2 (missing image) was completely filtered out,
  // leaving layer1 (solid red) intact across the entire render target.
  MapHostPointer(readback_info, 0, HostPointerAccessMode::kReadOnly,
                 [&](const uint8_t* vmo_host, uint32_t num_bytes) mutable {
                   uint8_t linear_vals[num_bytes];
                   sRGBtoLinear(vmo_host, linear_vals, num_bytes);
                   for (uint32_t i = 0; i < kTargetWidth; i++) {
                     for (uint32_t j = 0; j < kTargetHeight; j++) {
                       auto pixel = GetPixel(linear_vals, kTargetWidth, i, j);
                       EXPECT_EQ(pixel, glm::ivec4(255, 0, 0, 255));
                     }
                   }
                 });
}

class VulkanRendererParameterizedAFBCTest
    : public VulkanRendererTest,
      public ::testing::WithParamInterface<allocation::BufferCollectionUsage> {};

// Test ARM Framebuffer compression. As the name implies, this only runs on specific ARM platforms.
VK_TEST_P(VulkanRendererParameterizedAFBCTest, EnablesAFBC) {
  async::TestLoop loop;
  auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();

  // Run test on ARM/Mali only.
  if (env->GetVulkanDevice()->vk_physical_device().getProperties().vendorID != 0x13B5) {
    GTEST_SKIP();
  }

  auto sysmem_allocator = CreateSysmemAllocatorClient(loop.dispatcher());

  // Create renderer.
  auto unique_escher = std::make_unique<escher::Escher>(
      env->GetVulkanDevice(), env->GetFilesystem(), /*gpu_allocator*/ nullptr);
  VkRenderer renderer(unique_escher->GetWeakPtr());

  // Create a pair of tokens for the render target allocation.
  auto [local_token, dup_token] = SysmemTokens::Create(sysmem_allocator);

  // Register the render target tokens with the renderer.
  auto render_target_collection_id = allocation::GenerateUniqueBufferCollectionId();
  const uint32_t kTargetWidth = 1024;
  const uint32_t kTargetHeight = 600;
  const allocation::BufferCollectionUsage usage = GetParam();
  auto promise = renderer.ImportBufferCollection(
      render_target_collection_id, sysmem_allocator, std::move(dup_token), usage,
      std::optional<fuchsia_math::SizeU>({{kTargetWidth, kTargetHeight}}));
  ASSERT_TRUE(RunPromise(loop, std::move(promise)));

  // Create a client-side handle to the render target's buffer collection and set the client
  // constraints.
  auto [render_target_collection_client_end, render_target_collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(render_target_collection_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> render_target_collection(
      std::move(render_target_collection_client_end));
  auto set_constraints_result = render_target_collection->SetConstraints(
      fuchsia_sysmem2::BufferCollectionSetConstraintsRequest{});
  ASSERT_TRUE(set_constraints_result.is_ok());

  // Wait for buffers allocated so it can populate its information struct with the vmo data.
  fuchsia_sysmem2::BufferCollectionInfo render_target_collection_info;
  {
    auto wait_result = render_target_collection->WaitForAllBuffersAllocated();
    EXPECT_TRUE(wait_result.is_ok());
    render_target_collection_info = std::move(wait_result->buffer_collection_info().value());
  }

  ASSERT_TRUE(render_target_collection_info.settings()
                  .value()
                  .image_format_constraints()
                  .value()
                  .pixel_format_modifier()
                  .has_value());
  const fuchsia_images2::PixelFormatModifier format_modifier =
      render_target_collection_info.settings()
          .value()
          .image_format_constraints()
          .value()
          .pixel_format_modifier()
          .value();

  // The format modifier should not be linear.
  EXPECT_NE(format_modifier, fuchsia_images2::PixelFormatModifier::kArmLinearTe);
  EXPECT_NE(format_modifier, fuchsia_images2::PixelFormatModifier::kLinear);

  // We also need to make sure that the format is still a type of AFBC, which is indicated by the
  // top byte being equal to 0x08.
  const uint64_t afbc_mask = 0x0800000000000000;
  const bool afbc_enabled = ((static_cast<uint64_t>(format_modifier) & afbc_mask) == afbc_mask);
  EXPECT_TRUE(afbc_enabled) << "Format modifier: " << static_cast<uint64_t>(format_modifier);
}

INSTANTIATE_TEST_SUITE_P(BufferCollectionUsages, VulkanRendererParameterizedAFBCTest,
                         ::testing::Values(allocation::BufferCollectionUsage::kRenderTarget,
                                           allocation::BufferCollectionUsage::kClientImage));

TEST(VkRendererTest, GetNormalizedUvRect) {
  constexpr uint32_t kImageWidth = 20;
  constexpr uint32_t kImageHeight = 40;
  const ResolvedLayer::ImageContent content = {
      .image_id = allocation::GlobalImageId(1),
      .width = kImageWidth,
      .height = kImageHeight,
  };

  const types::RectangleF dest({.x = 10.f, .y = 20.f, .width = 30.f, .height = 40.f});
  const types::RectangleF src({.x = 2.f, .y = 4.f, .width = 6.f, .height = 8.f});

  // u0 = 2/20 = 0.1, u1 = (2+6)/20 = 0.4
  // v0 = 4/40 = 0.1, v1 = (4+8)/40 = 0.3
  const float u0 = 0.1f;
  const float u1 = 0.4f;
  const float v0 = 0.1f;
  const float v1 = 0.3f;

  struct TestCase {
    types::RotateFlip transform;
    std::array<glm::vec2, 4> expected_uvs;
  };

  const TestCase test_cases[] = {
      {types::RotateFlip::kIdentity(),
       {glm::vec2(u0, v0), glm::vec2(u1, v0), glm::vec2(u1, v1), glm::vec2(u0, v1)}},
      {types::RotateFlip::kReflectX(),
       {glm::vec2(u0, v1), glm::vec2(u1, v1), glm::vec2(u1, v0), glm::vec2(u0, v0)}},
      {types::RotateFlip::kReflectY(),
       {glm::vec2(u1, v0), glm::vec2(u0, v0), glm::vec2(u0, v1), glm::vec2(u1, v1)}},
      {types::RotateFlip::kRotateCcw180(),
       {glm::vec2(u1, v1), glm::vec2(u0, v1), glm::vec2(u0, v0), glm::vec2(u1, v0)}},
      {types::RotateFlip::kRotateCcw90(),
       {glm::vec2(u1, v0), glm::vec2(u1, v1), glm::vec2(u0, v1), glm::vec2(u0, v0)}},
      {types::RotateFlip::kRotateCcw90ReflectX(),
       {glm::vec2(u0, v0), glm::vec2(u0, v1), glm::vec2(u1, v1), glm::vec2(u1, v0)}},
      {types::RotateFlip::kRotateCcw90ReflectY(),
       {glm::vec2(u1, v1), glm::vec2(u1, v0), glm::vec2(u0, v0), glm::vec2(u0, v1)}},
      {types::RotateFlip::kRotateCcw270(),
       {glm::vec2(u0, v1), glm::vec2(u0, v0), glm::vec2(u1, v0), glm::vec2(u1, v1)}},
  };

  for (const auto& tc : test_cases) {
    ResolvedLayer layer = {
        .geometry = SrcToDest(src, dest, tc.transform),
        .content = content,
    };
    escher::Rectangle2D uv_rect = GetNormalizedUvRect(layer);
    EXPECT_EQ(uv_rect.origin, glm::vec2(10.f, 20.f));
    EXPECT_EQ(uv_rect.extent, glm::vec2(30.f, 40.f));
    for (size_t i = 0; i < 4; ++i) {
      EXPECT_FLOAT_EQ(uv_rect.clockwise_uvs[i].x, tc.expected_uvs[i].x);
      EXPECT_FLOAT_EQ(uv_rect.clockwise_uvs[i].y, tc.expected_uvs[i].y);
    }
  }

  // Also test default empty source rect falls back to full image (0,0)-(1,1).
  {
    ResolvedLayer layer = {
        .geometry = SrcToDest(dest),
        .content = content,
    };
    escher::Rectangle2D uv_rect = GetNormalizedUvRect(layer);
    EXPECT_EQ(uv_rect.origin, glm::vec2(10.f, 20.f));
    EXPECT_EQ(uv_rect.extent, glm::vec2(30.f, 40.f));
    EXPECT_EQ(uv_rect.clockwise_uvs[0], glm::vec2(0.f, 0.f));
    EXPECT_EQ(uv_rect.clockwise_uvs[1], glm::vec2(1.f, 0.f));
    EXPECT_EQ(uv_rect.clockwise_uvs[2], glm::vec2(1.f, 1.f));
    EXPECT_EQ(uv_rect.clockwise_uvs[3], glm::vec2(0.f, 1.f));
  }
}

}  // namespace flatland
