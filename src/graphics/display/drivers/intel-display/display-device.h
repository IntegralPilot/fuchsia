// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DISPLAY_DEVICE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DISPLAY_DEVICE_H_

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <optional>
#include <span>

#include <fbl/vector.h>
#include <region-alloc/region-alloc.h>

#include "src/graphics/display/drivers/intel-display/ddi-physical-layer-manager.h"
#include "src/graphics/display/drivers/intel-display/dpll.h"
#include "src/graphics/display/drivers/intel-display/pipe.h"
#include "src/graphics/display/drivers/intel-display/power.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace intel_display {

class Controller;

struct AddedDisplayInfo {
  static constexpr int kMaxPreferredModes = 32;

  display::DisplayId display_id;
  fbl::Vector<display::ModeAndId> preferred_modes;
};

class DisplayDevice {
 public:
  enum class Type {
    kEdp,
    kDp,
    kHdmi,
    kDvi,
  };

  DisplayDevice(Controller* controller, display::DisplayId id, DdiId ddi_id,
                DdiReference ddi_reference, Type type);

  DisplayDevice(const DisplayDevice&) = delete;
  DisplayDevice(DisplayDevice&&) = delete;
  DisplayDevice& operator=(const DisplayDevice&) = delete;
  DisplayDevice& operator=(DisplayDevice&&) = delete;

  virtual ~DisplayDevice();

  void SubmitConfiguration(display::ModeId mode_id,
                           const display::ColorConversion& color_conversion,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp config_stamp);

  // TODO(https://fxbug.dev/42167004): Initialization-related interactions between the Controller
  // class and DisplayDevice can currently take different paths, with Init() being called
  // conditionally in some cases (e.g. if the display has already been configured and powered up by
  // the bootloader), which means a DisplayDevice can hold many states before being considered
  // fully-initialized. It would be good to simplify this by:
  // 1. Eliminating the "partially initialized" DisplayDevice state from the point of its owner.
  // 2. Having a single Init factory function with options, such as the current DPLL state, which is
  // always called to construct a DisplayDevice, possibly merging Query, Init,
  // InitWithDdiPllConfig, and InitBacklight, into a single routine.
  // 3. Perhaps what represents a DDI and a display attached to a DDI should be separate
  // abstractions?

  // Query whether or not there is a display attached to this ddi. Does not
  // actually do any initialization - that is done by Init.
  virtual bool Query() = 0;
  // Does display mode agnostic ddi initialization - subclasses implement InitDdi.
  bool Init();
  // Initialize the display based on existing hardware state. This method should be used instead of
  // Init() when a display PLL has already been powered up and configured (e.g. by the bootlader)
  // when the driver discovers the display. DDI initialization will not be performed in this case.
  virtual bool InitWithDdiPllConfig(const DdiPllConfig& pll_config);
  // Initializes the display backlight for an already initialized display.
  void InitBacklight();
  // Resumes the ddi after suspend.
  bool Resume();
  // Loads ddi state from the hardware at driver startup.
  void LoadActiveMode();
  // Method to allow the display device to handle hotplug events. Returns
  // true if the device can handle the event without disconnecting. Otherwise
  // the device will be removed.
  virtual bool HandleHotplug(bool long_pulse) { return false; }

  display::DisplayId id() const { return id_; }
  DdiId ddi_id() const { return ddi_id_; }
  Controller* controller() { return controller_; }
  const std::optional<DdiReference>& ddi_reference() const { return ddi_reference_; }

  void set_pipe(Pipe* pipe) { pipe_ = pipe; }
  Pipe* pipe() const { return pipe_; }

  Type type() const { return type_; }
  void set_type(Type type) { type_ = type; }

  virtual bool HasBacklight() { return false; }
  virtual zx::result<> SetBacklightState(bool power, double brightness) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  virtual zx::result<fuchsia_hardware_backlight::wire::State> GetBacklightState() {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  virtual bool CheckPixelRate(int64_t pixel_rate_hz) = 0;

  virtual AddedDisplayInfo CreateAddedDisplayInfo() = 0;

  // Returns the display timing corresponding to the given `mode_id`.
  //
  // Returns nullopt if `mode_id` doesn't correspond to any timing parameters
  // supported by this display.
  //
  // `mode_id` must be valid.
  virtual std::optional<display::DisplayTiming> GetDisplayTiming(display::ModeId mode_id) const = 0;

 protected:
  // Attempts to initialize the ddi.
  virtual bool InitDdi() = 0;
  virtual bool InitBacklightHw() { return false; }

  // Configures the hardware to display content at the given resolution.
  virtual bool DdiModeset(const display::DisplayTiming& mode) = 0;

  // Returns an empty configuration if the desired pixel clock is unattainable.
  // Otherwise, the returned configuration is guaranteed to be valid.
  virtual DdiPllConfig ComputeDdiPllConfig(int32_t pixel_clock_khz) = 0;

  // Load the pixel rate from hardware if it's necessary when changing the
  // transcoder.
  //
  // The return value is in kHz.
  virtual int32_t LoadPixelRateForTranscoderKhz(TranscoderId transcoder_id) = 0;

  // Attaching a pipe to a display or configuring a pipe after display mode change has
  // 3 steps. The second step is generic pipe configuration, whereas PipeConfigPreamble
  // and PipeConfigEpilogue are responsible for display-type-specific configuration that
  // must be done before and after the generic configuration.
  virtual bool PipeConfigPreamble(const display::DisplayTiming& mode, PipeId pipe_id,
                                  TranscoderId transcoder_id) = 0;
  virtual bool PipeConfigEpilogue(const display::DisplayTiming& mode, PipeId pipe_id,
                                  TranscoderId transcoder_id) = 0;

  // Validates that a basic layer configuration can be supported for the
  // given modes of this display.
  bool CheckDisplayLimits(display::DisplayTiming display_timing,
                          std::span<const display::DriverLayer> layers);

  fdf::MmioBuffer* mmio_space() const;

 private:
  bool CheckNeedsModeset(const display::DisplayTiming& mode);

  Controller* const controller_;

  display::DisplayId id_;
  DdiId ddi_id_;

  Pipe* pipe_ = nullptr;

  std::optional<DdiReference> ddi_reference_;

  PowerWellRef ddi_power_;
  PowerWellRef ddi_io_power_;

  bool inited_ = false;
  display::DisplayTiming info_ = {};

  Type type_;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DISPLAY_DEVICE_H_
