// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_POSIX_DIR_READER_LINUX_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_POSIX_DIR_READER_LINUX_H_

#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

#include "src/developer/debug/debug_agent/posix/eintr_wrapper.h"

namespace linux {

struct linux_dirent {
  uint64_t d_ino;
  int64_t d_off;
  unsigned short d_reclen;
  unsigned char d_type;
  char d_name[0];
};

// Allows opening and reading from directories on Linux without allocating memory. Used for
// processing directories during the delicate forking operation which requires no memory allocs.
class DirReaderLinux {
 public:
  explicit DirReaderLinux(const char* directory_path)
      : fd_(open(directory_path, O_RDONLY | O_DIRECTORY)), offset_(0), size_(0) {
    memset(buf_, 0, sizeof(buf_));
  }

  DirReaderLinux(const DirReaderLinux&) = delete;
  DirReaderLinux& operator=(const DirReaderLinux&) = delete;

  ~DirReaderLinux() {
    if (fd_ >= 0) {
      IGNORE_EINTR(close(fd_));
    }
  }

  bool IsValid() const { return fd_ >= 0; }

  // Move to the next entry returning false if the iteration is complete.
  bool Next() {
    if (size_) {
      linux_dirent* dirent = reinterpret_cast<linux_dirent*>(&buf_[offset_]);
      offset_ += dirent->d_reclen;
    }

    if (offset_ != size_)
      return true;

    const long r = syscall(__NR_getdents64, fd_, buf_, sizeof(buf_));
    if (r == 0)
      return false;
    if (r < 0) {
      return false;
    }
    size_ = static_cast<size_t>(r);
    offset_ = 0;
    return true;
  }

  const char* name() const {
    if (!size_)
      return nullptr;

    const linux_dirent* dirent = reinterpret_cast<const linux_dirent*>(&buf_[offset_]);
    return dirent->d_name;
  }

  int fd() const { return fd_; }

  static bool IsFallback() { return false; }

 private:
  const int fd_;
  alignas(linux_dirent) unsigned char buf_[512];
  size_t offset_;
  size_t size_;
};

}  // namespace linux

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_POSIX_DIR_READER_LINUX_H_
