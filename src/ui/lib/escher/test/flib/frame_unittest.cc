// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/renderer/frame.h"

#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>

#include <chrono>
#include <thread>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/test/flib/util.h"
#include "src/ui/lib/escher/util/fuchsia_utils.h"

namespace escher::test {

using FrameTest = test::TestWithVkValidationLayer;

VK_TEST_F(FrameTest, SubmitFrameWithUnsignalledWaitSemaphore) {
  async::TestLoop loop;
  auto escher = test::GetEscher()->GetWeakPtr();
  auto frame = escher->NewFrame("test_frame", 0, false, CommandBuffer::Type::kGraphics);

  // Add a wait semaphore.
  auto acquire_semaphore_pair = escher::NewSemaphoreEventPair(escher.get());
  frame->cmds()->AddWaitSemaphore(acquire_semaphore_pair.first,
                                  vk::PipelineStageFlagBits::eTopOfPipe);
  EXPECT_FALSE(IsEventSignalled(acquire_semaphore_pair.second, ZX_EVENT_SIGNALED));

  // Add a release semaphore.
  auto release_semaphore_pair = escher::NewSemaphoreEventPair(escher.get());
  frame->cmds()->AddSignalSemaphore(release_semaphore_pair.first);
  EXPECT_FALSE(IsEventSignalled(release_semaphore_pair.second, ZX_EVENT_SIGNALED));

  // Signal the wait semaphore on a background thread.  It seems that some implementations (e.g.
  // Lavapipe and Goldfish) block in `vkQueueSubmit()`; earlier versions of the test assumed that
  // we could signal the wait event from the same thread after calling `vkQueueSubmit()`, but this
  // resulted in deadlock.
  std::thread t([&] {
    using namespace std::chrono_literals;
    std::this_thread::sleep_for(100ms);

    // Neither semaphore should be signaled yet.  We'll signal the acquire semaphore in a moment,
    // and there's no reason for anybody to signal the release semaphore.  This is paranoia, sweet
    // sweet paranoia.
    EXPECT_FALSE(IsEventSignalled(acquire_semaphore_pair.second, ZX_EVENT_SIGNALED));
    EXPECT_NE(release_semaphore_pair.second.wait_one(ZX_EVENT_SIGNALED,
                                                     zx::deadline_after(zx::msec(200)), nullptr),
              ZX_OK);

    // Signal wait semaphore.
    EXPECT_EQ(acquire_semaphore_pair.second.signal(0u, ZX_EVENT_SIGNALED), ZX_OK);
  });

  // Submit frame while wait semaphore is not signalled.
  frame->EndFrame(SemaphorePtr(), [] {});

  // Release semaphore should be signaled and acquire semaphore should be de-signaled by Vulkan.
  EXPECT_EQ(
      release_semaphore_pair.second.wait_one(ZX_EVENT_SIGNALED, zx::time::infinite(), nullptr),
      ZX_OK);
  loop.RunUntilIdle();
  if (!escher::test::GlobalEscherUsesVirtualGpu()) {
    // TODO(https://fxbug.dev/434039865): the semaphore should be de-signaled by Vulkan, but the
    // Goldfish driver doesn't do this.
    EXPECT_FALSE(IsEventSignalled(acquire_semaphore_pair.second, ZX_EVENT_SIGNALED));
  }

  // Cleanup
  EXPECT_EQ(vk::Result::eSuccess, escher->vk_device().waitIdle());
  loop.RunUntilIdle();
  t.join();
}

}  // namespace escher::test
