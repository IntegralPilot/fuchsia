// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_PACKET_STREAM_H_
#define SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_PACKET_STREAM_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/vmo.h>

#include <unordered_map>

namespace virtual_audio {

class VirtualAudioPacketStream : public fidl::Server<fuchsia_hardware_audio::PacketStreamControl>,
                                 public fidl::Server<fuchsia_hardware_audio::PacketStreamSink> {
 public:
  VirtualAudioPacketStream(
      bool is_outgoing, fuchsia_hardware_audio::Format2 format,
      const fuchsia_virtualaudio::PacketStream& config, async_dispatcher_t* dispatcher,
      fidl::ServerEnd<fuchsia_hardware_audio::PacketStreamControl> server,
      fit::callback<void(VirtualAudioPacketStream*, fidl::UnbindInfo)> on_close);
  virtual ~VirtualAudioPacketStream() = default;

 private:
  // fuchsia.hardware.audio.PacketStreamControl implementation
  void GetProperties(GetPropertiesCompleter::Sync& completer) override;
  void AllocateVmos(AllocateVmosRequest& request, AllocateVmosCompleter::Sync& completer) override;
  void DeallocateVmos(DeallocateVmosCompleter::Sync& completer) override;
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override;
  void UnregisterVmos(UnregisterVmosCompleter::Sync& completer) override;
  void GetPacketStreamSink(GetPacketStreamSinkCompleter::Sync& completer) override;
  void SetPacketStreamSink(SetPacketStreamSinkRequest& request,
                           SetPacketStreamSinkCompleter::Sync& completer) override;
  void Start(StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::PacketStreamControl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // fuchsia.hardware.audio.PacketStreamSink implementation
  void PutPacket(PutPacketRequest& request, PutPacketCompleter::Sync& completer) override;
  void FlushPackets(FlushPacketsCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_audio::PacketStreamSink> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  const bool is_outgoing_;
  fuchsia_virtualaudio::PacketStream config_;
  fuchsia_hardware_audio::Format2 format_;
  async_dispatcher_t* dispatcher_;
  fidl::ServerBinding<fuchsia_hardware_audio::PacketStreamControl> binding_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_audio::PacketStreamSink>> sink_binding_;

  bool is_started_ = false;
  std::unordered_map<uint64_t, fzl::VmoMapper> registered_vmos_;
};

}  // namespace virtual_audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_VIRTUAL_AUDIO_VIRTUAL_AUDIO_PACKET_STREAM_H_
