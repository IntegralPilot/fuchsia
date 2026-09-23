// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/goldfish-display/render_control.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/trace/event.h>

#include <memory>

#include "src/devices/lib/goldfish/pipe_io/pipe_io.h"

namespace goldfish {

namespace {

const char* kPipeName = "pipe:opengles";

constexpr uint32_t GL_UNSIGNED_BYTE = 0x1401;

// All the render control (rc*) functions are defined in Android device/generic/
// goldfish-opengl/system/renderControl_enc/renderControl.in file.
// The opcodes are available at Android device/generic/goldfish-opengl/system/
// renderControl_enc/renderControl_opcodes.h.
constexpr uint32_t kOP_rcGetFbParam = 10007;
struct GetFbParamCmd {
  uint32_t op = kOP_rcGetFbParam;
  uint32_t size = sizeof(GetFbParamCmd);
  uint32_t param;
};

constexpr uint32_t kOP_rcCreateColorBuffer = 10012;
struct CreateColorBufferCmd {
  uint32_t op = kOP_rcCreateColorBuffer;
  uint32_t size = sizeof(CreateColorBufferCmd);
  uint32_t width;
  uint32_t height;
  uint32_t internalformat;
};

constexpr uint32_t kOP_rcOpenColorBuffer = 10013;
struct OpenColorBufferCmd {
  uint32_t op = kOP_rcOpenColorBuffer;
  uint32_t size = sizeof(OpenColorBufferCmd);
  uint32_t id;
};

constexpr uint32_t kOP_rcCloseColorBuffer = 10014;
struct CloseColorBufferCmd {
  uint32_t op = kOP_rcCloseColorBuffer;
  uint32_t size = sizeof(CloseColorBufferCmd);
  uint32_t id;
};

constexpr uint32_t kOP_rcSetColorBufferVulkanMode = 10045;
struct SetColorBufferVulkanModeCmd {
  uint32_t op = kOP_rcSetColorBufferVulkanMode;
  uint32_t size = sizeof(SetColorBufferVulkanModeCmd);
  uint32_t color_buffer_id;
  uint32_t mode;
};

constexpr uint32_t kOP_rcUpdateColorBuffer = 10024;
struct UpdateColorBufferCmd {
  uint32_t op = kOP_rcUpdateColorBuffer;
  uint32_t size = sizeof(UpdateColorBufferCmd);
  uint32_t color_buffer_id;
  uint32_t x;
  uint32_t y;
  uint32_t width;
  uint32_t height;
  uint32_t format;
  uint32_t type;
  uint32_t size_pixels;
};

constexpr uint32_t kOP_rcFbPost = 10018;
struct FbPostCmd {
  uint32_t op = kOP_rcFbPost;
  uint32_t size = sizeof(FbPostCmd);
  uint32_t color_buffer_id;
};

constexpr uint32_t kOP_rcCreateDisplay = 10038;
struct CreateDisplayCmd {
  uint32_t op = kOP_rcCreateDisplay;
  uint32_t size = sizeof(CreateDisplayCmd);
  uint32_t size_display_id;
};

constexpr uint32_t kOP_rcDestroyDisplay = 10039;
struct DestroyDisplayCmd {
  uint32_t op = kOP_rcDestroyDisplay;
  uint32_t size = sizeof(DestroyDisplayCmd);
  uint32_t display_id;
};

constexpr uint32_t kOP_rcSetDisplayColorBuffer = 10040;
struct SetDisplayColorBufferCmd {
  uint32_t op = kOP_rcSetDisplayColorBuffer;
  uint32_t size = sizeof(SetDisplayColorBufferCmd);
  uint32_t display_id;
  uint32_t color_buffer_id;
};

constexpr uint32_t kOP_rcSetDisplayPose = 10044;
struct SetDisplayPoseCmd {
  uint32_t op = kOP_rcSetDisplayPose;
  uint32_t size = sizeof(SetDisplayPoseCmd);
  uint32_t display_id;
  int32_t x;
  int32_t y;
  uint32_t w;
  uint32_t h;
};

template <class T>
std::span<const uint8_t> ToByteSpan(const T& t) {
  return std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(&t),
                                  reinterpret_cast<const uint8_t*>(&t + 1));
}

}  // namespace

zx_status_t RenderControl::InitRcPipe(
    fidl::WireSyncClient<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus) {
  pipe_io_ = std::make_unique<PipeIo>(std::move(pipe_bus), kPipeName);
  if (!pipe_io_->valid()) {
    fdf::error("PipeIo failed to initialize");
    return ZX_ERR_NOT_SUPPORTED;
  }

  constexpr uint32_t kClientFlags = 0;
  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(kClientFlags)}};
  auto status = pipe_io_->Write(src, true);
  if (status != ZX_OK) {
    fdf::error("Write client flags failed");
    return ZX_ERR_NOT_SUPPORTED;
  }

  return ZX_OK;
}

int32_t RenderControl::GetFbParam(uint32_t param, int32_t default_value) {
  TRACE_DURATION("gfx", "RenderControl::GetFbParam", "param", param);

  GetFbParamCmd cmd = {
      .param = param,
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<int32_t>(src, 1, true);
  return (result.is_ok()) ? result.value()[0] : default_value;
}

zx::result<HostColorBufferId> RenderControl::CreateColorBuffer(uint32_t width, uint32_t height,
                                                               uint32_t format) {
  TRACE_DURATION("gfx", "RenderControl::CreateColorBuffer", "width", width, "height", height,
                 "format", format);

  CreateColorBufferCmd cmd = {
      .width = width,
      .height = height,
      .internalformat = format,
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<uint32_t>(src, 1, true);
  return result.is_ok() ? zx::ok(ToHostColorBufferId(result.value()[0]))
                        : zx::result<HostColorBufferId>(result.take_error());
}

zx_status_t RenderControl::OpenColorBuffer(HostColorBufferId host_color_buffer_id) {
  TRACE_DURATION("gfx", "RenderControl::OpenColorBuffer", "id", host_color_buffer_id.value());

  OpenColorBufferCmd cmd = {
      .id = ToRenderControlHostColorBufferId(host_color_buffer_id),
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  return pipe_io_->Write(src, true);
}

zx_status_t RenderControl::CloseColorBuffer(HostColorBufferId host_color_buffer_id) {
  TRACE_DURATION("gfx", "RenderControl::CloseColorBuffer", "id", host_color_buffer_id.value());

  CloseColorBufferCmd cmd = {
      .id = ToRenderControlHostColorBufferId(host_color_buffer_id),
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  return pipe_io_->Write(src, true);
}

zx::result<RenderControl::RcResult> RenderControl::SetColorBufferVulkanMode(
    HostColorBufferId host_color_buffer_id, uint32_t mode) {
  TRACE_DURATION("gfx", "RenderControl::SetColorBufferVulkanMode", "id",
                 host_color_buffer_id.value(), "mode", mode);

  SetColorBufferVulkanModeCmd cmd = {
      .color_buffer_id = ToRenderControlHostColorBufferId(host_color_buffer_id),
      .mode = mode,
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<RcResult>(src, 1, true);
  return result.is_ok() ? zx::ok(result.value()[0]) : zx::result<RcResult>(result.take_error());
}

zx::result<RenderControl::RcResult> RenderControl::UpdateColorBuffer(
    HostColorBufferId host_color_buffer_id, const fzl::PinnedVmo& pinned_vmo, uint32_t width,
    uint32_t height, uint32_t format, size_t size) {
  TRACE_DURATION("gfx", "RenderControl::UpdateColorBuffer", "size", size);

  UpdateColorBufferCmd cmd = {
      .size = static_cast<uint32_t>(size + sizeof(cmd)),
      .color_buffer_id = ToRenderControlHostColorBufferId(host_color_buffer_id),
      .x = 0,
      .y = 0,
      .width = width,
      .height = height,
      .format = format,
      .type = GL_UNSIGNED_BYTE,
      .size_pixels = static_cast<uint32_t>(size),
  };

  PipeIo::WriteSrc src[] = {
      {.data = ToByteSpan(cmd)},
      {.data =
           PipeIo::WriteSrc::PinnedVmo{
               .vmo = &pinned_vmo,
               .offset = 0,
               .size = size,
           }},
  };

  auto write_result = pipe_io_->Write(src, false);
  if (write_result != ZX_OK) {
    // It's possible that there's some back pressure when updating the color
    // buffer. In that case we just skip it for this frame.
    return zx::ok(0);
  }
  auto result = pipe_io_->Read<RcResult>(1, true);
  return result.is_ok() ? zx::ok(result.value()[0]) : zx::result<RcResult>(result.take_error());
}

zx_status_t RenderControl::FbPost(HostColorBufferId host_color_buffer_id) {
  TRACE_DURATION("gfx", "RenderControl::FbPost", "id", host_color_buffer_id.value());

  FbPostCmd cmd = {
      .color_buffer_id = ToRenderControlHostColorBufferId(host_color_buffer_id),
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Write(src, false);
  return result;
}

zx::result<HostDisplayId> RenderControl::CreateDisplay() {
  TRACE_DURATION("gfx", "RenderControl::CreateDisplay");

  CreateDisplayCmd cmd = {
      .size_display_id = sizeof(uint32_t),
  };

  using CreateDisplayResult = struct {
    uint32_t id;
    int32_t result;
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<CreateDisplayResult>(src, 1, true);

  if (result.is_error()) {
    return result.take_error();
  }
  if (result.value()[0].result != 0) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  return zx::ok(ToHostDisplayId(result.value()[0].id));
}

zx::result<RenderControl::RcResult> RenderControl::DestroyDisplay(HostDisplayId host_display_id) {
  TRACE_DURATION("gfx", "RenderControl::DestroyDisplay", "display_id", host_display_id.value());

  DestroyDisplayCmd cmd = {
      .display_id = ToRenderControlHostDisplayId(host_display_id),
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<RcResult>(src, 1, true);
  return result.is_ok() ? zx::ok(result.value()[0]) : zx::result<RcResult>(result.take_error());
}

zx::result<RenderControl::RcResult> RenderControl::SetDisplayColorBuffer(
    HostDisplayId host_display_id, HostColorBufferId host_color_buffer_id) {
  TRACE_DURATION("gfx", "RenderControl::SetDisplayColorBuffer", "display_id",
                 host_display_id.value(), "id", host_color_buffer_id.value());

  SetDisplayColorBufferCmd cmd = {
      .display_id = ToRenderControlHostDisplayId(host_display_id),
      .color_buffer_id = ToRenderControlHostColorBufferId(host_color_buffer_id),
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<RcResult>(src, 1, true);
  return result.is_ok() ? zx::ok(result.value()[0]) : zx::result<RcResult>(result.take_error());
}

zx::result<RenderControl::RcResult> RenderControl::SetDisplayPose(HostDisplayId host_display_id,
                                                                  int32_t x, int32_t y, uint32_t w,
                                                                  uint32_t h) {
  TRACE_DURATION("gfx", "RenderControl::SetDisplayPose", "display_id", host_display_id.value());

  SetDisplayPoseCmd cmd = {
      .display_id = ToRenderControlHostDisplayId(host_display_id),
      .x = x,
      .y = y,
      .w = w,
      .h = h,
  };

  PipeIo::WriteSrc src[] = {{.data = ToByteSpan(cmd)}};
  auto result = pipe_io_->Call<RcResult>(src, 1, true);
  return result.is_ok() ? zx::ok(result.value()[0]) : zx::result<RcResult>(result.take_error());
}

}  // namespace goldfish
