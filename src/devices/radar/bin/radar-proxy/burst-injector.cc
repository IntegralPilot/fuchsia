// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "burst-injector.h"

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <list>
#include <optional>

namespace radar {

namespace {

constexpr FidlRadar::StatusCode MapVmoStatus(zx_status_t status) {
  switch (status) {
    case ZX_ERR_BAD_HANDLE:
    case ZX_ERR_WRONG_TYPE:
    case ZX_ERR_BAD_STATE:
      return FidlRadar::StatusCode::kVmoBadHandle;
    case ZX_ERR_ACCESS_DENIED:
      return FidlRadar::StatusCode::kVmoAccessDenied;
    case ZX_ERR_OUT_OF_RANGE:
    case ZX_ERR_BUFFER_TOO_SMALL:
      return FidlRadar::StatusCode::kVmoTooSmall;
    default:
      return FidlRadar::StatusCode::kUnspecified;
  }
}

}  // namespace

BurstInjector::BurstInjector(async_dispatcher_t* const dispatcher,
                             fidl::ServerEnd<FidlRadar::RadarBurstInjector> server_end,
                             BurstInjectorManager* const parent)
    : dispatcher_(dispatcher),
      parent_(parent),
      server_(fidl::BindServer(dispatcher_, std::move(server_end), this,
                               [&](BurstInjector* injector, auto, auto) {
                                 injector->parent_->OnInjectorUnbound(injector);
                               })) {}

BurstInjector::~BurstInjector() {
  if (stop_burst_injection_completer_) {
    stop_burst_injection_completer_->Close(ZX_ERR_PEER_CLOSED);
  }
}

void BurstInjector::GetBurstProperties(GetBurstPropertiesCompleter::Sync& completer) {
  completer.Reply({parent_->burst_properties().size(), parent_->burst_properties().period()});
}

void BurstInjector::EnqueueBursts(EnqueueBurstsRequest& request,
                                  EnqueueBurstsCompleter::Sync& completer) {
  if (stop_burst_injection_completer_) {
    completer.Reply(fit::error(FidlRadar::StatusCode::kBadState));
    return;
  }
  if (request.bursts().burst_count() == 0) {
    completer.Reply(fit::error(FidlRadar::StatusCode::kInvalidArgs));
    return;
  }

  const uint32_t burst_size = parent_->burst_properties().size();
  const auto total_size = static_cast<size_t>(request.bursts().burst_count()) * burst_size;
  const uint32_t id = injector_vmo_queue_.empty() ? 1 : injector_vmo_queue_.back().id + 1;

  {
    MappedInjectorVmo vmo{.id = id};
    zx_status_t status = vmo.mapped_vmo.Map(request.bursts().vmo(), 0, total_size, ZX_VM_PERM_READ);
    if (status != ZX_OK) {
      completer.Reply(fit::error(MapVmoStatus(status)));
      return;
    }

    vmo.bursts = {reinterpret_cast<const uint8_t*>(vmo.mapped_vmo.start()), total_size};
    injector_vmo_queue_.push_back(std::move(vmo));
  }

  if (schedule_next_burst_) {
    ScheduleNextBurstInjection();
  }

  // The bursts VMO has been mapped, so we can let the handle go out of scope.

  completer.Reply(fit::success(id));
}

void BurstInjector::StartBurstInjection(StartBurstInjectionCompleter::Sync& completer) {
  if (inject_bursts_ || stop_burst_injection_completer_) {
    completer.Reply(fit::error(FidlRadar::StatusCode::kBadState));
    return;
  }

  inject_bursts_ = true;
  completer.Reply(fit::success());
  last_burst_timestamp_ = parent_->StartBurstInjection();
  ScheduleNextBurstInjection();
}

void BurstInjector::StopBurstInjection(StopBurstInjectionCompleter::Sync& completer) {
  if (!inject_bursts_ || stop_burst_injection_completer_) {
    completer.Reply(fit::error(FidlRadar::StatusCode::kBadState));
  } else if (injector_vmo_queue_.empty()) {
    FinishBurstInjection(completer.ToAsync());
  } else {
    stop_burst_injection_completer_.emplace(completer.ToAsync());
  }
}

void BurstInjector::FinishBurstInjection(StopBurstInjectionCompleter::Async completer) {
  inject_bursts_ = false;
  completer.Reply(fit::success());
  parent_->StopBurstInjection();
}

void BurstInjector::OnBurstInjectionTimerExpire() {
  if (!inject_bursts_) {
    // We may have been scheduled to run before injection was stopped.
    return;
  }
  if (injector_vmo_queue_.empty()) {
    // Don't continue sending bursts until we get a new VMO to inject.
    schedule_next_burst_ = true;
    return;
  }

  last_burst_timestamp_ = zx::clock::get_monotonic();

  const uint32_t burst_size = parent_->burst_properties().size();

  {
    MappedInjectorVmo& vmo = injector_vmo_queue_.front();
    ZX_DEBUG_ASSERT(vmo.bursts.size() >= burst_size);

    const cpp20::span burst = vmo.bursts.subspan(0, burst_size);
    parent_->SendBurst(burst, last_burst_timestamp_);

    vmo.bursts = vmo.bursts.subspan(burst_size);
    if (vmo.bursts.empty()) {
      fidl::SendEvent(server_)->OnBurstsDelivered(vmo.id).is_ok();
      injector_vmo_queue_.pop_front();
    }
  }

  if (injector_vmo_queue_.empty() && stop_burst_injection_completer_) {
    // Complete the StopBurstInjection() call if we just finished the last VMO in the queue.
    FinishBurstInjection(std::move(*stop_burst_injection_completer_));
    stop_burst_injection_completer_.reset();
  } else {
    ScheduleNextBurstInjection();
  }
}

void BurstInjector::ScheduleNextBurstInjection() {
  const zx::time next_burst_time =
      last_burst_timestamp_ + zx::nsec(parent_->burst_properties().period());
  async::PostTaskForTime(
      dispatcher_, [this]() { OnBurstInjectionTimerExpire(); }, next_burst_time);
  schedule_next_burst_ = false;
}

}  // namespace radar
