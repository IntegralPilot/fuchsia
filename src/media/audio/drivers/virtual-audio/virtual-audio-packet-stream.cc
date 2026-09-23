// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio/virtual-audio-packet-stream.h"

#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/status.h>

namespace virtual_audio {

VirtualAudioPacketStream::VirtualAudioPacketStream(
    bool is_outgoing, fuchsia_hardware_audio::Format2 format,
    const fuchsia_virtualaudio::PacketStream& config, async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_hardware_audio::PacketStreamControl> server,
    fit::callback<void(VirtualAudioPacketStream*, fidl::UnbindInfo)> on_close)
    : is_outgoing_(is_outgoing),
      config_(config),
      format_(std::move(format)),
      dispatcher_(dispatcher),
      binding_(dispatcher, std::move(server), this,
               [this, on_close = std::move(on_close)](fidl::UnbindInfo info) mutable {
                 if (on_close) {
                   on_close(this, info);
                 }
               }) {}

void VirtualAudioPacketStream::GetProperties(GetPropertiesCompleter::Sync& completer) {
  fuchsia_hardware_audio::PacketStreamProperties properties;
  properties.needs_cache_flush_or_invalidate(config_.needs_cache_flush_or_invalidate());
  properties.supported_buffer_types(config_.supported_buffer_types());

  completer.Reply(std::move(properties));
}

void VirtualAudioPacketStream::AllocateVmos(AllocateVmosRequest& request,
                                            AllocateVmosCompleter::Sync& completer) {
  if (is_started_) {
    fdf::error("AllocateVmos: called while started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!registered_vmos_.empty()) {
    fdf::error("AllocateVmos: called while VMOs already registered");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  if (request.min_vmo_size().value_or(0) == 0 || request.vmo_count().value_or(0) == 0) {
    fdf::error("AllocateVmos: invalid args, min_vmo_size={}, vmo_count={}",
               request.min_vmo_size().value_or(0), request.vmo_count().value_or(0));
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  std::vector<fuchsia_hardware_audio::VmoInfo> vmo_infos;
  for (uint32_t i = 0; i < *request.vmo_count(); ++i) {
    zx::vmo vmo;
    zx_status_t status = zx::vmo::create(*request.min_vmo_size(), 0, &vmo);
    if (status != ZX_OK) {
      fdf::error("AllocateVmos: failed to create VMO: {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }

    // Map the VMO.
    // If input (Capture), driver writes. If output (Playback), driver reads.
    zx_vm_option_t map_perms = ZX_VM_PERM_READ;
    if (!is_outgoing_) {
      map_perms |= ZX_VM_PERM_WRITE;
    }

    fzl::VmoMapper mapper;
    status = mapper.Map(vmo, 0, 0, map_perms);
    if (status != ZX_OK) {
      fdf::error("AllocateVmos: failed to map VMO: {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }

    // ID assignment. We can just use the index for now.
    // Client doesn't assign IDs for AllocateVmos; driver does.
    uint64_t id = i;

    // Duplicate handle for client.
    // If input (Capture), client reads. If output (Playback), client writes.
    zx::vmo client_vmo;
    zx_rights_t rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_MAP | ZX_RIGHT_READ;
    if (is_outgoing_) {
      rights |= ZX_RIGHT_WRITE;
    }

    status = vmo.duplicate(rights, &client_vmo);
    if (status != ZX_OK) {
      fdf::error("AllocateVmos: failed to duplicate VMO: {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }

    registered_vmos_.emplace(id, std::move(mapper));

    fuchsia_hardware_audio::VmoInfo info;
    info.id(id);
    info.vmo(std::move(client_vmo));
    vmo_infos.push_back(std::move(info));
  }

  completer.Reply(zx::ok(std::move(vmo_infos)));
}

void VirtualAudioPacketStream::DeallocateVmos(DeallocateVmosCompleter::Sync& completer) {
  if (is_started_) {
    fdf::error("DeallocateVmos: called while started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (registered_vmos_.empty()) {
    fdf::error("DeallocateVmos: called with no VMOs registered");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  registered_vmos_.clear();
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::RegisterVmos(RegisterVmosRequest& request,
                                            RegisterVmosCompleter::Sync& completer) {
  if (is_started_) {
    fdf::error("RegisterVmos: called while started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!registered_vmos_.empty()) {
    fdf::error("RegisterVmos: called while VMOs already registered");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }

  if (!request.vmo_infos().has_value()) {
    fdf::error("RegisterVmos: missing vmo_infos");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  for (auto& info : *request.vmo_infos()) {
    if (!info.id().has_value() || !info.vmo().has_value()) {
      fdf::error("RegisterVmos: missing id or vmo");
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    if (registered_vmos_.contains(*info.id())) {
      fdf::error("RegisterVmos: VMO ID {} already registered", *info.id());
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    fzl::VmoMapper mapper;
    // Map the VMO.
    size_t vmo_size = 0;
    info.vmo()->get_size(&vmo_size);  // Get actual size

    // If input (Capture), driver writes. If output (Playback), driver reads.
    zx_vm_option_t map_perms = ZX_VM_PERM_READ;
    if (!is_outgoing_) {
      map_perms |= ZX_VM_PERM_WRITE;
    }

    zx_status_t status = mapper.Map(*info.vmo(), 0, vmo_size, map_perms);
    if (status != ZX_OK) {
      fdf::error("RegisterVmos: failed to map VMO: {}", zx_status_get_string(status));
      completer.Reply(zx::error(status));
      return;
    }
    registered_vmos_.emplace(*info.id(), std::move(mapper));
  }
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::UnregisterVmos(UnregisterVmosCompleter::Sync& completer) {
  if (is_started_) {
    fdf::error("UnregisterVmos: called while started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (registered_vmos_.empty()) {
    fdf::error("UnregisterVmos: called with no VMOs registered");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  registered_vmos_.clear();
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::GetPacketStreamSink(GetPacketStreamSinkCompleter::Sync& completer) {
  if (sink_binding_.has_value()) {
    sink_binding_->Close(ZX_ERR_CANCELED);
  }

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_audio::PacketStreamSink>();
  if (endpoints.is_error()) {
    fdf::error("GetPacketStreamSink: failed to create endpoints: {}", endpoints.status_string());
    completer.Reply(zx::error(endpoints.status_value()));
    return;
  }

  sink_binding_.emplace(dispatcher_, std::move(endpoints->server), this,
                        fidl::kIgnoreBindingClosure);

  fuchsia_hardware_audio::PacketStreamControlGetPacketStreamSinkResponse response;
  response.stream(std::move(endpoints->client));
  completer.Reply(zx::ok(std::move(response)));
}

void VirtualAudioPacketStream::SetPacketStreamSink(SetPacketStreamSinkRequest& request,
                                                   SetPacketStreamSinkCompleter::Sync& completer) {
  fdf::warn("SetPacketStreamSink: not supported");
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
}

void VirtualAudioPacketStream::Start(StartCompleter::Sync& completer) {
  if (is_started_) {
    fdf::error("Start: already started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (registered_vmos_.empty()) {
    fdf::error("Start: called with no VMOs registered");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  is_started_ = true;
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::Stop(StopCompleter::Sync& completer) {
  if (!is_started_) {
    fdf::error("Stop: not started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  is_started_ = false;
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::PutPacket(PutPacketRequest& request,
                                         PutPacketCompleter::Sync& completer) {
  if (!is_started_) {
    fdf::error("PutPacket: not started");
    completer.Reply(zx::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!request.payload().has_value()) {
    fdf::error("PutPacket: missing payload");
    completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
    return;
  }

  // TODO(https://fxbug.dev/INSERT_BUG_ID_HERE): Implement back pressure.
  // We currently complete PutPacket immediately ("infinitely fast" consumption).
  // We should simulate rate-of-consumption (e.g. based on bitrate or fixed 1 byte/10us)
  // and delay completion until the data is "consumed".
  if (request.payload()->Which() == fuchsia_hardware_audio::DataTransfer::Tag::kVmoTransfer) {
    auto& transfer = request.payload()->vmo_transfer().value();
    if (!transfer.vmo_id().has_value() || !transfer.payload_size().has_value()) {
      fdf::error("PutPacket: missing vmo_id or payload_size");
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    if (!registered_vmos_.contains(*transfer.vmo_id())) {
      fdf::error("PutPacket: VMO ID {} not registered", *transfer.vmo_id());
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }

    uint64_t offset = transfer.vmo_offset().value_or(0);
    uint64_t size = *transfer.payload_size();
    auto& mapper = registered_vmos_.at(*transfer.vmo_id());

    // size+offset must <= mapper.size(). Make this comparison in a way that cannot overflow.
    if (offset > mapper.size() || size > mapper.size() - offset) {
      fdf::error("PutPacket: Out of bounds. offset={}, size={}, map_size={}", offset, size,
                 mapper.size());
      completer.Reply(zx::error(ZX_ERR_INVALID_ARGS));
      return;
    }
  }

  fuchsia_hardware_audio::PacketStreamSinkPutPacketResponse response;
  completer.Reply(zx::ok(std::move(response)));
}

void VirtualAudioPacketStream::FlushPackets(FlushPacketsCompleter::Sync& completer) {
  // No buffering, so nothing to flush.
  completer.Reply(zx::ok());
}

void VirtualAudioPacketStream::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::PacketStreamControl> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("PacketStreamControl: unknown method ordinal {}", metadata.method_ordinal);
}

void VirtualAudioPacketStream::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_audio::PacketStreamSink> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::warn("PacketStreamSink: unknown method ordinal {}", metadata.method_ordinal);
}

}  // namespace virtual_audio
