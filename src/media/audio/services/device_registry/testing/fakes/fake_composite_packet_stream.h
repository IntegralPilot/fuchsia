// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_PACKET_STREAM_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_PACKET_STREAM_H_

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/test_base.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fzl/vmo-mapper.h>

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string_view>

#include "src/media/audio/services/device_registry/basic_types.h"

namespace media_audio {

class FakeComposite;

class FakeCompositePacketStream final
    : public fidl::testing::TestBase<fuchsia_hardware_audio::PacketStreamControl> {
  static constexpr std::string_view kClassName = "FakeCompositePacketStream";

 public:
  static constexpr std::optional<bool> kDefaultNeedsCacheFlushInvalidate = false;

  FakeCompositePacketStream(FakeComposite* parent, ElementId element_id,
                            fuchsia_hardware_audio::Format2 format,
                            fuchsia_hardware_audio::BufferType supported_buffer_types);
  ~FakeCompositePacketStream() override;

  static void on_ps_unbind(FakeCompositePacketStream* fake_packet_stream, fidl::UnbindInfo info,
                           fidl::ServerEnd<fuchsia_hardware_audio::PacketStreamControl> server_end);

  // fuchsia_hardware_audio::PacketStreamControl implementation
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

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override;

  // Accessors
  ElementId element_id() const { return element_id_; }

  // To be used during run-time
  bool started() const { return started_; }
  zx::time mono_start_time() const { return mono_start_time_; }
  std::optional<zx_rights_t> vmo_rights(uint64_t vmo_id) const {
    if (auto it = vmos_.find(vmo_id); it != vmos_.end()) {
      zx_info_handle_basic_t info;
      if (it->second.vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr) ==
          ZX_OK) {
        return info.rights;
      }
    }
    return std::nullopt;
  }

  static uint64_t count() { return count_; }
  FakeComposite* parent() { return parent_; }

  void set_responsive(std::optional<bool> responsive) { responsive_ = responsive; }

  bool responsive();

 private:
  static inline uint64_t count_ = 0;

  // ctor
  FakeComposite* parent_;
  ElementId element_id_;
  fuchsia_hardware_audio::Format2 format_;
  fuchsia_hardware_audio::BufferType supported_buffer_types_;

  // Note: Completers below are all vectors because we need to be able to
  // save >1 completers so that we can simulate unresponsiveness.

  // Allocate / Deallocate / Register / Unregister
  std::vector<AllocateVmosCompleter::Async> allocate_vmos_completers_;
  std::vector<DeallocateVmosCompleter::Async> deallocate_vmos_completers_;
  std::vector<RegisterVmosCompleter::Async> register_vmos_completers_;
  std::vector<UnregisterVmosCompleter::Async> unregister_vmos_completers_;

  // GetProperties
  std::vector<GetPropertiesCompleter::Async> get_properties_completers_;
  std::optional<bool> needs_cache_flush_or_invalidate_ = kDefaultNeedsCacheFlushInvalidate;

  // Sinks
  std::vector<GetPacketStreamSinkCompleter::Async> get_packet_stream_sink_completers_;
  std::vector<SetPacketStreamSinkCompleter::Async> set_packet_stream_sink_completers_;

  // Start / Stop
  std::vector<StartCompleter::Async> start_completers_;
  std::vector<StopCompleter::Async> stop_completers_;
  bool started_ = false;
  zx::time mono_start_time_;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  struct VmoRecord {
    zx::vmo vmo;
    fzl::VmoMapper mapper;
  };
  std::unordered_map<uint64_t, VmoRecord> vmos_;
  bool buffers_configured_ = false;
  std::optional<bool> responsive_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_COMPOSITE_PACKET_STREAM_H_
