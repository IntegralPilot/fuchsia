// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_IMAGE_INFO_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_IMAGE_INFO_H_

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <cstddef>
#include <memory>

#include <fbl/intrusive_hash_table.h>
#include <fbl/intrusive_single_list.h>

#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"

namespace fake_display {

struct SysmemBufferInfo {
  // Retrieves BufferCollection information from sysmem.
  static zx::result<SysmemBufferInfo> GetSysmemMetadata(
      fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& sysmem_client,
      uint32_t buffer_index);

  zx::vmo image_vmo;
  uint64_t image_vmo_offset;

  fuchsia_images2::wire::PixelFormat pixel_format;
  fuchsia_images2::wire::PixelFormatModifier pixel_format_modifier;
  fuchsia_math::wire::SizeU minimum_size;
  uint32_t minimum_bytes_per_row;

  fuchsia_sysmem2::wire::CoherencyDomain coherency_domain;
};

class DisplayImageInfo : public fbl::SinglyLinkedListable<std::unique_ptr<DisplayImageInfo>> {
 public:
  using IdType = display::DriverImageId;
  using HashTable = fbl::HashTable<IdType, std::unique_ptr<DisplayImageInfo>>;

  DisplayImageInfo(IdType id, display::ImageMetadata metadata, SysmemBufferInfo buffer_info);
  ~DisplayImageInfo() = default;

  // Disallow copy and move.
  DisplayImageInfo(const DisplayImageInfo&) = delete;
  DisplayImageInfo& operator=(const DisplayImageInfo&) = delete;
  DisplayImageInfo(DisplayImageInfo&&) = delete;
  DisplayImageInfo& operator=(DisplayImageInfo&&) = delete;

  // Trait implementation for fbl::HashTable
  HashTable::KeyType GetKey() const;
  static size_t GetHash(HashTable::KeyType key);

  IdType id() const { return id_; }
  const SysmemBufferInfo& sysmem_buffer_info() const { return sysmem_buffer_info_; }
  const display::ImageMetadata& metadata() const { return metadata_; }
  const zx::vmo& vmo() const { return sysmem_buffer_info_.image_vmo; }

 private:
  IdType id_;
  display::ImageMetadata metadata_;
  SysmemBufferInfo sysmem_buffer_info_;
};

class CaptureImageInfo : public fbl::SinglyLinkedListable<std::unique_ptr<CaptureImageInfo>> {
 public:
  using IdType = display::DriverCaptureImageId;
  using HashTable = fbl::HashTable<IdType, std::unique_ptr<CaptureImageInfo>>;

  CaptureImageInfo(IdType id, display::ImageMetadata metadata, SysmemBufferInfo buffer_info);
  ~CaptureImageInfo() = default;

  // Disallow copy and move.
  CaptureImageInfo(const CaptureImageInfo&) = delete;
  CaptureImageInfo& operator=(const CaptureImageInfo&) = delete;
  CaptureImageInfo(CaptureImageInfo&&) = delete;
  CaptureImageInfo& operator=(CaptureImageInfo&&) = delete;

  // Trait implementation for fbl::HashTable
  HashTable::KeyType GetKey() const;
  static size_t GetHash(HashTable::KeyType key);

  IdType id() const { return id_; }
  const SysmemBufferInfo& sysmem_buffer_info() const { return sysmem_buffer_info_; }
  const display::ImageMetadata& metadata() const { return metadata_; }
  const zx::vmo& vmo() const { return sysmem_buffer_info_.image_vmo; }

 private:
  IdType id_;
  display::ImageMetadata metadata_;
  SysmemBufferInfo sysmem_buffer_info_;
};

}  // namespace fake_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FAKE_DISPLAY_STACK_IMAGE_INFO_H_
