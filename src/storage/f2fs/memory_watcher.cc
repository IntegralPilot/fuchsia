// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/memory_watcher.h"

#include <fidl/fuchsia.memorypressure/cpp/markers.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/time.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/assert.h>

namespace f2fs {

static constexpr std::array kMemoryPressureStrings = {"unknown", "low", "medium", "high"};

MemoryPressureWatcher::MemoryPressureWatcher(async_dispatcher_t* dispatcher,
                                             MemoryPressureCallback callback)
    : dispatcher_(dispatcher), callback_(std::move(callback)) {
  if (auto ret = Start(); ret.is_ok()) {
    is_connected_ = true;
  }
}

std::string_view MemoryPressureWatcher::ToString(MemoryPressure level) {
  switch (level) {
    case MemoryPressure::kLow:
    case MemoryPressure::kMedium:
    case MemoryPressure::kHigh:
      return kMemoryPressureStrings[static_cast<int>(level)];
    default:
      return kMemoryPressureStrings[static_cast<int>(MemoryPressure::kUnknown)];
  }
}

void MemoryPressureWatcher::OnLevelChanged(OnLevelChangedRequest& request,
                                           OnLevelChangedCompleter::Sync& completer) {
  MemoryPressure level = MemoryPressure::kUnknown;
  switch (request.level()) {
    case fuchsia_memorypressure::Level::kWarning:
      level = MemoryPressure::kMedium;
      break;
    case fuchsia_memorypressure::Level::kCritical:
      level = MemoryPressure::kHigh;
      break;
    case fuchsia_memorypressure::Level::kNormal:
      level = MemoryPressure::kLow;
      break;
  }

  if (callback_) {
    callback_(level);
  }
  completer.Reply();
}

void MemoryPressureWatcher::on_fidl_error(fidl::UnbindInfo error) {
  memory_pressure_server_->Unbind();
  FX_LOGS(ERROR) << "Connection to fuchsia.memorypressure lost: " << error;
}

zx::result<> MemoryPressureWatcher::Start() {
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  auto dir = fidl::ClientEnd<fuchsia_io::Directory>(context->svc()->CloneChannel().TakeChannel());
  zx::result mempressure_client_end =
      component::ConnectAt<fuchsia_memorypressure::Provider>(dir.borrow());
  if (mempressure_client_end.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to the memorypressure Provider"
                   << mempressure_client_end.status_string();
    return mempressure_client_end.take_error();
  }

  fidl::Client memory_provider(std::move(*mempressure_client_end), dispatcher_);
  auto mempressure_endpoints = fidl::CreateEndpoints<fuchsia_memorypressure::Watcher>();
  memory_pressure_server_ =
      fidl::BindServer(dispatcher_, std::move(mempressure_endpoints->server), this);
  if (auto res = memory_provider->RegisterWatcher(std::move(mempressure_endpoints->client));
      res.is_error()) {
    FX_LOGS(INFO) << "Failed to register memory pressure watcher.";
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  return zx::ok();
}

}  // namespace f2fs
