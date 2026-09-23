// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/waiting-image-list.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstddef>
#include <utility>
#include <vector>

#include <fbl/ref_ptr.h>

#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/image.h"

namespace display_coordinator {

WaitingImageList::Entry::Entry(fbl::RefPtr<Image> image, fbl::RefPtr<Fence> wait_fence)
    : image_(std::move(image)), wait_fence_(std::move(wait_fence)) {
  ZX_DEBUG_ASSERT(image_ != nullptr);
}

fbl::RefPtr<Image> WaitingImageList::Entry::TakeImage() {
  ZX_DEBUG_ASSERT(image_ != nullptr);
  ZX_DEBUG_ASSERT(wait_fence_ == nullptr);
  return std::move(image_);
}

void WaitingImageList::Entry::ResetWaitFence() { wait_fence_ = nullptr; }

void WaitingImageList::Entry::MarkFenceReady(Fence& fence) {
  if (wait_fence_.get() != &fence) {
    return;
  }

  wait_fence_ = nullptr;
}

WaitingImageList::~WaitingImageList() {
  // Ensure that all waiting fences are reset.
  RemoveAllImages();
}

void WaitingImageList::RemoveOldestImages(size_t count) {
  ZX_DEBUG_ASSERT(count <= size_);

  while (count--) {
    RemoveOldestImage();
  }
  CheckRepresentation();
}

void WaitingImageList::RemoveImage(const Image& image_to_retire) {
  // The read/write indices are "logical indices"; they haven't been mapped into the ring buffer.
  size_t write_index = 0;
  size_t erase_count = 0;
  for (size_t read_index = 0; read_index < size_; ++read_index) {
    Entry& read_entry = GetEntry(read_index);
    const bool should_erase = &image_to_retire == read_entry.image().get();
    if (should_erase) {
      // Images match, so reset the read entry without moving it.
      read_entry = Entry();
      ++erase_count;
    } else {
      // Images don't match, so move the read entry into the write entry.
      if (read_index != write_index) {
        Entry& write_entry = GetEntry(write_index);
        write_entry = std::move(read_entry);
      }
      ++write_index;
    }
  }
  size_ -= erase_count;

  CheckRepresentation();
}

zx::result<> WaitingImageList::PushImage(fbl::RefPtr<Image> image, fbl::RefPtr<Fence> wait_fence) {
  if (size_ >= kMaxSize) {
    fdf::error("Failed to allocate waiting-image");
    return zx::error_result(ZX_ERR_BAD_STATE);
  }
  if (wait_fence) {
    zx::result status = wait_fence->Wait();
    if (status.is_error()) {
      fdf::error("Failed to start waiting for image: {}", status);
      // Mark the image as ready. Displaying garbage is better than hanging or crashing.
      wait_fence = nullptr;
    }
  }

  Entry& new_entry = GetEntry(size_++);
  ZX_DEBUG_ASSERT(new_entry.image() == nullptr);
  ZX_DEBUG_ASSERT(new_entry.wait_fence() == nullptr);
  new_entry = Entry(std::move(image), std::move(wait_fence));
  CheckRepresentation();

  return zx::ok();
}

fbl::RefPtr<Image> WaitingImageList::PopNewestReadyImage() {
  // Count down from most recent image until we find one that is ready.
  size_t count = size();
  while (count--) {
    // Candidate entry, to check if image is ready.
    Entry& entry = GetEntry(count);

    if (entry.IsReady()) {
      // Erase all earlier entries.
      RemoveOldestImages(count);

      // Must also erase the entry containing the image we found.
      // The image is reset by the std::move(). No need to reset the wait fence; it is nullptr.
      ZX_DEBUG_ASSERT(size_ > 0);
      ZX_DEBUG_ASSERT(&GetEntry(0) == &entry);
      ZX_DEBUG_ASSERT(!entry.wait_fence());
      --size_;
      ring_base_ = (ring_base_ + 1) % kMaxSize;
      fbl::RefPtr<Image> image = entry.TakeImage();
      CheckRepresentation();
      return image;
    }
  }
  // No ready image was found.
  CheckRepresentation();
  return nullptr;
}

bool WaitingImageList::MarkFenceReady(Fence& fence) {
  bool any_image_ready = false;
  for (size_t i = 0; i < size(); ++i) {
    Entry& entry = GetEntry(i);
    entry.MarkFenceReady(fence);
    any_image_ready |= entry.IsReady();
  }
  CheckRepresentation();
  return any_image_ready;
}

void WaitingImageList::UpdateLatestClientConfigStamp(display::ConfigStamp stamp) {
  if (size_ == 0) {
    return;
  }
  const Entry& entry = GetEntry(size_ - 1);
  entry.image()->set_latest_client_config_stamp(stamp);
}

fbl::RefPtr<Image> WaitingImageList::GetNewestImageForTesting() const {
  if (size_) {
    return GetEntry(size_ - 1).image();
  }
  return nullptr;
}

std::vector<WaitingImageList::Entry> WaitingImageList::GetFullContentsForTesting() const {
  std::vector<WaitingImageList::Entry> result;
  result.reserve(size_);
  for (size_t i = 0; i < size_; ++i) {
    const Entry& entry = GetEntry(i);
    result.emplace_back(entry.image(), entry.wait_fence());
  }
  return result;
}

void WaitingImageList::RemoveOldestImage() {
  ZX_DEBUG_ASSERT(size_ > 0);
  Entry& entry = GetEntry(0);
  ZX_DEBUG_ASSERT(entry.image());
  entry.ResetWaitFence();
  entry = Entry();
  --size_;
  ring_base_ = (ring_base_ + 1) % kMaxSize;
}

void WaitingImageList::CheckRepresentation() const {
#ifndef NDEBUG
  ZX_ASSERT_MSG(ring_base_ < kMaxSize,
                "Invalidity detected: `ring_base_` is out of range (ring_base_=%lu, kMaxSize=%lu)",
                ring_base_, kMaxSize);

  ZX_ASSERT_MSG(size_ <= kMaxSize,
                "Invalidity detected: exceeded maximum size (size_=%lu, kMaxSize=%lu)", size_,
                kMaxSize);

  // Check validity of active entries.
  for (size_t i = 0; i < size_; ++i) {
    const Entry& entry = GetEntry(i);
    ZX_ASSERT_MSG(entry.image() != nullptr,
                  "Invalidity detected: image cannot be null.  ring_base_: %lu  size_: %lu  i: %lu",
                  ring_base_, size_, i);
  }

  // Check validity of inactive entries.
  for (size_t i = size_; i < kMaxSize; ++i) {
    // Can't use `GetEntry()` because it would fail bounds check.
    const Entry& entry = entries_[(ring_base_ + i) % kMaxSize];
    ZX_ASSERT_MSG(entry.image() == nullptr,
                  "Invalidity detected: image must be null.  ring_base_: %lu  size_: %lu  i: %lu",
                  ring_base_, size_, i);

    ZX_ASSERT_MSG(
        entry.wait_fence() == nullptr,
        "Invalidity detected: wait fence must be null.  ring_base_: %lu  size_: %lu  i: %lu",
        ring_base_, size_, i);
  }

#endif  // #ifndef NDEBUG
}

}  // namespace display_coordinator
