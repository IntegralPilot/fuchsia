// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_VK_RENDERER_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_VK_RENDERER_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fuchsia/images/cpp/fidl.h>

#include <memory_resource>
#include <set>
#include <unordered_map>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/lib/escher/debug/debug_font.h"
#include "src/ui/lib/escher/flatland/rectangle_compositor.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

namespace flatland {

using allocation::BufferCollectionUsage;
using allocation::GlobalBufferCollectionId;
using allocation::GlobalImageId;
using allocation::ImageMetadata;

// Helper function to normalize UV coordinates and build an escher::Rectangle2D from a
// ResolvedLayer.
escher::Rectangle2D GetNormalizedUvRect(const ResolvedLayer& layer);

// Implementation of the Flatland Renderer interface that relies on Escher and
// by extension the Vulkan API.
class VkRenderer final : public Renderer {
 public:
  explicit VkRenderer(escher::EscherWeakPtr escher);
  ~VkRenderer() override;

  // |BufferCollectionImporter|
  // Only called from the main thread.
  fpromise::promise<> ImportBufferCollection(
      GlobalBufferCollectionId collection_id,
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token,
      BufferCollectionUsage usage, std::optional<fuchsia_math::SizeU> size) override;

  // |BufferCollectionImporter|
  // Only called from the main thread.
  void ReleaseBufferCollection(GlobalBufferCollectionId collection_id,
                               BufferCollectionUsage usage) override;

  // |BufferCollectionImporter|
  // Called from main thread or Flatland threads.
  fpromise::promise<> ImportBufferImage(const ImageMetadata& metadata,
                                        BufferCollectionUsage usage) override;

  // |BufferCollectionImporter|
  // Called from main thread or Flatland threads.
  void ReleaseBufferImage(GlobalImageId image_id) override;

  // |Renderer|.
  // Only called from the main thread.
  void Render(const allocation::ImageMetadata& render_target, std::span<const ResolvedLayer> layers,
              const RenderArgs& render_args) override;

  // |Renderer|.
  // Only called from the main thread.
  void SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                const fidl::Array<float, 3>& preoffsets,
                                const fidl::Array<float, 3>& postoffsets) override;

  // |Renderer|.
  // Only called from the main thread.
  fuchsia_images2::PixelFormat ChoosePreferredRenderTargetFormat(
      const std::vector<fuchsia_images2::PixelFormat>& available_formats) const override;

  // |Renderer|.
  // Only called from the main thread.
  bool SupportsRenderInProtected() const override;

  // |Renderer|.
  // Only called from the main thread.
  bool RequiresRenderInProtected(std::span<const ResolvedLayer> layers) const override;

  // Wait for all gpu operations to complete.
  // Only called from the main thread.
  bool WaitIdle();

  // |Renderer|.
  void WarmPipelineCache() override;

  // Determines whether |CommandBuffer::DisableLazyPipelineCreation()| is invoked on the command
  // buffers created within |Render()|.
  void set_disable_lazy_pipeline_creation(bool b) { disable_lazy_pipeline_creation_ = b; }

 private:
  // Wrapper struct to contain the sysmem collection handle, the vulkan
  // buffer collection.
  struct CollectionData {
    fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection;
    vk::BufferCollectionFUCHSIA vk_collection;

    // Bit to track whether we've confirmed the allocation's completion.
    bool is_allocated = false;
  };

  // Creates a vk::BufferCollectionFUCHSIA with the proper constraints set.
  // Returns std::nullopt on failure.
  std::optional<vk::BufferCollectionFUCHSIA> SetConstraintsAndCreateVulkanBufferCollection(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, BufferCollectionUsage usage,
      std::optional<fuchsia_math::SizeU> size);

  // Finds the relevant vulkan buffer collection, checks that it is allocated, and returns it.
  // If the buffer is missing or not allocated it returns std::nullopt.
  std::optional<vk::BufferCollectionFUCHSIA> GetAllocatedVulkanBufferCollection(
      allocation::GlobalBufferCollectionId collection_id, BufferCollectionUsage usage);

  bool ImportRenderTargetImage(const allocation::ImageMetadata& metadata,
                               vk::BufferCollectionFUCHSIA vk_collection);
  bool ImportReadbackImage(const allocation::ImageMetadata& metadata,
                           vk::BufferCollectionFUCHSIA vk_collection);
  bool ImportClientImage(const allocation::ImageMetadata& metadata,
                         vk::BufferCollectionFUCHSIA vk_collection);

  // The function ExtractImage() creates an escher Image from a sysmem collection vmo.
  escher::ImagePtr ExtractImage(const ImageMetadata& metadata, BufferCollectionUsage bc_usage,
                                vk::BufferCollectionFUCHSIA collection,
                                vk::ImageUsageFlags image_usage, bool readback = false)
      FXL_LOCKS_EXCLUDED(lock_);

  // ExtractTexture() is a wrapper function to ExtractImage().
  escher::TexturePtr ExtractTexture(const ImageMetadata& metadata,
                                    vk::BufferCollectionFUCHSIA collection)
      FXL_LOCKS_EXCLUDED(lock_);

  // Copies |source_image| into |dest_image|.
  void BlitRenderTarget(escher::CommandBuffer* command_buffer, escher::ImagePtr source_image,
                        vk::ImageLayout* source_image_layout, escher::ImagePtr dest_image,
                        const ImageMetadata& metadata);

  // Returns whether an image with |image_id| and |usage| is already registered.
  bool ImageIsAlreadyRegisteredForUsage(allocation::GlobalImageId image_id,
                                        BufferCollectionUsage usage);

  // Returns a reference to the appropriate map of buffer collections for |usage|.
  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData>& GetBufferCollectionsFor(
      BufferCollectionUsage usage) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // Return a lazily-initialized |DebugFont|.
  escher::DebugFont* GetDebugFont();

  // Escher is how we access Vulkan.
  escher::EscherWeakPtr escher_;

  // Vulkan rendering component.
  escher::RectangleCompositor compositor_;

  // This mutex protects access to class members that are accessed on main thread and the Flatland
  // threads.
  mutable std::mutex lock_;

  // Pool resource to back the polymorphic memory resource containers below. Since all access to
  // the containers and this resource is serialized via |lock_|, using an unsynchronized pool is
  // completely safe.
  std::pmr::unsynchronized_pool_resource pool_resource_;

  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData> texture_collections_
      FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData> render_target_collections_
      FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalBufferCollectionId, CollectionData> readback_collections_
      FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalImageId, escher::TexturePtr> texture_map_ FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalImageId, escher::ImagePtr> render_target_map_ FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalImageId, escher::TexturePtr> depth_target_map_
      FXL_GUARDED_BY(lock_);
  std::pmr::unordered_map<GlobalImageId, escher::ImagePtr> readback_image_map_
      FXL_GUARDED_BY(lock_);
  std::pmr::set<GlobalImageId> pending_textures_ FXL_GUARDED_BY(lock_);
  std::pmr::set<GlobalImageId> pending_render_targets_ FXL_GUARDED_BY(lock_);

  std::unique_ptr<escher::DebugFont> debug_font_;

  uint32_t frame_number_ = 0;
  bool disable_lazy_pipeline_creation_ = false;

  const async_dispatcher_t* const main_dispatcher_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_VK_RENDERER_H_
