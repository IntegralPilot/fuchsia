// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include <chrono>
#include <ctime>
#include <sstream>

#include "src/ui/lib/escher/escher_flatland/escher_flatland.h"
#include "src/ui/lib/escher/renderer/frame.h"

std::string GetCurrentTimeString(void) {
  // TODO(https://fxbug.dev/344700148): This clock is broken, and always returns 21:10:26.
  // In the interim, consider replacing the following code with native Zircon calls.
  std::time_t now = std::chrono::system_clock::to_time_t(std::chrono::system_clock::now());
  std::tm* time_struct = std::localtime(&now);
  std::ostringstream oss;
  oss << time_struct->tm_hour << ":" << time_struct->tm_min << ":" << time_struct->tm_sec;
  return oss.str();
}

int main(int argc, char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  trace::TraceProviderWithFdio trace_provider(loop.dispatcher());
  escher::EscherFlatland escher_flatland(loop.dispatcher(), "escher-flatland");

  escher::DebugFont debug_font = escher_flatland.debug_font();
  escher::RenderFrameFn render_frame = [&debug_font](const escher::ImagePtr& output_image,
                                                     const vk::Extent2D image_extent,
                                                     const escher::FramePtr& frame) {
    frame->cmds()->vk().clearColorImage(
        output_image->vk(), vk::ImageLayout::eTransferDstOptimal,
        vk::ClearColorValue(std::array<float, 4>{0.f, 0.f, 0.f, 0.f}),
        {vk::ImageSubresourceRange(vk::ImageAspectFlagBits::eColor, 0, 1, 0, 1)});

    static int32_t dir_x = 3;
    static int32_t dir_y = 1;
    static int32_t pos_x = 0;
    static int32_t pos_y = 0;

    auto time_string = GetCurrentTimeString();

    constexpr int32_t kScale = 4;
    const int32_t kWidth =
        static_cast<int32_t>(time_string.length()) * escher::DebugFont::kGlyphWidth * kScale;
    constexpr int32_t kHeight = escher::DebugFont::kGlyphHeight * kScale;

    if (dir_x < 0 && pos_x <= 0)
      dir_x *= -1;
    if (dir_y < 0 && pos_y <= 0)
      dir_y *= -1;
    if (dir_x > 0 && (pos_x + kWidth) >= static_cast<int32_t>(image_extent.width))
      dir_x *= -1;
    if (dir_y > 0 && (pos_y + kHeight) >= static_cast<int32_t>(image_extent.height))
      dir_y *= -1;

    pos_x += dir_x;
    pos_y += dir_y;

    debug_font.Blit(frame->cmds(), time_string.c_str(), output_image, {pos_x, pos_y}, kScale);
  };

  async::PostTask(loop.dispatcher(), [&]() mutable {
    escher_flatland.RenderLoop(render_frame);
    escher_flatland.SetVisible(true);
  });

  loop.Run();

  return 0;
}
