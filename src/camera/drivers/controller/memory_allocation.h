// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_DRIVERS_CONTROLLER_MEMORY_ALLOCATION_H_
#define SRC_CAMERA_DRIVERS_CONTROLLER_MEMORY_ALLOCATION_H_

#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fuchsia/sysmem/cpp/fidl.h>

namespace camera {

struct BufferCollection {
  fuchsia::sysmem2::BufferCollectionPtr ptr;
  fuchsia::sysmem2::BufferCollectionInfo buffers;
};

class ControllerMemoryAllocator {
 public:
  explicit ControllerMemoryAllocator(fuchsia::sysmem2::AllocatorSyncPtr sysmem_allocator);

  // Takes in a set of constraints and allocates memory using sysmem based on those
  // constraints.
  zx_status_t AllocateSharedMemory(
      const std::vector<fuchsia::sysmem2::BufferCollectionConstraints>& constraints,
      BufferCollection& out_buffer_collection, const std::string& name) const;

  // Duplicates the provided token, assigns it "default" constraints, and returns the collection.
  fuchsia::sysmem2::BufferCollectionHandle AttachObserverCollection(
      fuchsia::sysmem2::BufferCollectionTokenHandle& token);

 private:
  fuchsia::sysmem2::AllocatorSyncPtr sysmem_allocator_;
};

}  // namespace camera

#endif  // SRC_CAMERA_DRIVERS_CONTROLLER_MEMORY_ALLOCATION_H_
