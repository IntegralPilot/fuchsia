// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SELINUX_USERSPACE_NETLINK_UTIL_H_
#define SRC_STARNIX_TESTS_SELINUX_USERSPACE_NETLINK_UTIL_H_

#include <lib/fit/result.h>
#include <unistd.h>

#include <string_view>

namespace netlink_util {
struct NetlinkSocketTestCase {
  int protocol;
  uint16_t message_type;
  uint16_t flags;
  std::string_view label_type;
  fit::result<int> expected_result;
};

int SendNetlinkMsg(const NetlinkSocketTestCase& test_case);

}  // namespace netlink_util
#endif  // SRC_STARNIX_TESTS_SELINUX_USERSPACE_NETLINK_UTIL_H_
