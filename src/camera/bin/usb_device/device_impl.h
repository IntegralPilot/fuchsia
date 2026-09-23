// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CAMERA_BIN_USB_DEVICE_DEVICE_IMPL_H_
#define SRC_CAMERA_BIN_USB_DEVICE_DEVICE_IMPL_H_

#include <fuchsia/camera/cpp/fidl.h>
#include <fuchsia/camera3/cpp/fidl.h>
#include <fuchsia/sysmem/cpp/fidl.h>
#include <fuchsia/ui/policy/cpp/fidl.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/result.h>
#include <lib/fpromise/scope.h>
#include <lib/sys/cpp/component_context.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <list>
#include <map>
#include <memory>
#include <vector>

#include "src/camera/bin/usb_device/stream_impl.h"
#include "src/camera/lib/actor/actor_base.h"
#include "src/camera/lib/hanging_get_helper/hanging_get_helper.h"

namespace camera {

// Represents a physical camera device, and serves multiple clients of the camera3.Device protocol.
class DeviceImpl final : public actor::ActorBase, public fuchsia::ui::policy::MediaButtonsListener {
 public:
  template <typename RetT_, typename ErrT_ = void>
  using promise = fpromise::promise<RetT_, ErrT_>;

  // Creates a DeviceImpl using the given |controller|.
  //
  // References to |dispatcher|, |executor|, and |context| may be retained by the instance so the
  // caller must ensure these outlive the returned DeviceImpl.
  static promise<std::unique_ptr<DeviceImpl>, zx_status_t> Create(
      async_dispatcher_t* dispatcher, fuchsia::camera::ControlSyncPtr control,
      fuchsia::sysmem2::AllocatorPtr allocator, zx::event bad_state_event);

  DeviceImpl(async_dispatcher_t* dispatcher, fuchsia::camera::ControlSyncPtr control,
             fuchsia::sysmem2::AllocatorPtr allocator, zx::event bad_state_event);
  ~DeviceImpl() override;

  // Returns a service handler for use with a service directory.
  fidl::InterfaceRequestHandler<fuchsia::camera3::Device> GetHandler();

 private:
  // Called by the request handler returned by GetHandler, i.e. when a new client connects to the
  // published service.
  void OnNewRequest(fidl::InterfaceRequest<fuchsia::camera3::Device> request);

  // Posts a task to bind a new client.
  promise<void> Bind(fidl::InterfaceRequest<fuchsia::camera3::Device> request);

  // Posts a task to remove the client with the given id.
  promise<void> RemoveClient(uint64_t id);

  // Sets the current configuration to the provided index.
  promise<void> SetConfiguration(uint32_t index);

  // Connects to a stream.
  promise<void> ConnectToStream(uint32_t index,
                                fidl::InterfaceRequest<fuchsia::camera3::Stream> request);

  // Called by a stream when it has sufficient information to connect to the legacy stream protocol.
  promise<void> OnStreamRequested(uint32_t index,
                                  fuchsia::sysmem::BufferCollectionInfo buffer_collection_info,
                                  fuchsia::camera::FrameRate frame_rate,
                                  fidl::InterfaceRequest<fuchsia::camera::Stream> request,
                                  zx::eventpair driver_token);

  void AllocatorBindSharedCollection(
      fuchsia::sysmem2::BufferCollectionTokenHandle token,
      fidl::InterfaceRequest<fuchsia::sysmem2::BufferCollection> request);

  // |fuchsia::ui::policy::MediaButtonsListener|
  void OnEvent(fuchsia::ui::input::MediaButtonsEvent event,
               fuchsia::ui::policy::MediaButtonsListener::OnEventCallback callback) override;
  void OnMediaButtonsEvent(fuchsia::ui::input::MediaButtonsEvent event) override {
    FX_LOGS(ERROR) << "OnMediaButtonsEvent not implemented";
#ifndef NDEBUG
    ZX_PANIC("Not Implemented: OnMediaButtonsEvent");
#endif
  }
  // Represents a single client connection to the DeviceImpl class.
  class Client : public fuchsia::camera3::Device {
   public:
    Client(DeviceImpl& device, uint64_t id,
           fidl::InterfaceRequest<fuchsia::camera3::Device> request);
    ~Client() override;

    // Inform the client of a new configuration.
    void ConfigurationUpdated(uint32_t index);

   private:
    // Closes |binding_| with the provided |status| epitaph, and removes the client instance from
    // the parent |clients_| map.
    void CloseConnection(zx_status_t status);

    // Called when the client endpoint of |binding_| is closed.
    void OnClientDisconnected(zx_status_t status);

    // |fuchsia::camera3::Device|
    void GetIdentifier(GetIdentifierCallback callback) override;
    void GetConfigurations(GetConfigurationsCallback callback) override;
    void GetConfigurations2(GetConfigurations2Callback callback) override;
    void WatchCurrentConfiguration(WatchCurrentConfigurationCallback callback) override;
    void SetCurrentConfiguration(uint32_t index) override;
    void WatchMuteState(WatchMuteStateCallback callback) override;
    void SetSoftwareMuteState(bool muted, SetSoftwareMuteStateCallback callback) override;
    void ConnectToStream(uint32_t index,
                         fidl::InterfaceRequest<fuchsia::camera3::Stream> request) override;
    void Rebind(fidl::InterfaceRequest<fuchsia::camera3::Device> request) override;

    DeviceImpl& device_;
    std::string log_prefix_;
    uint64_t id_;
    fidl::Binding<fuchsia::camera3::Device> binding_;
    HangingGetHelper<uint32_t> configuration_;
  };

  // Keep a copy of the dispatcher just for initializing streams.
  async_dispatcher_t* stream_dispatcher_;

  fuchsia::camera::ControlSyncPtr control_;
  fuchsia::sysmem2::AllocatorPtr allocator_;
  zx::event bad_state_event_;

  fidl::Binding<fuchsia::ui::policy::MediaButtonsListener> button_listener_binding_;
  std::vector<fuchsia::camera3::Configuration2> configurations_;
  std::map<uint64_t, std::unique_ptr<Client>> clients_;
  uint64_t client_id_next_ = 1;
  uint32_t current_configuration_index_ = 0;

  std::vector<zx::eventpair> deallocation_events_;
  std::vector<promise<void, zx_status_t>> deallocation_promises_;
  std::vector<std::unique_ptr<StreamImpl>> streams_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
  friend class Client;
};

}  // namespace camera

#endif  // SRC_CAMERA_BIN_USB_DEVICE_DEVICE_IMPL_H_
