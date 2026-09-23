// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <poll.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>

#include <gtest/gtest.h>
#include <linux/qrtr.h>

namespace {

enum class ControlCommand {
  kBlockRead,
  kUnblockRead,
  kBlockWrite,
  kUnblockWrite,
};

std::string_view ControlCommandToString(ControlCommand cmd) {
  switch (cmd) {
    case ControlCommand::kBlockRead:
      return "BLOCK_READ";
    case ControlCommand::kUnblockRead:
      return "UNBLOCK_READ";
    case ControlCommand::kBlockWrite:
      return "BLOCK_WRITE";
    case ControlCommand::kUnblockWrite:
      return "UNBLOCK_WRITE";
  }
}

class QipcrtrClientTest : public ::testing::Test {
 protected:
  void SetUp() override {
    sock_ = socket(AF_QIPCRTR, SOCK_DGRAM, 0);
    ASSERT_GE(sock_, 0) << "socket() failed, errno=" << errno;
  }

  void TearDown() override {
    if (sock_ >= 0) {
      close(sock_);
    }
  }

  void SendControl(ControlCommand cmd) {
    sockaddr_qrtr addr = {};
    addr.sq_family = AF_QIPCRTR;
    addr.sq_node = 1;
    addr.sq_port = 9999;
    std::string_view cmd_str = ControlCommandToString(cmd);
    ASSERT_EQ(sendto(sock_, cmd_str.data(), cmd_str.size(), 0, reinterpret_cast<sockaddr*>(&addr),
                     sizeof(addr)),
              static_cast<ssize_t>(cmd_str.size()));
  }

  int sock_ = -1;
};

TEST_F(QipcrtrClientTest, GetSockName) {
  sockaddr_qrtr sockname;
  socklen_t len = sizeof sockname;
  ASSERT_EQ(getsockname(sock_, reinterpret_cast<sockaddr*>(&sockname), &len), 0)
      << "getsockname() failed, errno=" << errno;

  EXPECT_EQ(sockname.sq_family, AF_QIPCRTR);
  EXPECT_EQ(len, sizeof sockname);
  EXPECT_EQ(sockname.sq_node, 1u);
  EXPECT_EQ(sockname.sq_port, 10u);
}

TEST_F(QipcrtrClientTest, SndBuf) {
  int optval = 1024;
  socklen_t optlen = sizeof optval;
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_SNDBUF, &optval, optlen), 0)
      << "setsockopt(SO_SNDBUF) failed, errno=" << errno;

  optval = 0;
  ASSERT_EQ(getsockopt(sock_, SOL_SOCKET, SO_SNDBUF, &optval, &optlen), 0)
      << "getsockopt(SO_SNDBUF) failed, errno=" << errno;

  EXPECT_EQ(optlen, sizeof(int));
  EXPECT_EQ(optval, 2048);
}

TEST_F(QipcrtrClientTest, RcvBuf) {
  int optval = 1024;
  socklen_t optlen = sizeof optval;
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_RCVBUF, &optval, optlen), 0)
      << "setsockopt(SO_RCVBUF) failed, errno=" << errno;

  optval = 0;
  ASSERT_EQ(getsockopt(sock_, SOL_SOCKET, SO_RCVBUF, &optval, &optlen), 0)
      << "getsockopt(SO_RCVBUF) failed, errno=" << errno;

  EXPECT_EQ(optlen, sizeof(int));
  EXPECT_EQ(optval, 2048);
}

TEST_F(QipcrtrClientTest, SendTo) {
  char data[] = "send_data";
  size_t len = sizeof data;
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  addr.sq_node = 2;
  addr.sq_port = 20;
  socklen_t addrlen = sizeof addr;

  ASSERT_GE(sendto(sock_, data, len, 0, reinterpret_cast<sockaddr*>(&addr), addrlen), 0)
      << "sendto() failed, errno=" << errno;
}

TEST_F(QipcrtrClientTest, RecvFrom) {
  char data[10] = {0};
  size_t len = sizeof data;
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  socklen_t addrlen = sizeof addr;

  SendControl(ControlCommand::kUnblockRead);
  ssize_t ret = recvfrom(sock_, data, len, 0, reinterpret_cast<sockaddr*>(&addr), &addrlen);
  ASSERT_GE(ret, 0) << "recvfrom() failed, errno=" << errno;

  EXPECT_EQ(addrlen, sizeof addr);
  EXPECT_STREQ(data, "recv_data");
  EXPECT_EQ(addr.sq_node, 2u);
  EXPECT_EQ(addr.sq_port, 20u);
}

TEST_F(QipcrtrClientTest, Shutdown) {
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  addr.sq_node = 2;
  addr.sq_port = 20;
  socklen_t addrlen = sizeof addr;

  ASSERT_EQ(connect(sock_, reinterpret_cast<sockaddr*>(&addr), addrlen), 0)
      << "connect() failed, errno=" << errno;

  sockaddr_qrtr peer;
  socklen_t peerlen = sizeof peer;
  ASSERT_EQ(getpeername(sock_, reinterpret_cast<sockaddr*>(&peer), &peerlen), 0)
      << "getpeername() failed, errno=" << errno;
  EXPECT_EQ(peer.sq_node, 2u);
  EXPECT_EQ(peer.sq_port, 20u);

  ASSERT_EQ(shutdown(sock_, SHUT_RDWR), 0) << "shutdown() failed, errno=" << errno;

  // After shutdown, the peer should be cleared for QIPCRTR (as implemented in socket_qipcrtr.rs).
  // Actually, wait, looking at socket_qipcrtr.rs:
  // fn shutdown(...) { self.close(); Ok(()) }
  // fn close(...) { *self.inner.lock() = None; }
  // So it completely destroys the inner state.
  // getpeername calls connecting_lock() which creates a NEW inner state if None.
  // The NEW inner state has peer=None.
  // So getpeername should fail with ENOTCONN.
  ASSERT_EQ(getpeername(sock_, reinterpret_cast<sockaddr*>(&peer), &peerlen), -1);
  EXPECT_EQ(errno, ENOTCONN);

  // Send without destination should fail because there is no peer.
  char data[] = "test";
  ASSERT_EQ(send(sock_, data, sizeof(data), 0), -1);
  EXPECT_EQ(errno, EDESTADDRREQ);
}

TEST_F(QipcrtrClientTest, GetPeerName) {
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  addr.sq_node = 5;
  addr.sq_port = 50;
  socklen_t addrlen = sizeof addr;

  ASSERT_EQ(connect(sock_, reinterpret_cast<sockaddr*>(&addr), addrlen), 0)
      << "connect() failed, errno=" << errno;

  sockaddr_qrtr peer;
  socklen_t peerlen = sizeof peer;
  ASSERT_EQ(getpeername(sock_, reinterpret_cast<sockaddr*>(&peer), &peerlen), 0)
      << "getpeername() failed, errno=" << errno;

  EXPECT_EQ(peer.sq_family, AF_QIPCRTR);
  EXPECT_EQ(peer.sq_node, 5u);
  EXPECT_EQ(peer.sq_port, 50u);
}

TEST_F(QipcrtrClientTest, Poll) {
  struct pollfd pfd;
  pfd.fd = sock_;
  pfd.events = POLLIN | POLLOUT;

  // Unblock read signal first
  SendControl(ControlCommand::kUnblockRead);

  // The mock infrastructure should immediately signal readability and writability.
  int ret = poll(&pfd, 1, 0);
  ASSERT_GE(ret, 0) << "poll() failed, errno=" << errno;
  EXPECT_EQ(ret, 1);
  EXPECT_TRUE(pfd.revents & POLLIN);
  EXPECT_TRUE(pfd.revents & POLLOUT);
}

TEST_F(QipcrtrClientTest, Select) {
  fd_set readfds;
  fd_set writefds;
  FD_ZERO(&readfds);
  FD_ZERO(&writefds);
  FD_SET(sock_, &readfds);
  FD_SET(sock_, &writefds);

  struct timeval tv;
  tv.tv_sec = 0;
  tv.tv_usec = 0;

  SendControl(ControlCommand::kUnblockRead);

  int ret = select(sock_ + 1, &readfds, &writefds, nullptr, &tv);
  ASSERT_GE(ret, 0) << "select() failed, errno=" << errno;
  EXPECT_GE(ret, 1);
  EXPECT_TRUE(FD_ISSET(sock_, &readfds));
  EXPECT_TRUE(FD_ISSET(sock_, &writefds));
}

TEST_F(QipcrtrClientTest, InvalidSocketOptions) {
  int optval = 1;
  socklen_t optlen = sizeof(optval);
  // SO_DEBUG is not supported by socket_qipcrtr.rs (only SNDBUF/RCVBUF).
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_DEBUG, &optval, optlen), -1);
  EXPECT_EQ(errno, ENOSYS);

  ASSERT_EQ(getsockopt(sock_, SOL_SOCKET, SO_DEBUG, &optval, &optlen), -1);
  EXPECT_EQ(errno, ENOSYS);
}

TEST_F(QipcrtrClientTest, RecvFromDontWait) {
  char data[10] = {0};
  size_t len = sizeof data;
  sockaddr_qrtr addr;
  socklen_t addrlen = sizeof addr;

  SendControl(ControlCommand::kUnblockRead);

  ssize_t ret =
      recvfrom(sock_, data, len, MSG_DONTWAIT, reinterpret_cast<sockaddr*>(&addr), &addrlen);
  ASSERT_GE(ret, 0) << "recvfrom(MSG_DONTWAIT) failed, errno=" << errno;

  SendControl(ControlCommand::kBlockRead);

  // The second recvfrom should fail because the socket is not readable anymore.
  ret = recvfrom(sock_, data, len, MSG_DONTWAIT, reinterpret_cast<sockaddr*>(&addr), &addrlen);
  ASSERT_EQ(ret, -1) << "recvfrom(MSG_DONTWAIT) failed, errno=" << errno;
  EXPECT_EQ(errno, EAGAIN);

  EXPECT_EQ(addrlen, sizeof addr);
  EXPECT_STREQ(data, "recv_data");
}

TEST_F(QipcrtrClientTest, SendToDontWait) {
  char data[] = "send_data";
  size_t len = sizeof data;
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  addr.sq_node = 2;
  addr.sq_port = 20;
  socklen_t addrlen = sizeof addr;

  ASSERT_GE(sendto(sock_, data, len, MSG_DONTWAIT, reinterpret_cast<sockaddr*>(&addr), addrlen), 0)
      << "sendto(MSG_DONTWAIT) failed, errno=" << errno;

  SendControl(ControlCommand::kBlockWrite);

  // The second sendto should fail because the socket is not writable anymore.
  ASSERT_EQ(sendto(sock_, data, len, MSG_DONTWAIT, reinterpret_cast<sockaddr*>(&addr), addrlen), -1)
      << "sendto(MSG_DONTWAIT) failed, errno=" << errno;
  EXPECT_EQ(errno, EAGAIN);
}

TEST_F(QipcrtrClientTest, SockOptTimeout) {
  struct timeval tv = {.tv_sec = 1, .tv_usec = 0};
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)), 0)
      << "setsockopt(SO_RCVTIMEO) failed, errno=" << errno;

  struct timeval get_tv;
  socklen_t len = sizeof(get_tv);
  ASSERT_EQ(getsockopt(sock_, SOL_SOCKET, SO_RCVTIMEO, &get_tv, &len), 0)
      << "getsockopt(SO_RCVTIMEO) failed, errno=" << errno;
  EXPECT_EQ(len, sizeof(get_tv));
  EXPECT_EQ(get_tv.tv_sec, 1);
  EXPECT_EQ(get_tv.tv_usec, 0);

  tv = {.tv_sec = 2, .tv_usec = 0};
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv)), 0)
      << "setsockopt(SO_SNDTIMEO) failed, errno=" << errno;

  len = sizeof(get_tv);
  ASSERT_EQ(getsockopt(sock_, SOL_SOCKET, SO_SNDTIMEO, &get_tv, &len), 0)
      << "getsockopt(SO_SNDTIMEO) failed, errno=" << errno;
  EXPECT_EQ(get_tv.tv_sec, 2);
  EXPECT_EQ(get_tv.tv_usec, 0);
}

TEST_F(QipcrtrClientTest, SendTimeout) {
  struct timeval tv = {.tv_sec = 0, .tv_usec = 10};  // 10ms
  ASSERT_EQ(setsockopt(sock_, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv)), 0)
      << "setsockopt(SO_SNDTIMEO) failed, errno=" << errno;

  SendControl(ControlCommand::kBlockWrite);

  char data[10] = {0};
  sockaddr_qrtr addr;
  addr.sq_family = AF_QIPCRTR;
  addr.sq_node = 2;
  addr.sq_port = 20;

  ssize_t ret =
      sendto(sock_, data, sizeof(data), 0, reinterpret_cast<sockaddr*>(&addr), sizeof(addr));
  ASSERT_EQ(ret, -1);
  EXPECT_EQ(errno, EAGAIN);
}

}  // namespace

int main(int argc, char** argv) {
  ::testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
