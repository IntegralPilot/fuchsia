// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_DEVICE_TESTING_FAKE_DEVICE_LISTENER_REGISTRY_H_
#define SRC_CAMERA_BIN_DEVICE_TESTING_FAKE_DEVICE_LISTENER_REGISTRY_H_

#include <fuchsia/ui/policy/cpp/fidl.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

namespace camera {

class FakeDeviceListenerRegistry final : public fuchsia::ui::policy::DeviceListenerRegistry {
 public:
  explicit FakeDeviceListenerRegistry(async_dispatcher_t* dispatcher);
  fidl::InterfaceRequestHandler<fuchsia::ui::policy::DeviceListenerRegistry> GetHandler();
  void SendMediaButtonsEvent(fuchsia::ui::input::MediaButtonsEvent event);

 private:
  void OnNewRequest(fidl::InterfaceRequest<fuchsia::ui::policy::DeviceListenerRegistry> request);

  // |fuchsia::ui::policy::DeviceListenerRegistry|
  void RegisterListener(
      fuchsia::ui::policy::MediaButtonsListenerHandle listener,
      fuchsia::ui::policy::DeviceListenerRegistry::RegisterListenerCallback callback) override;
  void RegisterMediaButtonsListener(
      ::fidl::InterfaceHandle<::fuchsia::ui::policy::MediaButtonsListener> listener) override {
    FX_LOGS(ERROR) << "OnMediaButtonsEvent not implemented";
#ifndef NDEBUG
    ZX_PANIC("Not Implemented: OnMediaButtonsEvent");
#endif
  }
  void RegisterTouchButtonsListener(
      fuchsia::ui::policy::TouchButtonsListenerHandle listener,
      fuchsia::ui::policy::DeviceListenerRegistry::RegisterTouchButtonsListenerCallback callback)
      override {
    FX_LOGS(ERROR) << "OnEvent not implemented";
#ifndef NDEBUG
    ZX_PANIC("Not Implemented: OnEvent");
#endif
  }
  async_dispatcher_t* dispatcher_;
  fidl::BindingSet<fuchsia::ui::policy::DeviceListenerRegistry> bindings_;
  std::map<uint32_t, fuchsia::ui::policy::MediaButtonsListenerPtr> listeners_;
  uint32_t listener_id_next_ = 1;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_DEVICE_TESTING_FAKE_DEVICE_LISTENER_REGISTRY_H_
