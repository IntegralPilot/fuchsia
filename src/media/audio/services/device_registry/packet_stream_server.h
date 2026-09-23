// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PACKET_STREAM_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PACKET_STREAM_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio.device/cpp/type_conversions.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <memory>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/control_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"

namespace media_audio {

class PacketStreamServer
    : public std::enable_shared_from_this<PacketStreamServer>,
      public BaseFidlServer<PacketStreamServer, fidl::Server, fuchsia_audio_device::PacketStream> {
 public:
  static std::shared_ptr<PacketStreamServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::PacketStream> server_end,
      std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device, ElementId element_id);

  ~PacketStreamServer() override;
  void OnShutdown(fidl::UnbindInfo info) override;
  void DeviceDroppedPacketStream();
  void ClientDroppedControl();

  // fuchsia.audio.device.PacketStream implementation
  void SetBuffers(SetBuffersRequest& request, SetBuffersCompleter::Sync& completer) override;

  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopRequest& request, StopCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_audio_device::PacketStream> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  ElementId element_id() const { return element_id_; }

  const std::shared_ptr<FidlServerInspectInstance>& inspect() {
    return packet_stream_inspect_instance_;
  }
  void SetInspect(std::shared_ptr<FidlServerInspectInstance> instance) {
    packet_stream_inspect_instance_ = std::move(instance);
  }

  std::shared_ptr<ControlServer> parent() { return parent_; }

  // Static object count, for debugging purposes.
  static uint64_t count() { return count_; }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "PacketStreamServer";
  static inline uint64_t count_ = 0;

  PacketStreamServer(std::shared_ptr<ControlServer> parent, std::shared_ptr<Device> device,
                     ElementId element_id);

  std::shared_ptr<ControlServer> parent_;
  std::shared_ptr<Device> device_;
  ElementId element_id_;

  std::optional<StartCompleter::Async> start_completer_;
  std::optional<StopCompleter::Async> stop_completer_;
  bool started_ = false;

  bool device_dropped_packet_stream_ = false;
  bool buffers_are_set_ = false;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::shared_ptr<FidlServerInspectInstance> packet_stream_inspect_instance_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PACKET_STREAM_SERVER_H_
