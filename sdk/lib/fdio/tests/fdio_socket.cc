// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.posix.socket/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/limits.h>
#include <lib/fdio/unsafe.h>
#include <lib/fit/defer.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <future>
#include <latch>
#include <mutex>
#include <thread>

#include <fbl/unique_fd.h>
#include <zxtest/base/parameterized-value.h>
#include <zxtest/zxtest.h>

#include "predicates.h"
#include "src/connectivity/network/netstack/udp_serde/udp_serde.h"
#include "src/connectivity/network/tests/socket/util.h"

namespace {

class Server final : public fidl::testing::WireTestBase<fuchsia_posix_socket::StreamSocket> {
 public:
  explicit Server(zx::socket peer) : peer_(std::move(peer)) {}

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    ADD_FAILURE("%s should not be called", name.c_str());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket::StreamSocket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Close(CloseCompleter::Sync& completer) override {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Shutdown(ShutdownRequestView request, ShutdownCompleter::Sync& completer) override {
    shutdown_count_++;
    completer.ReplySuccess();
  }

  void Describe(DescribeCompleter::Sync& completer) override {
    zx::socket peer;
    if (const zx_status_t status =
            peer_.duplicate(ZX_RIGHTS_BASIC | ZX_RIGHT_READ | ZX_RIGHT_WRITE, &peer);
        status != ZX_OK) {
      return completer.Close(status);
    }
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket::wire::StreamSocketDescribeResponse::Builder(alloc)
                        .socket(std::move(peer))
                        .Build());
  }

  void Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) override {
    if (on_connect_) {
      on_connect_(peer_, completer);
    } else {
      fidl::testing::WireTestBase<fuchsia_posix_socket::StreamSocket>::Connect(request, completer);
    }
  }

  void Accept(AcceptRequestView request, AcceptCompleter::Sync& completer) override {
    if (on_accept_) {
      on_accept_(completer);
    } else {
      fidl::testing::WireTestBase<fuchsia_posix_socket::StreamSocket>::Accept(request, completer);
    }
  }

  void GetError(GetErrorCompleter::Sync& completer) override { completer.ReplySuccess(); }

  void FillPeerSocket() const {
    zx_info_socket_t info;
    ASSERT_OK(peer_.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr));
    size_t tx_buf_available = info.tx_buf_max - info.tx_buf_size;
    std::unique_ptr<uint8_t[]> buf(new uint8_t[tx_buf_available + 1]);
    size_t actual;
    ASSERT_OK(peer_.write(0, buf.get(), tx_buf_available, &actual));
    ASSERT_EQ(actual, tx_buf_available);
  }

  void ResetSocket() { peer_.reset(); }

  void SetOnConnect(fit::function<void(zx::socket&, ConnectCompleter::Sync&)> cb) {
    on_connect_ = std::move(cb);
  }

  void SetOnAccept(fit::function<void(AcceptCompleter::Sync&)> cb) { on_accept_ = std::move(cb); }

  uint16_t ShutdownCount() const { return shutdown_count_.load(); }

 private:
  zx::socket peer_;
  std::atomic<uint16_t> shutdown_count_ = 0;

  fit::function<void(zx::socket&, ConnectCompleter::Sync&)> on_connect_;
  fit::function<void(AcceptCompleter::Sync&)> on_accept_;
};

template <int sock_type>
class BaseTest : public zxtest::Test {
  static_assert(sock_type == ZX_SOCKET_STREAM || sock_type == ZX_SOCKET_DATAGRAM);

 public:
  BaseTest() : loop_(&kAsyncLoopConfigNoAttachToCurrentThread) {}

 protected:
  void SetUp() override {
    zx::socket client_socket;
    ASSERT_OK(zx::socket::create(sock_type, &client_socket, &server_socket_));
    server_.emplace(std::move(client_socket));

    auto endpoints = fidl::Endpoints<fuchsia_posix_socket::StreamSocket>::Create();

    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), &server_.value());
    ASSERT_OK(loop_.StartThread("fake-socket-server"));
    ASSERT_OK(
        fdio_fd_create(endpoints.client.channel().release(), client_fd_.reset_and_get_address()));
  }

  const zx::socket& server_socket() { return server_socket_; }

  zx::socket& mutable_server_socket() { return server_socket_; }

  const fbl::unique_fd& client_fd() { return client_fd_; }

  fbl::unique_fd& mutable_client_fd() { return client_fd_; }

  const Server& server() { return server_.value(); }

  Server& mutable_server() { return server_.value(); }

  async_dispatcher_t* dispatcher() { return loop_.dispatcher(); }

  void set_connected() {
    mutable_server().SetOnConnect(
        [connected = false](zx::socket& peer, Server::ConnectCompleter::Sync& completer) mutable {
          switch (sock_type) {
            case ZX_SOCKET_STREAM:
              if (!connected) {
                connected = true;
                // We need the FDIO to act like it's connected.
                EXPECT_OK(peer.signal(0, fuchsia_posix_socket::wire::kSignalStreamConnected));
                completer.ReplyError(fuchsia_posix::wire::Errno::kEinprogress);
                break;
              }
              __FALLTHROUGH;
            case ZX_SOCKET_DATAGRAM:
              completer.ReplySuccess();
              break;
          }
        });
    const sockaddr_in addr = {
        .sin_family = AF_INET,
    };
    ASSERT_SUCCESS(
        connect(client_fd().get(), reinterpret_cast<const sockaddr*>(&addr), sizeof(addr)));
  }

  void set_nonblocking_io() {
    int flags;
    ASSERT_GE(flags = fcntl(client_fd().get(), F_GETFL), 0, "%s", strerror(errno));
    ASSERT_SUCCESS(fcntl(client_fd().get(), F_SETFL, flags | O_NONBLOCK));
  }

 private:
  zx::socket clientSocket() {}

  zx::socket server_socket_;
  fbl::unique_fd client_fd_;
  std::optional<Server> server_;
  async::Loop loop_;
};

using TcpSocketTest = BaseTest<ZX_SOCKET_STREAM>;
TEST_F(TcpSocketTest, CloseZXSocketOnTransfer) {
  // A socket's peer is not closed until all copies of that peer are closed. Since the server holds
  // one of those copies (and the file descriptor holds the other), we must destroy the server's
  // copy before asserting that fdio_fd_transfer closes the file descriptor's copy.
  mutable_server().ResetSocket();

  // The file descriptor still holds a copy of the peer; the peer is still open.
  ASSERT_OK(server_socket().wait_one(ZX_SOCKET_WRITABLE, zx::time::infinite_past(), nullptr));

  zx::handle handle;
  ASSERT_OK(fdio_fd_transfer(client_fd().get(), handle.reset_and_get_address()));

  // The file descriptor has been destroyed; the peer is closed.
  ASSERT_OK(server_socket().wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite_past(), nullptr));
}

// Verify scenario, where multi-segment recvmsg is requested, but the socket has
// just enough data to *completely* fill one segment.
// In this scenario, an attempt to read data for the next segment immediately
// fails with ZX_ERR_SHOULD_WAIT, and this may lead to bogus EAGAIN even if some
// data has actually been read.
TEST_F(TcpSocketTest, RecvmsgNonblockBoundary) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  ASSERT_NO_FATAL_FAILURE(set_nonblocking_io());

  // Write 4 bytes of data to socket.
  size_t actual;
  const uint32_t data_out = 0x12345678;
  EXPECT_OK(server_socket().write(0, &data_out, sizeof(data_out), &actual));
  EXPECT_EQ(actual, sizeof(data_out));

  uint32_t data_in1, data_in2;
  // Fail at compilation stage if anyone changes types.
  // This is mandatory here: we need the first chunk to be exactly the same
  // length as total size of data we just wrote.
  static_assert(sizeof(data_in1) == sizeof(data_out));

  struct iovec iov[] = {
      {
          .iov_base = &data_in1,
          .iov_len = sizeof(data_in1),
      },
      {
          .iov_base = &data_in2,
          .iov_len = sizeof(data_in2),
      },
  };

  struct msghdr msg = {
      .msg_iov = iov,
      .msg_iovlen = std::size(iov),
  };

  EXPECT_EQ(recvmsg(client_fd().get(), &msg, 0), ssize_t(sizeof(data_out)), "%s", strerror(errno));

  EXPECT_SUCCESS(close(mutable_client_fd().release()));
}

// Make sure we can successfully read zero bytes if we pass a zero sized input buffer.
TEST_F(TcpSocketTest, RecvmsgEmptyBuffer) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  ASSERT_NO_FATAL_FAILURE(set_nonblocking_io());

  // Write 4 bytes of data to socket.
  size_t actual;
  const uint32_t data_out = 0x12345678;
  EXPECT_OK(server_socket().write(0, &data_out, sizeof(data_out), &actual));
  EXPECT_EQ(actual, sizeof(data_out));

  // Try to read into an empty set of io vectors.
  struct msghdr msg = {};

  // We should "successfully" read zero bytes.
  EXPECT_SUCCESS(recvmsg(client_fd().get(), &msg, 0));
}

// Verify scenario, where multi-segment sendmsg is requested, but the socket has
// just enough spare buffer to *completely* read one segment.
// In this scenario, an attempt to send second segment should immediately fail
// with ZX_ERR_SHOULD_WAIT, but the sendmsg should report first segment length
// rather than failing with EAGAIN.
TEST_F(TcpSocketTest, SendmsgNonblockBoundary) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  ASSERT_NO_FATAL_FAILURE(set_nonblocking_io());

  const size_t memlength = 65536;
  std::unique_ptr<uint8_t[]> memchunk(new uint8_t[memlength]);

  struct iovec iov[]{
      {
          .iov_base = memchunk.get(),
          .iov_len = memlength,
      },
      {
          .iov_base = memchunk.get(),
          .iov_len = memlength,
      },
  };

  const struct msghdr msg = {
      .msg_iov = iov,
      .msg_iovlen = std::size(iov),
  };

  // 1. Fill up the client socket.
  server().FillPeerSocket();

  // 2. Consume one segment of the data
  size_t actual;
  EXPECT_OK(server_socket().read(0, memchunk.get(), memlength, &actual));
  EXPECT_EQ(memlength, actual);

  // 3. Push again 2 packets of <memlength> bytes, observe only one sent.
  EXPECT_EQ(sendmsg(client_fd().get(), &msg, 0), (ssize_t)memlength, "%s", strerror(errno));

  EXPECT_SUCCESS(close(mutable_client_fd().release()));
}

TEST_F(TcpSocketTest, WaitBeginEndConnecting) {
  ASSERT_NO_FATAL_FAILURE(set_nonblocking_io());

  // Like set_connected, but does not advance to the connected state.
  mutable_server().SetOnConnect([](zx::socket& peer, Server::ConnectCompleter::Sync& completer) {
    completer.ReplyError(fuchsia_posix::wire::Errno::kEinprogress);
  });
  const sockaddr_in addr = {
      .sin_family = AF_INET,
  };
  ASSERT_EQ(connect(client_fd().get(), reinterpret_cast<const sockaddr*>(&addr), sizeof(addr)), -1);
  ASSERT_ERRNO(EINPROGRESS);

  fdio_t* io = fdio_unsafe_fd_to_io(client_fd().get());
  auto release = fit::defer([io]() { fdio_unsafe_release(io); });

  zx_handle_t handle;

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLIN, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(fuchsia_posix_socket::wire::kSignalStreamIncoming | ZX_SOCKET_READABLE |
                  ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_PEER_WRITE_DISABLED,
              signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLOUT, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(fuchsia_posix_socket::wire::kSignalStreamConnected | ZX_SOCKET_PEER_CLOSED |
                  ZX_SOCKET_WRITE_DISABLED,
              signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLRDHUP, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_PEER_WRITE_DISABLED, signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLHUP, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_PEER_CLOSED, signals);
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_READABLE, &events);
    EXPECT_EQ(0, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_PEER_CLOSED, &events);
    EXPECT_EQ(POLLIN | POLLOUT | POLLERR | POLLHUP | POLLRDHUP, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_PEER_WRITE_DISABLED, &events);
    EXPECT_EQ(POLLIN | POLLRDHUP, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_WRITABLE, &events);
    EXPECT_EQ(0, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_WRITE_DISABLED, &events);
    EXPECT_EQ(POLLOUT | POLLHUP, int32_t(events));
  }
}

TEST_F(TcpSocketTest, WaitBeginEndConnected) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  fdio_t* io = fdio_unsafe_fd_to_io(client_fd().get());
  auto release = fit::defer([io]() { fdio_unsafe_release(io); });

  zx_handle_t handle;

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLIN, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_PEER_WRITE_DISABLED, signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLOUT, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_WRITABLE | ZX_SOCKET_WRITE_DISABLED, signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLRDHUP, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_PEER_WRITE_DISABLED, signals);
  }

  {
    zx_signals_t signals;
    fdio_unsafe_wait_begin(io, POLLHUP, &handle, &signals);
    EXPECT_NE(ZX_HANDLE_INVALID, handle);
    EXPECT_EQ(ZX_SOCKET_PEER_CLOSED, signals);
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_READABLE, &events);
    EXPECT_EQ(POLLIN, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_PEER_CLOSED, &events);
    EXPECT_EQ(POLLIN | POLLOUT | POLLERR | POLLHUP | POLLRDHUP, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_PEER_WRITE_DISABLED, &events);
    EXPECT_EQ(POLLIN | POLLRDHUP, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_WRITABLE, &events);
    EXPECT_EQ(POLLOUT, int32_t(events));
  }

  {
    uint32_t events;
    fdio_unsafe_wait_end(io, ZX_SOCKET_WRITE_DISABLED, &events);
    EXPECT_EQ(POLLOUT | POLLHUP, int32_t(events));
  }
}

TEST_F(TcpSocketTest, Shutdown) {
  ASSERT_EQ(shutdown(client_fd().get(), SHUT_RD), 0, "%s", strerror(errno));
  ASSERT_EQ(server().ShutdownCount(), 1);
}

TEST_F(TcpSocketTest, GetReadBufferAvailable) {
  int available = 0;
  EXPECT_EQ(ioctl(client_fd().get(), FIONREAD, &available), 0, "%s", strerror(errno));
  EXPECT_EQ(available, 0);

  constexpr size_t data_size = 47;
  std::array<char, data_size> data_buf;
  size_t actual = 0;
  EXPECT_OK(server_socket().write(0u, data_buf.data(), data_buf.size(), &actual));
  EXPECT_EQ(actual, data_size);

  EXPECT_EQ(ioctl(client_fd().get(), FIONREAD, &available), 0, "%s", strerror(errno));
  EXPECT_EQ(available, data_size);
}

TEST_F(TcpSocketTest, PollNoEvents) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  struct pollfd pfds[] = {
      {
          .fd = client_fd().get(),
          .events = 0,
      },
  };

  EXPECT_EQ(poll(pfds, std::size(pfds), 5), 0, "error: %s", strerror(errno));
}

TEST_F(TcpSocketTest, GetFlags) { ASSERT_GE(fcntl(client_fd().get(), F_GETFL), 0); }

TEST_F(TcpSocketTest, AcceptStolenReservation) {
  // Find the lowest available FD. This is what accept will try to reserve first.
  int reserved_fd = -1;
  for (int fd = 0; fd < FDIO_MAX_FD; ++fd) {
    if (fcntl(fd, F_GETFD) == -1 && errno == EBADF) {
      reserved_fd = fd;
      break;
    }
  }
  ASSERT_GE(reserved_fd, 0);

  std::mutex accept_called_mtx;
  std::condition_variable accept_called_cv;
  bool accept_called = false;
  std::mutex reservation_stolen_mtx;
  std::condition_variable reservation_stolen_cv;
  bool reservation_stolen = false;

  std::unique_ptr<Server> accepted_server;

  mutable_server().SetOnAccept([&](Server::AcceptCompleter::Sync& completer) {
    auto endpoints = fidl::Endpoints<fuchsia_posix_socket::StreamSocket>::Create();
    zx::socket client_socket, server_socket;
    ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &client_socket, &server_socket));

    // We need to keep the accepted server alive.
    accepted_server = std::make_unique<Server>(std::move(server_socket));
    fidl::BindServer(dispatcher(), std::move(endpoints.server), accepted_server.get());

    {
      std::lock_guard l(accept_called_mtx);
      accept_called = true;
      accept_called_cv.notify_all();
    }
    {
      std::unique_lock l(reservation_stolen_mtx);
      while (!reservation_stolen) {
        reservation_stolen_cv.wait(l);
      }
    }

    fidl::Arena alloc;
    fuchsia_net::wire::Ipv4SocketAddress fidl_addr = {};
    completer.ReplySuccess(fuchsia_net::wire::SocketAddress::WithIpv4(alloc, fidl_addr),
                           std::move(endpoints.client));
  });

  fbl::unique_fd accepted_fd;
  std::thread thr([&] { accepted_fd.reset(accept(client_fd().get(), nullptr, nullptr)); });

  {
    std::unique_lock l(accept_called_mtx);
    while (!accept_called) {
      accept_called_cv.wait(l);
    }
  }

  // Now steal the reservation.
  fbl::unique_fd stolen_fd(dup2(client_fd().get(), reserved_fd));
  ASSERT_EQ(stolen_fd.get(), reserved_fd);

  {
    std::unique_lock l(reservation_stolen_mtx);
    reservation_stolen = true;
    reservation_stolen_cv.notify_all();
  }

  thr.join();
  ASSERT_GE(accepted_fd.get(), 0, "accept failed: %s", strerror(errno));
  ASSERT_NE(accepted_fd.get(), reserved_fd);
}

using UdpSocketTest = BaseTest<ZX_SOCKET_DATAGRAM>;
TEST_F(UdpSocketTest, DatagramSendMsg) {
  ASSERT_NO_FATAL_FAILURE(set_connected());

  {
    const struct msghdr msg = {};
    // sendmsg should accept 0 length payload.
    EXPECT_SUCCESS(sendmsg(client_fd().get(), &msg, 0));
    // no data will have arrived on the other end.
    constexpr size_t prior = 1337;
    size_t actual = prior;
    std::array<char, 1> rcv_buf;
    EXPECT_EQ(server_socket().read(0, rcv_buf.data(), rcv_buf.size(), &actual), ZX_ERR_SHOULD_WAIT);
    EXPECT_EQ(actual, prior);
  }

  struct sockaddr_in addr = {
      .sin_family = AF_INET,
      .sin_addr =
          {
              .s_addr = htonl(INADDR_LOOPBACK),
          },
  };
  const socklen_t addrlen = sizeof(addr);

  constexpr char payload[] = "hello";
  struct iovec iov[] = {
      {
          .iov_base = static_cast<void*>(const_cast<char*>(payload)),
          .iov_len = sizeof(payload),
      },
  };

  struct msghdr msg = {
      .msg_name = &addr,
      .msg_namelen = addrlen,
      .msg_iov = iov,
      .msg_iovlen = std::size(iov),
  };

  EXPECT_EQ(sendmsg(client_fd().get(), &msg, 0), ssize_t(sizeof(payload)), "%s", strerror(errno));

  // sendmsg doesn't fail when msg_namelen is greater than sizeof(struct sockaddr_storage) because
  // what's being tested here is a fuchsia.posix.socket.StreamSocket backed by a
  // zx::socket(ZX_SOCKET_DATAGRAM), a Frankenstein's monster which implements stream semantics on
  // the network and datagram semantics on the transport to the netstack.
  msg.msg_namelen = sizeof(sockaddr_storage) + 1;
  EXPECT_EQ(sendmsg(client_fd().get(), &msg, 0), ssize_t(sizeof(payload)), "%s", strerror(errno));

  {
    size_t actual;
    std::array<char, sizeof(payload) + 1> rcv_buf;
    for (int i = 0; i < 2; i++) {
      EXPECT_OK(server_socket().read(0, rcv_buf.data(), rcv_buf.size(), &actual));
      EXPECT_EQ(actual, sizeof(payload));
    }
  }

  EXPECT_SUCCESS(close(mutable_client_fd().release()));
}

TEST_F(UdpSocketTest, Shutdown) {
  ASSERT_EQ(shutdown(client_fd().get(), SHUT_RD), 0, "%s", strerror(errno));
  ASSERT_EQ(server().ShutdownCount(), 1);
}

TEST_F(UdpSocketTest, GetFlags) { ASSERT_GE(fcntl(client_fd().get(), F_GETFL), 0); }

class TcpSocketTimeoutTest : public TcpSocketTest {
 protected:
  template <int optname>
  void timeout(fbl::unique_fd& fd, zx::socket& server_socket) {
    static_assert(optname == SO_RCVTIMEO || optname == SO_SNDTIMEO);

    // We want this to be a small number so the test is fast, but at least 1
    // second so that we exercise `tv_sec`.
    const auto timeout = std::chrono::seconds(1) + std::chrono::milliseconds(50);
    {
      const auto sec = std::chrono::duration_cast<std::chrono::seconds>(timeout);
      const struct timeval tv = {
          .tv_sec = sec.count(),
          .tv_usec = std::chrono::duration_cast<std::chrono::microseconds>(timeout - sec).count(),
      };
      ASSERT_SUCCESS(setsockopt(fd.get(), SOL_SOCKET, optname, &tv, sizeof(tv)));
      struct timeval actual_tv;
      socklen_t optlen = sizeof(actual_tv);
      ASSERT_EQ(getsockopt(fd.get(), SOL_SOCKET, optname, &actual_tv, &optlen), 0, "%s",
                strerror(errno));
      ASSERT_EQ(optlen, sizeof(actual_tv));
      ASSERT_EQ(actual_tv.tv_sec, tv.tv_sec);
      ASSERT_EQ(actual_tv.tv_usec, tv.tv_usec);
    }

    const auto margin = std::chrono::milliseconds(50);

    uint8_t buf[16];

    // Perform the read/write. This is the core of the test - we expect the operation to time out
    // per our setting of the timeout above.

    const auto start = std::chrono::steady_clock::now();

    switch (optname) {
      case SO_RCVTIMEO:
        ASSERT_EQ(read(fd.get(), buf, sizeof(buf)), -1);
        break;
      case SO_SNDTIMEO:
        ASSERT_EQ(write(fd.get(), buf, sizeof(buf)), -1);
        break;
    }
    ASSERT_TRUE(errno == EAGAIN || errno == EWOULDBLOCK, "%s", strerror(errno));

    const auto elapsed = std::chrono::steady_clock::now() - start;

    // Check that the actual time waited was close to the expectation.
    const auto elapsed_ms = std::chrono::duration_cast<std::chrono::milliseconds>(elapsed);
    const auto timeout_ms = std::chrono::duration_cast<std::chrono::milliseconds>(timeout);

    // TODO(https://fxbug.dev/42116074): Only the lower bound of the elapsed time is checked. The
    // upper bound check is ignored as the syscall could far miss the defined deadline to return.
    EXPECT_GT(elapsed, timeout - margin, "elapsed=%lld ms (which is not within %lld ms of %lld ms)",
              elapsed_ms.count(), margin.count(), timeout_ms.count());

    // Remove the timeout.
    const struct timeval tv = {};
    ASSERT_SUCCESS(setsockopt(fd.get(), SOL_SOCKET, optname, &tv, sizeof(tv)));
    // Wrap the read/write in a future to enable a timeout. We expect the future
    // to time out.
    std::latch fut_started(1);
    auto fut = std::async(std::launch::async, [&]() -> std::pair<ssize_t, int> {
      fut_started.count_down();

      switch (optname) {
        case SO_RCVTIMEO:
          return std::make_pair(read(fd.get(), buf, sizeof(buf)), errno);
        case SO_SNDTIMEO:
          return std::make_pair(write(fd.get(), buf, sizeof(buf)), errno);
      }
    });
    fut_started.wait();
    EXPECT_EQ(fut.wait_for(margin), std::future_status::timeout);
    // Resetting the remote end socket should cause the read/write to complete.
    server_socket.reset();
    // Closing the socket without returning an error from `getsockopt(_, SO_ERROR, ...)` looks like
    // the connection was gracefully closed. The same behavior is exercised in
    // src/connectivity/network/tests/bsdsocket_test.cc:{StopListenWhileConnect,BlockedIOTest/CloseWhileBlocked}.
    auto return_code_and_errno = fut.get();
    switch (optname) {
      case SO_RCVTIMEO:
        EXPECT_EQ(return_code_and_errno.first, 0);
        break;
      case SO_SNDTIMEO:
        EXPECT_EQ(return_code_and_errno.first, -1);
        ASSERT_EQ(return_code_and_errno.second, EPIPE, "%s",
                  strerror(return_code_and_errno.second));
        break;
    }

    ASSERT_SUCCESS(close(fd.release()));
  }
};

TEST_F(TcpSocketTimeoutTest, Rcv) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  timeout<SO_RCVTIMEO>(mutable_client_fd(), mutable_server_socket());
}

TEST_F(TcpSocketTimeoutTest, Snd) {
  ASSERT_NO_FATAL_FAILURE(set_connected());
  server().FillPeerSocket();
  timeout<SO_SNDTIMEO>(mutable_client_fd(), mutable_server_socket());
}

// An arbitrary maximum payload size.
constexpr size_t kUdpMaxPayloadSize = 60000;

class DatagramSocketServer final
    : public fidl::testing::WireTestBase<fuchsia_posix_socket::DatagramSocket> {
 public:
  explicit DatagramSocketServer(zx::socket socket) : socket_(std::move(socket)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE("unexpected message received: %s", name.c_str());
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) override {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket::DatagramSocket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    ASSERT_TRUE(socket_.is_valid());
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket::wire::DatagramSocketDescribeResponse::Builder(alloc)
                        .socket(std::move(socket_))
                        .tx_meta_buf_size(kTxUdpPreludeSize)
                        .rx_meta_buf_size(kRxUdpPreludeSize)
                        .metadata_encoding_protocol_version({})
                        .Build());
  }

  void SendMsgPreflight(fuchsia_posix_socket::wire::DatagramSocketSendMsgPreflightRequest* request,
                        SendMsgPreflightCompleter::Sync& completer) override {
    fuchsia_net::wire::Ipv4SocketAddress fidl_addr = {
        .port = 8080,
    };
    in_addr_t addr = htonl(INADDR_LOOPBACK);
    memcpy(fidl_addr.address.addr.data(), &addr, sizeof(addr));
    // Providing no eventpairs means that the client's cache will never be
    // invalidated; every subsequent preflight check will succeed client-side.
    std::array<zx::eventpair, 0> eventpairs = {};

    fidl::Arena alloc;
    fidl::WireTableBuilder response_builder =
        fuchsia_posix_socket::wire::DatagramSocketSendMsgPreflightResponse::Builder(alloc);
    response_builder.to(fuchsia_net::wire::SocketAddress::WithIpv4(alloc, fidl_addr))
        .validity(fidl::VectorView<zx::eventpair>::FromExternal(eventpairs))
        .maximum_size(kUdpMaxPayloadSize);
    completer.ReplySuccess(response_builder.Build());
  }

  zx::socket socket_;
};

class BlockingOp {
 public:
  enum class Op {
    SEND,
    POLL,
    SELECT,
  };

  explicit BlockingOp(const Op& op) : op_(op) {}

  void Execute(const fbl::unique_fd& fd, const std::vector<char>& buf) {
    switch (op_) {
      case Op::SEND: {
        ssize_t n = send(fd.get(), buf.data(), buf.size(), /* flags */ 0);
        ASSERT_GE(n, 0, "%s", strerror(errno));
        ASSERT_EQ(n, ssize_t(buf.size()));
      } break;
      case Op::POLL: {
        pollfd pfd = {
            .fd = fd.get(),
            .events = POLLOUT,
        };
        int n = poll(&pfd, 1, /* infinite timeout */ -1);
        ASSERT_GE(n, 0, "%s", strerror(errno));
        ASSERT_EQ(n, 1);
        ASSERT_EQ(pfd.revents, POLLOUT);
      } break;
      case Op::SELECT: {
        fd_set writefds;
        FD_ZERO(&writefds);
        FD_SET(fd.get(), &writefds);
        int n = select(fd.get() + 1, /* readfds */ nullptr, &writefds,
                       /* exceptfds */ nullptr, /* infinite timeout */ nullptr);
        ASSERT_GE(n, 0, "%s", strerror(errno));
        ASSERT_EQ(n, 1);
        ASSERT_TRUE(FD_ISSET(fd.get(), &writefds));
      } break;
    }
  }

  static constexpr const char* Name(enum Op op) {
    switch (op) {
      case Op::SEND:
        return "Send";
      case Op::POLL:
        return "Poll";
      case Op::SELECT:
        return "Select";
    }
  }

 private:
  const enum Op op_;
};

class DatagramSocketTest : public zxtest::TestWithParam<BlockingOp::Op> {
 public:
  const size_t kWriteThreshold = kUdpMaxPayloadSize + kTxUdpPreludeSize;

  void SetUp() final {
    zx::socket socket;
    ASSERT_OK(zx::socket::create(ZX_SOCKET_DATAGRAM, &socket, &peer_));
    ASSERT_OK(socket.get_info(ZX_INFO_SOCKET, &info_, sizeof(info_), nullptr, nullptr));
    ASSERT_OK(socket.set_property(ZX_PROP_SOCKET_TX_THRESHOLD, &kWriteThreshold,
                                  sizeof(kWriteThreshold)));

    zx::result server_end = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(server_end.status_value());
    fidl::BindServer(control_loop_.dispatcher(), std::move(server_end.value()),
                     &server_.emplace(std::move(socket)));
    control_loop_.StartThread("control");
  }

  void TearDown() final { control_loop_.Shutdown(); }

  const zx_info_socket_t& info() const { return info_; }
  zx::socket TakePeer() { return std::move(peer_); }
  fidl::ClientEnd<fsocket::DatagramSocket> TakeClientEnd() { return std::move(client_end_); }

 private:
  zx::socket peer_;
  zx_info_socket_t info_;
  fidl::ClientEnd<fsocket::DatagramSocket> client_end_;
  std::optional<DatagramSocketServer> server_;
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_P(DatagramSocketTest, WriteWithTxZirconSocketRemainder) {
  // Fast datagram sockets on Fuchsia use multiple buffers to store outbound
  // payloads, some of which are in Netstack memory and some of which are in
  // kernel memory. Bytes are shuttled between these buffers using goroutines.
  //
  // One edge case arises when the kernel buffers have free space, but not so
  // much space that they can accept the next datagram payload. In this case,
  // the client waits until the kernel object can accept the maximum payload
  // size to ensure that the next write will succeed, in an operation known as a
  // threshold wait.
  //
  // This test exercises this scenario.
  fbl::unique_fd fd;
  ASSERT_OK(fdio_fd_create(TakeClientEnd().TakeChannel().release(), fd.reset_and_get_address()));

  // Pick a payload size which ensures that the zircon socket will have a
  // "remainder".
  //
  // In other words, choose a size by which the zircon socket's capacity is not
  // divisible, such that even when the maximum amount of payloads are written
  // into the socket, there will be some capacity remaining, and therefore the
  // socket will still be considered writable.
  size_t payload_size = std::min(kUdpMaxPayloadSize, info().tx_buf_max - kTxUdpPreludeSize);
  size_t total_size = payload_size + kTxUdpPreludeSize;
  {
    for (; payload_size > 0; --payload_size) {
      total_size = payload_size + kTxUdpPreludeSize;
      if (info().tx_buf_max % total_size != 0) {
        break;
      }
    }
    ASSERT_GT(
        payload_size, 0,
        "couldn't find valid UDP payload size for which (zx_socket_info.tx_buf_max %% payload size) != 0");
  }

  // Send enough packets to fill the zircon socket.
  std::vector<char> buf(payload_size, 'a');
  for (size_t remaining = info().tx_buf_max; remaining > total_size; remaining -= total_size) {
    ASSERT_EQ(send(fd.get(), buf.data(), buf.size(), /* flags */ 0), ssize_t(buf.size()),
              "%s: %zu capacity remaining in socket", strerror(errno), remaining);
  }

  // The next operation should block because the zircon socket is full.
  BlockingOp op(GetParam());
  std::latch op_started(1);
  const auto fut = std::async(std::launch::async, [&]() {
    op_started.count_down();
    op.Execute(fd, buf);
  });
  op_started.wait();
  ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

  // Dequeue packets from the netstack's end of the zircon socket; the operation
  // should continue to block until capacity has reached the write threshold.
  zx::socket peer = TakePeer();
  std::vector<char> recvbuf;
  recvbuf.resize(buf.size() + kTxUdpPreludeSize);
  for (size_t capacity = info().tx_buf_max % total_size; capacity < kWriteThreshold;
       capacity += total_size) {
    ASSERT_NO_FATAL_FAILURE(AssertBlocked(fut));

    size_t actual;
    ASSERT_OK(peer.read(/* options */ 0, recvbuf.data(), recvbuf.size(), &actual));
    ASSERT_EQ(actual, recvbuf.size());
    EXPECT_EQ(std::string_view(recvbuf.data() + kTxUdpPreludeSize, buf.size()),
              std::string_view(buf.data(), buf.size()));
  }

  // Now that the capacity in the socket has crossed the write threshold, the
  // blocking operation should be unblocked.
  fut.wait();
}

INSTANTIATE_TEST_SUITE_P(DatagramSocketTests, DatagramSocketTest,
                         zxtest::Values(BlockingOp::Op::SEND, BlockingOp::Op::POLL,
                                        BlockingOp::Op::SELECT),
                         [](const auto info) { return BlockingOp::Name(info.param); });

}  // namespace
