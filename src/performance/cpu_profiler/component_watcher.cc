// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "component_watcher.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

zx::result<> profiler::ComponentWatcher::Watch() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  auto client_end = component::Connect<fuchsia_component::EventStream>();
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to event stream";
    return client_end.take_error();
  }
  fidl::SyncClient sync_stream_client{std::move(*client_end)};
  fidl::Result<fuchsia_component::EventStream::WaitForReady> ready_result =
      sync_stream_client->WaitForReady();
  if (ready_result.is_error()) {
    zx_status_t err = ready_result.error_value().status();
    FX_PLOGS(ERROR, err) << "Failed to wait for ready";
    return zx::error(err);
  }
  stream_client_ = fidl::Client(sync_stream_client.TakeClientEnd(), dispatcher_);
  stream_client_->GetNext().Then(
      [this](fidl::Result<fuchsia_component::EventStream::GetNext> &res) {
        this->HandleEvent(res);
      });
  return zx::ok();
}

void profiler::ComponentWatcher::Clear() {
  moniker_watchers_.clear();
  url_watchers_.clear();
}

zx::result<> profiler::ComponentWatcher::WatchForMoniker(std::string moniker,
                                                         ComponentEventHandler handler) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__, "moniker", moniker);
  // The events api strips leading "./"s or "/" from monikers
  std::string normalized = moniker;
  if (normalized.starts_with("./")) {
    normalized = normalized.substr(2);
  } else if (normalized.starts_with("/")) {
    normalized = normalized.substr(1);
  }

  if (moniker_watchers_.contains(normalized)) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }
  moniker_watchers_[std::move(normalized)] = std::move(handler);
  return zx::ok();
}

zx::result<> profiler::ComponentWatcher::WatchForUrl(std::string url,
                                                     ComponentEventHandler handler) {
  if (url_watchers_.contains(url)) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }
  url_watchers_[std::move(url)] = std::move(handler);
  return zx::ok();
}

void profiler::ComponentWatcher::HandleEvent(
    fidl::Result<fuchsia_component::EventStream::GetNext> &res) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (res.is_error()) {
    FX_LOGS(ERROR) << "GetEventFailed";
    return;
  }
  std::vector<fuchsia_component::Event> events = std::move(res->events());
  if (events.empty()) {
    return;
  }

  for (fuchsia_component::Event &event : events) {
    if (!event.header().has_value()) {
      continue;
    }
    if (!event.header()->moniker() || !event.header()->component_url()) {
      FX_LOGS(WARNING) << "Event didn't have a moniker or url?";
      continue;
    }
    std::string component_url = *event.header()->component_url();
    std::string moniker = *event.header()->moniker();
    if (event.header()->event_type()) {
      fuchsia_component::EventType event_type = event.header()->event_type().value();
      switch (event_type) {
        case fuchsia_component::EventType::kDebugStarted: {
          ComponentStartEvent start_event{.moniker = moniker, .url = component_url};
          if (event.payload() && event.payload()->debug_started()) {
            auto &debug_started = event.payload()->debug_started().value();
            if (debug_started.runtime_dir()) {
              start_event.runtime_dir = std::move(*debug_started.runtime_dir());
            }
            if (debug_started.break_on_start()) {
              start_event.break_on_start = std::move(*debug_started.break_on_start());
            }
          }
          auto moniker_handler = moniker_watchers_.find(moniker);
          if (moniker_handler != moniker_watchers_.end()) {
            auto handler = std::move(moniker_handler->second);
            moniker_watchers_.erase(moniker_handler);
            handler(std::move(start_event));
          } else if (auto url_handler = url_watchers_.find(component_url);
                     url_handler != url_watchers_.end()) {
            auto handler = std::move(url_handler->second);
            url_watchers_.erase(url_handler);
            handler(std::move(start_event));
          }
          break;
        }
        default:
          // We only have subscribed to debug started events
          break;
      }
    }
  }

  stream_client_->GetNext().Then(
      [this](fidl::Result<fuchsia_component::EventStream::GetNext> &res) {
        this->HandleEvent(res);
      });
}
