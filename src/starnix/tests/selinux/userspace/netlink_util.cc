// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/selinux/userspace/netlink_util.h"

#include <netinet/in.h>
#include <sys/socket.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/audit.h>
#include <linux/genetlink.h>
#include <linux/inet_diag.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <linux/sock_diag.h>
#include <linux/xfrm.h>

#include "src/starnix/tests/selinux/userspace/util.h"

namespace netlink_util {

int SendNetlinkMsg(const netlink_util::NetlinkSocketTestCase& test_case) {
  auto sockcreate = ScopedTaskAttrResetter::SetTaskAttr(
      "sockcreate", MakeTestSecurityContext(test_case.label_type));
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd sock_fd(socket(AF_NETLINK, SOCK_RAW, test_case.protocol));
  if (!sock_fd.is_valid()) {
    ADD_FAILURE() << "Creating socket failed with error:" << strerror(errno);
    return errno;
  }
  union {
    struct rtmsg rtmsg;
    struct audit_status audit_status;
    struct inet_diag_req_v2 diag_req;
    struct xfrm_usersa_info xfrm_sa;
    struct genlmsghdr genl_hdr;
  } payload;
  memset(&payload, 0, sizeof(payload));

  char buffer[NLMSG_ALIGN(sizeof(struct nlmsghdr) + sizeof(payload))];
  memset(buffer, 0, sizeof(buffer));

  struct nlmsghdr* nlh = (struct nlmsghdr*)buffer;
  nlh->nlmsg_type = test_case.message_type;
  nlh->nlmsg_flags = test_case.flags;
  nlh->nlmsg_pid = getpid();
  nlh->nlmsg_seq = 1;

  size_t payload_len = 0;
  if (test_case.protocol == NETLINK_AUDIT) {
    payload_len = sizeof(struct audit_status);
    nlh->nlmsg_len = NLMSG_LENGTH((__u32)payload_len);
    struct audit_status* audit_status = (struct audit_status*)NLMSG_DATA(nlh);
    audit_status->mask = AUDIT_STATUS_ENABLED | AUDIT_STATUS_PID;
    audit_status->enabled = 1;
    audit_status->pid = getpid();
  } else if (test_case.protocol == NETLINK_SOCK_DIAG) {
    payload_len = sizeof(struct inet_diag_req_v2);
    nlh->nlmsg_len = NLMSG_LENGTH((__u32)payload_len);
    struct inet_diag_req_v2* inet_req = (struct inet_diag_req_v2*)NLMSG_DATA(nlh);
    inet_req->sdiag_family = AF_INET;
    inet_req->sdiag_protocol = IPPROTO_UDP;
  } else if (test_case.protocol == NETLINK_XFRM) {
    payload_len = sizeof(struct xfrm_usersa_info);
    nlh->nlmsg_len = NLMSG_LENGTH((__u32)payload_len);
    struct xfrm_usersa_info* xfrm_sa = (struct xfrm_usersa_info*)NLMSG_DATA(nlh);
    xfrm_sa->family = AF_INET;
  } else if (test_case.protocol == NETLINK_GENERIC) {
    struct genlmsghdr* genl_hdr = (struct genlmsghdr*)NLMSG_DATA(nlh);
    genl_hdr->cmd = CTRL_CMD_GETFAMILY;
    genl_hdr->version = 1;
    genl_hdr->reserved = 0;

    const char family_name[] = "nlctrl";
    struct nlattr* nla = (struct nlattr*)((char*)genl_hdr + sizeof(struct genlmsghdr));
    nla->nla_type = CTRL_ATTR_FAMILY_NAME;
    nla->nla_len = sizeof(struct nlattr) + sizeof(family_name);
    memcpy((char*)nla + sizeof(struct nlattr), family_name, sizeof(family_name));
    payload_len = sizeof(struct genlmsghdr) + NLA_ALIGN(nla->nla_len);
    nlh->nlmsg_len = NLMSG_LENGTH((__u32)payload_len);
  } else {
    payload_len = sizeof(struct rtmsg);
    nlh->nlmsg_len = NLMSG_LENGTH((__u32)payload_len);
    struct rtmsg* rt_msg = (struct rtmsg*)NLMSG_DATA(nlh);
    rt_msg->rtm_family = AF_INET;
    rt_msg->rtm_table = RT_TABLE_MAIN;
    rt_msg->rtm_protocol = RTPROT_STATIC;
    rt_msg->rtm_scope = RT_SCOPE_UNIVERSE;
    rt_msg->rtm_type = RTN_UNICAST;
  }

  struct sockaddr_nl sa;
  memset(&sa, 0, sizeof(sa));
  sa.nl_family = AF_NETLINK;

  int ret = (int)sendto(sock_fd.get(), nlh, nlh->nlmsg_len, 0, (struct sockaddr*)&sa, sizeof(sa));
  if (ret < 0) {
    return ret;
  }

  char recv_buffer[4096];
  struct sockaddr_nl nladdr;
  struct iovec iov = {recv_buffer, sizeof(recv_buffer)};
  struct msghdr msg = {&nladdr, sizeof(nladdr), &iov, 1, NULL, 0, 0};

  // Await for reply to synchronize after send which checks capabilities asynchronously in Starnix.
  ret = (int)recvmsg(sock_fd.get(), &msg, 0);
  if (ret < 0) {
    ADD_FAILURE() << "recvmsg failed with error: " << strerror(errno);
    return ret;
  }

  return 0;
}

}  // namespace netlink_util
