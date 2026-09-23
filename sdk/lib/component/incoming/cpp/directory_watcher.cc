// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/component/incoming/cpp/directory_watcher.h>
#include <lib/fdio/directory.h>

namespace component {

zx_status_t DirectoryWatcher::Begin(fidl::UnownedClientEnd<fuchsia_io::Directory> dir,
                                    DirectoryWatcher::Callback callback,
                                    async_dispatcher_t* dispatcher) {
  if (callback_ != nullptr) {
    return ZX_ERR_UNAVAILABLE;
  }
  callback_ = std::move(callback);
  auto [client, server] = fidl::Endpoints<fuchsia_io::DirectoryWatcher>::Create();

  auto watch_mask = fuchsia_io::wire::WatchMask::kAdded | fuchsia_io::wire::WatchMask::kExisting |
                    fuchsia_io::wire::WatchMask::kIdle;
  const fidl::WireResult<::fuchsia_io::Directory::Watch> result =
      fidl::WireCall(dir)->Watch(watch_mask, 0, std::move(server));
  if (!result.ok()) {
    return result.status();
  }
  const fidl::WireResponse<::fuchsia_io::Directory::Watch> response = result.value();
  if (zx_status_t status = response.s; status != ZX_OK) {
    return status;
  }
  buf_ = std::shared_ptr<std::array<uint8_t, fuchsia_io::wire::kMaxBuf>>(
      new std::array<uint8_t, fuchsia_io::wire::kMaxBuf>);
  client_end_ = client.TakeChannel();
  wait_.set_object(client_end_.get());
  wait_.set_trigger(ZX_CHANNEL_READABLE);
  return wait_.Begin(dispatcher);
}

zx_status_t DirectoryWatcher::Cancel() { return wait_.Cancel(); }

void DirectoryWatcher::OnWatchedEvent(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                      zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK || !(signal->observed & ZX_CHANNEL_READABLE)) {
    return;
  }

  uint32_t size;
  status = client_end_.read(0, buf_->data(), nullptr, fuchsia_io::wire::kMaxBuf, 0, &size, nullptr);
  if (status != ZX_OK) {
    return;
  }

  std::weak_ptr<std::array<uint8_t, fuchsia_io::wire::kMaxBuf>> weak_buf = buf_;
  uint8_t* msg = buf_->data();
  while (size >= 2) {
    // Process message structure, as described by fuchsia_io::wire::WatchedEvent.
    auto event = static_cast<fuchsia_io::wire::WatchEvent>(*msg++);
    uint64_t len = *msg++;
    // Restrict the length to the remaining size of the buffer.
    if (size < (len + 2u)) {
      break;
    }

    // If the entry is valid, invoke the callback.
    if (event == fuchsia_io::wire::WatchEvent::kIdle) {  // we don't need to look for a filename
      callback_(event, "");
    } else {
      std::string filename(reinterpret_cast<char*>(msg), len);
      // "." is not a device, so ignore it.
      if (filename != ".") {
        callback_(event, filename);
      }
    }
    // The callback may have destroyed the ServiceWatcher, so make sure we still exist.
    if (weak_buf.expired()) {
      return;
    }
    msg += len;
    size -= len + 2;
  }

  wait_.Begin(dispatcher);
}

zx::result<> SyncDirectoryWatcher::Begin() {
  auto callback = fit::bind_member<&SyncDirectoryWatcher::OnWatchedEvent>(this);
  // Get the actual directory client depending on how this was initialized:
  if (path_ && dir_) {
    auto result = OpenDirectoryAt(*dir_, *path_);
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::make_result(
        directory_watcher_.Begin(result.value().borrow(), std::move(callback), loop_.dispatcher()));
  }
  // full path case:
  if (path_) {
    auto result = OpenDirectory(*path_);
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::make_result(
        directory_watcher_.Begin(result.value().borrow(), std::move(callback), loop_.dispatcher()));
  }
  // just directory case
  return zx::make_result(directory_watcher_.Begin(*dir_, std::move(callback), loop_.dispatcher()));
}

zx::result<std::string> SyncDirectoryWatcher::GetNextEntry(bool stop_at_idle, zx::time deadline) {
  if (!has_begun_iterating_) {
    auto result = Begin();
    if (result.is_error()) {
      return result.take_error();
    }
    has_begun_iterating_ = true;
  }
  // Run the loop to get the file events
  // Due to the nature of the fuchsia_io::Watcher protocol, one event for the service watcher may
  // correspond to multiple file events.  For this reason, we just run the loop until idle and
  // then process all the file events that we have received.
  zx_status_t run_status;
  do {
    run_status = loop_.RunUntilIdle();
    if (run_status != ZX_OK) {  // loop was cancelled or shutdown
      return zx::error(run_status);
    }
    // First get all the entries that were existing/added:
    if (!entries_.empty()) {
      std::string ret_instance = entries_.front();
      entries_.pop_front();
      return zx::ok(ret_instance);
    }
    // Once the queue is emptied, we can let the user know that the idle callback was invoked.
    if (stop_at_idle && idle_called_) {
      return zx::error(ZX_ERR_STOP);
    }
    // At this point, we are either in a race for the idle signal, or stop_at_idle == false,
    // and we are just waiting for any future file events.
    run_status = loop_.Run(deadline, true);
  } while (run_status == ZX_OK);
  // loop_.Run exited with a timeout or because it was canceled.
  return zx::error(run_status);
}

void SyncDirectoryWatcher::OnWatchedEvent(fuchsia_io::wire::WatchEvent event,
                                          const std::string& instance) {
  if (event == fuchsia_io::wire::WatchEvent::kIdle) {
    idle_called_ = true;
    return;
  }
  if (event == fuchsia_io::wire::WatchEvent::kRemoved || instance.size() < 2) {
    return;
  }
  entries_.push_back(instance);
}

}  // namespace component
