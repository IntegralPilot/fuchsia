// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <linux/audit.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>

#include "src/starnix/tests/selinux/userspace/netlink_util.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "netlink_xperms_policy"; }

namespace {

class NetlinkSocketXpermsTest
    : public ::testing::TestWithParam<netlink_util::NetlinkSocketTestCase> {
  void SetUp() override {
    constexpr char kNetlinkXpermPolicyCap[] = "netlink_xperm";
    // TODO(498907493): Remove once the tests always run on a kernel that supports netlink xperms.
    if (!IsPolicyCapSupported(kNetlinkXpermPolicyCap)) {
      GTEST_SKIP() << "netlink_xperm policy capability not supported by kernel";
      return;
    }
    ASSERT_TRUE(IsPolicyCapEnabled(kNetlinkXpermPolicyCap));
  }
};

INSTANTIATE_TEST_SUITE_P(
    NetlinkSocketTests, NetlinkSocketXpermsTest,
    ::testing::Values(
        // NETLINK_ROUTE xperms test cases
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getlink_yes_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getlink_no_xperms_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getlink_no_nlmsg_t", fit::error(EACCES)},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_no_getlink_t", fit::error(EACCES)},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getroute_yes_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getroute_no_xperms_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_getroute_no_nlmsg_t", fit::error(EACCES)},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_no_getroute_t", fit::error(EACCES)},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_NEWROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_newroute_yes_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_ROUTE, RTM_NEWROUTE, NLM_F_REQUEST | NLM_F_ACK,
                                            "nlmsg_xperms_newroute_no_t", fit::error(EACCES)},

        // NETLINK_AUDIT xperms test cases
        netlink_util::NetlinkSocketTestCase{NETLINK_AUDIT, AUDIT_SET, NLM_F_REQUEST,
                                            "nlmsg_xperms_audit_set_yes_t", fit::ok()},
        netlink_util::NetlinkSocketTestCase{NETLINK_AUDIT, AUDIT_SET, NLM_F_REQUEST,
                                            "nlmsg_xperms_audit_set_no_t", fit::error(EACCES)}

        ));

TEST_P(NetlinkSocketXpermsTest, CheckNetlinkMsgXperms) {
  const netlink_util::NetlinkSocketTestCase& test_case = GetParam();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:nlmsg_xperms_test_t:s0", [test_case]() {
    int result = netlink_util::SendNetlinkMsg(test_case);

    if (test_case.expected_result.is_ok()) {
      EXPECT_THAT(result, SyscallSucceeds());
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(test_case.expected_result.error_value()));
    }
  }));
}

}  // namespace
