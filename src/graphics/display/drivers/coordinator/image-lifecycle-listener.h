  // Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_LIFECYCLE_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_LIFECYCLE_LISTENER_H_

#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"

namespace display_coordinator {

class ImageLifecycleListener {
  public:
  ImageLifecycleListener() = default;

  // ImageLifecycleListener pointers must remain stable.
  ImageLifecycleListener(const ImageLifecycleListener&) = delete;
  ImageLifecycleListener(ImageLifecycleListener&&) = delete;
  ImageLifecycleListener& operator=(const ImageLifecycleListener&) = default;
  ImageLifecycleListener& operator=(ImageLifecycleListener&&) = delete;

  // Called when the image is about to be destroyed.
  virtual void ImageWillBeDestroyed(display::DriverImageId driver_image_id) = 0;

  protected:
  // ImageLifecycleListener is not intended to be an owning pointer type.
  ~ImageLifecycleListener() = default;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_IMAGE_LIFECYCLE_LISTENER_H_
