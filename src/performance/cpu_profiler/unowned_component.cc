// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "unowned_component.h"

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include "component.h"

namespace {
std::optional<fidl::ClientEnd<fuchsia_io::Directory>> OpenRuntimeDir(
    const fidl::SyncClient<fuchsia_sys2::RealmQuery>& client, const std::string& moniker) {
  auto [directory_client_endpoint, directory_server] =
      fidl::Endpoints<fuchsia_io::Directory>::Create();
  fidl::Result<fuchsia_sys2::RealmQuery::OpenDirectory> open_result = client->OpenDirectory({{
      .moniker = moniker,
      .dir_type = fuchsia_sys2::OpenDirType::kRuntimeDir,
      .object = std::move(directory_server),
  }});
  if (open_result.is_ok()) {
    return std::move(directory_client_endpoint);
  }
  return std::nullopt;
}
}  // namespace

zx::result<std::unique_ptr<profiler::UnownedComponent>> profiler::UnownedComponent::Create(
    const std::optional<std::string>& moniker, const std::optional<std::string>& url,
    ComponentWatcher& component_watcher) {
  std::optional<Moniker> parsed_moniker;
  if (moniker.has_value()) {
    zx::result parsed = profiler::Moniker::Parse(moniker.value_or(""));
    if (parsed.is_error()) {
      return parsed.take_error();
    }
    parsed_moniker = *parsed;
  }
  return zx::ok(std::make_unique<UnownedComponent>(url, parsed_moniker, component_watcher));
}

zx::result<> profiler::UnownedComponent::Attach(
    const fidl::SyncClient<fuchsia_sys2::RealmQuery>& client, const Moniker& moniker) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__, "moniker", moniker.ToString());
  fidl::Result<fuchsia_sys2::RealmQuery::GetInstance> result =
      client->GetInstance(moniker.ToString());
  if (result.is_error()) {
    FX_LOGS(WARNING) << "Failed to find moniker: " << moniker.ToString() << ". "
                     << result.error_value();
    return result.error_value().is_domain_error()
               ? zx::error(ZX_ERR_BAD_PATH)
               : zx::error(result.error_value().framework_error().status());
  }
  if (!result->instance().url()) {
    return zx::error(ZX_ERR_BAD_PATH);
  }

  if (!result->instance().resolved_info() ||
      !result->instance().resolved_info()->execution_info()) {
    return component_watcher_.WatchForMoniker(
        moniker.ToString(),
        [this](ComponentStartEvent event) { on_start_.value()(std::move(event)); });
  }
  on_start_.value()(ComponentStartEvent{
      .moniker = moniker.ToString(),
      .url = *result->instance().url(),
      .runtime_dir = OpenRuntimeDir(client, moniker.ToString()),
  });
  return zx::ok();
}

zx::result<> profiler::UnownedComponent::Start(fxl::WeakPtr<Sampler> notify) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  on_start_ = profiler::MakeOnStartHandler(std::move(notify));

  zx::result<fidl::ClientEnd<fuchsia_sys2::RealmQuery>> client_end =
      component::Connect<fuchsia_sys2::RealmQuery>("/svc/fuchsia.sys2.RealmQuery.root");
  if (client_end.is_error()) {
    FX_LOGS(WARNING) << "Unable to connect to RealmQuery. Attaching to components isn't supported!";
    return client_end.take_error();
  }
  fidl::SyncClient realm_query_client{std::move(*client_end)};

  // If we don't have a moniker specified, that means we're waiting for a component with a matching
  // url to show up.
  if (!moniker_.has_value()) {
    FX_DCHECK(url_.has_value());  // This should have been checked earlier in configuration
    return component_watcher_.WatchForUrl(
        *url_, [this](ComponentStartEvent event) { on_start_.value()(std::move(event)); });
  }

  // If the component exists, we'll attach to it, if not, we'll need to wait for it to launch
  fidl::Result<fuchsia_sys2::RealmQuery::GetInstance> result =
      realm_query_client->GetInstance(moniker_->ToString());
  if (result.is_error()) {
    if (result.error_value().is_domain_error() &&
        result.error_value().domain_error() == fuchsia_sys2::GetInstanceError::kInstanceNotFound) {
      FX_LOGS(INFO) << "Watching for: " << moniker_->ToString();
      return component_watcher_.WatchForMoniker(
          moniker_->ToString(), [this, realm_query_client = std::move(realm_query_client)](
                                    ComponentStartEvent start_event) mutable {
            auto res =
                profiler::TraverseRealm(start_event.moniker,
                                        [this, client = std::move(realm_query_client)](
                                            const std::string& moniker_string) -> zx::result<> {
                                          auto moniker = Moniker::Parse(moniker_string);
                                          if (moniker.is_error()) {
                                            return moniker.take_error();
                                          }
                                          return this->Attach(client, *moniker);
                                        });
            if (res.is_error()) {
              FX_PLOGS(ERROR, res.status_value())
                  << "Failed to recursively attach to moniker: " << start_event.moniker;
            }
          });
    }
    FX_LOGS(INFO) << "Invalid moniker: " << moniker_->ToString();
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  return profiler::TraverseRealm(moniker_->ToString(),
                                 [this, client = std::move(realm_query_client)](
                                     const std::string& moniker_string) -> zx::result<> {
                                   auto moniker = Moniker::Parse(moniker_string);
                                   if (moniker.is_error()) {
                                     return moniker.take_error();
                                   }
                                   return this->Attach(client, *moniker);
                                 });
}

zx::result<> profiler::UnownedComponent::Stop() { return zx::ok(); }

zx::result<> profiler::UnownedComponent::Destroy() { return zx::ok(); }
