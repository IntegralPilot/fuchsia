// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/firmware_crash/firmware_crash_service.h"

#include <lib/zx/vmo.h>
#include <zircon/rights.h>
#include <zircon/types.h>

namespace ffc = fuchsia_firmware_crash;

namespace driver_manager {

namespace {
ffc::Crash CloneCrash(ffc::Crash& crash) {
  ffc::Crash clone;
  clone.subsystem_name(crash.subsystem_name());
  clone.timestamp(crash.timestamp());
  clone.reason(crash.reason());
  clone.count(crash.count());
  clone.firmware_version(crash.firmware_version());
  if (auto& crash_dump = crash.crash_dump(); crash_dump.has_value()) {
    zx::vmo dup;
    ZX_ASSERT(crash_dump->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup) == ZX_OK);
    clone.crash_dump(std::move(dup));
  }
  return clone;
}
}  // namespace

void FirmwareCrashService::Publish(component::OutgoingDirectory& outgoing) {
  zx::result result = outgoing.AddUnmanagedProtocol<ffc::Reporter>(
      bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  ZX_ASSERT(result.is_ok());

  result =
      outgoing.AddUnmanagedProtocol<ffc::Watcher>([this](fidl::ServerEnd<ffc::Watcher> request) {
        auto watcher = std::make_unique<Watcher>(this, std::move(request), dispatcher_);
        watchers_.push_back(std::move(watcher));
      });
  ZX_ASSERT(result.is_ok());
}

void FirmwareCrashService::Report(ReportRequest& request, ReportCompleter::Sync& completer) {
  // First update the crash count for this subsystem.
  if (auto& name = request.subsystem_name(); name.has_value()) {
    if (crash_count_.contains(*name)) {
      crash_count_[*name]++;
    } else {
      crash_count_[*name] = 1;
    }
    request.count(crash_count_[*name]);
  }

  // Then notify watchers.
  crashes_.push_back(std::move(request));
  for (auto& watcher : watchers_) {
    watcher->NewCrashAvailable();
  }
}

FirmwareCrashService::Watcher::Watcher(FirmwareCrashService* parent,
                                       fidl::ServerEnd<ffc::Watcher> request,
                                       async_dispatcher_t* dispatcher)
    : parent_(parent), binding_(dispatcher, std::move(request), this, [this](auto unused) {
        std::erase_if(parent_->watchers_, [this](const auto& iter) { return iter.get() == this; });
      }) {}

void FirmwareCrashService::Watcher::NewCrashAvailable() {
  ZX_ASSERT(parent_->crashes_.size() > crash_index_);

  if (completer_.has_value()) {
    auto crash = CloneCrash(parent_->crashes_[crash_index_]);
    completer_->Reply(fit::success(std::move(crash)));
    crash_index_++;
    completer_.reset();
  }
}

void FirmwareCrashService::Watcher::GetCrash(GetCrashCompleter::Sync& completer) {
  if (completer_.has_value()) {
    completer.Reply(fit::error(ffc::Error::kAlreadyPending));
    return;
  }
  if (parent_->crashes_.size() > crash_index_) {
    // There is a crash available. Report it.
    auto crash = CloneCrash(parent_->crashes_[crash_index_]);
    crash_index_++;
    completer.Reply(fit::success(std::move(crash)));
    return;
  }

  completer_ = completer.ToAsync();
}

}  // namespace driver_manager
