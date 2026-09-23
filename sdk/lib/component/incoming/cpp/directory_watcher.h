// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_COMPONENT_INCOMING_CPP_DIRECTORY_WATCHER_H_
#define LIB_COMPONENT_INCOMING_CPP_DIRECTORY_WATCHER_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/wait.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>

#include <deque>

namespace component {

// A watcher for directory entries.
//
// Watching is automatically stopped on destruction.
class DirectoryWatcher final {
 public:
  // A callback to be invoked when directory entries are added or removed.
  //
  // |event| will be either fuchsia_io::wire::WatchEvent::kExisting, if an instance was
  // existing at the beginning, fuchsia_io::wire::WatchEvent::kAdded, if an instance
  // was added, or fuchsia_io::wire::WatchEvent::kIdle, if all the existing instances
  // have been reported.
  // |instance| will be the name of the instance associated with the event.
  using Callback = fit::function<void(fuchsia_io::wire::WatchEvent event, std::string instance)>;

  // Begins watching for entries in the provided directory.
  // When an entry is found, calls
  // the callback function with the entry name and the event type.
  // Begin will return ZX_ERR_UNAVAILABLE if it is called multiple times
  // without Cancel called first.
  zx_status_t Begin(fidl::UnownedClientEnd<fuchsia_io::Directory> dir, Callback callback,
                    async_dispatcher_t* dispatcher);

  // Cancels watching for directory entries.
  zx_status_t Cancel();

 private:
  void OnWatchedEvent(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                      const zx_packet_signal_t* signal);

  Callback callback_;
  std::shared_ptr<std::array<uint8_t, fuchsia_io::wire::kMaxBuf>> buf_;
  zx::channel client_end_;
  async::WaitMethod<DirectoryWatcher, &DirectoryWatcher::OnWatchedEvent> wait_{this};
};

// SyncDirectoryWatcher allows directories to be waited for synchronously.
//
// The directory can be specified in multiple forms:
// - a fidl::UnownedClientEnd<fuchsia_io::Directory>
// - a fidl::UnownedClientEnd<fuchsia_io::Directory> and a path relative to it
// - a full path, which will be opened relative to the root of the component's namespace.
// The instantiation arguments will not be checked until GetNextEntry is first called.
class SyncDirectoryWatcher final {
 public:
  explicit SyncDirectoryWatcher(fidl::UnownedClientEnd<fuchsia_io::Directory> dir) : dir_(dir) {}
  explicit SyncDirectoryWatcher(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                const std::string& relative_path)
      : dir_(dir), path_(relative_path) {}
  explicit SyncDirectoryWatcher(const std::string& full_path) : path_(full_path) {}

  // Sequentially query for directory entries at the directory given at initialization
  //
  // This call will block until a directory entry is found. When an entry is detected, the
  // name of the entry is returned.
  //
  // Subsequent calls to GetNextEntry will return other entries if they exist.
  // GetNextEntry will iterate through all directory entries of a given directory.
  // When all of the existing directory entries have been returned,
  // if |stop_at_idle| is true, GetNextEntry will return a zx::error(ZX_ERR_STOP).
  // Otherwise, GetNextEntry will wait until |deadline| for a new instance to appear.
  zx::result<std::string> GetNextEntry(bool stop_at_idle, zx::time deadline = zx::time::infinite());

 private:
  zx::result<> Begin();
  void OnWatchedEvent(fuchsia_io::wire::WatchEvent event, const std::string& instance);

  bool has_begun_iterating_ = false;
  bool idle_called_ = false;

  // For doing blocking waits:
  std::deque<std::string> entries_;
  std::optional<fidl::UnownedClientEnd<fuchsia_io::Directory>> dir_;
  std::optional<std::string> path_;
  DirectoryWatcher directory_watcher_;
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

}  // namespace component

#endif  // LIB_COMPONENT_INCOMING_CPP_DIRECTORY_WATCHER_H_
