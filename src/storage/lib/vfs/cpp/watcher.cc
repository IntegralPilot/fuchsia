// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/watcher.h"

#include <fidl/fuchsia.io/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/fdio/limits.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/defer.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <mutex>
#include <shared_mutex>
#include <string_view>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_container_utils.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>

#include "src/storage/lib/vfs/cpp/fuchsia_vfs.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace fio = fuchsia_io;

namespace fs {

// A simple structure which holds a channel to a watching client, as well as a mask of signals
// they are interested in hearing about.
struct WatcherContainer::VnodeWatcher
    : public fbl::DoublyLinkedListable<VnodeWatcher*, fbl::NodeOptions::AllowRemoveFromContainer> {
  VnodeWatcher(fidl::ServerEnd<fuchsia_io::DirectoryWatcher> server_end,
               fuchsia_io::wire::WatchMask mask)
      : server_end(std::move(server_end)),
        mask(mask - (fio::wire::WatchMask::kExisting | fio::wire::WatchMask::kIdle)),
        waiter(this->server_end.channel().get(), ZX_CHANNEL_PEER_CLOSED | ZX_USER_SIGNAL_0) {}

  fidl::ServerEnd<fuchsia_io::DirectoryWatcher> server_end;
  fuchsia_io::WatchMask mask;
  async::WaitOnce waiter;
};

WatcherContainer::WatcherContainer() = default;
WatcherContainer::~WatcherContainer() {
  std::unique_lock guard(lock_);
  for (VnodeWatcher& watcher : watch_list_) {
    watcher.server_end.channel().signal(0, ZX_USER_SIGNAL_0);
  }
  watch_list_.clear();
}

std::shared_mutex WatcherContainer::lock_;

namespace {

// Watch event messages are sent via the provided channel and take the form:
// { uint8_t event; uint8_t namelen; uint8_t name[namelen]; }
// Multiple events may arrive in one message, one after another.
// Names do not include a terminating null.
using vfs_watch_msg_t = struct {
  uint8_t event;
  uint8_t len;
  char name[];
};

}  // namespace

// Transmission buffer for sending directory watcher notifications to clients. Allows enqueueing
// multiple messages in a buffer before sending an IPC message to a client.
class WatchBuffer {
 public:
  DISALLOW_COPY_ASSIGN_AND_MOVE(WatchBuffer);
  WatchBuffer() = default;

  zx_status_t AddMsg(const fidl::ServerEnd<fuchsia_io::DirectoryWatcher>& server_end,
                     fio::wire::WatchEvent event, std::string_view name) {
    size_t slen = name.length();
    size_t mlen = sizeof(vfs_watch_msg_t) + slen;
    if (mlen + watch_buf_size_ > sizeof(watch_buf_)) {
      // This message won't fit in the watch_buf; transmit first.
      zx_status_t status = Send(server_end);
      if (status != ZX_OK) {
        return status;
      }
    }
    vfs_watch_msg_t& vmsg = *reinterpret_cast<vfs_watch_msg_t*>(&watch_buf_[watch_buf_size_]);
    vmsg.event = static_cast<uint8_t>(event);
    vmsg.len = static_cast<uint8_t>(slen);
    memcpy(vmsg.name, name.data(), slen);
    watch_buf_size_ += mlen;
    return ZX_OK;
  }
  zx_status_t Send(const fidl::ServerEnd<fuchsia_io::DirectoryWatcher>& server_end) {
    if (watch_buf_size_ > 0) {
      // Only write if we have something to write
      zx_status_t status = server_end.channel().write(
          0, watch_buf_, static_cast<uint32_t>(watch_buf_size_), nullptr, 0);
      watch_buf_size_ = 0;
      if (status != ZX_OK) {
        return status;
      }
    }
    return ZX_OK;
  }

 private:
  size_t watch_buf_size_ = 0;
  char watch_buf_[fio::wire::kMaxBuf]{};
};

zx_status_t WatcherContainer::WatchDir(FuchsiaVfs* vfs, Vnode* vn, fio::wire::WatchMask mask,
                                       uint32_t options,
                                       fidl::ServerEnd<fuchsia_io::DirectoryWatcher> server_end) {
  if (!mask) {
    // No events to watch
    return ZX_ERR_INVALID_ARGS;
  }

  fbl::AllocChecker ac;
  std::unique_ptr<VnodeWatcher> watcher(new (&ac) VnodeWatcher(std::move(server_end), mask));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  if (mask & fio::wire::WatchMask::kExisting) {
    VdirCookie dircookie;
    memset(&dircookie, 0, sizeof(dircookie));
    char readdir_buf[FDIO_CHUNK_SIZE];
    WatchBuffer wb;
    {
      // Send "fio::wire::WatchEvent::kExisting" for all entries in readdir.
      while (true) {
        size_t actual;
        zx_status_t status =
            vfs->Readdir(vn, &dircookie, readdir_buf, sizeof(readdir_buf), &actual);
        if (status != ZX_OK || actual == 0) {
          break;
        }
        char* ptr = readdir_buf;
        while (actual >= sizeof(fs::DirectoryEntry)) {
          auto dirent = reinterpret_cast<fs::DirectoryEntry*>(ptr);
          if (dirent->name[0]) {
            wb.AddMsg(watcher->server_end, fio::wire::WatchEvent::kExisting,
                      std::string_view(dirent->name, dirent->name_length));
          }
          size_t entry_len = dirent->name_length + sizeof(fs::DirectoryEntry);
          ZX_ASSERT(entry_len <= actual);  // Prevent underflow
          actual -= entry_len;
          ptr += entry_len;
        }
      }
    }

    // Send fio::wire::WatchEvent::kIdle to signify that readdir has completed.
    if (mask & fio::wire::WatchMask::kIdle) {
      wb.AddMsg(watcher->server_end, fio::wire::WatchEvent::kIdle, "");
    }

    wb.Send(watcher->server_end);
  }

  VnodeWatcher* watcher_ptr = watcher.get();
  {
    std::unique_lock guard(lock_);
    watch_list_.push_back(watcher_ptr);
  }

  auto cleanup = fit::defer([watcher = std::move(watcher)] {
    std::unique_lock guard(lock_);
    if (watcher->InContainer()) {
      watcher->RemoveFromContainer();
    }
  });

  // Start watching for signals on the channel. This takes ownership of the cleanup callback.
  return watcher_ptr->waiter.Begin(
      vfs->dispatcher(), [cleanup = std::move(cleanup)](async_dispatcher_t*, async::WaitOnce*,
                                                        zx_status_t, const zx_packet_signal_t*) {});
}

void WatcherContainer::Notify(std::string_view name, fio::wire::WatchEvent event) {
  if (name.length() > fio::wire::kMaxNameLength) {
    return;
  }

  std::shared_lock guard(lock_);

  if (watch_list_.is_empty()) {
    return;
  }

  uint8_t msg[sizeof(vfs_watch_msg_t) + fio::wire::kMaxNameLength];
  size_t msg_length = sizeof(vfs_watch_msg_t) + name.length();
  vfs_watch_msg_t* vmsg = reinterpret_cast<vfs_watch_msg_t*>(msg);
  vmsg->event = static_cast<uint8_t>(event);
  vmsg->len = static_cast<uint8_t>(name.length());
  memcpy(vmsg->name, name.data(), name.length());

  for (VnodeWatcher& watcher : watch_list_) {
    if (!(watcher.mask & fio::wire::WatchMask(1 << static_cast<uint8_t>(event)))) {
      continue;
    }

    zx_status_t status =
        watcher.server_end.channel().write(0, msg, static_cast<uint32_t>(msg_length), nullptr, 0);
    if (status < 0) {
      // Signal so the watcher can be cleaned up.
      watcher.server_end.channel().signal(0, ZX_USER_SIGNAL_0);
      // Clear the mask so that we don't try and send to this watcher again.
      watcher.mask = fio::wire::WatchMask(0);
    }
  }
}

}  // namespace fs
