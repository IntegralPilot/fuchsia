// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_STATUS_WATCHER_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_STATUS_WATCHER_H_

#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sync/completion.h>
#include <threads.h>

#include <queue>

#include <fbl/auto_lock.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/mutex.h>

#include "definitions.h"

namespace network::internal {

template <typename F>
void WithWireStatus(F fn, netdev::wire::StatusFlags flags, uint32_t mtu) {
  fidl::Arena arena;
  fn(netdev::wire::PortStatus::Builder(arena).flags(flags).mtu(mtu).Build());
}

class StatusWatcher : public fbl::DoublyLinkedListable<std::unique_ptr<StatusWatcher>>,
                      public fidl::WireServer<netdev::StatusWatcher> {
 public:
  explicit StatusWatcher(uint32_t max_queue);
  ~StatusWatcher() override;

  zx_status_t Bind(async_dispatcher_t* dispatcher, fidl::ServerEnd<netdev::StatusWatcher> channel,
                   fit::callback<void(StatusWatcher*)> closed_callback);
  void Unbind();

  void PushStatus(const fuchsia_hardware_network::wire::PortStatus& status);

 private:
  void WatchStatus(WatchStatusCompleter::Sync& _completer) override;

  StatusWatcher(const StatusWatcher&) = delete;
  StatusWatcher& operator=(const StatusWatcher&) = delete;

  fbl::Mutex lock_;
  uint32_t max_queue_;

  // We need a value type to store the port status. It's possible to use FIDL types such as
  // WireTableFrames to do this but they're cumbersome to work with.
  struct PortStatus {
    explicit PortStatus(const netdev::wire::PortStatus ps) : flags(ps.flags()), mtu(ps.mtu()) {}
    bool operator==(const netdev::wire::PortStatus& ps) const {
      return ps.has_flags() && ps.has_mtu() && ps.flags() == flags && ps.mtu() == mtu;
    }
    netdev::wire::StatusFlags flags;
    uint32_t mtu;
  };

  std::optional<PortStatus> last_observed_ __TA_GUARDED(lock_);
  std::queue<PortStatus> queue_ __TA_GUARDED(lock_);
  std::optional<WatchStatusCompleter::Async> pending_txn_ __TA_GUARDED(lock_);
  std::optional<fidl::ServerBindingRef<netdev::StatusWatcher>> binding_ __TA_GUARDED(lock_);
  fit::callback<void(StatusWatcher*)> closed_cb_;
};

using StatusWatcherList = fbl::DoublyLinkedList<std::unique_ptr<StatusWatcher>>;

}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_STATUS_WATCHER_H_
