// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_ALLOCATION_IMAGE_METADATA_H_
#define SRC_UI_SCENIC_LIB_ALLOCATION_IMAGE_METADATA_H_

// Remove when C++-23 is available.
#include <lib/stdcompat/utility.h>

#include "src/ui/scenic/lib/allocation/id.h"

namespace allocation {

// Struct representing the data needed to extract an image from a buffer collection.
// All pixel information is stored within the Vmo of the collection so this struct
// only needs information regarding which collection and which vmo to point to, and
// the overall size of the image. Only supports fuchsia::images2::PixelFormat::B8G8R8A8
// as the image format type.
struct ImageMetadata {
  // The unique id of the buffer collection this image is backed by.
  GlobalBufferCollectionId collection_id = kInvalidId;

  // The unique ID for this particular image.
  display::ImageId identifier = display::kInvalidImageId;

  // A single buffer collection may have several vmos. This tells the importer
  // which vmo in the collection specified by |collection_id| to use as the memory
  // for this image. This value must be less than the total number of vmos of the
  // buffer collection we are constructing the image from.
  uint32_t vmo_index;

  // The dimensions of the image in pixels.
  uint32_t width = 0;
  uint32_t height = 0;

  bool operator==(const ImageMetadata& other) const {
    return (collection_id == other.collection_id && vmo_index == other.vmo_index &&
            width == other.width && height == other.height);
  }
};

inline std::ostream& operator<<(std::ostream& str, const ImageMetadata& m) {
  str << "size=" << m.width << "x" << m.height;
  return str;
}

}  // namespace allocation

#endif  // SRC_UI_SCENIC_LIB_ALLOCATION_IMAGE_METADATA_H_
