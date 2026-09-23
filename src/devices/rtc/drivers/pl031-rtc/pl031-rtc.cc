// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/rtc/drivers/pl031-rtc/pl031-rtc.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include "src/devices/rtc/lib/rtc/include/librtc_llcpp.h"

namespace rtc {

zx::result<> Pl031::Start(fdf::DriverContext context) {
  zx::result pdev_client =
      context.incoming().Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client.status_string());
    return pdev_client.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client.value())};

  // Carve out some address space for this device.
  zx::result mmio = pdev.MapMmio(0);
  if (mmio.is_error()) {
    fdf::error("Failed to map mmio: {}", mmio.status_string());
    return mmio.take_error();
  }
  mmio_ = std::move(mmio.value());
  regs_ = reinterpret_cast<MMIO_PTR Pl031Regs*>(mmio_->get());

  // Retrieve and sanitize the RTC value. Set the RTC to the value.
  FidlRtc::wire::Time rtc = SecondsToRtc(MmioRead32(&regs_->dr));
  rtc = SanitizeRtc(rtc);
  zx_status_t status = SetRtc(rtc);
  if (status != ZX_OK) {
    fdf::error("Failed to set rtc: {}", zx_status_get_string(status));
  }

  // Serve FIDL service.
  FidlRtc::Service::InstanceHandler handler({
      .device = bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });

  zx::result service_result = outgoing()->AddService<FidlRtc::Service>(std::move(handler));
  if (service_result.is_error()) {
    fdf::error("Failed to add RTC service: {}", service_result.status_string());
    return service_result.take_error();
  }

  return zx::ok();
}

void Pl031::Get(GetCompleter::Sync& completer) {
  FidlRtc::wire::Time rtc = SecondsToRtc(MmioRead32(&regs_->dr));
  // TODO(https://fxbug.dev/42074113): Reply with error if RTC time is known to be invalid.
  completer.ReplySuccess(rtc);
}

void Pl031::Set2(Set2RequestView request, Set2Completer::Sync& completer) {
  zx_status_t status{SetRtc(request->rtc)};
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess();
  }
}

zx_status_t Pl031::SetRtc(FidlRtc::wire::Time rtc) {
  if (!IsRtcValid(rtc)) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  MmioWrite32(static_cast<uint32_t>(SecondsSinceEpoch(rtc)), &regs_->lr);

  return ZX_OK;
}

}  // namespace rtc

FUCHSIA_DRIVER_EXPORT2(rtc::Pl031);
