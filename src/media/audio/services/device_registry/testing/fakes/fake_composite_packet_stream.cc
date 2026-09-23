// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/testing/fakes/fake_composite_packet_stream.h"

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.audio/cpp/test_base.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/result.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <cstddef>
#include <optional>

#include "src/media/audio/services/device_registry/basic_types.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/testing/fakes/fake_composite.h"
#include "src/media/audio/services/device_registry/testing/fakes/logging.h"

namespace media_audio {

namespace fha = fuchsia_hardware_audio;

FakeCompositePacketStream::FakeCompositePacketStream(FakeComposite* parent, ElementId element_id,
                                                     fha::Format2 format,
                                                     fha::BufferType supported_buffer_types)
    : TestBase(),
      parent_(parent),
      element_id_(element_id),
      format_(std::move(format)),
      supported_buffer_types_(supported_buffer_types) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);

  ++count_;
  ADR_LOG_METHOD(kLogFakeCompositePacketStream) << "There are now " << count_ << " instances";
}

FakeCompositePacketStream::~FakeCompositePacketStream() {
  --count_;
  ADR_LOG_METHOD(kLogFakeCompositePacketStream) << "There are now " << count_ << " instances";
}

bool FakeCompositePacketStream::responsive() {
  return responsive_.value_or(parent()->responsive());
}

void FakeCompositePacketStream::AllocateVmos(AllocateVmosRequest& request,
                                             AllocateVmosCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    allocate_vmos_completers_.emplace_back(completer.ToAsync());
    return;
  }
  if (buffers_configured_ || started_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!request.vmo_count() || *request.vmo_count() == 0 || !request.min_vmo_size() ||
      *request.min_vmo_size() == 0) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  if (auto match = parent_->inject_packet_stream_allocate_vmos_error_.find(element_id_);
      match != parent_->inject_packet_stream_allocate_vmos_error_.end()) {
    completer.Reply(fit::error(match->second));
    return;
  }

  std::vector<fha::VmoInfo> out_vmos;
  for (uint32_t i = 0; i < *request.vmo_count(); ++i) {
    VmoRecord record;
    auto status = record.mapper.CreateAndMap(
        *request.min_vmo_size(), ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, nullptr, &record.vmo);
    if (status != ZX_OK) {
      completer.Reply(fit::error(status));
      return;
    }
    zx::vmo out_vmo;
    record.vmo.duplicate(ZX_RIGHT_READ | ZX_RIGHT_WRITE | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER,
                         &out_vmo);
    out_vmos.push_back(fha::VmoInfo{{
        .id = i,
        .vmo = std::move(out_vmo),
    }});
    vmos_[i] = std::move(record);
  }
  buffers_configured_ = true;
  completer.Reply(fit::success(std::move(out_vmos)));
}

void FakeCompositePacketStream::DeallocateVmos(DeallocateVmosCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    deallocate_vmos_completers_.emplace_back(completer.ToAsync());
    return;
  }
  if (started_ || !buffers_configured_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }
  vmos_.clear();
  buffers_configured_ = false;
  completer.Reply(fit::success());
}

void FakeCompositePacketStream::RegisterVmos(RegisterVmosRequest& request,
                                             RegisterVmosCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    register_vmos_completers_.emplace_back(completer.ToAsync());
    return;
  }
  if (buffers_configured_ || started_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }
  if (!request.vmo_infos() || request.vmo_infos()->empty()) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  if (auto match = parent_->inject_packet_stream_register_vmos_error_.find(element_id_);
      match != parent_->inject_packet_stream_register_vmos_error_.end()) {
    completer.Reply(fit::error(match->second));
    return;
  }

  for (auto& vmo_info : *request.vmo_infos()) {
    if (!vmo_info.id() || !vmo_info.vmo()) {
      completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    if (vmos_.contains(*vmo_info.id())) {
      completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    VmoRecord record;
    zx_info_handle_basic_t info;
    auto status =
        vmo_info.vmo()->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    if (status != ZX_OK) {
      completer.Reply(fit::error(status));
      return;
    }

    // Registering VMOs requires READ, MAP and TRANSFER.
    constexpr zx_rights_t kRequiredRights = ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER;
    if ((info.rights & kRequiredRights) != kRequiredRights) {
      completer.Reply(fit::error(ZX_ERR_ACCESS_DENIED));
      return;
    }

    size_t size;
    vmo_info.vmo()->get_size(&size);
    if (size == 0) {
      completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
      return;
    }
    zx_vm_option_t perms =
        ZX_VM_PERM_READ | ((info.rights & ZX_RIGHT_WRITE) ? ZX_VM_PERM_WRITE : 0);
    status = record.mapper.Map(*vmo_info.vmo(), 0, size, perms);
    if (status != ZX_OK) {
      completer.Reply(fit::error(status));
      return;
    }
    // Note: record.vmo is the zx::vmo member.
    // The natural bindings vmo_info.vmo() returns std::optional<zx::vmo>&.
    // We want to move it.
    record.vmo = std::move(*vmo_info.vmo());
    vmos_[*vmo_info.id()] = std::move(record);
  }
  buffers_configured_ = true;
  completer.Reply(fit::success());
}

void FakeCompositePacketStream::UnregisterVmos(UnregisterVmosCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    unregister_vmos_completers_.emplace_back(completer.ToAsync());
    return;
  }
  if (started_ || !buffers_configured_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }
  vmos_.clear();
  buffers_configured_ = false;
  completer.Reply(fit::success());
}

void FakeCompositePacketStream::GetPacketStreamSink(GetPacketStreamSinkCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    get_packet_stream_sink_completers_.emplace_back(completer.ToAsync());
    return;
  }

  auto [client_end, server_end] = fidl::Endpoints<fha::PacketStreamSink>::Create();
  // We don't currently bind the server_end to anything, but we keep the protocol open.
  completer.Reply(fit::success(fha::PacketStreamControlGetPacketStreamSinkResponse{{
      .stream = std::move(client_end),
  }}));
}

void FakeCompositePacketStream::SetPacketStreamSink(SetPacketStreamSinkRequest& request,
                                                    SetPacketStreamSinkCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);
  if (!responsive()) {
    set_packet_stream_sink_completers_.emplace_back(completer.ToAsync());
    return;
  }
  if (!request.stream()) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  NotImplemented_("SetPacketStreamSink", completer);
}

void FakeCompositePacketStream::NotImplemented_(const std::string& name,
                                                ::fidl::CompleterBase& completer) {
  ADR_WARN_METHOD() << name;
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void FakeCompositePacketStream::GetProperties(GetPropertiesCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);

  // If we've been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    get_properties_completers_.emplace_back(completer.ToAsync());
    return;
  }

  fha::PacketStreamProperties props;
  props.needs_cache_flush_or_invalidate(needs_cache_flush_or_invalidate_);
  props.supported_buffer_types(supported_buffer_types_);
  completer.Reply({{.properties = std::move(props)}});
}

void FakeCompositePacketStream::Start(StartCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);

  // If we’ve been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    start_completers_.emplace_back(completer.ToAsync());
    return;
  }

  if (!buffers_configured_ && !(supported_buffer_types_ & fha::BufferType::kInline)) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }

  if (started_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }

  started_ = true;
  mono_start_time_ = zx::clock::get_monotonic();
  completer.Reply(zx::ok());
}

void FakeCompositePacketStream::Stop(StopCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogFakeCompositePacketStream);

  // If we’ve been instructed to be unresponsive, pend the completer - indefinitely.
  if (!responsive()) {
    stop_completers_.emplace_back(completer.ToAsync());
    return;
  }

  if (!started_) {
    completer.Reply(fit::error(ZX_ERR_BAD_STATE));
    return;
  }

  started_ = false;
  completer.Reply(zx::ok());
}

void FakeCompositePacketStream::handle_unknown_method(
    fidl::UnknownMethodMetadata<fha::PacketStreamControl> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "FakeCompositePacketStream: unknown method (PacketStreamControl) ordinal "
                    << metadata.method_ordinal;
  if (!responsive()) {
    unknown_method_completers_.emplace_back(completer.ToAsync());
    return;
  }
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace media_audio
