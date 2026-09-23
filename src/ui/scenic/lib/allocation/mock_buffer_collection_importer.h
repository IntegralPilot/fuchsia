// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_ALLOCATION_MOCK_BUFFER_COLLECTION_IMPORTER_H_
#define SRC_UI_SCENIC_LIB_ALLOCATION_MOCK_BUFFER_COLLECTION_IMPORTER_H_

#include <gmock/gmock.h>

#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"

namespace allocation {

// Mock class of BufferCollectionImporter for API testing.
class MockBufferCollectionImporter : public BufferCollectionImporter {
 public:
  MOCK_METHOD(fpromise::promise<>, ImportBufferCollection,
              (GlobalBufferCollectionId, fidl::WireClient<fuchsia_sysmem2::Allocator>&,
               fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>,
               allocation::BufferCollectionUsage, std::optional<fuchsia_math::SizeU>),
              (override));
  MOCK_METHOD(void, ReleaseBufferCollection,
              (GlobalBufferCollectionId, allocation::BufferCollectionUsage), (override));

  MOCK_METHOD(fpromise::promise<>, ImportBufferImage,
              (const ImageMetadata&, allocation::BufferCollectionUsage), (override));

  MOCK_METHOD(void, ReleaseBufferImage, (GlobalImageId), (override));
};

}  // namespace allocation

#endif  // SRC_UI_SCENIC_LIB_ALLOCATION_MOCK_BUFFER_COLLECTION_IMPORTER_H_
