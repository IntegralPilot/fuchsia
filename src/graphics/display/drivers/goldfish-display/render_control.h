// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_RENDER_CONTROL_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_RENDER_CONTROL_H_

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <lib/fzl/pinned-vmo.h>
#include <zircon/types.h>

#include <map>
#include <memory>

#include <fbl/strong_int.h>

#include "src/devices/lib/goldfish/pipe_io/pipe_io.h"

namespace goldfish {

// A strong-typed ID of the ColorBuffer used in goldfish rendering.
DEFINE_STRONG_INT(HostColorBufferId, uint32_t);

constexpr HostColorBufferId kInvalidHostColorBufferId = HostColorBufferId(0u);

constexpr inline uint32_t ToRenderControlHostColorBufferId(HostColorBufferId host_color_buffer_id) {
  return host_color_buffer_id.value();
}
constexpr inline HostColorBufferId ToHostColorBufferId(
    uint32_t render_control_host_color_buffer_id) {
  return HostColorBufferId(render_control_host_color_buffer_id);
}

// A strong-typed ID of the goldfish host renderer display.
DEFINE_STRONG_INT(HostDisplayId, uint32_t);

constexpr HostDisplayId kInvalidHostDisplayId = HostDisplayId(0u);

constexpr inline uint32_t ToRenderControlHostDisplayId(HostDisplayId host_display_id) {
  return host_display_id.value();
}
constexpr inline HostDisplayId ToHostDisplayId(uint32_t render_control_host_display_id) {
  return HostDisplayId(render_control_host_display_id);
}

// This implements a client of goldfish renderControl API over
// goldfish pipe communication. The methods are defined in
// https://android.googlesource.com/device/generic/goldfish-opengl/+/master/system/renderControl_enc/README
class RenderControl {
 public:
  RenderControl() = default;
  zx_status_t InitRcPipe(fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus>);

  int32_t GetFbParam(uint32_t param, int32_t default_value);

  zx::result<HostColorBufferId> CreateColorBuffer(uint32_t width, uint32_t height, uint32_t format);
  zx_status_t OpenColorBuffer(HostColorBufferId host_color_buffer_id);
  zx_status_t CloseColorBuffer(HostColorBufferId host_color_buffer_id);

  // Zero means success; non-zero value means the call failed.
  using RcResult = int32_t;
  zx::result<RcResult> SetColorBufferVulkanMode(HostColorBufferId host_color_buffer_id,
                                                uint32_t mode);
  zx::result<RcResult> UpdateColorBuffer(HostColorBufferId host_color_buffer_id,
                                         const fzl::PinnedVmo& pinned_vmo, uint32_t width,
                                         uint32_t height, uint32_t format, size_t size);
  zx_status_t FbPost(HostColorBufferId host_color_buffer_id);

  zx::result<HostDisplayId> CreateDisplay();
  zx::result<RcResult> DestroyDisplay(HostDisplayId display_id);
  zx::result<RcResult> SetDisplayColorBuffer(HostDisplayId display_id,
                                             HostColorBufferId host_color_buffer_id);
  zx::result<RcResult> SetDisplayPose(HostDisplayId display_id, int32_t x, int32_t y, uint32_t w,
                                      uint32_t h);

  PipeIo* pipe_io() { return pipe_io_.get(); }

 private:
  std::unique_ptr<PipeIo> pipe_io_;
  DISALLOW_COPY_ASSIGN_AND_MOVE(RenderControl);
};

}  // namespace goldfish

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_GOLDFISH_DISPLAY_RENDER_CONTROL_H_
