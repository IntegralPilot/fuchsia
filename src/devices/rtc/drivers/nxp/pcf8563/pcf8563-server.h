// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_SERVER_H_
#define SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_SERVER_H_

#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>

namespace pcf8563 {

class RtcDriver;

class RtcServer : public fidl::Server<fuchsia_hardware_rtc::Device> {
 public:
  explicit RtcServer(RtcDriver* device) : device_(device) {}

  fuchsia_hardware_rtc::Service::InstanceHandler GetInstanceHandler();

  // fuchsia_hardware_rtc::Device protocol.
  void Get(GetCompleter::Sync& completer) override;
  void Set2(Set2Request& req, Set2Completer::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_rtc::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}  // No-op

  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<fuchsia_hardware_rtc::Device> server_end);

  fidl::ServerBindingGroup<fuchsia_hardware_rtc::Device>& bindings() { return bindings_; }

 private:
  RtcDriver* device_;  // Must outlive this class.
  fidl::ServerBindingGroup<fuchsia_hardware_rtc::Device> bindings_;
};

}  // namespace pcf8563

#endif  // SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_PCF8563_SERVER_H_
