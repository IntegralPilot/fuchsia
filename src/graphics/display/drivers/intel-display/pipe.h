// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_PIPE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_PIPE_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fit/function.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <list>
#include <span>
#include <unordered_map>

#include <region-alloc/region-alloc.h>

#include "src/graphics/display/drivers/intel-display/gtt.h"
#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/drivers/intel-display/registers-ddi.h"
#include "src/graphics/display/drivers/intel-display/registers-pipe.h"
#include "src/graphics/display/drivers/intel-display/registers-transcoder.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"

namespace intel_display {

class Controller;
class DisplayDevice;

class Pipe {
 public:
  Pipe(fdf::MmioBuffer* mmio_space, registers::Platform platform, PipeId pipe_id,
       PowerWellRef pipe_power);
  virtual ~Pipe() = default;

  Pipe(const Pipe&) = delete;
  Pipe(Pipe&&) = delete;
  Pipe& operator=(const Pipe&) = delete;
  Pipe& operator=(Pipe&&) = delete;

  void AttachToDisplay(display::DisplayId display_id, bool is_edp);
  void Detach();

  void ApplyModeConfig(const display::DisplayTiming& mode);

  using GetImagePixelFormatFunc =
      fit::function<PixelFormatAndModifier(display::DriverImageId image_id)>;
  using SetupGttImageFunc = fit::function<const GttRegion&(
      const display::ImageMetadata& image_metadata, display::DriverImageId image_id,
      display::CoordinateTransformation coordinate_transformation)>;
  void SubmitConfiguration(const display::ColorConversion& color_conversion,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp config_stamp,
                           const SetupGttImageFunc& setup_gtt_image,
                           const GetImagePixelFormatFunc& get_pixel_format);

  // Reset pipe registers and transcoders.
  void Reset();

  // Reset the pipe planes (layers).
  void ResetPlanes();

  // Resets the transcoder identified by `transcoder_id`.
  static void ResetTranscoder(TranscoderId transcoder_id, registers::Platform platform,
                              fdf::MmioBuffer* mmio_space);

  void LoadActiveMode(display::DisplayTiming* mode);

  PipeId pipe_id() const { return pipe_id_; }

  // Identifies the transcoder that is always tied to the pipe.
  //
  // Each pipe has a transcoder tied to it, which can output most display
  // protocols (DisplayPort, HDMI, DVI). This method identifies the pipe's tied
  // transcoder. The return value never changes, for a given pipe.
  //
  // See `connected_transcoder_id()` for identifying the transcoder that the
  // pipe is currently using.
  TranscoderId tied_transcoder_id() const { return static_cast<TranscoderId>(pipe_id_); }

  // Identifies the transcoder that is currently receiving the pipe's output.
  //
  // Each pipe has a tied transcoder, which can output most display protocols.
  // The display engine also has some specialized transcoders, which can be
  // connected to any pipe. The specialized transcoders are tied to DDIs that
  // use specialized protocols (Embedded DisplayPort, DDI), and used for writing
  // back to memory ("WD / Wireless Display" in Intel's docs).
  //
  // This method returns the transcoder that is currently connected to the pipe
  // output, which can be the general-purpose transcoder tied to the pipe, or
  // one of the shared specialized transcoders. The return value depends on how
  // we configure the display engine.
  virtual TranscoderId connected_transcoder_id() const = 0;

  display::DisplayId attached_display_id() const { return attached_display_id_; }
  bool in_use() const { return attached_display_id_ != display::kInvalidDisplayId; }

  // Display device registers only store image handles / addresses. We should
  // convert the handles to corresponding config stamps using the existing
  // mapping updated in |SubmitConfiguration()|.
  display::DriverConfigStamp GetVsyncConfigStamp(
      const std::vector<display::DriverImageId>& image_ids);

 protected:
  bool attached_edp() const { return attached_edp_; }
  registers::Platform platform() const { return platform_; }

 private:
  // Configures the plane `plane_num` using the layer `primary`.
  //
  // If `primary` is null, the primary plane will be disabled.
  // If `primary` is not null, it must have an Identity transformation
  // applied to the image source, and the image in `primary` must use linear
  // tiling.
  void ConfigurePrimaryPlane(uint32_t plane_num, const display::DriverLayer* primary,
                             bool enable_csc, bool* scaler_1_claimed,
                             registers::pipe_arming_regs* regs,
                             display::DriverConfigStamp config_stamp,
                             const SetupGttImageFunc& setup_gtt_image,
                             const GetImagePixelFormatFunc& get_pixel_format);
  void DisableCursorPlane(registers::pipe_arming_regs* regs,
                          display::DriverConfigStamp config_stamp);
  void SetColorConversionOffsets(bool preoffsets, std::span<const float, 3> vals);
  void ResetActiveTranscoder();
  void ResetScaler();

  // Borrowed reference to Controller instance
  fdf::MmioBuffer* mmio_space_ = nullptr;

  display::DisplayId attached_display_id_ = display::kInvalidDisplayId;
  bool attached_edp_ = false;

  registers::Platform platform_;
  PipeId pipe_id_;

  PowerWellRef pipe_power_;

  // For any scaled planes, this contains the (1-based) index of the active scaler
  uint32_t scaled_planes_[PipeIds<registers::Platform::kKabyLake>().size()]
                         [registers::kImagePlaneCount] = {};

  // Configuration stamps that have been applied and are pending eviction.
  // The values of the config stamps in this list must be strictly increasing.
  //
  // Unused configuration stamps, which are older than all the current config
  // stamps used in the display layers, will be evicted from the list on each
  // Vsync.
  std::list<display::DriverConfigStamp> pending_eviction_config_stamps_;

  // The pipe registers only store the handle (address) of the images that are
  // being displayed. We need to keep a mapping from *image handle* to the
  // latest *config stamp* where this image is used so that we can know which
  // layer has the oldest configuration.
  std::unordered_map<display::DriverImageId, display::DriverConfigStamp>
      latest_config_stamp_with_image_;

  // If the (there can be at most one) background color layer is enabled on the
  // pipe, we need to keep the config stamp of the configuration that enables
  // the color layer, so that we can return it on a Vsync event of a frame with
  // background color.
  // Set to `kInvalidConfigStamp` if there's no background color layer.
  display::DriverConfigStamp config_stamp_with_color_layer_ = display::kInvalidDriverConfigStamp;
};

class PipeSkylake : public Pipe {
 public:
  PipeSkylake(fdf::MmioBuffer* mmio_space, PipeId pipe_id, PowerWellRef pipe_power)
      : Pipe(mmio_space, registers::Platform::kSkylake, pipe_id, std::move(pipe_power)) {}
  ~PipeSkylake() override = default;

  TranscoderId connected_transcoder_id() const override {
    return attached_edp() ? TranscoderId::TRANSCODER_EDP : tied_transcoder_id();
  }
};

class PipeTigerLake : public Pipe {
 public:
  PipeTigerLake(fdf::MmioBuffer* mmio_space, PipeId pipe_id, PowerWellRef pipe_power)
      : Pipe(mmio_space, registers::Platform::kTigerLake, pipe_id, std::move(pipe_power)) {}
  ~PipeTigerLake() override = default;

  TranscoderId connected_transcoder_id() const override { return tied_transcoder_id(); }
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_PIPE_H_
