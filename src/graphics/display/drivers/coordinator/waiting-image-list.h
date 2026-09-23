// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_WAITING_IMAGE_LIST_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_WAITING_IMAGE_LIST_H_

#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <array>
#include <cstddef>
#include <vector>

#include <fbl/ref_ptr.h>

#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"

namespace display_coordinator {

// Maintains a bounded queue of entries, each consisting of an image and an optional wait fence.
//
// Thread-safety: this class is not thread safe. It must be externally synchronized if used by
// multiple threads.
class WaitingImageList {
 public:
  WaitingImageList() = default;
  WaitingImageList(const WaitingImageList& other) = delete;
  WaitingImageList& operator=(const WaitingImageList& other) = delete;
  ~WaitingImageList();

  // Maximum number of images that can be stored.
  static constexpr size_t kMaxSize = 10;

  // Only used internally; public for testing.
  class Entry {
   public:
    // The default constructor creates an empty entry.
    Entry() = default;

    // `image` must not be null. `wait_fence` may be null.
    explicit Entry(fbl::RefPtr<Image> image, fbl::RefPtr<Fence> wait_fence);

    // Entries are move-only.
    Entry(const Entry&) = delete;
    Entry(Entry&& entry) noexcept = default;
    Entry& operator=(const Entry&) = delete;
    Entry& operator=(Entry&& entry) noexcept = default;

    const fbl::RefPtr<Image>& image() const { return image_; }
    const fbl::RefPtr<Fence>& wait_fence() const { return wait_fence_; }

    // Takes the image out of the entry, leaving it null afterward.
    fbl::RefPtr<Image> TakeImage();

    // Resets the wait fence, if it exists.
    void ResetWaitFence();

    // Clears the entry's fence if it matches `fence`.
    void MarkFenceReady(Fence& fence);

   private:
    friend class WaitingImageList;

    bool IsReady() const { return wait_fence_ == nullptr; }

    fbl::RefPtr<Image> image_;
    fbl::RefPtr<Fence> wait_fence_;
  };

  // Retires the oldest `count` images. `count` must be <= `size()`.
  void RemoveOldestImages(size_t count);

  // Retires all images.
  void RemoveAllImages() { RemoveOldestImages(size()); }

  // Retires all occurrences of `image`.
  void RemoveImage(const Image& image);

  // Attempts to add an entry.  The following error results are possible:
  // - ZX_ERR_BAD_STATE when there is no space available (since the client can know/avoid this).
  // - ZX_ERR_BAD_STATE if `wait_fence` is already being waited upon.
  zx::result<> PushImage(fbl::RefPtr<Image> image, fbl::RefPtr<Fence> wait_fence);

  // Returns the newest "ready" image (i.e. with no unsignaled wait fence), or nullptr if none
  // exists. If a ready image is found, erase it and all earlier images from the list.
  fbl::RefPtr<Image> PopNewestReadyImage();

  // Marks any image that was waiting on `fence` as ready. Return true if *any* image is ready
  // (i.e. not just the images that were waiting on `fence`).
  bool MarkFenceReady(Fence& fence);

  // Finds the most recent waiting image and, if it exists, pass `stamp` to
  // `set_latest_client_config_stamp()`. This is part of the mechanism that
  void UpdateLatestClientConfigStamp(display::ConfigStamp stamp);

  // Used by tests. If there are any waiting images, returns the most recent one.
  fbl::RefPtr<Image> GetNewestImageForTesting() const;

  // Used by tests. Copies entries into a vector for easy inspection.
  std::vector<Entry> GetFullContentsForTesting() const;

  size_t size() const { return size_; }

 private:
  // Returns the entry at the given virtual index within the ring buffer.
  Entry& GetEntry(size_t index) {
    ZX_DEBUG_ASSERT(index < size_);
    return entries_[(ring_base_ + index) % kMaxSize];
  }

  // Returns the entry at the given virtual index within the ring buffer.
  const Entry& GetEntry(size_t index) const {
    ZX_DEBUG_ASSERT(index < size_);
    return entries_[(ring_base_ + index) % kMaxSize];
  }

  // Removes the oldest entry in the list; the list must not already be empty.
  void RemoveOldestImage();

  // Checks that various internal invariants are satisfied.
  void CheckRepresentation() const;

  // `WaitingImageList` is implemented as a fixed-size ring buffer.  `ring_base_` is the actual
  // index of the 0-th virtual index in the queue, and `size_` is the number of entries currently
  // held in the queue.
  size_t ring_base_ = 0U;
  size_t size_ = 0U;

  std::array<Entry, kMaxSize> entries_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_WAITING_IMAGE_LIST_H_
