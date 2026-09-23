// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-device.h"

#include <lib/ddk/debug.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>

#include <memory>

#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-codec.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-dai.h"
#include "src/media/audio/drivers/virtual-audio-legacy/virtual-audio-stream.h"

namespace virtual_audio {

// static
fit::result<fuchsia_virtualaudio::Error, std::shared_ptr<VirtualAudioDevice>>
VirtualAudioDevice::Create(const fuchsia_virtualaudio::Configuration& cfg,
                           fidl::ServerEnd<fuchsia_virtualaudio::Device> server,
                           zx_device_t* dev_node, fit::closure on_shutdown) {
  if (!cfg.device_specific().has_value()) {
    return fit::error(fuchsia_virtualaudio::Error::kInternal);
  }
  std::optional<bool> is_input;
  switch (cfg.device_specific()->Which()) {
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kCodec:
      if (!cfg.device_specific()->codec().has_value()) {
        return fit::error(fuchsia_virtualaudio::Error::kInternal);
      }
      is_input = cfg.device_specific()->codec()->is_input();
      break;
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kDai:
      if (!cfg.device_specific()->dai().has_value()) {
        return fit::error(fuchsia_virtualaudio::Error::kInternal);
      }
      is_input = cfg.device_specific()->dai()->is_input();
      break;
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kStreamConfig:
      if (!cfg.device_specific()->stream_config().has_value()) {
        return fit::error(fuchsia_virtualaudio::Error::kInternal);
      }
      is_input = cfg.device_specific()->stream_config()->is_input();
      break;
    default:
      zxlogf(ERROR, "Device type creation not supported");
      return fit::error(fuchsia_virtualaudio::Error::kInternal);
  }
  auto device = std::make_shared<VirtualAudioDevice>(is_input, std::move(on_shutdown));

  // The `device` shared_ptr is held until the server is unbound (i.e. until the channel is closed).
  device->binding_ = fidl::BindServer(device->dispatcher_, std::move(server), device,
                                      &VirtualAudioDevice::OnFidlServerUnbound);

  switch (cfg.device_specific()->Which()) {
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kCodec:
      device->driver_ = std::make_shared<VirtualAudioCodec>(
          cfg, device, dev_node,
          fit::bind_member<&VirtualAudioDevice::OnDriverShutdown>(device.get()));
      break;
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kDai:
      device->driver_ = std::make_shared<VirtualAudioDai>(
          cfg, device, dev_node,
          fit::bind_member<&VirtualAudioDevice::OnDriverShutdown>(device.get()));
      break;
    case fuchsia_virtualaudio::DeviceSpecific::Tag::kStreamConfig:
      device->driver_ = std::make_shared<VirtualAudioStreamWrapper>(
          cfg, device, dev_node,
          fit::bind_member<&VirtualAudioDevice::OnDriverShutdown>(device.get()));
      break;
    default:
      zxlogf(ERROR, "Device type creation not supported");
      return fit::error(fuchsia_virtualaudio::Error::kInternal);
  }
  // Ensure the driver was created successfully.
  if (!device->driver_) {
    zxlogf(ERROR, "Device creation failed with unspecified internal error");
    return fit::error(fuchsia_virtualaudio::Error::kInternal);
  }

  return fit::ok(device);
}

void VirtualAudioDevice::OnFidlServerUnbound(
    VirtualAudioDevice* device, fidl::UnbindInfo unbind_info,
    fidl::ServerEnd<fuchsia_virtualaudio::Device> server_end) {
  if (!unbind_info.is_user_initiated() && !unbind_info.is_peer_closed()) {
    zxlogf(INFO, "virtualaudio.Device FIDL server unbound: %s",
           unbind_info.FormatDescription().c_str());
  }
  device->binding_.reset();
  if (device->driver_ != nullptr) {
    device->driver_->ShutdownAsync();
  } else {
    device->on_shutdown_();
  }
}

VirtualAudioDevice::VirtualAudioDevice(std::optional<bool> is_input, fit::closure on_shutdown)
    : is_input_(is_input), on_shutdown_(std::move(on_shutdown)) {}

VirtualAudioDevice::~VirtualAudioDevice() {
  // The driver should have been shutdown.
  ZX_ASSERT(driver_ == nullptr);

  // The FIDL server should have been unbound.
  ZX_ASSERT(!binding_.has_value());
}

// Post the given task with automatic cancellation if the device is cancelled before the task fires.
void VirtualAudioDevice::PostToDispatcher(fit::closure task_to_post) {
  async::PostTask(dispatcher_, [weak = weak_from_this(), task_to_post = std::move(task_to_post)]() {
    if (weak.lock()) {
      task_to_post();
    }
  });
}

void VirtualAudioDevice::ShutdownAsync() {
  // The FIDL server should be unbound before the driver is shutdown so that the FIDL server doesn't
  // receive requests to manipulate a driver that is shutdown.
  if (binding_.has_value()) {
    // The unbinding will trigger a callback to shutdown the driver.
    binding_->Unbind();
  } else if (driver_ != nullptr) {
    driver_->ShutdownAsync();
  } else {
    on_shutdown_();
  }
}

void VirtualAudioDevice::OnDriverShutdown() {
  PostToDispatcher([weak = weak_from_this()]() {
    auto self = weak.lock();
    if (!self) {
      return;
    }
    self->driver_.reset();
    if (self->binding_.has_value()) {
      zxlogf(WARNING, "Driver was shutdown before the FIDL server was unbound");
      self->binding_->Unbind();
    } else {
      self->on_shutdown_();
    }
  });
}

//
// virtualaudio::Device implementation
//

void VirtualAudioDevice::GetFormat(GetFormatCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver for this request", __func__, this);
    return;
  }

  driver_->GetFormatForVA([completer = completer.ToAsync()](auto result) mutable {
    if (!result.is_ok()) {
      completer.ReplyError(result.error_value());
      return;
    }
    auto& v = result.value();
    completer.ReplySuccess(v.frames_per_second, v.sample_format, v.num_channels,
                           v.external_delay.get());
  });
}

// Deliver SetFormat notification on binding's thread, if binding is valid.
void VirtualAudioDevice::NotifySetFormat(uint32_t frames_per_second, uint32_t sample_format,
                                         uint32_t num_channels, zx_duration_t external_delay) {
  PostToDispatcher(
      [weak = weak_from_this(), frames_per_second, sample_format, num_channels, external_delay]() {
        auto self = weak.lock();
        if (!self || !self->binding_.has_value()) {
          return;
        }
        fidl::Status status =
            fidl::WireSendEvent(self->binding_.value())
                ->OnSetFormat(frames_per_second, sample_format, num_channels, external_delay);
        if (status.status() != ZX_OK) {
          zxlogf(WARNING, "OnSetFormat failed with status %s", status.status_string());
        }
      });
}

void VirtualAudioDevice::GetGain(GetGainCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver for this request", __func__, this);
    return;
  }

  driver_->GetGainForVA([completer = completer.ToAsync()](auto result) mutable {
    if (!result.is_ok()) {
      completer.ReplyError(result.error_value());
      return;
    }
    auto& v = result.value();
    completer.ReplySuccess(v.mute, v.agc, v.gain_db);
  });
}

void VirtualAudioDevice::NotifySetGain(bool current_mute, bool current_agc, float current_gain_db) {
  PostToDispatcher([weak = weak_from_this(), current_mute, current_agc, current_gain_db]() {
    auto self = weak.lock();
    if (!self || !self->binding_.has_value()) {
      return;
    }
    fidl::Status status = fidl::WireSendEvent(self->binding_.value())
                              ->OnSetGain(current_mute, current_agc, current_gain_db);
    if (status.status() != ZX_OK) {
      zxlogf(WARNING, "OnSetGain failed with status %s", status.status_string());
    }
  });
}

void VirtualAudioDevice::GetBuffer(GetBufferCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver for this request", __func__, this);
    return;
  }

  driver_->GetBufferForVA([completer = completer.ToAsync()](auto result) mutable {
    if (!result.is_ok()) {
      completer.ReplyError(result.error_value());
      return;
    }
    auto& v = result.value();
    completer.ReplySuccess(std::move(v.vmo), v.num_frames, v.notifications_per_ring);
  });
}

void VirtualAudioDevice::NotifyBufferCreated(zx::vmo ring_buffer_vmo,
                                             uint32_t num_ring_buffer_frames,
                                             uint32_t notifications_per_ring) {
  PostToDispatcher([weak = weak_from_this(), ring_buffer_vmo = std::move(ring_buffer_vmo),
                    num_ring_buffer_frames, notifications_per_ring]() mutable {
    auto self = weak.lock();
    if (!self || !self->binding_.has_value()) {
      return;
    }
    fidl::Status status = fidl::WireSendEvent(self->binding_.value())
                              ->OnBufferCreated(std::move(ring_buffer_vmo), num_ring_buffer_frames,
                                                notifications_per_ring);
    if (status.status() != ZX_OK) {
      zxlogf(WARNING, "OnBufferCreated failed with status %s", status.status_string());
    }
  });
}

void VirtualAudioDevice::SetNotificationFrequency(
    SetNotificationFrequencyRequestView request,
    SetNotificationFrequencyCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver for this request", __func__, this);
    return;
  }

  driver_->SetNotificationFrequencyFromVA(request->notifications_per_ring,
                                          [completer = completer.ToAsync()](auto result) mutable {
                                            if (!result.is_ok()) {
                                              completer.ReplyError(result.error_value());
                                              return;
                                            }
                                            completer.ReplySuccess();
                                          });
}

void VirtualAudioDevice::NotifyStart(zx_time_t start_time) {
  PostToDispatcher([weak = weak_from_this(), start_time]() {
    auto self = weak.lock();
    if (!self || !self->binding_.has_value()) {
      return;
    }
    fidl::Status status = fidl::WireSendEvent(self->binding_.value())->OnStart(start_time);
    if (status.status() != ZX_OK) {
      zxlogf(WARNING, "OnStart failed with status %s", status.status_string());
    }
  });
}

void VirtualAudioDevice::NotifyStop(zx_time_t stop_time, uint32_t ring_buffer_position) {
  PostToDispatcher([weak = weak_from_this(), stop_time, ring_buffer_position]() {
    auto self = weak.lock();
    if (!self || !self->binding_.has_value()) {
      return;
    }
    fidl::Status status =
        fidl::WireSendEvent(self->binding_.value())->OnStop(stop_time, ring_buffer_position);
    if (status.status() != ZX_OK) {
      zxlogf(WARNING, "OnStop failed with status %s", status.status_string());
    }
  });
}

void VirtualAudioDevice::GetPosition(GetPositionCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver for this request", __func__, this);
    return;
  }

  driver_->GetPositionForVA([completer = completer.ToAsync()](auto result) mutable {
    if (!result.is_ok()) {
      completer.ReplyError(result.error_value());
      return;
    }
    auto& v = result.value();
    completer.ReplySuccess(v.monotonic_time.get(), v.ring_position);
  });
}

void VirtualAudioDevice::NotifyPosition(zx_time_t monotonic_time, uint32_t ring_buffer_position) {
  PostToDispatcher([weak = weak_from_this(), monotonic_time, ring_buffer_position]() {
    auto self = weak.lock();
    if (!self || !self->binding_.has_value()) {
      return;
    }
    fidl::Status status = fidl::WireSendEvent(self->binding_.value())
                              ->OnPositionNotify(monotonic_time, ring_buffer_position);
    if (status.status() != ZX_OK) {
      zxlogf(WARNING, "OnPositionNotify failed with status %s", status.status_string());
    }
  });
}

void VirtualAudioDevice::ChangePlugState(ChangePlugStateRequestView request,
                                         ChangePlugStateCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver; cannot change dynamic plug state", __func__, this);
    return;
  }

  driver_->ChangePlugStateFromVA(request->plugged,
                                 [completer = completer.ToAsync()](auto result) mutable {
                                   if (!result.is_ok()) {
                                     completer.ReplyError(result.error_value());
                                     return;
                                   }
                                   completer.ReplySuccess();
                                 });
}

void VirtualAudioDevice::AdjustClockRate(AdjustClockRateRequestView request,
                                         AdjustClockRateCompleter::Sync& completer) {
  if (!driver_) {
    zxlogf(WARNING, "%s: %p has no driver; cannot change clock rate", __func__, this);
    return;
  }

  driver_->AdjustClockRateFromVA(request->ppm_from_monotonic,
                                 [completer = completer.ToAsync()](auto result) mutable {
                                   if (!result.is_ok()) {
                                     completer.ReplyError(result.error_value());
                                     return;
                                   }
                                   completer.ReplySuccess();
                                 });
}

}  // namespace virtual_audio
