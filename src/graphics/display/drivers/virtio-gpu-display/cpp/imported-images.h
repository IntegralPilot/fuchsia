// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGES_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGES_H_

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <memory>

#include <fbl/intrusive_hash_table.h>
#include <fbl/intrusive_single_list.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-image.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace virtio_display {

// Information relevant to this driver for an item in a sysmem BufferCollection.
struct SysmemBufferInfo {
  zx::vmo image_vmo;
  uint64_t image_vmo_offset;

  display::PixelFormat pixel_format;
  fuchsia_images2::wire::PixelFormatModifier pixel_format_modifier;
  fuchsia_math::wire::SizeU minimum_size;
  uint32_t minimum_bytes_per_row;

  // Guaranteed to be positive.
  uint32_t bytes_per_row_divisor;

  fuchsia_sysmem2::wire::CoherencyDomain coherency_domain;
};

class ImportedBufferCollection {
 public:
  explicit ImportedBufferCollection(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> sysmem_client);

  ImportedBufferCollection(const ImportedBufferCollection&) = delete;
  ImportedBufferCollection(ImportedBufferCollection&&) noexcept = default;
  ImportedBufferCollection& operator=(const ImportedBufferCollection&) = delete;
  ImportedBufferCollection& operator=(ImportedBufferCollection&&) noexcept = default;

  ~ImportedBufferCollection();

  // Retrieves BufferCollection information from sysmem.
  zx::result<SysmemBufferInfo> GetSysmemMetadata(uint32_t buffer_index);

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& sysmem_client() {
    return sysmem_client_;
  }

 private:
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> sysmem_client_;
};

// Manages a display engine's collection of imported images.
//
// Instances are not thread-safe, and must be used on a single thread or
// synchronized dispatcher.
class ImportedImages {
 public:
  explicit ImportedImages(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client);

  ImportedImages(const ImportedImages&) = delete;
  ImportedImages(ImportedImages&&) = delete;
  ImportedImages& operator=(const ImportedImages&) = delete;
  ImportedImages& operator=(ImportedImages&&) = delete;

  ~ImportedImages();

  // Initialization work that is not suitable for the constructor.
  zx::result<> Initialize();

  // Similar contract to [`fuchsia.hardware.display.engine/Engine.ImportBufferCollection`].
  zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token);

  // Similar contract to [`fuchsia.hardware.display.engine/Engine.ReleaseBufferCollection`].
  zx::result<> ReleaseBufferCollection(display::DriverBufferCollectionId buffer_collection_id);

  // Similar contract to [`fuchsia.hardware.display.engine/Engine.ImportImage`].
  //
  // Upon success, `FindSysmemInfoById()` will return the image buffer
  // information retrieved from sysmem, and `FindImageById()` will return an
  // empty instance. The driver code calling this method should check that the
  // sysmem buffer and image constraints are acceptable, and should then
  // populate the `ImportedImage` instance with valid data.
  zx::result<display::DriverImageId> ImportImage(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index);

  // Similar contract to [`fuchsia.hardware.display.engine/Engine.ReleaseImage`].
  zx::result<> ReleaseImage(display::DriverImageId driver_image_id);

  // Returns null if no collection with the given ID exists.
  //
  // The returned pointer is valid until the collection is mutated by calling an
  // Import*() or Release*() method.
  ImportedBufferCollection* FindBufferCollectionById(
      display::DriverBufferCollectionId buffer_collection_id);

  // Returns null if no imported image with the given ID exists.
  //
  // The returned pointer is valid until the collection is mutated by calling an
  // Import*() or Release*() method.
  SysmemBufferInfo* FindSysmemInfoById(display::DriverImageId image_id);

  // Returns null if no imported image with the given ID exists.
  //
  // The returned pointer is valid until the collection is mutated by calling an
  // Import*() or Release*() method.
  ImportedImage* FindImageById(display::DriverImageId image_id);

 private:
  // One node in the dictionary of imported BufferCollections.
  struct ImportedBufferCollectionNode
      : public fbl::SinglyLinkedListable<std::unique_ptr<ImportedBufferCollectionNode>> {
    display::DriverBufferCollectionId id;
    ImportedBufferCollection imported_buffer_collection;

    explicit ImportedBufferCollectionNode(
        display::DriverBufferCollectionId id,
        fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> client);
    ~ImportedBufferCollectionNode();

    // fbl::HashTable membership requirements:
    display::DriverBufferCollectionId GetKey() const { return id; }
    static uint64_t GetHash(display::DriverBufferCollectionId key) { return key.value(); }
  };

  // One node in the dictionary of imported images.
  struct ImportedImageNode : public fbl::SinglyLinkedListable<std::unique_ptr<ImportedImageNode>> {
    display::DriverImageId id;
    SysmemBufferInfo sysmem_buffer_info;
    ImportedImage imported_image;

    explicit ImportedImageNode(display::DriverImageId id, SysmemBufferInfo sysmem_buffer_info);
    ~ImportedImageNode();

    // fbl::HashTable membership requirements:
    display::DriverImageId GetKey() const { return id; }
    static uint64_t GetHash(display::DriverImageId key) { return key.value(); }
  };

  // Maps DriverBufferCollectionId to per-collection data.
  using ImportedBufferCollectionMap = fbl::HashTable<
      display::DriverBufferCollectionId, std::unique_ptr<ImportedBufferCollectionNode>,
      fbl::SinglyLinkedList<std::unique_ptr<ImportedBufferCollectionNode>>, uint64_t>;

  // Maps DriverImageId to per-image data.
  using ImportedImageMap =
      fbl::HashTable<display::DriverImageId, std::unique_ptr<ImportedImageNode>,
                     fbl::SinglyLinkedList<std::unique_ptr<ImportedImageNode>>, uint64_t>;

  ImportedBufferCollectionMap buffer_collections_;
  ImportedImageMap images_;
  display::DriverImageId next_image_id_{1};

  // The sysmem allocator client used to bind incoming buffer collection tokens.
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client_;
};

}  // namespace virtio_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGES_H_
