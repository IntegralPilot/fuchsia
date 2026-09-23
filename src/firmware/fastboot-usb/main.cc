// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.fastboot/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fastboot/fastboot.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/syscalls.h>

namespace {

/// Helper that suppresses the same error message from being spammed.
class LogLimiter {
  static constexpr zx::duration kSuppressionWindow = zx::sec(5);

 public:
  LogLimiter() { Reset(); }
  ~LogLimiter() { Flush(); }

  void LogError(zx_status_t status, const char *message) {
    const zx::time now = zx::clock::get_monotonic();
    const zx::duration elapsed = now - last_log_time_;

    // Suppress adjacent runs of the same status/message within the suppression window.
    if (status == last_status_ && message == last_message_ && elapsed < kSuppressionWindow) {
      ++suppressed_;
      return;
    }

    Flush();
    FX_LOGS(ERROR) << message << ": " << zx_status_get_string(status);
    last_status_ = status;
    last_message_ = message;
    last_log_time_ = now;
  }

  void Flush() {
    if (suppressed_ > 0) {
      FX_LOGS(ERROR) << "Previous error repeated " << suppressed_ << " times: " << last_message_
                     << ": " << zx_status_get_string(last_status_);
    }
    Reset();
  }

 private:
  void Reset() {
    suppressed_ = 0;
    last_status_ = ZX_OK;
    last_message_ = nullptr;
    last_log_time_ = zx::time::infinite_past();
  }

  zx_status_t last_status_;
  const char *last_message_;
  zx::time last_log_time_;
  size_t suppressed_;
};

class UsbPacketTransport : public fastboot::Transport {
 public:
  UsbPacketTransport(fidl::WireSyncClient<fuchsia_hardware_fastboot::FastbootImpl> &device,
                     std::string_view packet)
      : device_(&device), packet_(packet) {}

  zx::result<size_t> ReceivePacket(void *dst, size_t capacity) override {
    if (capacity < PeekPacketSize()) {
      return zx::error(ZX_ERR_BUFFER_TOO_SMALL);
    }
    memcpy(dst, packet_.data(), packet_.size());
    return zx::ok(packet_.size());
  }

  size_t PeekPacketSize() override { return packet_.size(); }

  zx::result<> Send(std::string_view packet) override {
    fzl::OwnedVmoMapper mapper;
    zx_status_t status = mapper.CreateAndMap(packet.size(), "fastboot usb send");
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to create vmo mapper for sending " << zx_status_get_string(status);
      return zx::error(status);
    }
    memcpy(mapper.start(), packet.data(), packet.size());

    if (zx_status_t status = mapper.vmo().set_prop_content_size(packet.size()); status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to set content size " << zx_status_get_string(status);
      return zx::error(status);
    }

    auto res = (*device_)->Send(mapper.Release());
    if (res->is_error()) {
      return res->take_error();
    }
    return zx::ok();
  }

 private:
  fidl::WireSyncClient<fuchsia_hardware_fastboot::FastbootImpl> *device_ = nullptr;
  std::string_view packet_;
};
}  // namespace

int main(int argc, const char **argv) {
  FX_LOGS(INFO) << "Starting fastboot usb";
  auto connect_device =
      component::SyncServiceMemberWatcher<fuchsia_hardware_fastboot::Service::Fastboot>()
          .GetNextInstance(false);
  if (connect_device.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to usb fastboot device" << connect_device.status_string();
    return 1;
  }
  fidl::WireSyncClient<fuchsia_hardware_fastboot::FastbootImpl> device(
      std::move(connect_device.value()));

  fastboot::Fastboot fastboot;
  LogLimiter log_limiter;
  while (true) {
    // Note: fastboot.remaining_download_size() returns 0 in command stage.
    size_t request_size =
        std::min(fastboot.remaining_download_size(), static_cast<size_t>(512 * 1024));

    auto response = device->Receive(request_size);
    if (response.status() != ZX_OK) {
      log_limiter.LogError(response.status(), "Failed to receive packet (transport error)");
      continue;
    }
    if (response->is_error()) {
      log_limiter.LogError(response->error_value(), "Failed while receiving packet");
      continue;
    }
    log_limiter.Flush();

    const auto &packet = *response;
    fzl::VmoMapper mapper;
    zx_status_t status = mapper.Map(packet->data);
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to map packet vmo" << zx_status_get_string(status);
      continue;
    }

    size_t packet_size = 0;
    if (zx_status_t status = packet->data.get_prop_content_size(&packet_size); status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to get content size " << zx_status_get_string(status);
      continue;
    }

    std::string_view data{reinterpret_cast<const char *>(mapper.start()), packet_size};
    UsbPacketTransport transport(device, data);
    auto fastboot_res = fastboot.ProcessPacket(&transport);
    if (fastboot_res.is_error()) {
      FX_LOGS(ERROR) << "Failed to process fastboot packet " << fastboot_res.status_string();
    }
  }

  return 0;
}
