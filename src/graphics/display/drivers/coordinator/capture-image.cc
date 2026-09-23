// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/capture-image.h"

#include <lib/inspect/cpp/inspect.h>
#include <zircon/assert.h>

#include <fbl/string_printf.h>

#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"

namespace display_coordinator {

CaptureImage::CaptureImage(Controller* controller, display::ImageId id,
                           display::DriverCaptureImageId driver_capture_image_id,
                           inspect::Node* parent_node, ClientId client_id)
    : IdMappable(id),
      driver_capture_image_id_(driver_capture_image_id),
      client_id_(client_id),
      controller_(controller) {
  ZX_DEBUG_ASSERT(controller_ != nullptr);
  ZX_DEBUG_ASSERT(id != display::kInvalidImageId);
  ZX_DEBUG_ASSERT(driver_capture_image_id != display::kInvalidDriverCaptureImageId);

  InitializeInspect(parent_node);
}

CaptureImage::~CaptureImage() { controller_->ReleaseCaptureImage(driver_capture_image_id_); }

void CaptureImage::InitializeInspect(inspect::Node* parent_node) {
  if (!parent_node)
    return;
  node_ = parent_node->CreateChild(fbl::StringPrintf("capture-image-%p", this).c_str());
  node_.CreateUint("client_id", client_id_.value(), &properties_);
}

}  // namespace display_coordinator
