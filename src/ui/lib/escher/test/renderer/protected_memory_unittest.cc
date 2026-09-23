// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/escher.h"
#include "src/ui/lib/escher/renderer/frame.h"
#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/util/image_utils.h"
#include "src/ui/lib/escher/vk/command_buffer.h"
#include "src/ui/lib/escher/vk/image_factory.h"

namespace {
using namespace escher;

using ProtectedMemoryTest = escher::test::TestWithVkValidationLayer;

// Tests that we can create Escher with a protected Vk instance if platform supports.
VK_TEST_F(ProtectedMemoryTest, CreateProtectedEnabledEscher) {
  auto escher = test::CreateEscherWithProtectedMemoryEnabled();
  EXPECT_TRUE(!escher || escher->allow_protected_memory());
}

// Tests that we can ask platform to provide protected enabled CommandBuffer.
VK_TEST_F(ProtectedMemoryTest, CreateProtectedEnabledCommandBuffer) {
  auto escher = test::CreateEscherWithProtectedMemoryEnabled();
  if (!escher) {
    GTEST_SKIP();
  }

  auto cb = CommandBuffer::NewForType(escher.get(), CommandBuffer::Type::kGraphics,
                                      /*use_protected_memory=*/true);
  EXPECT_TRUE(cb->Submit(nullptr));
}

// Tests that we can create protected enabled Escher::Frame.
VK_TEST_F(ProtectedMemoryTest, CreateProtectedEnabledFrame) {
  auto escher = test::CreateEscherWithProtectedMemoryEnabled();
  if (!escher) {
    GTEST_SKIP();
  }

  {
    auto frame = escher->NewFrame("test_frame", 0, false, escher::CommandBuffer::Type::kGraphics,
                                  /*use_protected_memory=*/true);
    frame->EndFrame(SemaphorePtr(), [] {});
  }
}

}  // namespace
