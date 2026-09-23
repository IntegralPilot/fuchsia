// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_DEVICE_SYSMEM_ALLOCATOR_H_
#define SRC_CAMERA_BIN_DEVICE_SYSMEM_ALLOCATOR_H_

#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/fpromise/sequencer.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/trace/event.h>

namespace camera {

struct BufferCollectionWithLifetime {
  fuchsia::sysmem2::BufferCollectionInfo buffers;
  zx::eventpair deallocation_complete;
};

class SysmemAllocator {
 public:
  explicit SysmemAllocator(fuchsia::sysmem2::AllocatorHandle allocator);

  const fuchsia::sysmem2::AllocatorPtr& fidl() const { return allocator_; }

  // This method will bind `token` to a new `BufferCollection` and `name` and `constraints` will
  // be set on the collection. Upon a successful allocation, the collection will be closed and
  // the `BufferCollectionInfo_2` will be returned by the promise. It does not check for free space
  // before allocation.
  //
  // An error is returned from the promise if the allocation is unable to complete for any reason.
  fpromise::promise<BufferCollectionWithLifetime, zx_status_t> BindSharedCollection(
      fuchsia::sysmem2::BufferCollectionTokenHandle token,
      fuchsia::sysmem2::BufferCollectionConstraints constraints, std::string name);

 private:
  fuchsia::sysmem2::AllocatorPtr allocator_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_DEVICE_SYSMEM_ALLOCATOR_H_
