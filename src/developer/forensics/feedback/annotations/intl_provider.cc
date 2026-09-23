// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotations/intl_provider.h"

#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <utility>

#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/backoff/backoff.h"

namespace forensics::feedback {

IntlProvider::IntlProvider(async_dispatcher_t* dispatcher,
                           std::shared_ptr<sys::ServiceDirectory> services,
                           std::unique_ptr<backoff::Backoff> backoff)
    : dispatcher_(dispatcher), services_(std::move(services)), backoff_(std::move(backoff)) {
  GetInternationalization();
}

std::set<std::string> IntlProvider::GetAnnotationKeys() {
  return {
      kSystemLocalePrimaryKey,
      kSystemTimezonePrimaryKey,
  };
}

std::set<std::string> IntlProvider::GetKeys() const { return IntlProvider::GetAnnotationKeys(); }

bool IntlProvider::Connect() {
  if (client_.is_valid()) {
    return true;
  }

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_intl::PropertyProvider>();
  if (endpoints.is_error()) {
    FX_LOGS(ERROR) << "Failed to create endpoints: " << endpoints.status_string();
    return false;
  }

  services_->Connect(fidl::DiscoverableProtocolName<fuchsia_intl::PropertyProvider>,
                     endpoints->server.TakeChannel());

  client_ =
      fidl::Client<fuchsia_intl::PropertyProvider>(std::move(endpoints->client), dispatcher_, this);
  return true;
}

void IntlProvider::OnChange() { GetInternationalization(); }

void IntlProvider::on_fidl_error(fidl::UnbindInfo info) {
  const internal::DisconnectResponse disconnect = internal::DisconnectResponse::BuildFrom(
      info.status(), fidl::DiscoverableProtocolName<fuchsia_intl::PropertyProvider>);

  client_ = fidl::Client<fuchsia_intl::PropertyProvider>();

  if (!disconnect.should_reconnect) {
    FX_LOGS(WARNING) << disconnect.log_message;
    error_ = disconnect.error;
    OnUpdate();
    return;
  }

  FX_PLOGS(WARNING, info.status()) << disconnect.log_message;
  reconnect_task_.PostDelayed(dispatcher_, backoff_->GetNext());
}

void IntlProvider::OnUpdate() {
  if (!on_update_) {
    return;
  }

  if (error_.has_value()) {
    Annotations annotations;
    for (const std::string& key : GetKeys()) {
      annotations.insert({key, ErrorOrString(*error_)});
    }

    on_update_(annotations);
    return;
  }

  Annotations annotations;

  if (locale_.has_value()) {
    annotations.insert({kSystemLocalePrimaryKey, ErrorOrString(*locale_)});
  }

  if (timezone_.has_value()) {
    annotations.insert({kSystemTimezonePrimaryKey, ErrorOrString(*timezone_)});
  }

  if (!annotations.empty()) {
    on_update_(annotations);
  }
}

void IntlProvider::GetOnUpdate(::fit::function<void(Annotations)> callback) {
  on_update_ = std::move(callback);

  OnUpdate();
}

void IntlProvider::GetInternationalization() {
  if (!Connect()) {
    return;
  }

  client_->GetProfile().Then([this](
                                 fidl::Result<fuchsia_intl::PropertyProvider::GetProfile>& result) {
    if (result.is_error()) {
      FX_LOGS(WARNING) << "Failed to get internationalization profile: " << result.error_value();
      return;
    }

    error_ = std::nullopt;
    backoff_->Reset();

    const fuchsia_intl::Profile& profile = result->profile();
    if (profile.locales().has_value() && !profile.locales()->empty()) {
      locale_ = profile.locales()->front().id();
    }

    if (profile.time_zones().has_value() && !profile.time_zones()->empty()) {
      timezone_ = profile.time_zones()->front().id();
    }

    OnUpdate();
  });
}

}  // namespace forensics::feedback
