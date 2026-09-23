// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_MEMORY_WATCHER_H_
#define SRC_STORAGE_F2FS_MEMORY_WATCHER_H_

#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/client.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/zx/result.h>

#include <optional>

namespace f2fs {

enum class MemoryPressure {
  kUnknown = 0,
  kLow,
  kMedium,
  kHigh,
};

enum class MemoryStatus {
  kNeedReclaim = 0,
  kNeedCheckpoint,
  kNeedWriteback,
};

using MemoryPressureCallback = fit::callback<void(MemoryPressure level)>;
constexpr uint32_t kMaxID = std::numeric_limits<uint32_t>::max();

// This class is not thread-safe.
class MemoryPressureWatcher : public fidl::AsyncEventHandler<fuchsia_memorypressure::Watcher>,
                              public fidl::Server<fuchsia_memorypressure::Watcher> {
 public:
  explicit MemoryPressureWatcher(async_dispatcher_t* dispatcher, MemoryPressureCallback callback);
  bool IsConnected() const { return is_connected_; }
  static std::string_view ToString(MemoryPressure level);

 private:
  zx::result<> Start();

  void on_fidl_error(fidl::UnbindInfo error) final;

  // |fuchsia::memorypressure::Watcher|
  void OnLevelChanged(OnLevelChangedRequest& request,
                      OnLevelChangedCompleter::Sync& completer) final;

  async_dispatcher_t* dispatcher_ = nullptr;

  std::optional<fidl::ServerBindingRef<fuchsia_memorypressure::Watcher>> memory_pressure_server_;

  // handling memorypressure events.
  MemoryPressureCallback callback_;
  bool is_connected_ = false;
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_MEMORY_WATCHER_H_
