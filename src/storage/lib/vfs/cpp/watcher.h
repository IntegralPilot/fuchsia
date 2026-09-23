// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_WATCHER_H_
#define SRC_STORAGE_LIB_VFS_CPP_WATCHER_H_

#ifndef __Fuchsia__
#error "Fuchsia-only header"
#endif

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <zircon/types.h>

#include <cstdint>
#include <shared_mutex>
#include <string_view>

#include <fbl/intrusive_double_list.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fs {

// Implements directory watching , holding a list of watchers
class WatcherContainer {
 public:
  WatcherContainer();
  ~WatcherContainer();

  // Not copyable or movable.
  WatcherContainer(const WatcherContainer&) = delete;
  WatcherContainer& operator=(WatcherContainer&) = delete;

  zx_status_t WatchDir(FuchsiaVfs* vfs, Vnode* vn, fuchsia_io::wire::WatchMask mask,
                       uint32_t options, fidl::ServerEnd<fuchsia_io::DirectoryWatcher> server_end);

  // Notifies all VnodeWatchers in the watch list, if their mask indicates they are interested in
  // the incoming event.
  void Notify(std::string_view name, fuchsia_io::wire::WatchEvent event);

  bool HasWatchers() const {
    std::shared_lock guard(lock_);
    return !watch_list_.is_empty();
  }

 private:
  struct VnodeWatcher;

  // This lock is static to allow the dispatcher, which effectively owns the VnodeWatcher items, to
  // outlive the container. When the dispatcher shuts down, the callbacks can safely check whether
  // the item is within in the container using this mutex. It is unlikely this lock will be heavily
  // contended; the lock is only acquired uniquely when adding or removing from the watch list,
  // whilst dispatching messages uses a shared lock. This is simpler than alternatives such as
  // putting the watch list into a shared object (which will incur memory overheads).
  static std::shared_mutex lock_;

  fbl::DoublyLinkedList<VnodeWatcher*> watch_list_;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_WATCHER_H_
