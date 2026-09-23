// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_EXAMPLES_VKPROTO_FUCHSIA_FUCHSIA_SURFACE_H_
#define SRC_GRAPHICS_EXAMPLES_VKPROTO_FUCHSIA_FUCHSIA_SURFACE_H_

#include <memory>

#include "src/graphics/examples/vkproto/common/surface.h"

namespace vkp {

class FuchsiaSurface : public Surface {
 public:
  explicit FuchsiaSurface(std::shared_ptr<vk::Instance> instance);
  virtual ~FuchsiaSurface();

  bool Init() override;

 private:
  VKP_DISALLOW_COPY_AND_ASSIGN(FuchsiaSurface);
};

}  // namespace vkp

#endif  // SRC_GRAPHICS_EXAMPLES_VKPROTO_FUCHSIA_FUCHSIA_SURFACE_H_
