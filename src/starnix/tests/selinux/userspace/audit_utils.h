// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SELINUX_USERSPACE_AUDIT_UTILS_H_
#define SRC_STARNIX_TESTS_SELINUX_USERSPACE_AUDIT_UTILS_H_

#include <lib/fit/result.h>
#include <unistd.h>

#include <fbl/unique_fd.h>

namespace audit_utils {

/// The `audit_status` structure in `audit.h` does not have the last
/// field, which makes it incompatible with Starnix.
/// The current sysroot version of `audit.h` is out of date.
struct audit_status {
  uint32_t mask;
  uint32_t enabled;
  uint32_t failure;
  uint32_t pid;
  uint32_t rate_limit;
  uint32_t backlog_limit;
  uint32_t lost;
  uint32_t backlog;
  union {
    uint32_t version;
    uint32_t feature_bitmap;
  };
  uint32_t backlog_wait_time;
  /// Needed for Starnix compatibility.
  uint32_t backlog_wait_time_actual;
};

}  // namespace audit_utils

fbl::unique_fd OpenNetlinkAuditSocket();

fit::result<int, ssize_t> SendNetlinkMessage(int fd, const void* buf, size_t len);

fit::result<int, ssize_t> ReceiveNetlinkMessage(int fd, void* buf, size_t len, bool wait = true);

fit::result<int> RegisterAsAuditDaemon(int fd, pid_t pid = getpid());

fit::result<int> UnregisterAuditDaemon(int fd);

fit::result<int> SendUserAuditMessage(int fd, int seq, const std::string& message_text,
                                      bool ack = true);

#endif  // SRC_STARNIX_TESTS_SELINUX_USERSPACE_AUDIT_UTILS_H_
