// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_TESTS_COMMON_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_TESTS_COMMON_H_

#include <lib/fdio/directory.h>

#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"

namespace flatland {

std::pair<std::unique_ptr<escher::Escher>, std::unique_ptr<VkRenderer>>
CreateEscherAndPrewarmedRenderer(bool use_protected_memory = false);

// Common testing base class to be used across different unittests that
// require Vulkan and a SysmemAllocator.
class RendererTest : public escher::test::TestWithVkValidationLayer {
 protected:
  void SetUp() override;
  void TearDown() override;

  fidl::WireClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocatorClient(
      async_dispatcher_t* dispatcher);
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_RENDERER_TESTS_COMMON_H_
