// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RTC_DRIVERS_PL031_RTC_PL031_RTC_H_
#define SRC_DEVICES_RTC_DRIVERS_PL031_RTC_PL031_RTC_H_

#include <fidl/fuchsia.hardware.rtc/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/mmio/cpp/mmio.h>

namespace rtc {

namespace FidlRtc = fuchsia_hardware_rtc;

struct Pl031Regs {
  uint32_t dr;
  uint32_t mr;
  uint32_t lr;
  uint32_t cr;
  uint32_t msc;
  uint32_t ris;
  uint32_t mis;
  uint32_t icr;
};

class Pl031 : public fdf::DriverBase2, public fidl::WireServer<FidlRtc::Device> {
 public:
  Pl031() : fdf::DriverBase2("pl031-rtc") {}

  zx::result<> Start(fdf::DriverContext context) override;

  // fidl::WireServer<FidlRtc::Device> implementation.
  void Get(GetCompleter::Sync& completer) override;
  void Set2(Set2RequestView request, Set2Completer::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<FidlRtc::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}  // No-op

 private:
  zx_status_t SetRtc(FidlRtc::wire::Time rtc);

  std::optional<fdf::MmioBuffer> mmio_;
  MMIO_PTR Pl031Regs* regs_ = nullptr;
  fidl::ServerBindingGroup<FidlRtc::Device> bindings_;
};

}  // namespace rtc

#endif  // SRC_DEVICES_RTC_DRIVERS_PL031_RTC_PL031_RTC_H_
