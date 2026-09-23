// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/device_id_provider.h"

#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/syslog/cpp/macros.h>

#include <optional>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/uuid/uuid.h"

namespace forensics::feedback {
namespace {

// Reads a device id from the file at |path|. If the device id doesn't exist or is invalid, return
// a nullopt.
std::optional<std::string> ReadDeviceId(const std::string& path) {
  std::string id;
  if (!files::ReadFileToString(path, &id)) {
    return std::nullopt;
  }

  return id;
}

// Creates a new device id and stores it at |path|.
//
// The id is a 128-bit (pseudo) random UUID in the form of version 4 as described in RFC 4122,
// section 4.4.
std::string InitializeDeviceId(const std::string& path) {
  if (const std::optional<std::string> read_id = ReadDeviceId(path);
      read_id.has_value() && uuid::IsValid(read_id.value())) {
    return read_id.value();
  }

  std::string new_id = uuid::Generate();
  if (!files::WriteFile(path, new_id.c_str(), new_id.size())) {
    FX_LOGS(ERROR) << fxl::StringPrintf("Cannot write device id '%s' to '%s'", new_id.c_str(),
                                        path.c_str());
  }

  FX_LOGS(INFO) << "Created new feedback device id";
  return new_id;
}

}  // namespace

LocalDeviceIdProvider::LocalDeviceIdProvider(const std::string& path)
    : device_id_(InitializeDeviceId(path)) {}

std::set<std::string> LocalDeviceIdProvider::GetKeys() const { return {kDeviceFeedbackIdKey}; }

void LocalDeviceIdProvider::GetOnUpdate(::fit::function<void(Annotations)> callback) {
  callback(DeviceIdToAnnotations()(device_id_));
}

Annotations DeviceIdToAnnotations::operator()(const std::string& device_id) {
  return {{kDeviceFeedbackIdKey, ErrorOrString(device_id)}};
}

RemoteDeviceIdProvider::RemoteDeviceIdProvider(async_dispatcher_t* dispatcher,
                                               std::shared_ptr<sys::ServiceDirectory> services,
                                               std::unique_ptr<backoff::Backoff> backoff)
    : dispatcher_(dispatcher), services_(std::move(services)), backoff_(std::move(backoff)) {
  Call();
}

void RemoteDeviceIdProvider::on_fidl_error(fidl::UnbindInfo info) {
  const internal::DisconnectResponse disconnect = internal::DisconnectResponse::BuildFrom(
      info.status(), fidl::DiscoverableProtocolName<fuchsia_feedback::DeviceIdProvider>);

  client_ = fidl::Client<fuchsia_feedback::DeviceIdProvider>();

  if (!disconnect.should_reconnect) {
    FX_LOGS(ERROR)
        << fidl::DiscoverableProtocolName<
               fuchsia_feedback::DeviceIdProvider> << " not found, will not attempt to reconnect";
    return;
  }

  FX_PLOGS(WARNING, info.status()) << disconnect.log_message;
  if (backoff_) {
    reconnect_task_.PostDelayed(dispatcher_, backoff_->GetNext());
  }
}

void RemoteDeviceIdProvider::GetOnUpdate(::fit::function<void(Annotations)> callback) {
  FX_CHECK(on_update_ == nullptr) << "GetOnUpdate can only be called once";
  on_update_ = std::move(callback);

  if (last_annotations_.has_value()) {
    on_update_(*last_annotations_);
  }
}

std::set<std::string> RemoteDeviceIdProvider::GetKeys() const { return {kDeviceFeedbackIdKey}; }

bool RemoteDeviceIdProvider::Connect() {
  if (client_.is_valid()) {
    return true;
  }

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_feedback::DeviceIdProvider>();
  if (endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
    return false;
  }

  services_->Connect(fidl::DiscoverableProtocolName<fuchsia_feedback::DeviceIdProvider>,
                     endpoints->server.TakeChannel());
  client_ = fidl::Client<fuchsia_feedback::DeviceIdProvider>(std::move(endpoints->client),
                                                             dispatcher_, this);
  return true;
}

void RemoteDeviceIdProvider::Call() {
  if (!Connect()) {
    return;
  }

  client_->GetId().Then([this](fidl::Result<fuchsia_feedback::DeviceIdProvider::GetId>& result) {
    if (result.is_error()) {
      return;
    }

    if (backoff_) {
      backoff_->Reset();
    }

    last_annotations_ = DeviceIdToAnnotations()(result->feedback_id());
    if (on_update_) {
      on_update_(*last_annotations_);
    }

    Call();
  });
}

}  // namespace forensics::feedback
