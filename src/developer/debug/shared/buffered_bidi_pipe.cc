// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/shared/buffered_bidi_pipe.h"

#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <algorithm>

namespace debug {

BufferedBidiPipe::BufferedBidiPipe() = default;

BufferedBidiPipe::BufferedBidiPipe(fbl::unique_fd read_fd, fbl::unique_fd write_fd)
    : read_fd_(std::move(read_fd)), write_fd_(std::move(write_fd)) {
  FX_DCHECK(read_fd_.is_valid());
  FX_DCHECK(write_fd_.is_valid());
}

BufferedBidiPipe::~BufferedBidiPipe() = default;

bool BufferedBidiPipe::Start() {
  if (!IsValid())
    return false;

  // Register for socket updates from the message loop. Here we assume we're in a writable state
  // already (this will be re-evaluated when we actually try to write) so only need to watch for
  // readable.
  MessageLoop* loop = MessageLoop::Current();
  FX_DCHECK(loop);
  read_watch_handle_ = loop->WatchFD(
      MessageLoop::WatchMode::kRead, read_fd_.get(),
      [this](int fd, bool read, bool write, bool err) { OnFDReady(fd, read, write, err); });
  return read_watch_handle_.watching();
}

bool BufferedBidiPipe::Stop() {
  if (!IsValid() || read_watch_handle_.watching())
    return false;
  read_watch_handle_ = MessageLoop::WatchHandle();
  write_watch_handle_ = MessageLoop::WatchHandle();
  return true;
}

void BufferedBidiPipe::ResetInternal() {
  // The watch must be disabled before the file descriptor is reset.
  read_watch_handle_.StopWatching();
  write_watch_handle_.StopWatching();
  read_fd_.reset();
  write_fd_.reset();
}

void BufferedBidiPipe::OnFDReady(int fd, bool readable, bool writable, bool err) {
  if (writable) {
    // If we get a writable notifications, stop watching. We'll notify the stream that it can write
    // nad will re-register for write notifications if that blocks.
    write_watch_handle_ = MessageLoop::WatchHandle();
    stream().SetWritable();
  }

  if (readable) {
    // Messages from the client to the agent are typically small so we don't
    // need a very large buffer.
    constexpr size_t kBufSize = 1024;

    // Add all available data to the socket buffer.
    while (true) {
      std::vector<char> buffer;
      buffer.resize(kBufSize);

      ssize_t num_read = read(read_fd_.get(), buffer.data(), kBufSize);
      if (num_read == 0) {
        // We asked for data and it had none. Since this assumes async input,
        // that means EOF (otherwise it will return -1 and errno will be
        // EAGAIN).
        OnFDError();
        return;
      } else if (num_read == -1) {
        if (errno == EAGAIN) {
          // No data now.
          break;
        } else if (errno == EINTR) {
          // Try again.
          continue;
        } else {
          // Unrecoverable.
          //
          OnFDError();
          return;
        }
      } else if (num_read > 0) {
        buffer.resize(num_read);
        stream().AddReadData(std::move(buffer));
      } else {
        break;
      }
      // TODO(brettw) it would be nice to yield here after reading "a bunch" of
      // data so this pipe doesn't starve the entire app.
    }

    if (callback())
      callback()();
  }

  if (err) {
    OnFDError();
  }
}

void BufferedBidiPipe::OnFDError() {
  read_watch_handle_ = MessageLoop::WatchHandle();
  write_watch_handle_ = MessageLoop::WatchHandle();
  read_fd_.reset();
  write_fd_.reset();
  if (error_callback())
    error_callback()();
}

size_t BufferedBidiPipe::ConsumeStreamBufferData(const char* data, size_t len) {
  // Loop for handling EINTR.
  ssize_t written;
  while (true) {
    written = write(write_fd_.get(), data, len);
    if (written == 0) {
      // We asked for data and it had none. Since this assumes async input,
      // that means EOF (otherwise it will return -1 and errno will be EAGAIN).
      OnFDError();
      return 0;
    } else if (written == -1) {
      if (errno == EAGAIN) {
        // Can't write data, fall through to partial write case below.
        written = 0;
      } else if (errno == EINTR) {
        // Try write again.
        continue;
      } else {
        // Unrecoverable.
        OnFDError();
        return 0;
      }
    }
    break;
  }

  if (written < static_cast<ssize_t>(len)) {
    // Partial write, register for updates.
    write_watch_handle_ = MessageLoop::Current()->WatchFD(
        MessageLoop::WatchMode::kWrite, write_fd_.get(),
        [this](int fd, bool read, bool write, bool err) { OnFDReady(fd, read, write, err); });
  }
  return written;
}

}  // namespace debug
