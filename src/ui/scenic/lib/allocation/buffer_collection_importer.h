// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_ALLOCATION_BUFFER_COLLECTION_IMPORTER_H_
#define SRC_UI_SCENIC_LIB_ALLOCATION_BUFFER_COLLECTION_IMPORTER_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/fpromise/promise.h>

#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/allocation/image_metadata.h"

namespace allocation {

// The usage of the buffer collection that is being used in ImportBufferCollection(),
// ReleaseBufferCollection(), and ImportBufferImage().
// |kClientImage| is for collections that contain textures.
// |kRenderTarget| is for collections that contain render targets.
// |kReadback| is for collections that are for copying from render targets. If the
// buffer collection is imported with this type, calling Render() also copies the render
// output of the buffer.
enum class BufferCollectionUsage { kClientImage, kRenderTarget, kReadback };

// This interface is used for importing Flatland buffer collections and images to external services
// that would like to also have access to the collection and set their own constraints. This
// interface allows Flatland to remain agnostic as to the implementation details of a buffer
// collection consumer.
//
// NOTE: Implementations of BufferCollectionImporter must be thread-safe.
class BufferCollectionImporter {
 public:
  // Allows the service to set its own constraints on the buffer collection. Must be set before
  // the buffer collection is fully allocated/validated. The return value indicates successful
  // importation via |true| and a failed importation via |false|. Returns false if |collection_id|
  // is already imported. The collection_id can be reused if the importation fails.
  // |token| must be a valid sysmem token.
  // |usage| determines the type of buffer collection to be imported.
  // |size| may be optionally set to indicate the intended size usage so that it may be specified
  // when setting constraints in |token|, i.e. for kRenderTarget allocations.
  virtual fpromise::promise<> ImportBufferCollection(
      GlobalBufferCollectionId collection_id,
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token, BufferCollectionUsage usage,
      std::optional<fuchsia_math::SizeU> size) = 0;

  // Releases the buffer collection from the service. It may be called while there are associated
  // Images alive.
  virtual void ReleaseBufferCollection(GlobalBufferCollectionId collection_id,
                                       BufferCollectionUsage usage_type) = 0;

  // Has the service create an image for itself from the provided buffer collection. Returns
  // true upon a successful import and false otherwise.
  virtual fpromise::promise<> ImportBufferImage(const ImageMetadata& metadata,
                                                BufferCollectionUsage usage_type) = 0;

  // Releases the provided image from the service.  Safe to call for an image id this importer
  // never imported or has already released; such calls are no-ops.
  // TODO(https://fxbug.dev/560366075): semantics are undefined if called while an image
  // is still being imported.  Currently not an issue because implementations are all
  // (almost: see bug) synchronous.
  virtual void ReleaseBufferImage(GlobalImageId image_id) = 0;

  virtual ~BufferCollectionImporter() = default;
};

}  // namespace allocation

#endif  // SRC_UI_SCENIC_LIB_ALLOCATION_BUFFER_COLLECTION_IMPORTER_H_
