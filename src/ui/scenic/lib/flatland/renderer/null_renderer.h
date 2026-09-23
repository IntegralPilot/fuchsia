// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_NULL_RENDERER_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_NULL_RENDERER_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>

#include <mutex>
#include <unordered_map>

#include "src/lib/fxl/synchronization/thread_annotations.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/buffers/buffer_collection.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

namespace flatland {

using allocation::BufferCollectionUsage;

// A renderer implementation used for validation. It does everything a standard
// renderer implementation does except for actually rendering.
class NullRenderer final : public Renderer {
 public:
  ~NullRenderer() override = default;

  // |BufferCollectionImporter|
  fpromise::promise<> ImportBufferCollection(
      allocation::GlobalBufferCollectionId collection_id,
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, BufferCollectionUsage usage,
      std::optional<fuchsia_math::SizeU> size) override;

  // |BufferCollectionImporter|
  void ReleaseBufferCollection(allocation::GlobalBufferCollectionId collection_id,
                               BufferCollectionUsage usage) override;

  // |BufferCollectionImporter|
  fpromise::promise<> ImportBufferImage(const allocation::ImageMetadata& metadata,
                                        BufferCollectionUsage usage) override;

  // |BufferCollectionImporter|
  void ReleaseBufferImage(allocation::GlobalImageId image_id) override;

  // |Renderer|.
  void Render(const allocation::ImageMetadata& render_target, std::span<const ResolvedLayer> layers,
              const RenderArgs& render_args) override;

  // |Renderer|.
  void SetColorConversionValues(const fidl::Array<float, 9>& coefficients,
                                const fidl::Array<float, 3>& preoffsets,
                                const fidl::Array<float, 3>& postoffsets) override;

  // |Renderer|.
  fuchsia_images2::PixelFormat ChoosePreferredRenderTargetFormat(
      const std::vector<fuchsia_images2::PixelFormat>& available_formats) const override;

  // |Renderer|.
  bool SupportsRenderInProtected() const override;

  // |Renderer|.
  bool RequiresRenderInProtected(std::span<const ResolvedLayer> layers) const override;

 private:
  std::unordered_map<allocation::GlobalBufferCollectionId, BufferCollectionInfo>&
  GetBufferCollectionInfosFor(BufferCollectionUsage usage) FXL_EXCLUSIVE_LOCKS_REQUIRED(lock_);

  // This mutex protects access to class members that are accessed on main thread and the Flatland
  // threads.
  mutable std::mutex lock_;
  std::unordered_map<allocation::GlobalBufferCollectionId, BufferCollectionInfo> client_image_map_
      FXL_GUARDED_BY(lock_);
  std::unordered_map<allocation::GlobalBufferCollectionId, BufferCollectionInfo> render_target_map_
      FXL_GUARDED_BY(lock_);
  std::unordered_map<allocation::GlobalBufferCollectionId, BufferCollectionInfo> readback_map_
      FXL_GUARDED_BY(lock_);
  std::unordered_map<allocation::GlobalImageId, fuchsia_sysmem2::ImageFormatConstraints> image_map_
      FXL_GUARDED_BY(lock_);
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_NULL_RENDERER_H_
