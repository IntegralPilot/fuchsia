// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_RING_BUFFER_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_RING_BUFFER_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.audio.device/cpp/type_conversions.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <memory>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/control_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"

namespace media_audio {

class RingBufferServer
    : public std::enable_shared_from_this<RingBufferServer>,
      public BaseFidlServer<RingBufferServer, fidl::Server, fuchsia_audio_device::RingBuffer> {
 public:
  static std::shared_ptr<RingBufferServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::RingBuffer> server_end,
      std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device, ElementId element_id);

  RingBufferServer(const RingBufferServer&) = delete;
  RingBufferServer(RingBufferServer&&) = delete;
  RingBufferServer& operator=(const RingBufferServer&) = delete;
  RingBufferServer& operator=(RingBufferServer&&) = delete;

  ~RingBufferServer() override;
  void DeviceDroppedRingBuffer();
  void ClientDroppedControl();

  // fuchsia.audio.device.RingBuffer implementation
  void SetActiveChannels(SetActiveChannelsRequest& request,
                         SetActiveChannelsCompleter::Sync& completer) override;
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopRequest& request, StopCompleter::Sync& completer) override;
  void WatchDelayInfo(WatchDelayInfoCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_audio_device::RingBuffer> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  // Forwarded from ControlNotify
  void DelayInfoIsChanged(const fuchsia_audio_device::DelayInfo& delay_info);

  void MaybeCompleteWatchDelayInfo();

  ElementId element_id() const { return element_id_; }

  const std::shared_ptr<FidlServerInspectInstance>& inspect() {
    return ring_buffer_inspect_instance_;
  }
  void SetInspect(std::shared_ptr<FidlServerInspectInstance> instance) {
    ring_buffer_inspect_instance_ = std::move(instance);
  }

  std::shared_ptr<ControlServer> parent() { return parent_; }

  // Static object count, for debugging purposes.
  static uint64_t count() { return count_; }

 protected:
  void OnShutdown(fidl::UnbindInfo info) override;

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "RingBufferServer";
  static inline uint64_t count_ = 0;

  RingBufferServer(std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device,
                   ElementId element_id);

  std::shared_ptr<ControlServer> parent_;
  std::shared_ptr<Device> device_;
  ElementId element_id_;

  std::optional<SetActiveChannelsCompleter::Async> active_channels_completer_;

  std::optional<StartCompleter::Async> start_completer_;
  std::optional<StopCompleter::Async> stop_completer_;
  bool started_ = false;

  std::optional<WatchDelayInfoCompleter::Async> delay_info_completer_;
  std::optional<fuchsia_audio_device::DelayInfo> new_delay_info_to_notify_;

  bool device_dropped_ring_buffer_ = false;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::shared_ptr<FidlServerInspectInstance> ring_buffer_inspect_instance_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_RING_BUFFER_SERVER_H_
