// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <net/ethernet.h>
#include <netinet/icmp6.h>
#include <netinet/in.h>
#include <netinet/ip_icmp.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>

#include <fstream>
#include <optional>
#include <set>
#include <thread>

#include <asm-generic/socket.h>
#include <fbl/unaligned.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/bpf.h>
#include <linux/capability.h>
#include <linux/filter.h>
#include <linux/if_ether.h>
#include <linux/if_packet.h>
#include <linux/ipv6.h>
#include <linux/netfilter/x_tables.h>
#include <linux/rtnetlink.h>

#include "fault_test.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "test_helper.h"

#if !defined(__NR_memfd_create)
#if defined(__x86_64__)
#define __NR_memfd_create 319
#elif defined(__i386__)
#define __NR_memfd_create 356
#elif defined(__aarch64__)
#define __NR_memfd_create 279
#elif defined(__arm__)
#define __NR_memfd_create 385
#endif
#endif  // !defined(__NR_memfd_create)

// `IPT_SO_GET_REVISION_TARGET` is defined in `linux/netfilter_ipv4/ip_tables.h`,
// but that header is not usable with `clang` due to broken pointer math in
// `ipt_get_target()`. Bionic's version of that header is usable.
#if defined(__BIONIC__)
#include <linux/netfilter_ipv4/ip_tables.h>
#else
#define IPT_BASE_CTL 64
#define IPT_SO_GET_REVISION_TARGET (IPT_BASE_CTL + 3)
#endif

namespace {

class ParameterizedUnixSocketTest : public testing::TestWithParam<int> {
 protected:
  int GetSocketType() const { return GetParam(); }
};

INSTANTIATE_TEST_SUITE_P(
    UnixSocket, ParameterizedUnixSocketTest, testing::Values(SOCK_STREAM, SOCK_DGRAM),
    [](const testing::TestParamInfo<ParameterizedUnixSocketTest::ParamType>& info) {
      return info.param == SOCK_STREAM ? "SOCK_STREAM" : "SOCK_DGRAM";
    });

TEST(UnixSocket, ReadAfterClose) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));
  ASSERT_EQ(1, write(fds[0], "0", 1));
  ASSERT_EQ(0, close(fds[0]));
  char buf[1];
  ASSERT_EQ(1, read(fds[1], buf, 1));
  ASSERT_EQ('0', buf[0]);
  ASSERT_EQ(0, read(fds[1], buf, 1));
  close(fds[1]);
}

TEST(UnixSocket, ReadAfterCloseDgram) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_DGRAM, 0, fds));
  int flags = fcntl(fds[1], F_GETFL, 0);
  ASSERT_EQ(0, fcntl(fds[1], F_SETFL, flags | O_NONBLOCK));

  ASSERT_EQ(1, write(fds[0], "0", 1));
  ASSERT_EQ(0, close(fds[0]));
  char buf[1];
  ASSERT_EQ(1, read(fds[1], buf, 1));
  ASSERT_EQ('0', buf[0]);
  ASSERT_THAT(read(fds[1], buf, 1), SyscallFailsWithErrno(EAGAIN));
  close(fds[1]);
}

TEST_P(ParameterizedUnixSocketTest, ReadAfterReadShutdown) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, GetSocketType(), 0, fds));
  ASSERT_EQ(1, write(fds[0], "0", 1));
  ASSERT_EQ(0, shutdown(fds[1], SHUT_RD));
  char buf[1];
  ASSERT_EQ(1, read(fds[1], buf, 1));
  ASSERT_EQ('0', buf[0]);
  ASSERT_EQ(0, read(fds[1], buf, 1));
  close(fds[0]);
  close(fds[1]);
}

TEST_P(ParameterizedUnixSocketTest, PeerWriteAfterReadShutdown) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, GetSocketType(), 0, fds));
  ASSERT_EQ(0, shutdown(fds[1], SHUT_RD));

  auto old_handler = signal(SIGPIPE, SIG_IGN);
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(EPIPE));
  signal(SIGPIPE, old_handler);

  close(fds[0]);
  close(fds[1]);
}

TEST(UnixSocket, PeerWriteAfterCloseStream) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));
  close(fds[1]);

  auto old_handler = signal(SIGPIPE, SIG_IGN);
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(EPIPE));
  signal(SIGPIPE, old_handler);

  close(fds[0]);
}

TEST(UnixSocket, PeerWriteAfterCloseDgram) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_DGRAM, 0, fds));
  close(fds[1]);

  auto old_handler = signal(SIGPIPE, SIG_IGN);
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(ECONNREFUSED));
  signal(SIGPIPE, old_handler);

  close(fds[0]);
}

TEST(UnixSocket, PeerWriteAfterCloseDgramTwice) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_DGRAM, 0, fds));
  close(fds[1]);

  auto old_handler = signal(SIGPIPE, SIG_IGN);
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(ECONNREFUSED));

  // Second write should return ENOTCONN because the socket was disconnected
  // by the first failed write.
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(ENOTCONN));
  signal(SIGPIPE, old_handler);

  close(fds[0]);
}

TEST(UnixSocket, PeerWriteAfterShutdownAndCloseDgram) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_DGRAM, 0, fds));
  ASSERT_EQ(0, shutdown(fds[1], SHUT_RD));
  close(fds[1]);

  auto old_handler = signal(SIGPIPE, SIG_IGN);
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(ECONNREFUSED));
  ASSERT_THAT(write(fds[0], "0", 1), SyscallFailsWithErrno(ENOTCONN));
  signal(SIGPIPE, old_handler);

  close(fds[0]);
}

TEST(UnixSocket, UnconnectedShutdownStream) {
  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  ASSERT_LT(-1, fd);
  ASSERT_EQ(0, shutdown(fd, SHUT_RDWR));
  close(fd);
}

TEST(UnixSocket, UnconnectedShutdownDgram) {
  int fd = socket(AF_UNIX, SOCK_DGRAM, 0);
  ASSERT_LT(-1, fd);
  ASSERT_EQ(0, shutdown(fd, SHUT_RDWR));
  close(fd);
}

TEST(UnixSocket, DatagramDisconnectAfUnspec) {
  fbl::unique_fd sock1;
  ASSERT_TRUE(sock1 = fbl::unique_fd(socket(AF_UNIX, SOCK_DGRAM, 0))) << strerror(errno);

  fbl::unique_fd sock2;
  ASSERT_TRUE(sock2 = fbl::unique_fd(socket(AF_UNIX, SOCK_DGRAM, 0))) << strerror(errno);

  struct sockaddr_un addr2 = {.sun_family = AF_UNIX};
  const char name[] = "\0test_unix_dgram_unspec";
  memcpy(addr2.sun_path, name, sizeof(name));
  ASSERT_THAT(bind(sock2.get(), reinterpret_cast<struct sockaddr*>(&addr2), sizeof(addr2)),
              SyscallSucceeds());

  // Connect sock1 to sock2.
  ASSERT_THAT(connect(sock1.get(), reinterpret_cast<struct sockaddr*>(&addr2), sizeof(addr2)),
              SyscallSucceeds());

  // getpeername should succeed while connected.
  struct sockaddr_un peer_addr = {};
  socklen_t addrlen = sizeof(peer_addr);
  ASSERT_THAT(getpeername(sock1.get(), reinterpret_cast<struct sockaddr*>(&peer_addr), &addrlen),
              SyscallSucceeds());

  // Disconnect sock1 using AF_UNSPEC.
  struct sockaddr unspec_addr = {.sa_family = AF_UNSPEC};
  ASSERT_THAT(connect(sock1.get(), &unspec_addr, sizeof(unspec_addr)), SyscallSucceeds());

  // getpeername should now fail with ENOTCONN.
  ASSERT_THAT(getpeername(sock1.get(), reinterpret_cast<struct sockaddr*>(&peer_addr), &addrlen),
              SyscallFailsWithErrno(ENOTCONN));
}

TEST(UnixSocket, HupEvent) {
  int fds[2];

  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  int epfd = epoll_create1(0);
  ASSERT_LT(-1, epfd);
  epoll_event ev = {EPOLLIN, {.u64 = 42}};
  ASSERT_EQ(0, epoll_ctl(epfd, EPOLL_CTL_ADD, fds[0], &ev));

  epoll_event outev = {0, {.u64 = 0}};

  int no_ready = epoll_wait(epfd, &outev, 1, 0);
  ASSERT_EQ(0, no_ready);

  close(fds[1]);

  no_ready = epoll_wait(epfd, &outev, 1, 0);
  ASSERT_EQ(1, no_ready);
  ASSERT_EQ(EPOLLIN | EPOLLHUP, outev.events);
  ASSERT_EQ(42ul, fbl::UnalignedLoad<uint64_t>(&outev.data.u64));

  close(fds[0]);
  close(epfd);
}

// Tests the behavior of a unix domain socket after it connects to a peer and that peer is closed.
TEST(UnixSocket, PeerNameAfterPeerClosed) {
  char* tmp = getenv("TEST_TMPDIR");
  auto socket_path =
      tmp == nullptr ? "/tmp/socktest_stream_hup" : std::string(tmp) + "/socktest_stream_hup";
  struct sockaddr_un sun;
  sun.sun_family = AF_UNIX;
  strcpy(sun.sun_path, socket_path.c_str());
  struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&sun);

  fbl::unique_fd server(SAFE_SYSCALL(socket(AF_UNIX, SOCK_STREAM, 0)));
  SAFE_SYSCALL(bind(server.get(), addr, sizeof(sun)));
  auto cleanup_sockpath = fit::defer([&socket_path]() { unlink(socket_path.c_str()); });

  SAFE_SYSCALL(listen(server.get(), 1));

  fbl::unique_fd client(SAFE_SYSCALL(socket(AF_UNIX, SOCK_STREAM, 0)));
  SAFE_SYSCALL(connect(client.get(), addr, sizeof(sun)));

  fbl::unique_fd accepted(SAFE_SYSCALL(accept(server.get(), nullptr, nullptr)));

  // getpeername() on client while connected.
  struct sockaddr_un peer_name{};
  socklen_t peer_name_len = sizeof(peer_name);
  SAFE_SYSCALL(
      getpeername(client.get(), reinterpret_cast<struct sockaddr*>(&peer_name), &peer_name_len));

  EXPECT_GT(peer_name_len, sizeof(sa_family_t));
  EXPECT_STREQ(sun.sun_path, peer_name.sun_path);

  peer_name = {};

  accepted.reset();

  // Even though the peer of |client| is now closed, getpeername() on client still returns the
  // address that the connection used to have.
  EXPECT_EQ(
      0, getpeername(client.get(), reinterpret_cast<struct sockaddr*>(&peer_name), &peer_name_len));
  EXPECT_GT(peer_name_len, sizeof(sa_family_t));
  EXPECT_STREQ(sun.sun_path, peer_name.sun_path);
}

TEST(UnixSocket, ReadConnReset) {
  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  ASSERT_EQ(1, write(fds[1], "A", 1));
  ASSERT_EQ(1, write(fds[0], "B", 1));
  ASSERT_EQ(0, close(fds[0]));
  char buf[1];
  ASSERT_EQ(1, read(fds[1], buf, 1));
  ASSERT_EQ('B', buf[0]);
  ASSERT_EQ(-1, read(fds[1], buf, 1));
  EXPECT_EQ(errno, ECONNRESET);
}

TEST(UnixSocket, ReadConnResetEmptyBuffer) {
  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  ASSERT_EQ(1, write(fds[1], "A", 1));
  ASSERT_EQ(0, close(fds[0]));
  char buf[1];
  ASSERT_EQ(0, read(fds[1], buf, 0));
}

TEST(UnixSocket, ReadConnResetAndShutdown) {
  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  ASSERT_EQ(1, write(fds[1], "A", 1));
  ASSERT_EQ(1, write(fds[0], "B", 1));
  ASSERT_EQ(0, close(fds[0]));
  ASSERT_EQ(0, shutdown(fds[1], SHUT_RD));

  char buf[1];
  ASSERT_EQ(1, read(fds[1], buf, 1));
  ASSERT_EQ('B', buf[0]);
  ASSERT_EQ(-1, read(fds[1], buf, 1));
  EXPECT_EQ(errno, ECONNRESET);
}

struct read_info_spec {
  unsigned char* mem;
  size_t length;
  size_t bytes_read;
  int fd;
};

void* reader(void* arg) {
  read_info_spec* read_info = reinterpret_cast<read_info_spec*>(arg);
  while (read_info->bytes_read < read_info->length) {
    size_t to_read = read_info->length - read_info->bytes_read;
    fflush(stdout);
    ssize_t bytes_read = read(read_info->fd, read_info->mem + read_info->bytes_read, to_read);
    EXPECT_LT(-1, bytes_read) << strerror(errno);
    if (bytes_read < 0) {
      return nullptr;
    }
    read_info->bytes_read += bytes_read;
  }
  return nullptr;
}

TEST(UnixSocket, BigWrite) {
  const size_t write_size = 300000;
  unsigned char* send_mem = new unsigned char[write_size];
  ASSERT_TRUE(send_mem != nullptr);

  for (size_t i = 0; i < write_size; i++) {
    send_mem[i] = 0xff & random();
  }

  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds)) << strerror(errno);

  read_info_spec read_info;
  read_info.mem = new unsigned char[write_size];
  bzero(read_info.mem, sizeof(unsigned char) * write_size);
  ASSERT_TRUE(read_info.mem != nullptr);
  read_info.length = write_size;
  read_info.fd = fds[1];
  read_info.bytes_read = 0;

  pthread_t read_thread;
  ASSERT_EQ(0, pthread_create(&read_thread, nullptr, reader, &read_info));
  size_t write_count = 0;
  while (write_count < write_size) {
    size_t to_send = write_size - write_count;
    ssize_t bytes_read = write(fds[0], send_mem + write_count, to_send);
    ASSERT_LT(-1, bytes_read) << strerror(errno);
    write_count += bytes_read;
  }

  ASSERT_EQ(0, pthread_join(read_thread, nullptr)) << strerror(errno);

  close(fds[0]);
  close(fds[1]);

  ASSERT_EQ(write_count, read_info.bytes_read);
  ASSERT_EQ(0, memcmp(send_mem, read_info.mem, sizeof(unsigned char) * write_size));

  delete[] send_mem;
  delete[] read_info.mem;
}

TEST(UnixSocket, SeqPacket) {
  char* tmp = getenv("TEST_TMPDIR");
  auto socket_path =
      tmp == nullptr ? "/tmp/socktest_seqpacket" : std::string(tmp) + "/socktest_seqpacket";
  struct sockaddr_un sun;
  sun.sun_family = AF_UNIX;
  strcpy(sun.sun_path, socket_path.c_str());
  struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&sun);

  auto server = socket(AF_UNIX, SOCK_SEQPACKET, 0);
  ASSERT_GE(server, 0);
  ASSERT_EQ(bind(server, addr, sizeof(sun)), 0);
  ASSERT_EQ(listen(server, 1), 0);
  int pipes[2];
  ASSERT_EQ(pipe(pipes), 0);
  pid_t pid = fork();
  if (pid == 0) {
    // Child
    auto client = socket(AF_UNIX, SOCK_SEQPACKET, 0);
    if (client < 0) {
      exit(1);
    }
    if (connect(client, addr, sizeof(sun)) < 0) {
      exit(1);
    }
    const char* message = "hello";
    if (send(client, message, strlen(message), 0) < 0) {
      exit(1);
    }
    char data;
    read(pipes[0], &data, 1);
    close(pipes[0]);
    close(client);
    exit(0);
  }

  // Parent
  auto accepted_fd = accept(server, nullptr, nullptr);
  ASSERT_GE(accepted_fd, 0);
  char byte = 0;
  write(accepted_fd, &byte, 1);

  char buf[1024];
  ssize_t bytes_read = recv(accepted_fd, buf, sizeof(buf), 0);
  ASSERT_EQ(bytes_read, 5);
  ASSERT_EQ(memcmp(buf, "hello", 5), 0);

  char buffer[256];
  struct iovec iov[1];
  iov[0].iov_base = buffer;
  iov[0].iov_len = sizeof(buffer);

  struct msghdr msg;
  memset(&msg, 0, sizeof(msg));
  msg.msg_iov = iov;
  msg.msg_iovlen = 1;
  ASSERT_EQ(write(pipes[1], buffer, 1), 1);
  int status;
  ASSERT_EQ(waitpid(pid, &status, 0), pid);
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  // recvmsg should return ECONNRESET when the peer closes the connection
  // followed by 0 on the subsequent read.
  ssize_t n = recvmsg(accepted_fd, &msg, MSG_CMSG_CLOEXEC);
  ASSERT_EQ(errno, ECONNRESET);
  ASSERT_EQ(n, -1);
  n = recvmsg(accepted_fd, &msg, MSG_CMSG_CLOEXEC);
  ASSERT_EQ(n, 0);

  ASSERT_EQ(unlink(socket_path.c_str()), 0);
  ASSERT_EQ(close(accepted_fd), 0);
  ASSERT_EQ(close(server), 0);
  close(pipes[1]);
}

TEST(UnixSocket, ConnectZeroBacklog) {
  char* tmp = getenv("TEST_TMPDIR");
  auto socket_path =
      tmp == nullptr ? "/tmp/socktest_connect" : std::string(tmp) + "/socktest_connect";
  struct sockaddr_un sun;
  sun.sun_family = AF_UNIX;
  strcpy(sun.sun_path, socket_path.c_str());
  struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&sun);

  auto server = socket(AF_UNIX, SOCK_STREAM, 0);
  ASSERT_EQ(bind(server, addr, sizeof(sun)), 0);
  ASSERT_EQ(listen(server, 0), 0);

  auto client = socket(AF_UNIX, SOCK_STREAM, 0);
  ASSERT_GT(client, -1);
  ASSERT_EQ(connect(client, addr, sizeof(sun)), 0);

  ASSERT_EQ(unlink(socket_path.c_str()), 0);
  ASSERT_EQ(close(client), 0);
  ASSERT_EQ(close(server), 0);
}

TEST(UnixSocket, ConnectLargeSize) {
  struct sockaddr_un sun;
  sun.sun_family = AF_UNIX;
  strcpy(sun.sun_path, "/bogus/path/value");
  struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&sun);

  auto client = socket(AF_UNIX, SOCK_STREAM, 0);
  ASSERT_GT(client, -1);
  ASSERT_EQ(connect(client, addr, sizeof(struct sockaddr_un) + 1), -1);
  EXPECT_EQ(errno, EINVAL);
}

TEST(InetSocket, ConnectLargeSize) {
  struct sockaddr_in in;
  in.sin_family = AF_INET;
  struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&in);

  auto client = socket(AF_INET, SOCK_STREAM, 0);
  ASSERT_GT(client, -1);
  ASSERT_EQ(connect(client, addr, sizeof(struct sockaddr_storage) + 1), -1);
  EXPECT_EQ(errno, EINVAL);
}

class UnixSocketTest : public testing::Test {
  // SetUp() - make socket
 protected:
  void SetUp() override {
    char* tmp = getenv("TEST_TMPDIR");
    socket_path_ = tmp == nullptr ? "/tmp/socktest" : std::string(tmp) + "/socktest";
    struct sockaddr_un sun;
    sun.sun_family = AF_UNIX;
    strcpy(sun.sun_path, socket_path_.c_str());
    struct sockaddr* addr = reinterpret_cast<struct sockaddr*>(&sun);

    server_ = socket(AF_UNIX, SOCK_STREAM, 0);
    ASSERT_GT(server_, -1);
    ASSERT_EQ(bind(server_, addr, sizeof(sun)), 0);
    ASSERT_EQ(listen(server_, 1), 0);

    client_ = socket(AF_UNIX, SOCK_STREAM, 0);
    ASSERT_GT(client_, -1);
    ASSERT_EQ(connect(client_, addr, sizeof(sun)), 0);
  }

  void TearDown() override {
    ASSERT_EQ(unlink(socket_path_.c_str()), 0);
    ASSERT_EQ(close(client_), 0);
    ASSERT_EQ(close(server_), 0);
  }

  int client() const { return client_; }

 private:
  int client_ = 0;
  int server_ = 0;
  std::string socket_path_;
};

TEST_F(UnixSocketTest, ImmediatePeercredCheck) {
  struct ucred cred;
  socklen_t cred_size = sizeof(cred);
  ASSERT_EQ(getsockopt(client(), SOL_SOCKET, SO_PEERCRED, &cred, &cred_size), 0);
  ASSERT_NE(cred.pid, 0);
  ASSERT_EQ(cred.uid, getuid());
  ASSERT_EQ(cred.gid, getgid());
}

TEST(UnixSocket, SendZeroFds) {
  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  char data[] = "a";
  struct iovec iov[] = {{
      .iov_base = data,
      .iov_len = 1,
  }};
  char buf[CMSG_SPACE(0)];
  struct msghdr msg = {
      .msg_iov = iov,
      .msg_iovlen = 1,
      .msg_control = buf,
      .msg_controllen = sizeof(buf),
  };
  *CMSG_FIRSTHDR(&msg) = (struct cmsghdr){
      .cmsg_len = CMSG_LEN(0),
      .cmsg_level = SOL_SOCKET,
      .cmsg_type = SCM_RIGHTS,
  };
  ASSERT_EQ(sendmsg(fds[0], &msg, 0), 1);

  memset(data, 0, sizeof(data));
  memset(buf, 0, sizeof(buf));
  ASSERT_EQ(recvmsg(fds[1], &msg, 0), 1);
  EXPECT_EQ(data[0], 'a');
  EXPECT_EQ(msg.msg_controllen, 0u);
  EXPECT_EQ(msg.msg_flags, 0);
}

#if defined(__NR_memfd_create)
TEST(UnixSocket, SendMemFd) {
  int fds[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, fds));

  int memfd = static_cast<int>(syscall(__NR_memfd_create, "test_memfd", 0));

  char data[] = "";
  struct iovec iov[] = {{
      .iov_base = data,
      .iov_len = 1,
  }};
  char buf[CMSG_SPACE(sizeof(int))];
  struct msghdr msg = {
      .msg_iov = iov,
      .msg_iovlen = 1,
      .msg_control = buf,
      .msg_controllen = sizeof(buf),
  };
  struct cmsghdr* cmsg = CMSG_FIRSTHDR(&msg);
  *cmsg = (struct cmsghdr){
      .cmsg_len = CMSG_LEN(sizeof(int)),
      .cmsg_level = SOL_SOCKET,
      .cmsg_type = SCM_RIGHTS,
  };
  memmove(CMSG_DATA(cmsg), &memfd, sizeof(int));
  msg.msg_controllen = cmsg->cmsg_len;

  ASSERT_EQ(sendmsg(fds[0], &msg, 0), 1);

  memset(data, 0, sizeof(data));
  memset(buf, 0, sizeof(buf));
  ASSERT_EQ(recvmsg(fds[1], &msg, 0), 1);
  EXPECT_EQ(data[0], '\0');
  EXPECT_GT(msg.msg_controllen, 0u);
  EXPECT_EQ(msg.msg_flags, 0);
}
#endif  // defined(__NR_memfd_create)

TEST(UnixSocket, ScmRightsTruncation) {
  int sv_raw[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, sv_raw));
  fbl::unique_fd sv[2] = {fbl::unique_fd(sv_raw[0]), fbl::unique_fd(sv_raw[1])};

  constexpr int kSendFdCount = 4;
  fbl::unique_fd send_fds[kSendFdCount];
  for (int i = 0; i < kSendFdCount; i++) {
    int p[2];
    ASSERT_EQ(0, pipe(p));
    send_fds[i].reset(p[0]);
    close(p[1]);
  }

  char data = 'x';
  struct iovec iov = {.iov_base = &data, .iov_len = 1};
  char sendbuf[CMSG_SPACE(kSendFdCount * sizeof(int))];
  memset(sendbuf, 0, sizeof(sendbuf));
  struct msghdr send_msg = {
      .msg_iov = &iov,
      .msg_iovlen = 1,
      .msg_control = sendbuf,
      .msg_controllen = sizeof(sendbuf),
  };
  struct cmsghdr* cmsg = CMSG_FIRSTHDR(&send_msg);
  cmsg->cmsg_level = SOL_SOCKET;
  cmsg->cmsg_type = SCM_RIGHTS;
  cmsg->cmsg_len = CMSG_LEN(kSendFdCount * sizeof(int));
  int* cmsg_fds = reinterpret_cast<int*>(CMSG_DATA(cmsg));
  for (int i = 0; i < kSendFdCount; i++) {
    cmsg_fds[i] = send_fds[i].get();
  }
  ASSERT_THAT(sendmsg(sv[0].get(), &send_msg, 0), SyscallSucceedsWithValue(1));

  constexpr int kMaxNumberOfFileDescriptors = 1024;
  std::set<int> open_fds_before;
  for (int fd = 0; fd < kMaxNumberOfFileDescriptors; fd++) {
    if (fcntl(fd, F_GETFD) != -1) {
      open_fds_before.insert(fd);
    }
  }

  char recvbuf[CMSG_LEN(sizeof(int))];
  memset(recvbuf, 0, sizeof(recvbuf));
  char rdata = 0;
  struct iovec riov = {.iov_base = &rdata, .iov_len = 1};
  struct msghdr recv_msg = {
      .msg_iov = &riov,
      .msg_iovlen = 1,
      .msg_control = recvbuf,
      .msg_controllen = sizeof(recvbuf),
  };
  ASSERT_THAT(recvmsg(sv[1].get(), &recv_msg, 0), SyscallSucceedsWithValue(1));

  EXPECT_NE(0, recv_msg.msg_flags & MSG_CTRUNC);

  int new_fds = 0;
  for (int fd = 0; fd < kMaxNumberOfFileDescriptors; fd++) {
    if (fcntl(fd, F_GETFD) != -1 && !open_fds_before.count(fd)) {
      new_fds++;
      close(fd);
    }
  }

  int parsed_fds = 0;
  for (struct cmsghdr* c = CMSG_FIRSTHDR(&recv_msg); c; c = CMSG_NXTHDR(&recv_msg, c)) {
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
      ASSERT_GE(c->cmsg_len, CMSG_LEN(0));
      parsed_fds = static_cast<int>((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
    }
  }

  EXPECT_EQ(1, parsed_fds);
  EXPECT_EQ(parsed_fds, new_fds);
}

class CmsgAlignmentTest : public testing::TestWithParam<int> {};

TEST_P(CmsgAlignmentTest, ScmRightsFollowedByCredentials) {
  const int num_fds = GetParam();

  int sv_raw[2];
  ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, sv_raw));
  fbl::unique_fd sv[2] = {fbl::unique_fd(sv_raw[0]), fbl::unique_fd(sv_raw[1])};

  int one = 1;
  ASSERT_THAT(setsockopt(sv[1].get(), SOL_SOCKET, SO_PASSCRED, &one, sizeof(one)),
              SyscallSucceeds());

  constexpr int kMaxFds = 3;
  ASSERT_LE(num_fds, kMaxFds);

  fbl::unique_fd pipes[kMaxFds];
  for (int i = 0; i < num_fds; i++) {
    int p[2];
    ASSERT_EQ(0, pipe(p));
    pipes[i].reset(p[0]);
    close(p[1]);
  }

  char payload = 'A';
  struct iovec iov = {.iov_base = &payload, .iov_len = 1};

  char sendbuf[CMSG_SPACE(kMaxFds * sizeof(int))];
  memset(sendbuf, 0, sizeof(sendbuf));
  struct msghdr send_msg = {
      .msg_iov = &iov,
      .msg_iovlen = 1,
      .msg_control = sendbuf,
      .msg_controllen = CMSG_SPACE(num_fds * sizeof(int)),
  };

  struct cmsghdr* cmsg = CMSG_FIRSTHDR(&send_msg);
  cmsg->cmsg_level = SOL_SOCKET;
  cmsg->cmsg_type = SCM_RIGHTS;
  cmsg->cmsg_len = CMSG_LEN(num_fds * sizeof(int));
  int* fds = reinterpret_cast<int*>(CMSG_DATA(cmsg));
  for (int i = 0; i < num_fds; i++) {
    fds[i] = pipes[i].get();
  }
  ASSERT_THAT(sendmsg(sv[0].get(), &send_msg, 0), SyscallSucceedsWithValue(1));

  char recvbuf[CMSG_SPACE(kMaxFds * sizeof(int)) + CMSG_SPACE(sizeof(struct ucred))];
  memset(recvbuf, 0, sizeof(recvbuf));
  char rdata = 0;
  struct iovec riov = {.iov_base = &rdata, .iov_len = 1};
  struct msghdr recv_msg = {
      .msg_iov = &riov,
      .msg_iovlen = 1,
      .msg_control = recvbuf,
      .msg_controllen = sizeof(recvbuf),
  };
  ASSERT_THAT(recvmsg(sv[1].get(), &recv_msg, 0), SyscallSucceedsWithValue(1));

  EXPECT_EQ(recv_msg.msg_flags & MSG_CTRUNC, 0);

  int cmsg_count = 0;
  bool found_rights = false;
  bool found_creds = false;
  for (struct cmsghdr* c = CMSG_FIRSTHDR(&recv_msg); c; c = CMSG_NXTHDR(&recv_msg, c)) {
    cmsg_count++;
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
      found_rights = true;
      ASSERT_GE(c->cmsg_len, CMSG_LEN(0));
      int received_fds = static_cast<int>((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
      ASSERT_EQ(received_fds, num_fds);
      const int* recv_fds = reinterpret_cast<const int*>(CMSG_DATA(c));
      for (int i = 0; i < received_fds; i++) {
        int recv_fd = recv_fds[i];
        EXPECT_GE(recv_fd, 0);
        close(recv_fd);
      }
    }
    if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_CREDENTIALS) {
      found_creds = true;
      EXPECT_EQ(c->cmsg_len, CMSG_LEN(sizeof(struct ucred)));
      struct ucred cred;
      memcpy(&cred, CMSG_DATA(c), sizeof(cred));
      EXPECT_EQ(cred.pid, getpid());
      EXPECT_EQ(cred.uid, geteuid());
      EXPECT_EQ(cred.gid, getegid());
    }
  }

  EXPECT_EQ(cmsg_count, 2);
  EXPECT_TRUE(found_rights);
  EXPECT_TRUE(found_creds);
}

INSTANTIATE_TEST_SUITE_P(UnixSocket, CmsgAlignmentTest, testing::Values(1, 2, 3));

// This test verifies that we can concurrently attempt to create the same type of socket from
// multiple threads.
TEST(Socket, ConcurrentCreate) {
  std::atomic_int barrier{0};
  std::atomic_int child_ready{0};
  auto child = std::thread([&] {
    child_ready.store(1);
    while (barrier.load() == 0) {
    }
    fbl::unique_fd fd;
    EXPECT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  });
  while (child_ready.load() == 0) {
  }
  barrier.store(1);

  fbl::unique_fd fd;
  EXPECT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  child.join();
}

TEST(Socket, SocketCookie) {
  fbl::unique_fd fd;

  // Create a socket and get its cookie.
  EXPECT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  uint64_t cookie1 = 0;
  socklen_t optlen = sizeof(cookie1);
  ASSERT_EQ(getsockopt(fd.get(), SOL_SOCKET, SO_COOKIE, &cookie1, &optlen), 0) << strerror(errno);
  EXPECT_EQ(optlen, sizeof(cookie1));

  // Create another socket verify that it has a different cookie value.
  EXPECT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  uint64_t cookie2 = 0;
  ASSERT_EQ(getsockopt(fd.get(), SOL_SOCKET, SO_COOKIE, &cookie2, &optlen), 0) << strerror(errno);
  EXPECT_EQ(optlen, sizeof(cookie2));

  EXPECT_NE(cookie1, cookie2);
}

class SocketFault : public FaultTest, public testing::WithParamInterface<std::pair<int, int>> {
 protected:
  void SetUp() override {
    const auto [type, protocol] = GetParam();

    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (type == SOCK_DGRAM && protocol == IPPROTO_ICMP && getuid() != 0) {
      GTEST_SKIP() << "Ping sockets require root.";
    }

    sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_addr = {htonl(INADDR_LOOPBACK)},
    };
    socklen_t addrlen = sizeof(addr);
    ASSERT_TRUE(recv_fd_ = fbl::unique_fd(socket(AF_INET, type, protocol))) << strerror(errno);
    ASSERT_EQ(bind(recv_fd_.get(), reinterpret_cast<sockaddr*>(&addr), addrlen), 0)
        << strerror(errno);
    ASSERT_EQ(getsockname(recv_fd_.get(), reinterpret_cast<sockaddr*>(&addr), &addrlen), 0)
        << strerror(errno);
    ASSERT_EQ(addrlen, sizeof(addr));
    if (type == SOCK_STREAM) {
      ASSERT_EQ(listen(recv_fd_.get(), 0), 0) << strerror(errno);
      listen_fd_ = std::move(recv_fd_);
    }

    ASSERT_TRUE(send_fd_ = fbl::unique_fd(socket(AF_INET, type, protocol))) << strerror(errno);
    ASSERT_EQ(connect(send_fd_.get(), reinterpret_cast<const sockaddr*>(&addr), sizeof(addr)), 0)
        << strerror(errno);

    if (type == SOCK_STREAM) {
      ASSERT_TRUE(recv_fd_ = fbl::unique_fd(accept(listen_fd_.get(), nullptr, nullptr)))
          << strerror(errno);
    } else if (protocol == IPPROTO_ICMP) {
      // ICMP sockets only get the packet on the sending socket since sockets do not
      // receive ICMP requests, only replies. Note that the netstack internally
      // responds to ICMP requests without any user-application needing to handle
      // requests.
      ASSERT_TRUE(recv_fd_ = fbl::unique_fd(dup(send_fd_.get()))) << strerror(errno);
    }
  }

  void TearDown() override {
    send_fd_.reset();
    recv_fd_.reset();
    listen_fd_.reset();
  }

  void SetRecvFdNonBlocking() {
    int flags = fcntl(recv_fd_.get(), F_GETFL, 0);
    ASSERT_GE(flags, 0) << strerror(errno);
    ASSERT_EQ(fcntl(recv_fd_.get(), F_SETFL, flags | O_NONBLOCK), 0) << strerror(errno);
  }

  fbl::unique_fd recv_fd_;
  fbl::unique_fd listen_fd_;
  fbl::unique_fd send_fd_;
};

// Test sending a packet from invalid memory.
TEST_P(SocketFault, Write) {
  EXPECT_EQ(write(send_fd_.get(), faulting_ptr_, kFaultingSize_), -1);
  EXPECT_EQ(errno, EFAULT);
}

// Test receiving a packet to invalid memory.
TEST_P(SocketFault, Read) {
  // First send a valid message that we can read.
  //
  // We send an ICMP message since this test is generic over UDP/TCP/ICMP.
  // UDP/TCP do not care about the shape of the payload but ICMP does so we just
  // use an ICMP compatible payload for simplicity.
  constexpr icmphdr kSendIcmp = {
      .type = ICMP_ECHO,
  };
  ASSERT_EQ(write(send_fd_.get(), &kSendIcmp, sizeof(kSendIcmp)),
            static_cast<ssize_t>(sizeof(kSendIcmp)));

  pollfd p = {
      .fd = recv_fd_.get(),
      .events = POLLIN,
  };
  ASSERT_EQ(poll(&p, 1, -1), 1);
  ASSERT_EQ(p.revents, POLLIN);

  static_assert(kFaultingSize_ >= sizeof(kSendIcmp));
  EXPECT_EQ(read(recv_fd_.get(), faulting_ptr_, sizeof(kSendIcmp)), -1);
  EXPECT_EQ(errno, EFAULT);
}

TEST_P(SocketFault, ReadV) {
  // First send a valid message that we can read.
  //
  // We send an ICMP message since this test is generic over UDP/TCP/ICMP.
  // UDP/TCP do not care about the shape of the payload but ICMP does so we just
  // use an ICMP compatible payload for simplicity.
  constexpr icmphdr kSendIcmp = {
      .type = ICMP_ECHO,
  };
  ASSERT_EQ(write(send_fd_.get(), &kSendIcmp, sizeof(kSendIcmp)),
            static_cast<ssize_t>(sizeof(kSendIcmp)));

  pollfd p = {
      .fd = recv_fd_.get(),
      .events = POLLIN,
  };
  ASSERT_EQ(poll(&p, 1, -1), 1);
  ASSERT_EQ(p.revents, POLLIN);

  char base0[1];
  char base2[sizeof(kSendIcmp) - 1];
  iovec iov[] = {
      {
          .iov_base = base0,
          .iov_len = sizeof(base0),
      },
      {
          .iov_base = faulting_ptr_,
          .iov_len = sizeof(kFaultingSize_),
      },
      {
          .iov_base = base2,
          .iov_len = sizeof(base2),
      },
  };

  // Read once with iov holding the invalid pointer.
  ASSERT_EQ(readv(recv_fd_.get(), iov, std::size(iov)), -1);
  EXPECT_EQ(errno, EFAULT);

  // Read again after clearing the invalid buffer. This read will fail on UDP/ICMP
  // sockets since they deque the message before checking the validity of buffers
  // but TCP sockets will not remove bytes from the unread bytes held by the kernel
  // if any buffer faults. Note that what UDP/ICMP does is ~acceptable since they are
  // not meant to be a reliable protocol and the behaviour for TCP also makes sense
  // because when the socket returns EFAULT, there is no way to know how many
  // bytes the kernel write into our buffers. Since the kernel has no way to tell us
  // how many bytes were read when a fault occurred, it has no other option than to
  // keep the bytes before the fault to prevent userspace from dropping part of a
  // byte stream.
  ASSERT_NO_FATAL_FAILURE(SetRecvFdNonBlocking());
  const auto [type, protocol] = GetParam();
  iov[1] = iovec{};
  if (type == SOCK_STREAM) {
    ASSERT_EQ(readv(recv_fd_.get(), iov, std::size(iov)), static_cast<ssize_t>(sizeof(kSendIcmp)));
  } else {
    ASSERT_EQ(readv(recv_fd_.get(), iov, std::size(iov)), -1);
    EXPECT_EQ(errno, EAGAIN);
  }
}

TEST_P(SocketFault, WriteV) {
  icmphdr kSendIcmp = {
      .type = ICMP_ECHO,
  };
  constexpr size_t kBase0Size = 1;
  iovec iov[] = {
      {
          .iov_base = &kSendIcmp,
          .iov_len = kBase0Size,
      },
      {
          .iov_base = faulting_ptr_,
          .iov_len = sizeof(kFaultingSize_),
      },
      {
          .iov_base = reinterpret_cast<char*>(&kSendIcmp) + kBase0Size,
          .iov_len = sizeof(kSendIcmp) - kBase0Size,
      },
  };
  ASSERT_EQ(writev(send_fd_.get(), iov, std::size(iov)), -1);
  EXPECT_EQ(errno, EFAULT);

  // Reading should fail since nothing should have been written.
  ASSERT_NO_FATAL_FAILURE(SetRecvFdNonBlocking());
  char recv_buf[sizeof(kSendIcmp)];
  ASSERT_EQ(read(recv_fd_.get(), &recv_buf, sizeof(recv_buf)), -1);
  EXPECT_EQ(errno, EAGAIN);
}

TEST_P(SocketFault, SendmsgENOBUFS) {
  int sockfd;
  sockfd = socket(AF_INET, SOCK_DGRAM, 0);
  ASSERT_GT(sockfd, 0) << strerror(errno);

  char data[] = "a";
  struct iovec iov[] = {{
      .iov_base = data,
      .iov_len = 1,
  }};
  char buf[CMSG_SPACE(0)];
  struct msghdr msg = {
      .msg_iov = iov,
      .msg_iovlen = 1,
      .msg_control = buf,
      .msg_controllen = UINT_MAX,
  };
  *CMSG_FIRSTHDR(&msg) = (struct cmsghdr){
      .cmsg_len = UINT_MAX,
      .cmsg_level = SOL_SOCKET,
      .cmsg_type = SCM_RIGHTS,
  };

  ASSERT_EQ(sendmsg(sockfd, &msg, 0), -1);
  ASSERT_EQ(errno, ENOBUFS);
}

INSTANTIATE_TEST_SUITE_P(SocketFault, SocketFault,
                         testing::Values(std::make_pair(SOCK_DGRAM, 0),
                                         std::make_pair(SOCK_DGRAM, IPPROTO_ICMP),
                                         std::make_pair(SOCK_STREAM, 0)));
class SndRcvBufSockOpt : public testing::TestWithParam<int> {};

// This test asserts that the value of SO_RCVBUF and SO_SNDBUF are doubled on
// set, and this doubled value is returned on get, as described in the Linux
// socket(7) man page.
TEST_P(SndRcvBufSockOpt, DoubledOnGet) {
  fbl::unique_fd fd;
  EXPECT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);

  int buf_size;
  socklen_t optlen = sizeof(buf_size);
  ASSERT_EQ(getsockopt(fd.get(), SOL_SOCKET, GetParam(), &buf_size, &optlen), 0) << strerror(errno);

  ASSERT_EQ(setsockopt(fd.get(), SOL_SOCKET, GetParam(), &buf_size, optlen), 0) << strerror(errno);

  int new_buf_size;
  ASSERT_EQ(getsockopt(fd.get(), SOL_SOCKET, GetParam(), &new_buf_size, &optlen), 0)
      << strerror(errno);
  ASSERT_EQ(new_buf_size, 2 * buf_size);
}

INSTANTIATE_TEST_SUITE_P(SndRcvBufSockOpt, SndRcvBufSockOpt, testing::Values(SO_SNDBUF, SO_RCVBUF),
                         [](const testing::TestParamInfo<int>& info) {
                           switch (info.param) {
                             case SO_SNDBUF:
                               return std::string("SO_SNDBUF");
                             case SO_RCVBUF:
                               return std::string("SO_RCVBUF");
                           }
                           return std::string("UNKNOWN(") + std::to_string(info.param) + ")";
                         });

struct PrivilegedSockOptParam {
  int domain;
  int type;
  int level;
  int optname;
};

class PrivilegedSockOptTest : public testing::TestWithParam<PrivilegedSockOptParam> {
 protected:
  void SetUp() override {
    if (!test_helper::HasCapability(CAP_NET_ADMIN) || !test_helper::HasCapability(CAP_NET_RAW)) {
      GTEST_SKIP()
          << "Need CAP_NET_ADMIN and CAP_NET_RAW to run capability-restricted sockopt tests";
    }

    auto param = GetParam();
    EXPECT_TRUE(fd_ = fbl::unique_fd(socket(param.domain, param.type, 0))) << strerror(errno);
  }

  fbl::unique_fd fd_;
};

std::string PrintPrivilegedSockOptParam(
    const testing::TestParamInfo<PrivilegedSockOptParam>& info) {
  auto param = info.param;
  std::string domain_str = (param.domain == AF_INET) ? "IPv4" : "IPv6";
  std::string type_str = (param.type == SOCK_STREAM) ? "TCP" : "UDP";
  std::string level_str = (param.level == SOL_SOCKET) ? "SOL_SOCKET" : "SOL_IP";
  std::string optname_str = (param.optname == SO_MARK) ? "SO_MARK" : "IP_TRANSPARENT";
  return domain_str + "_" + type_str + "_" + level_str + "_" + optname_str;
}

TEST_P(PrivilegedSockOptTest, SetAndGet) {
  auto param = GetParam();

  // Verify initial value is 0.
  int initial_val = -1;
  socklen_t optlen = sizeof(initial_val);
  ASSERT_EQ(getsockopt(fd_.get(), param.level, param.optname, &initial_val, &optlen), 0)
      << strerror(errno);
  ASSERT_EQ(initial_val, 0);

  // Set to 1.
  int val = 1;
  ASSERT_EQ(setsockopt(fd_.get(), param.level, param.optname, &val, sizeof(val)), 0)
      << strerror(errno);

  // Get and verify.
  int retrieved_val = 0;
  optlen = sizeof(retrieved_val);
  ASSERT_EQ(getsockopt(fd_.get(), param.level, param.optname, &retrieved_val, &optlen), 0)
      << strerror(errno);
  ASSERT_EQ(optlen, sizeof(val));
  ASSERT_EQ(val, retrieved_val);
}

TEST_P(PrivilegedSockOptTest, NoCapabilities) {
  auto param = GetParam();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    test_helper::UnsetCapabilityEffective(CAP_NET_RAW);

    // `setsockopt` must fail with EPERM.
    int val = 1;
    EXPECT_EQ(setsockopt(fd_.get(), param.level, param.optname, &val, sizeof(val)), -1);
    EXPECT_EQ(errno, EPERM);
  });
}

TEST_P(PrivilegedSockOptTest, RawCapability) {
  auto param = GetParam();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    // Drop the `NET_ADMIN` capability, but keep `NET_RAW`.
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);

    int val = 1;
    ASSERT_EQ(setsockopt(fd_.get(), param.level, param.optname, &val, sizeof(val)), 0)
        << strerror(errno);
  });
}

TEST_P(PrivilegedSockOptTest, AdminCapability) {
  auto param = GetParam();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    // Drop the `NET_RAW` capability, but keep `NET_ADMIN`.
    test_helper::UnsetCapabilityEffective(CAP_NET_RAW);

    int val = 1;
    ASSERT_EQ(setsockopt(fd_.get(), param.level, param.optname, &val, sizeof(val)), 0)
        << strerror(errno);
  });
}

INSTANTIATE_TEST_SUITE_P(PrivilegedSockOpt, PrivilegedSockOptTest,
                         testing::Values(
                             // SO_MARK
                             PrivilegedSockOptParam{AF_INET, SOCK_STREAM, SOL_SOCKET, SO_MARK},
                             PrivilegedSockOptParam{AF_INET, SOCK_DGRAM, SOL_SOCKET, SO_MARK},
                             PrivilegedSockOptParam{AF_INET6, SOCK_STREAM, SOL_SOCKET, SO_MARK},
                             PrivilegedSockOptParam{AF_INET6, SOCK_DGRAM, SOL_SOCKET, SO_MARK},
                             // IP_TRANSPARENT
                             PrivilegedSockOptParam{AF_INET, SOCK_DGRAM, SOL_IP, IP_TRANSPARENT},
                             PrivilegedSockOptParam{AF_INET6, SOCK_DGRAM, SOL_IP, IP_TRANSPARENT}),
                         PrintPrivilegedSockOptParam);

class BpfTest : public testing::Test {
 protected:
  void SetUp() override {
    if (!test_helper::HasCapability(CAP_NET_RAW)) {
      GTEST_SKIP() << "Need CAP_NET_RAW to run BpfTest";
    }

    packet_socket_fd_ = fbl::unique_fd(socket(AF_PACKET, SOCK_RAW, 0));
    ASSERT_TRUE(packet_socket_fd_) << strerror(errno);
    sockaddr_ll addr_ll = {
        .sll_family = AF_PACKET,
        .sll_protocol = htons(ETH_P_ALL),
    };
    ASSERT_EQ(bind(packet_socket_fd_.get(), reinterpret_cast<sockaddr*>(&addr_ll), sizeof(addr_ll)),
              0);
  }

  void SendPacketAndCheckReceived(int domain, uint16_t dst_port, bool expected);

  fbl::unique_fd packet_socket_fd_;
};

void BpfTest::SendPacketAndCheckReceived(int domain, uint16_t dst_port, bool expected) {
  sockaddr_in addr4 = {
      .sin_family = AF_INET,
      .sin_port = htons(dst_port),
      .sin_addr =
          {
              .s_addr = htonl(INADDR_LOOPBACK),
          },
  };
  sockaddr_in6 addr6 = {
      .sin6_family = AF_INET6,
      .sin6_port = htons(dst_port),
      .sin6_addr = IN6ADDR_LOOPBACK_INIT,
  };
  sockaddr* addr = domain == AF_INET6 ? reinterpret_cast<sockaddr*>(&addr6)
                                      : reinterpret_cast<sockaddr*>(&addr4);
  socklen_t addrlen = domain == AF_INET6 ? sizeof(addr6) : sizeof(addr4);

  const char data[] = "test message";
  fbl::unique_fd sendfd;
  ASSERT_TRUE(sendfd = fbl::unique_fd(socket(domain, SOCK_DGRAM, 0))) << strerror(errno);
  ASSERT_EQ(sendto(sendfd.get(), data, sizeof(data), 0, addr, addrlen),
            static_cast<int>(sizeof(data)))
      << strerror(errno);

  pollfd pfd = {
      .fd = packet_socket_fd_.get(),
      .events = POLLIN,
  };

  const int kPositiveCheckTimeoutMs = 10000;
  const int kNegativeCheckTimeoutMs = 1000;
  int timeout = expected ? kPositiveCheckTimeoutMs : kNegativeCheckTimeoutMs;
  int n = poll(&pfd, 1, timeout);
  ASSERT_GE(n, 0) << strerror(errno);
  if (expected) {
    ASSERT_EQ(n, 1);
    char buf[4096];
    ASSERT_GT(recv(packet_socket_fd_.get(), buf, sizeof(buf), 0), 0);

    // The packet was sent to loopback, so we expect to receive it twice.
    ASSERT_EQ(poll(&pfd, 1, 1000), 1);
    ASSERT_GT(recv(packet_socket_fd_.get(), buf, sizeof(buf), 0), 0);
  } else {
    ASSERT_EQ(n, 0);
  }
}

TEST_F(BpfTest, SoAttachFilter) {
  const uint16_t kTestDstPortIpv4 = 1234;
  const uint16_t kTestDstPortIpv6 = 1236;

  // This filter accepts IPv4 UDP packets on port kTestDstPortIpv4 and IPv6 UDP
  // packets on port kTestDstPortIpv6.
  static sock_filter filter_code[] = {
      // Load the protocol.
      BPF_STMT(BPF_LD | BPF_H | BPF_ABS, (__u32)SKF_AD_OFF + SKF_AD_PROTOCOL),

      // Check if this is IPv4, skip below otherwise.
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, ETHERTYPE_IP, 0, 8),

      // Check that the protocol is UDP.
      BPF_STMT(BPF_LD | BPF_B | BPF_ABS, (__u32)SKF_NET_OFF + 9),

      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, IPPROTO_UDP, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, 0),

      // Get the IP header length.
      BPF_STMT(BPF_LDX | BPF_B | BPF_MSH, (__u32)SKF_NET_OFF),

      // Check the destination port.
      BPF_STMT(BPF_LD | BPF_H | BPF_IND, (__u32)SKF_NET_OFF + 2),

      // Reject if not kTestDstPortIpv4.
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, kTestDstPortIpv4, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, 0),

      // Accept.
      BPF_STMT(BPF_RET | BPF_K, 0xFFFFFFFF),

      // Check if this is IPv6.
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, ETHERTYPE_IPV6, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, 0),

      // Check the protocol is UDP.
      BPF_STMT(BPF_LD | BPF_B | BPF_ABS, (__u32)SKF_NET_OFF + 6),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, IPPROTO_UDP, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, 0),

      // Load destination port, assuming standard, 40-byte IPv6 packet.
      BPF_STMT(BPF_LD | BPF_H | BPF_ABS, (__u32)SKF_NET_OFF + 42),

      // Check destination port.
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, kTestDstPortIpv6, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, 0),

      // Accept.
      BPF_STMT(BPF_RET | BPF_K, 0xFFFFFFFF),
  };

  static const sock_fprog filter = {
      sizeof(filter_code) / sizeof(filter_code[0]),
      filter_code,
  };

  ASSERT_EQ(
      setsockopt(packet_socket_fd_.get(), SOL_SOCKET, SO_ATTACH_FILTER, &filter, sizeof(filter)),
      0);

  SendPacketAndCheckReceived(AF_INET, kTestDstPortIpv4, true);
  SendPacketAndCheckReceived(AF_INET6, kTestDstPortIpv6, true);
  SendPacketAndCheckReceived(AF_INET, kTestDstPortIpv6, false);
  SendPacketAndCheckReceived(AF_INET6, kTestDstPortIpv4, false);
}

TEST(IpTables, IpTablesAdminCap) {
  if (!test_helper::HasCapability(CAP_NET_ADMIN)) {
    GTEST_SKIP() << "Need CAP_NET_ADMIN to access iptables";
  }

  fbl::unique_fd sock;
  ASSERT_TRUE(sock = fbl::unique_fd(socket(AF_INET, SOCK_RAW, IPPROTO_ICMP))) << strerror(errno);

  struct xt_get_revision optval = {};
  strncpy(optval.name, "REDIRECT", sizeof(optval.name));
  optval.revision = 0;

  socklen_t optval_size = sizeof(optval);
  ASSERT_EQ(getsockopt(sock.get(), SOL_IP, IPT_SO_GET_REVISION_TARGET, &optval, &optval_size), 0);

  test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);

  ASSERT_EQ(getsockopt(sock.get(), SOL_IP, IPT_SO_GET_REVISION_TARGET, &optval, &optval_size), -1);
  EXPECT_EQ(errno, EPERM);

  test_helper::SetCapabilityEffective(CAP_NET_ADMIN);
}

// Helper to override `/proc/sys/net/ipv4/ping_group_range`.
class IcmpPingGidOverride {
 public:
  static constexpr char FILE_NAME[] = "/proc/sys/net/ipv4/ping_group_range";

  IcmpPingGidOverride() {
    {
      // First check that the file is writable.
      std::ofstream outfile(FILE_NAME);
      can_override_ = outfile.is_open();
    }

    if (can_override_) {
      std::tie(original_min_gid_, original_max_gid_) = Get();
    }
  }
  ~IcmpPingGidOverride() {
    if (can_override_) {
      Set(original_min_gid_, original_max_gid_);
    }
  }

  bool can_override() { return can_override_; }

  std::pair<gid_t, gid_t> Get() {
    std::ifstream ping_group_range_file(FILE_NAME);
    EXPECT_TRUE(ping_group_range_file.is_open()) << strerror(errno);
    gid_t min, max;
    ping_group_range_file >> min >> max;
    return std::make_pair(min, max);
  }

  void Set(gid_t min, gid_t max) {
    std::ofstream ping_group_range_file(FILE_NAME);
    ASSERT_TRUE(ping_group_range_file.is_open()) << strerror(errno);
    ping_group_range_file << min << " " << max;
    ping_group_range_file.close();
    EXPECT_FALSE(ping_group_range_file.fail());
  }

  void SetMin(gid_t min) {
    std::ofstream ping_group_range_file(FILE_NAME);
    ASSERT_TRUE(ping_group_range_file.is_open()) << strerror(errno);
    ping_group_range_file << min;
    ping_group_range_file.close();
    EXPECT_FALSE(ping_group_range_file.fail());
  }

 private:
  bool can_override_ = false;
  gid_t original_min_gid_ = 0;
  gid_t original_max_gid_ = 0;
};

class IcmpPingSocket : public testing::Test {
 protected:
  void SetUp() override {
    if (!test_helper::HasCapability(CAP_SETGID)) {
      GTEST_SKIP() << "Need CAP_SET_GID to access iptables";
    }

    if (!gid_range_.can_override()) {
      GTEST_SKIP() << "Need writable " << IcmpPingGidOverride::FILE_NAME;
    }
  }

  int TryCreateSocket(bool use_v6 = false) {
    fbl::unique_fd sock(
        socket(use_v6 ? AF_INET6 : AF_INET, SOCK_DGRAM,
               use_v6 ? static_cast<int>(IPPROTO_ICMPV6) : static_cast<int>(IPPROTO_ICMP)));
    if (sock) {
      return 0;
    } else {
      return -errno;
    }
  }

  void TestWriteRangeFail(const char contents[]) {
    std::ofstream ping_group_range_file(IcmpPingGidOverride::FILE_NAME);
    ASSERT_TRUE(ping_group_range_file.is_open()) << strerror(errno);
    ping_group_range_file << contents;
    ping_group_range_file.close();
    EXPECT_TRUE(ping_group_range_file.fail());
  }

  void TryCreateSocketWithGid(gid_t gid, int expected_result, bool use_v6 = false) {
    SCOPED_TRACE(fxl::StringPrintf("GID: %d", gid));
    ASSERT_EQ(setegid(gid), 0) << strerror(errno);
    ASSERT_EQ(TryCreateSocket(use_v6), expected_result);
  }

  IcmpPingGidOverride gid_range_;
};

TEST_F(IcmpPingSocket, UpdateGidRange) {
  gid_range_.Set(10, 100);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(10, 100));

  gid_range_.Set(10, 0);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(1, 0));

  gid_range_.Set(10, 30);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(10, 30));

  gid_range_.SetMin(20);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(20, 30));

  gid_range_.SetMin(30);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(30, 30));

  gid_range_.SetMin(31);
  ASSERT_EQ(gid_range_.Get(), std::make_pair(1, 0));
}

TEST_F(IcmpPingSocket, CreateSocket) {
  gid_t original_gid = getegid();

  gid_range_.Set(10, 100);

  TryCreateSocketWithGid(9, -EACCES);
  TryCreateSocketWithGid(10, 0);
  TryCreateSocketWithGid(50, 0);
  TryCreateSocketWithGid(100, 0);
  TryCreateSocketWithGid(101, -EACCES);

  ASSERT_EQ(setegid(original_gid), 0) << strerror(errno);
}

TEST_F(IcmpPingSocket, CreateSocketV6) {
  gid_t original_gid = getegid();

  gid_range_.Set(10, 100);

  TryCreateSocketWithGid(9, -EACCES, true);
  TryCreateSocketWithGid(10, 0, true);
  TryCreateSocketWithGid(50, 0, true);
  TryCreateSocketWithGid(100, 0, true);
  TryCreateSocketWithGid(101, -EACCES, true);

  ASSERT_EQ(setegid(original_gid), 0) << strerror(errno);
}

// Verifies that attempts to write an out-of-range value to the
// `ping_group_range` file are rejected. Starnix and current versions of Linux
// (e.g. 6.12) allow any values in range [0, 2^32-2] (i.e. any uint32_t, except
// UINT32_MAX). Older Linux versions (e.g. 5.15) allow only values in range
// [0, 2^31-1] (i.e. non-negative int32_t), which will cause this test to fail.
TEST_F(IcmpPingSocket, SetInvalid) {
  TestWriteRangeFail("---");
  // Min and max cannot be higher than 2^32-2.
  TestWriteRangeFail("0 4294967295");
  TestWriteRangeFail("0 4294967296");
  TestWriteRangeFail("4294967295 0");
  TestWriteRangeFail("4294967296 0");

  // We should be able to set both values to 2^32-2.
  gid_range_.Set(0, 4294967294);
  gid_range_.Set(4294967294, 0);
}

TEST(InetSocket, BindToDevice) {
  if (!test_helper::HasCapability(CAP_NET_RAW)) {
    GTEST_SKIP() << "Need CAP_NET_RAW to access SO_BINDTODEVICE";
  }
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test verifies behavior specific to Starnix";
  }

  test_helper::UnsetCapabilityEffective(CAP_NET_RAW);

  fbl::unique_fd sock;
  ASSERT_TRUE(sock = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);

  char iface[] = "lo";
  EXPECT_EQ(setsockopt(sock.get(), SOL_SOCKET, SO_BINDTODEVICE, iface, sizeof(iface)), -1);
  EXPECT_EQ(errno, EPERM);

  test_helper::SetCapabilityEffective(CAP_NET_RAW);
}

class ReusePortSharingTest : public testing::Test {
 protected:
  void SetUp() override {
    if (!test_helper::HasCapability(CAP_SETUID)) {
      GTEST_SKIP() << "Need CAP_SETUID to access seteuid()";
    }
  }

  void SetReusePort(int fd) {
    int optval = 1;
    EXPECT_THAT(setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &optval, sizeof(optval)),
                SyscallSucceeds());
  }
};

TEST_F(ReusePortSharingTest, ReusePortSharing) {
  struct sockaddr_in addr;
  addr.sin_family = AF_INET;
  addr.sin_port = htons(12345);
  addr.sin_addr.s_addr = htonl(INADDR_ANY);

  // Bind first socket from UID=1.
  EXPECT_THAT(seteuid(1), SyscallSucceeds());
  fbl::unique_fd sock1;
  EXPECT_TRUE(sock1 = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  SetReusePort(sock1.get());
  EXPECT_THAT(bind(sock1.get(), (struct sockaddr*)(&addr), sizeof(addr)), SyscallSucceeds());

  {
    // Should be able to bind another socket from the same UID.
    fbl::unique_fd sock2;
    EXPECT_TRUE(sock2 = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
    SetReusePort(sock2.get());
    EXPECT_THAT(bind(sock2.get(), (struct sockaddr*)(&addr), sizeof(addr)), SyscallSucceeds());
  }

  // Restore `CAP_SETUID`, otherwise `seteuid()` will fail.
  test_helper::SetCapabilityEffective(CAP_SETUID);

  EXPECT_THAT(seteuid(2), SyscallSucceeds());

  fbl::unique_fd sock3;
  EXPECT_TRUE(sock3 = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  SetReusePort(sock3.get());

  // Should not be able to bind another socket from a different UID.
  EXPECT_THAT(bind(sock3.get(), (struct sockaddr*)(&addr), sizeof(addr)),
              SyscallFailsWithErrno(EADDRINUSE));

  // Restore original uid.
  EXPECT_THAT(setresuid(0, 0, 0), SyscallSucceeds());
}

TEST(Socket, GetSockOptZeroSize) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);

  uint8_t buf = 'x';
  socklen_t optlen = 0;
  // This might return an error or succeed depending on the platform/kernel version,
  // but importantly, it shouldn't crash the kernel (b/356896105).
  getsockopt(fd.get(), SOL_SOCKET, SO_BINDTODEVICE, &buf, &optlen);

  // And the original TCP_CONGESTION option
  optlen = 0;
  getsockopt(fd.get(), SOL_TCP, TCP_CONGESTION, &buf, &optlen);
}

void VerifyGetsockoptTruncation(int fd, int level, int optname, socklen_t expected_full_size) {
  ASSERT_LE(expected_full_size, 248u);

  // First, get the full option value.
  uint8_t full_buf[256] = {0};
  socklen_t full_len = sizeof(full_buf);
  ASSERT_THAT(getsockopt(fd, level, optname, full_buf, &full_len), SyscallSucceeds());
  ASSERT_EQ(full_len, expected_full_size);

  uint8_t truncated_buf[256];
  constexpr uint8_t kPresetValue = 0xcd;

  // Test different truncation lengths.
  for (socklen_t optlen = 0; optlen <= expected_full_size + 8; ++optlen) {
    memset(truncated_buf, kPresetValue, sizeof(truncated_buf));

    socklen_t optlen_out = optlen;
    ASSERT_THAT(getsockopt(fd, level, optname, truncated_buf, &optlen_out), SyscallSucceeds())
        << "Failed for optlen=" << optlen;

    if (optlen > expected_full_size) {
      EXPECT_EQ(optlen_out, expected_full_size);
    } else {
      EXPECT_EQ(optlen_out, optlen);
    }

    // Verify that the first `optlen_out` bytes match the full option value.
    for (socklen_t i = 0; i < optlen_out; ++i) {
      EXPECT_EQ(truncated_buf[i], full_buf[i])
          << "Mismatch at index " << i << " for optlen=" << optlen;
    }

    // Verify that the bytes after `optlen_out` are untouched.
    for (size_t i = optlen_out; i < sizeof(truncated_buf); ++i) {
      EXPECT_EQ(truncated_buf[i], kPresetValue)
          << "Buffer overflow at index " << i << " for optlen=" << optlen;
    }
  }
}

TEST(GetsockoptTruncationTest, TcpNodelay) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  int one = 1;
  ASSERT_EQ(setsockopt(fd.get(), SOL_TCP, TCP_NODELAY, &one, sizeof(one)), 0) << strerror(errno);
  VerifyGetsockoptTruncation(fd.get(), SOL_TCP, TCP_NODELAY, sizeof(int));
}

TEST(GetsockoptTruncationTest, TcpReuseAddr) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  int one = 1;
  ASSERT_EQ(setsockopt(fd.get(), SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)), 0)
      << strerror(errno);
  VerifyGetsockoptTruncation(fd.get(), SOL_SOCKET, SO_REUSEADDR, sizeof(int));
}

TEST(GetsockoptTruncationTest, UdpReuseAddr) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0))) << strerror(errno);
  int one = 1;
  ASSERT_EQ(setsockopt(fd.get(), SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)), 0)
      << strerror(errno);
  VerifyGetsockoptTruncation(fd.get(), SOL_SOCKET, SO_REUSEADDR, sizeof(int));
}

TEST(GetsockoptTruncationTest, UnixReuseAddr) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_UNIX, SOCK_STREAM, 0))) << strerror(errno);
  int one = 1;
  ASSERT_EQ(setsockopt(fd.get(), SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)), 0)
      << strerror(errno);
  VerifyGetsockoptTruncation(fd.get(), SOL_SOCKET, SO_REUSEADDR, sizeof(int));
}

TEST(GetsockoptTruncationTest, RcvTimeo) {
  fbl::unique_fd fd;
  ASSERT_TRUE(fd = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0))) << strerror(errno);
  struct timeval tv = {.tv_sec = 42, .tv_usec = 123456};
  ASSERT_EQ(setsockopt(fd.get(), SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv)), 0) << strerror(errno);
  VerifyGetsockoptTruncation(fd.get(), SOL_SOCKET, SO_RCVTIMEO, sizeof(struct timeval));
}

class ScmCredentialsTest : public testing::Test {
 protected:
  void SetUp() override {
    int tmp[2];
    int one = 1;

    if (geteuid() != 0 && !test_helper::HasCapabilityEffective(CAP_SYS_ADMIN)) {
      GTEST_SKIP() << "Test requires root or CAP_SYS_ADMIN to setresuid/setresgid";
    }

    ASSERT_EQ(0, socketpair(AF_UNIX, SOCK_STREAM, 0, tmp));
    sv_[0].reset(tmp[0]);
    sv_[1].reset(tmp[1]);
    setsockopt(sv_[1].get(), SOL_SOCKET, SO_PASSCRED, &one, sizeof(one));

    memset(cmsgbuf_, 0, sizeof(cmsgbuf_));
    msg_.msg_iov = &iov_;
    msg_.msg_iovlen = 1;
    msg_.msg_control = cmsgbuf_;
    msg_.msg_controllen = sizeof(cmsgbuf_);
  }

  struct SenderCredentials {
    uid_t uid, euid, suid;
    gid_t gid, egid, sgid;
  };

  struct Caps {
    bool has_cap_sys_admin = false;
    bool has_cap_setuid = false;
    bool has_cap_setgid = false;
  };

  struct Outcome {
    bool success = false;
    int error;
  };

  void TestForgery(std::optional<pid_t> forged_pid, uid_t forged_uid, gid_t forged_gid,
                   SenderCredentials sender, Caps caps, Outcome expected_outcome) {
    test_helper::ForkHelper helper;
    helper.RunInForkedProcess([this, forged_pid, forged_uid, forged_gid, sender, caps,
                               expected_outcome] {
      ASSERT_EQ(0, prctl(PR_SET_KEEPCAPS, 1, 0, 0, 0));

      ASSERT_THAT(setresgid(sender.gid, sender.egid, sender.sgid), SyscallSucceeds());
      ASSERT_THAT(setresuid(sender.uid, sender.euid, sender.suid), SyscallSucceeds());

      if (caps.has_cap_sys_admin) {
        test_helper::SetCapabilityEffective(CAP_SYS_ADMIN);
      } else {
        test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);
      }

      if (caps.has_cap_setuid) {
        test_helper::SetCapabilityEffective(CAP_SETUID);
      } else {
        test_helper::UnsetCapabilityEffective(CAP_SETUID);
      }

      if (caps.has_cap_setgid) {
        test_helper::SetCapabilityEffective(CAP_SETGID);
      } else {
        test_helper::UnsetCapabilityEffective(CAP_SETGID);
      }

      struct cmsghdr* cmsg = CMSG_FIRSTHDR(&msg_);
      cmsg->cmsg_level = SOL_SOCKET;
      cmsg->cmsg_type = SCM_CREDENTIALS;
      cmsg->cmsg_len = CMSG_LEN(sizeof(struct ucred));

      struct ucred cred;
      cred.pid = forged_pid.value_or(getpid());
      cred.uid = forged_uid;
      cred.gid = forged_gid;

      fbl::UnalignedStore(CMSG_DATA(cmsg), cred);

      if (expected_outcome.success) {
        EXPECT_THAT(sendmsg(sv_[0].get(), &msg_, 0), SyscallSucceeds());
      } else {
        EXPECT_THAT(sendmsg(sv_[0].get(), &msg_, 0), SyscallFailsWithErrno(expected_outcome.error));
      }
    });
    ASSERT_TRUE(helper.WaitForChildren());
  }

  fbl::unique_fd sv_[2] = {};
  char data_[1] = {'x'};
  struct iovec iov_ = {.iov_base = data_, .iov_len = 1};
  char cmsgbuf_[CMSG_SPACE(sizeof(struct ucred))] = {};
  struct msghdr msg_ = {};
};

TEST_F(ScmCredentialsTest, PidForgeryRequiresCapSysAdmin) {
  Caps no_caps = {};
  SenderCredentials sender = {
      .uid = 100, .euid = 100, .suid = 100, .gid = 200, .egid = 200, .sgid = 200};

  // PID mismatch rejected
  TestForgery(1, 100, 200, sender, no_caps, {.error = EPERM});

  // PID mismatch allowed by CAP_SYS_ADMIN
  TestForgery(1, 100, 200, sender, {.has_cap_sys_admin = true}, {.success = true});
}

TEST_F(ScmCredentialsTest, ZombiePidForgeryFails) {
  Caps no_caps = {};
  SenderCredentials sender = {
      .uid = 100, .euid = 100, .suid = 100, .gid = 200, .egid = 200, .sgid = 200};

  pid_t zombie_pid;
  test_helper::ForkHelper helper;
  zombie_pid = helper.RunInForkedProcess([] { _exit(0); });

  fbl::unique_fd pid_fd(static_cast<int>(syscall(SYS_pidfd_open, zombie_pid, 0u)));
  ASSERT_THAT(pid_fd.get(), SyscallSucceeds());

  pollfd pfd = {.fd = pid_fd.get(), .events = POLLIN};
  ASSERT_EQ(poll(&pfd, 1, -1), 1);
  EXPECT_EQ(pfd.revents, POLLIN);

  // Without CAP_SYS_ADMIN, it should fail with EPERM (forgery not allowed).
  TestForgery(zombie_pid, 100, 200, sender, no_caps, {.error = EPERM});

  // With CAP_SYS_ADMIN, it fails with ESRCH because the process is a zombie.
  TestForgery(zombie_pid, 100, 200, sender, {.has_cap_sys_admin = true}, {.error = ESRCH});

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(ScmCredentialsTest, ForgeryUid) {
  Caps no_caps = {};
  SenderCredentials sender = {
      .uid = 100, .euid = 101, .suid = 102, .gid = 200, .egid = 200, .sgid = 200};

  // UID mismatch (matches none of ruid, euid, suid) rejected
  TestForgery(std::nullopt, 103, 200, sender, no_caps, {.error = EPERM});

  // UID mismatch allowed by CAP_SETUID
  TestForgery(std::nullopt, 103, 200, sender, {.has_cap_setuid = true}, {.success = true});

  // Success if at least one UID matches
  TestForgery(std::nullopt, 100, 200, sender, no_caps, {.success = true});
  TestForgery(std::nullopt, 101, 200, sender, no_caps, {.success = true});
  TestForgery(std::nullopt, 102, 200, sender, no_caps, {.success = true});
}

TEST_F(ScmCredentialsTest, ForgeryGid) {
  Caps no_caps = {};
  SenderCredentials sender = {
      .uid = 100, .euid = 100, .suid = 100, .gid = 200, .egid = 201, .sgid = 202};

  // GID mismatch (matches none of rgid, egid, sgid) rejected
  TestForgery(std::nullopt, 100, 203, sender, no_caps, {.error = EPERM});

  // GID mismatch allowed by CAP_SETGID
  TestForgery(std::nullopt, 100, 203, sender, {.has_cap_setgid = true}, {.success = true});

  // Success if at least one GID matches
  TestForgery(std::nullopt, 100, 200, sender, no_caps, {.success = true});
  TestForgery(std::nullopt, 100, 201, sender, no_caps, {.success = true});
  TestForgery(std::nullopt, 100, 202, sender, no_caps, {.success = true});
}

}  // namespace
