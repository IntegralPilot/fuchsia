// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/common/delay_watcher_server.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>

#include <string>
#include <string_view>

#include "src/media/audio/services/common/logging.h"

namespace media_audio {

// static
std::shared_ptr<DelayWatcherServer> DelayWatcherServer::Create(
    std::shared_ptr<const FidlThread> fidl_thread,
    fidl::ServerEnd<fuchsia_audio::DelayWatcher> server_end, Args args) {
  return BaseFidlServer::Create(std::move(fidl_thread), std::move(server_end), std::move(args));
}

DelayWatcherServer::DelayWatcherServer(Args args)
    : name_(std::move(args.name)), delay_(args.initial_delay) {}

void DelayWatcherServer::WatchDelay(WatchDelayRequestView request,
                                    WatchDelayCompleter::Sync& completer) {
  ScopedThreadChecker checker(thread().checker());

  if (completer_) {
    FX_LOGS(WARNING)
        << "concurrent DelayWatcher.WatchDelay calls not allowed: shutting down DelayWatcher "
        << "'" << name_ << "'";
    Shutdown(ZX_ERR_BAD_STATE);
    return;
  }

  if (first_ || delay_ != last_sent_delay_) {
    first_ = false;
    fidl::Arena<> arena;
    completer.Reply(BuildResponse(arena));
    last_sent_delay_ = delay_;
    return;
  }

  completer_ = completer.ToAsync();
}

void DelayWatcherServer::set_delay(zx::duration new_delay) {
  if (new_delay == *delay_) {
    return;
  }

  delay_ = new_delay;

  if (completer_) {
    fidl::Arena<> arena;
    completer_->Reply(BuildResponse(arena));
    completer_ = std::nullopt;
    last_sent_delay_ = delay_;
  }
}

fuchsia_audio::wire::DelayWatcherWatchDelayResponse DelayWatcherServer::BuildResponse(
    fidl::AnyArena& arena) {
  auto builder = fuchsia_audio::wire::DelayWatcherWatchDelayResponse::Builder(arena);
  if (delay_) {
    builder.delay(delay_->to_nsecs());
  }
  return builder.Build();
}

DelayWatcherServerGroup::DelayWatcherServerGroup(std::string_view group_name,
                                                 std::shared_ptr<const FidlThread> fidl_thread)
    : group_name_(group_name), fidl_thread_(std::move(fidl_thread)) {}

void DelayWatcherServerGroup::Add(fidl::ServerEnd<fuchsia_audio::DelayWatcher> server_end) {
  // The result of DelayWatcherServer::Create is immediately converted to a std::weak_ptr. This
  // doesn't immediately delete the server because the server's `on_unbound` handler holds a
  // shared_ptr to the server. The server won't be deleted until that handler runs, which happens
  // when the channel is closed.
  servers_.push_back(DelayWatcherServer::Create(
      fidl_thread_, std::move(server_end),
      {
          .name = group_name_ + ".Server" + std::to_string(num_created_++),
          .initial_delay = delay_,
      }));
}

void DelayWatcherServerGroup::Shutdown() {
  for (auto& weak : servers_) {
    if (auto server = weak.lock(); server) {
      server->Shutdown();
    }
  }
}

void DelayWatcherServerGroup::set_delay(zx::duration delay) {
  delay_ = delay;
  for (auto& weak : servers_) {
    if (auto server = weak.lock(); server) {
      server->set_delay(delay);
    }
  }
}

}  // namespace media_audio
