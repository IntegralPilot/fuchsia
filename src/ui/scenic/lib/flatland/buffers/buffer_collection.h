// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_BUFFER_COLLECTION_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_BUFFER_COLLECTION_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/fit/result.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"

namespace flatland {

// |BufferCollectionInfo| stores the information regarding a BufferCollection.
// Instantiated via calls to |New| below.
class BufferCollectionInfo {
 public:
  // Creates a new |BufferCollectionInfo| instance. The return value is null if the buffer was
  // not created successfully. This function sets the server-side sysmem image constraints.
  //
  // TODO(https://fxbug.dev/42125043): Make this an asynchronous call. This function is currently
  // thread safe as Allocator_Sync pointers are thread safe, but if this becomes async it may become
  // unsafe.
  static fit::result<fit::failed, BufferCollectionInfo> New(
      fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token,
      std::optional<fuchsia_sysmem2::ImageFormatConstraints> image_format_constraints =
          std::nullopt,
      fuchsia_sysmem2::BufferUsage buffer_usage =
          [] {
            fuchsia_sysmem2::BufferUsage result;
            result.none(fuchsia_sysmem2::kNoneUsage);
            return result;
          }(),
      allocation::BufferCollectionUsage usage = allocation::BufferCollectionUsage::kClientImage);

  // Creates a non-initialized instance of this class. Fully initialized instances must
  // be created via a call to |New|.
  BufferCollectionInfo() = default;
  ~BufferCollectionInfo();
  BufferCollectionInfo& operator=(BufferCollectionInfo&& other) noexcept;
  BufferCollectionInfo(BufferCollectionInfo&& other) noexcept;
  BufferCollectionInfo(const BufferCollectionInfo& other) = delete;
  BufferCollectionInfo& operator=(const BufferCollectionInfo& other) = delete;

  // This BufferCollectionInfo may not be allocated due to the fact that it may not necessarily
  // have all constraints set from every client with a token. This function will return false if
  // that is the case and true if the buffer collection has actually been allocated. Additionally,
  // if this function returns true, the client will be able to access the sysmem information of
  // the collection via a call to GetSysmemInfo(). Once this function returns true, it won't ever
  // return |false| again.
  //
  // This function is thread-safe because |buffer_collection_ptr_|, which is a
  // SynchronousInterfacePtr, is thread-safe. This function will return false if the buffers are not
  // able to be constructed, for example if there are incompatible constraints that are set on the
  // server and client.
  bool BuffersAreAllocated();

  // Info describing |buffer_collection_ptr|. Do not call this until after verifying the allocation
  // status of the buffer collection with BuffersAreAllocated().
  const fuchsia_sysmem2::BufferCollectionInfo& GetSysmemInfo() const {
    // DCHECK if the struct is uninitialized.
    FX_DCHECK(buffer_collection_info_.buffers().has_value() &&
              buffer_collection_info_.buffers()->size() >= 1);
    return buffer_collection_info_;
  }

 private:
  BufferCollectionInfo(fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection_ptr)
      : buffer_collection_ptr_(std::move(buffer_collection_ptr)) {}

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection_ptr_;
  fuchsia_sysmem2::BufferCollectionInfo buffer_collection_info_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_BUFFERS_BUFFER_COLLECTION_H_
