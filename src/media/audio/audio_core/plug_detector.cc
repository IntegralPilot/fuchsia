// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/audio_core/plug_detector.h"

#include <fidl/fuchsia.hardware.audio/cpp/fidl.h>
#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <zircon/compiler.h>

#include <memory>
#include <vector>

#include "src/media/audio/audio_core/reporter.h"

namespace media::audio {
namespace {

class PlugDetectorImpl : public PlugDetector {
 public:
  explicit PlugDetectorImpl(fidl::ClientEnd<fuchsia_io::Directory> svc_dir)
      : svc_dir_(std::move(svc_dir)) {}

  zx_status_t Start(Observer observer) final {
    TRACE_DURATION("audio", "PlugDetectorImpl::Start");
    // Start should only be called once.
    FX_DCHECK(!observer_);
    FX_DCHECK(observer);

    observer_ = std::move(observer);

    auto error_cleanup = fit::defer([this]() { Stop(); });

    // Start watching input service
    {
      input_watcher_ = std::make_unique<component::ServiceMemberWatcher<
          fuchsia_hardware_audio::StreamConfigConnectorInputService::StreamConfigConnector>>(
          svc_dir_.borrow());
      zx::result<> result = input_watcher_->Begin(
          async_get_default_dispatcher(),
          [this](fidl::ClientEnd<fuchsia_hardware_audio::StreamConfigConnector> client_end,
                 std::string instance) {
            AddAudioDevice(std::move(client_end), instance, /*is_input=*/true);
          });
      if (result.is_error()) {
        if (result.status_value() != ZX_ERR_NOT_FOUND) {
          FX_LOGS(ERROR) << "Failed to start input service watcher: " << result.status_string();
          return result.status_value();
        }
        input_watcher_.reset();
      }
    }

    // Start watching output service
    {
      output_watcher_ = std::make_unique<component::ServiceMemberWatcher<
          fuchsia_hardware_audio::StreamConfigConnectorOutputService::StreamConfigConnector>>(
          svc_dir_.borrow());
      zx::result<> result = output_watcher_->Begin(
          async_get_default_dispatcher(),
          [this](fidl::ClientEnd<fuchsia_hardware_audio::StreamConfigConnector> client_end,
                 std::string instance) {
            AddAudioDevice(std::move(client_end), instance, /*is_input=*/false);
          });
      if (result.is_error()) {
        if (result.status_value() != ZX_ERR_NOT_FOUND) {
          FX_LOGS(ERROR) << "Failed to start output service watcher: " << result.status_string();
          return result.status_value();
        }
        output_watcher_.reset();
      }
    }

    error_cleanup.cancel();
    return ZX_OK;
  }

  void Stop() final {
    TRACE_DURATION("audio", "PlugDetectorImpl::Stop");
    observer_ = nullptr;
    if (input_watcher_) {
      (void)input_watcher_->Cancel();
      input_watcher_.reset();
    }
    if (output_watcher_) {
      (void)output_watcher_->Cancel();
      output_watcher_.reset();
    }
  }

 private:
  void AddAudioDevice(
      fidl::ClientEnd<fuchsia_hardware_audio::StreamConfigConnector> connector_client_end,
      const std::string& name, bool is_input) {
    TRACE_DURATION("audio", "PlugDetectorImpl::AddAudioDevice");
    if (!observer_) {
      return;
    }
    fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig> stream_config_client;
    fidl::InterfaceRequest<fuchsia::hardware::audio::StreamConfig> stream_config_server =
        stream_config_client.NewRequest();

    auto result = fidl::Call(connector_client_end)
                      ->Connect(fidl::ServerEnd<fuchsia_hardware_audio::StreamConfig>(
                          stream_config_server.TakeChannel()));
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Failed to send Connect request: " << result.error_value();
      Reporter::Singleton().FailedToConnectToDevice(name, is_input, result.error_value().status());
      return;
    }

    observer_(name, is_input, std::move(stream_config_client));
  }

  fidl::ClientEnd<fuchsia_io::Directory> svc_dir_;
  Observer observer_;
  std::unique_ptr<component::ServiceMemberWatcher<
      fuchsia_hardware_audio::StreamConfigConnectorInputService::StreamConfigConnector>>
      input_watcher_;
  std::unique_ptr<component::ServiceMemberWatcher<
      fuchsia_hardware_audio::StreamConfigConnectorOutputService::StreamConfigConnector>>
      output_watcher_;
};

}  // namespace

std::unique_ptr<PlugDetector> PlugDetector::Create(fidl::ClientEnd<fuchsia_io::Directory> svc_dir) {
  if (!svc_dir.is_valid()) {
    auto client = component::OpenServiceRoot();
    if (client.is_error()) {
      FX_LOGS(ERROR) << "Failed to open /svc: " << client.status_string();
      return nullptr;
    }
    svc_dir = std::move(*client);
  }
  return std::make_unique<PlugDetectorImpl>(std::move(svc_dir));
}

}  // namespace media::audio
