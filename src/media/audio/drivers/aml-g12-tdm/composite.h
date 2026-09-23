// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_H_
#define SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_H_

#include <fidl/fuchsia.hardware.audio/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <memory>

#include "src/media/audio/drivers/aml-g12-tdm/composite-server.h"

namespace audio::aml_g12 {

class Driver : public fdf::DriverBase2 {
 public:
  static constexpr std::string_view kDriverName = "aml-g12-audio-composite";
  static constexpr std::string_view kClockGateParentName = "clock-gate";
  static constexpr std::string_view kClockPllParentName = "clock-pll";
  static constexpr std::string_view kGpioTdmASclkParentName = "gpio-tdm-a-sclk";
  static constexpr std::string_view kGpioTdmBSclkParentName = "gpio-tdm-b-sclk";
  static constexpr std::string_view kGpioTdmCSclkParentName = "gpio-tdm-c-sclk";

  Driver()
      : fdf::DriverBase2(kDriverName), devfs_connector_(fit::bind_member<&Driver::Serve>(this)) {}

  ~Driver() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  zx::result<> CreateDevfsNode();
  void Serve(fidl::ServerEnd<fuchsia_hardware_audio::Composite> server) {
    bindings_.AddBinding(dispatcher(), std::move(server), server_.get(),
                         fidl::kIgnoreBindingClosure);
  }

  std::optional<inspect::ComponentInspector> inspector_;
  std::unique_ptr<AudioCompositeServer> server_;
  fidl::ServerBindingGroup<fuchsia_hardware_audio::Composite> bindings_;
  fdf::OwnedChildNode child_;
  driver_devfs::Connector<fuchsia_hardware_audio::Composite> devfs_connector_;
};

}  // namespace audio::aml_g12

#endif  // SRC_MEDIA_AUDIO_DRIVERS_AML_G12_TDM_COMPOSITE_H_
