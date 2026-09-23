// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <fidl/fuchsia.net/cpp/wire.h>
#include <fidl/fuchsia.posix.socket.packet/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket.raw/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/socket.h>
#include <lib/zxio/cpp/create_with_type.h>
#include <lib/zxio/zxio.h>
#include <zircon/types.h>

#include <zxtest/zxtest.h>

#include "sdk/lib/zxio/dgram_cache.h"
#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/socket_address.h"
#include "sdk/lib/zxio/tests/test_socket_server.h"

namespace fnet = fuchsia_net;
namespace fposix = fuchsia_posix;
namespace fsocket = fuchsia_posix_socket;
namespace fsocket_packet = fuchsia_posix_socket_packet;
namespace fsocket_raw = fuchsia_posix_socket_raw;

namespace std {
ostream& operator<<(ostream& os, const ErrOrOutCode& error) {
  return os << (error.is_error() ? error.status_string() : strerror(error.value()));
}
}  // namespace std

namespace {

class SynchronousDatagramSocketTest : public zxtest::Test {
 public:
  void SetUp() final {
    ASSERT_OK(zx::eventpair::create(0u, &event0_, &event1_));

    zx::result node_server = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(node_server.status_value());

    fidl::BindServer(control_loop_.dispatcher(), std::move(*node_server), &server_);
    control_loop_.StartThread("control");
  }

  void Init() {
    ASSERT_OK(zxio_synchronous_datagram_socket_init(&storage_, TakeEvent(), TakeClientEnd()));
    zxio_ = &storage_.io;
  }

  void TearDown() final {
    if (zxio_) {
      ASSERT_OK(zxio_close(zxio_));
      zxio_destroy(zxio_);
    }
    control_loop_.Shutdown();
  }

  zx::eventpair TakeEvent() { return std::move(event0_); }
  fidl::ClientEnd<fsocket::SynchronousDatagramSocket> TakeClientEnd() {
    return std::move(client_end_);
  }
  zxio_storage_t* storage() { return &storage_; }
  zxio_t* zxio() { return zxio_; }

 private:
  zxio_storage_t storage_;
  zxio_t* zxio_{nullptr};
  zx::eventpair event0_, event1_;
  fidl::ClientEnd<fsocket::SynchronousDatagramSocket> client_end_;
  zxio_tests::SynchronousDatagramSocketServer server_{{}};
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(SynchronousDatagramSocketTest, Basic) { Init(); }

TEST_F(SynchronousDatagramSocketTest, Release) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_release(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);

  EXPECT_OK(zx_handle_close(handle));
}

TEST_F(SynchronousDatagramSocketTest, Borrow) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_borrow(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);
}

TEST_F(SynchronousDatagramSocketTest, CreateWithType) {
  ASSERT_OK(zxio_create_with_type(storage(), ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET,
                                  TakeEvent().release(), TakeClientEnd().TakeChannel().release()));
  ASSERT_OK(zxio_close(&storage()->io));
  zxio_destroy(&storage()->io);
}

TEST_F(SynchronousDatagramSocketTest, Attr) {
  Init();

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(zxio(), &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET, attr.object_type);
}

class StreamSocketTest : public zxtest::Test {
 public:
  void SetUp() final {
    ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &socket_, &peer_));
    ASSERT_OK(socket_.get_info(ZX_INFO_SOCKET, &info_, sizeof(info_), nullptr, nullptr));

    zx::result server_end = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(server_end.status_value());

    fidl::BindServer(control_loop_.dispatcher(), std::move(*server_end), &server_);
    control_loop_.StartThread("control");
  }

  void Init() {
    ASSERT_OK(zxio_stream_socket_init(&storage_, TakeSocket(), info(), /*is_connected=*/false,
                                      TakeClientEnd()));
    zxio_ = &storage_.io;
  }

  void TearDown() final {
    if (zxio_) {
      ASSERT_OK(zxio_close(zxio_));
      zxio_destroy(zxio_);
    }
    control_loop_.Shutdown();
  }

  zx_info_socket_t& info() { return info_; }
  zx::socket TakeSocket() { return std::move(socket_); }
  fidl::ClientEnd<fsocket::StreamSocket> TakeClientEnd() { return std::move(client_end_); }
  zxio_storage_t* storage() { return &storage_; }
  zxio_t* zxio() { return zxio_; }

 private:
  zxio_storage_t storage_;
  zxio_t* zxio_{nullptr};
  zx_info_socket_t info_;
  zx::socket socket_, peer_;
  fidl::ClientEnd<fsocket::StreamSocket> client_end_;
  zxio_tests::StreamSocketServer server_{{}};
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(StreamSocketTest, Basic) { Init(); }

TEST_F(StreamSocketTest, Release) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_release(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);

  EXPECT_OK(zx_handle_close(handle));
}

TEST_F(StreamSocketTest, Borrow) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_borrow(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);
}

TEST_F(StreamSocketTest, CreateWithType) {
  ASSERT_OK(zxio_create_with_type(storage(), ZXIO_OBJECT_TYPE_STREAM_SOCKET, TakeSocket().release(),
                                  &info(), /*is_connected=*/false,
                                  TakeClientEnd().TakeChannel().release()));
  ASSERT_OK(zxio_close(&storage()->io));
  zxio_destroy(&storage()->io);
}

TEST_F(StreamSocketTest, Attr) {
  Init();

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(zxio(), &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_STREAM_SOCKET, attr.object_type);
}

class DatagramSocketTest : public zxtest::Test {
 public:
  void SetUp() final {
    ASSERT_OK(zx::socket::create(ZX_SOCKET_DATAGRAM, &socket_, &peer_));
    ASSERT_OK(socket_.get_info(ZX_INFO_SOCKET, &info_, sizeof(info_), nullptr, nullptr));

    zx::result server_end = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(server_end.status_value());

    fidl::BindServer(control_loop_.dispatcher(), std::move(*server_end), &server_);
    control_loop_.StartThread("control");
  }

  void Init() {
    ASSERT_OK(zxio_datagram_socket_init(&storage_, TakeSocket(), info(), prelude_size(),
                                        TakeClientEnd()));
    zxio_ = &storage_.io;
  }

  void TearDown() final {
    if (zxio_) {
      ASSERT_OK(zxio_close(zxio_));
      zxio_destroy(zxio_);
    }
    control_loop_.Shutdown();
  }

  const zx_info_socket_t& info() const { return info_; }
  const zxio_datagram_prelude_size_t& prelude_size() const { return prelude_size_; }
  zx::socket TakeSocket() { return std::move(socket_); }
  fidl::ClientEnd<fsocket::DatagramSocket> TakeClientEnd() { return std::move(client_end_); }
  zxio_storage_t* storage() { return &storage_; }
  zxio_t* zxio() { return zxio_; }

 private:
  zxio_storage_t storage_;
  zxio_t* zxio_{nullptr};
  zx_info_socket_t info_;
  const zxio_datagram_prelude_size_t prelude_size_{};
  zx::socket socket_, peer_;
  fidl::ClientEnd<fsocket::DatagramSocket> client_end_;
  zxio_tests::DatagramSocketServer server_{{}};
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(DatagramSocketTest, Basic) { Init(); }

TEST_F(DatagramSocketTest, Release) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_release(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);

  EXPECT_OK(zx_handle_close(handle));
}

TEST_F(DatagramSocketTest, Borrow) {
  Init();

  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_borrow(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);
}

TEST_F(DatagramSocketTest, CreateWithType) {
  ASSERT_OK(zxio_create_with_type(storage(), ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET,
                                  TakeSocket().release(), &info(), &prelude_size(),
                                  TakeClientEnd().TakeChannel().release()));
  ASSERT_OK(zxio_close(&storage()->io));
  zxio_destroy(&storage()->io);
}

TEST_F(DatagramSocketTest, Attr) {
  Init();

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(zxio(), &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET, attr.object_type);
}

TEST_F(DatagramSocketTest, WaitBegin) {
  Init();
  zx_handle_t h = ZX_HANDLE_INVALID;
  zx_signals_t signals = 0;

  // `WRITABLE` should be mapped to `WRITE_THRESHOLD`.
  zxio_wait_begin(zxio(), ZXIO_SIGNAL_WRITABLE, &h, &signals);
  ASSERT_FALSE(signals & ZX_SOCKET_WRITABLE) << signals;
  ASSERT_TRUE(signals & ZX_SOCKET_WRITE_THRESHOLD) << signals;

  // `WRITE_THRESHOLD` should be left the same.
  zxio_wait_begin(zxio(), ZXIO_SIGNAL_WRITE_THRESHOLD, &h, &signals);
  ASSERT_FALSE(signals & ZX_SOCKET_WRITABLE) << signals;
  ASSERT_TRUE(signals & ZX_SOCKET_WRITE_THRESHOLD) << signals;
}

TEST_F(DatagramSocketTest, WaitEnd) {
  Init();
  zxio_signals_t zxio_signals = 0;

  // `WRITE_THRESHOLD` should be mapped to `WRITE_THRESHOLD` | `WRITABLE`.
  zxio_wait_end(zxio(), ZX_SOCKET_WRITE_THRESHOLD, &zxio_signals);
  ASSERT_TRUE(zxio_signals & ZXIO_SIGNAL_WRITE_THRESHOLD | ZXIO_SIGNAL_WRITABLE) << zxio_signals;

  // `WRITABLE` should be masked out.
  zxio_wait_end(zxio(), ZX_SOCKET_WRITABLE, &zxio_signals);
  ASSERT_EQ(zxio_signals, 0);
}

class DatagramSocketServer final : public fidl::testing::WireTestBase<fsocket::DatagramSocket> {
 public:
  DatagramSocketServer() {
    constexpr char kConnectedAddr[] = "192.0.2.99";
    constexpr uint16_t kConnectedPort = 45678;

    struct sockaddr_in sockaddr;
    sockaddr.sin_family = AF_INET;
    if (inet_pton(AF_INET, kConnectedAddr, &sockaddr.sin_addr) != 1) {
      ADD_FAILURE() << "failed to create IPv4 sockaddr from addr " << kConnectedAddr << " and port "
                    << kConnectedPort;
    }
    sockaddr.sin_port = htons(kConnectedPort);
    connected_addr_.LoadSockAddr(reinterpret_cast<struct sockaddr*>(&sockaddr), sizeof(sockaddr));
  }

  [[nodiscard]] bool TakeGetErrorCalled() { return get_error_called_.exchange(false); }

  [[nodiscard]] bool TakeSendMsgPreflightCalled() {
    return send_msg_preflight_called_.exchange(false);
  }

  void InvalidateClientCache() {
    std::lock_guard lock(lock_);
    ASSERT_NO_FATAL_FAILURE(InvalidateClientCacheLocked());
  }

  void SetConnectedAddress(SocketAddress addr) {
    std::lock_guard lock(lock_);
    connected_addr_ = addr;
    ASSERT_NO_FATAL_FAILURE(InvalidateClientCacheLocked());
  }

  static constexpr fposix::wire::Errno kSocketError = fposix::wire::Errno::kEio;

  void GetError(GetErrorCompleter::Sync& completer) override {
    bool previously_called = get_error_called_.exchange(true);
    EXPECT_FALSE(previously_called) << "GetError was called but unacknowledged by the test";
    completer.ReplyError(kSocketError);
  }

  static constexpr size_t kMaximumSize = 1337;

  void SendMsgPreflight(fsocket::wire::DatagramSocketSendMsgPreflightRequest* request,
                        SendMsgPreflightCompleter::Sync& completer) override {
    bool previously_called = send_msg_preflight_called_.exchange(true);
    EXPECT_FALSE(previously_called) << "SendMsgPreflight was called but unacknowledged by the test";
    fidl::Arena alloc;
    fidl::WireTableBuilder response_builder =
        fsocket::wire::DatagramSocketSendMsgPreflightResponse::Builder(alloc);
    if (request->has_to()) {
      response_builder.to(request->to());
    } else {
      std::lock_guard lock(lock_);
      connected_addr_.WithFIDL(
          [&response_builder](fnet::wire::SocketAddress address) { response_builder.to(address); });
    }
    zx::result<zx::eventpair> event = DuplicateCachePeer();
    ASSERT_OK(event.status_value()) << "failed to duplicate peer event for cache invalidation";
    std::array validity{std::move(event.value())};
    response_builder.validity(fidl::VectorView<zx::eventpair>::FromExternal(validity))
        .maximum_size(kMaximumSize);
    completer.ReplySuccess(response_builder.Build());
  }

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  zx::result<zx::eventpair> DuplicateCachePeer() {
    std::lock_guard lock(lock_);
    zx::eventpair dup;
    if (zx_status_t status = cache_peer_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(dup));
  }

  void InvalidateClientCacheLocked() __TA_REQUIRES(lock_) {
    ASSERT_OK(zx::eventpair::create(0u, &cache_local_, &cache_peer_));
  }

  std::atomic<bool> get_error_called_;
  std::atomic<bool> send_msg_preflight_called_;

  std::mutex lock_;
  SocketAddress connected_addr_ __TA_GUARDED(lock_);
  zx::eventpair cache_local_ __TA_GUARDED(lock_);
  zx::eventpair cache_peer_ __TA_GUARDED(lock_);
};

class DatagramSocketRouteCacheTest : public zxtest::Test {
 public:
  void SetUp() override {
    ASSERT_OK(zx::eventpair::create(0u, &error_local_, &error_peer_));
    ASSERT_NO_FATAL_FAILURE(server_.InvalidateClientCache());

    auto endpoints = fidl::Endpoints<fsocket::DatagramSocket>::Create();
    client_ = fidl::WireSyncClient<fsocket::DatagramSocket>{std::move(endpoints.client)};

    fidl::BindServer(control_loop_.dispatcher(), std::move(endpoints.server), &server_);
    control_loop_.StartThread("control");
  }

  void TearDown() final { control_loop_.Shutdown(); }

  void MakeSockAddrV4(uint16_t port, std::optional<SocketAddress>& out_addr) {
    constexpr char kSomeIpv4Addr[] = "192.0.2.55";
    struct sockaddr_in sockaddr;
    sockaddr.sin_family = AF_INET;
    ASSERT_EQ(inet_pton(AF_INET, kSomeIpv4Addr, &sockaddr.sin_addr), 1)
        << "failed to create IPv4 sockaddr from addr '" << kSomeIpv4Addr << "' and port '" << port
        << "'";
    sockaddr.sin_port = htons(port);
    SocketAddress addr;
    addr.LoadSockAddr(reinterpret_cast<struct sockaddr*>(&sockaddr), sizeof(sockaddr));
    out_addr.emplace(addr);
  }

  void MakeSockAddrV6(uint16_t port, std::optional<SocketAddress>& out_addr) {
    constexpr char kSomeIpv6Addr[] = "2001:db8::55";
    struct sockaddr_in6 sockaddr;
    sockaddr.sin6_family = AF_INET6;
    ASSERT_EQ(inet_pton(AF_INET6, kSomeIpv6Addr, &sockaddr.sin6_addr), 1)
        << "failed to create IPv6 sockaddr from addr '" << kSomeIpv6Addr << "' and port '" << port
        << "'";
    sockaddr.sin6_port = htons(port);
    SocketAddress addr;
    addr.LoadSockAddr(reinterpret_cast<struct sockaddr*>(&sockaddr), sizeof(sockaddr));
    out_addr.emplace(addr);
  }

  void SignalErrorOnSocket() {
    ASSERT_OK(error_local_.signal_peer(0, fsocket::wire::kSignalDatagramError));
  }

  void ClearErrorOnSocket() { ASSERT_OK(zx::eventpair::create(0u, &error_local_, &error_peer_)); }

  void AssertGetFromCacheSucceeds(std::optional<SocketAddress>& addr) {
    zx_wait_item_t error_wait_item{
        .handle = error_peer_.get(),
        .waitfor = fsocket::wire::kSignalDatagramError,
    };
    RouteCache::Result result = cache_.Get(addr, std::nullopt, error_wait_item, client_);
    ASSERT_TRUE(result.is_ok()) << "RouteCache::Get failed: " << result.error_value();
    ASSERT_EQ(result.value(), DatagramSocketServer::kMaximumSize);
  }

  void AssertGetFromCacheReturnsSocketError(std::optional<SocketAddress>& addr) {
    zx_wait_item_t error_wait_item{
        .handle = error_peer_.get(),
        .waitfor = fsocket::wire::kSignalDatagramError,
    };
    RouteCache::Result result = cache_.Get(addr, std::nullopt, error_wait_item, client_);
    ASSERT_TRUE(result.is_error());
    ErrOrOutCode err_value = result.error_value();
    ASSERT_OK(err_value.status_value())
        << "RouteCache::Get returned an error instead of an out code";
    ASSERT_EQ(err_value.value(), static_cast<int16_t>(DatagramSocketServer::kSocketError));
    ASSERT_TRUE(server_.TakeGetErrorCalled());
  }

 protected:
  DatagramSocketServer server_;

 private:
  RouteCache cache_;
  fidl::WireSyncClient<fsocket::DatagramSocket> client_;
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  zx::eventpair error_local_, error_peer_;
};

constexpr uint16_t kSomePort = 10000;

TEST_F(DatagramSocketRouteCacheTest, GetNewItemCallsPreflight) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, GetExistingItemDoesntCallPreflight) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, InvalidateClientCacheGetCallsPreflight) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV6(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  // When the server-side eventpair is closed for an existing item in the cache,
  // the client should observe the cache invalidation and call SendMsgPreflight
  // again the next time the item is retrieved from the cache.
  ASSERT_NO_FATAL_FAILURE(server_.InvalidateClientCache());

  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, ErrorSignaledGetCallsGetError) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  // When the designated error signal is signaled on the error wait item, the
  // client should call `GetError` and propagate the error it receives to the
  // caller.
  ASSERT_NO_FATAL_FAILURE(SignalErrorOnSocket());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV6(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheReturnsSocketError(to));
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, ErrorPropagatedEvenIfCacheAlsoInvalidated) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV6(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  // Close the server-side eventpair, *and* signal an error on the error wait
  // item. The error should take precedence and be returned to the caller
  // without the client calling `SendMsgPreflight`.
  ASSERT_NO_FATAL_FAILURE(server_.InvalidateClientCache());
  ASSERT_NO_FATAL_FAILURE(SignalErrorOnSocket());

  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheReturnsSocketError(to));
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, SameAddressDifferentPortIsDifferentItem) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(kSomePort, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(kSomePort + 1, to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, GetNulloptCachesConnectedAddr) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());
}

TEST_F(DatagramSocketRouteCacheTest, LruDiscardedGetCallsPreflight) {
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());
  constexpr uint16_t kEphemeralPortStart = 32768;

  // For each new address we `Get`, the client should call `SendMsgPreflight`
  // since the address is not yet present in the cache.
  std::array<std::optional<SocketAddress>, RouteCache::kMaxEntries> addrs;
  for (size_t i = 0; i < addrs.size(); i++) {
    addrs[i] = std::optional<SocketAddress>{};
    ASSERT_NO_FATAL_FAILURE(
        MakeSockAddrV4(static_cast<uint16_t>(kEphemeralPortStart + i), addrs[i]));
    ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addrs[i]),
                            "RouteCache::Get failed on addr %zu", i);
    ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
  }

  // Once the addresses are in the cache, even though the cache is full, `Get`
  // should not require a call to `SendMsgPreflight`.
  for (size_t i = 0; i < addrs.size(); i++) {
    ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addrs[i]),
                            "RouteCache::Get failed on addr %zu", i);
    ASSERT_FALSE(server_.TakeSendMsgPreflightCalled())
        << "RouteCache::Get should not call SendMsgPreflight for a cached address; did for addr "
        << i;
  }

  // Adding a new address causes the cache to go over capacity, and the least-
  // recently-used entry will be evicted, thus requiring a call to
  // `SendMsgPreflight` the next time it's queried.
  std::optional<SocketAddress> to;
  ASSERT_NO_FATAL_FAILURE(
      MakeSockAddrV4(static_cast<uint16_t>(kEphemeralPortStart + RouteCache::kMaxEntries), to));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addrs[0]));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
}

class DatagramSocketRouteCacheLruTest : public DatagramSocketRouteCacheTest {
 public:
  void FillCache() {
    for (size_t i = 0; i < RouteCache::kMaxEntries; i++) {
      ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
    }
  }

  void InsertNewEntry() {
    std::optional<SocketAddress> to;
    ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(static_cast<uint16_t>(next_port_), to));
    ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(to));
    ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());
    ++next_port_;
  }

  void GetFirstEntryInserted(std::optional<SocketAddress>& first_entry) {
    ASSERT_GT(next_port_, kEphemeralPortStart)
        << "cannot get first entry since no entry has been inserted";
    ASSERT_NO_FATAL_FAILURE(
        MakeSockAddrV4(static_cast<uint16_t>(kEphemeralPortStart), first_entry));
  }

  void AssertEntryEvicted(std::optional<SocketAddress> addr, bool evicted) {
    ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addr));
    ASSERT_EQ(server_.TakeSendMsgPreflightCalled(), evicted);
  }

 private:
  static constexpr uint16_t kEphemeralPortStart = 32768;
  uint16_t next_port_ = kEphemeralPortStart;
};

TEST_F(DatagramSocketRouteCacheLruTest, CacheEntryRefreshUpdatesLru) {
  // Trigger a SendMsgPreflight by attempting to retrieve a new address from the
  // cache.
  ASSERT_NO_FATAL_FAILURE(InsertNewEntry());

  // Invalidate the eventpair associated with that cache entry.
  ASSERT_NO_FATAL_FAILURE(server_.InvalidateClientCache());

  // Get the same address from the cache and assert that the cache calls
  // SendMsgPreflight again, because the eventpair has been invalidated. At this
  // point, the invalidated entry should have been overwritten in the cache AND
  // removed from the LRU list.
  std::optional<SocketAddress> first_entry;
  GetFirstEntryInserted(first_entry);
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(first_entry));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  // Now get (kMaxEntries + 1) more addresses in order to fill the cache and
  // cause the 2 least-recently-used entries to be evicted from both the cache
  // and LRU list. This ensures that a 1-1 mapping between LRU elements and
  // cache elements is maintained; a dangling LRU node will cause the cache to
  // attempt to remove a nonexistent entry and panic.
  for (size_t i = 1; i <= RouteCache::kMaxEntries + 1; i++) {
    ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
  }
}

TEST_F(DatagramSocketRouteCacheLruTest, MovedToFrontOfLruWhenEntryStillValid) {
  ASSERT_NO_FATAL_FAILURE(FillCache());

  // Retrieving the oldest entry from the cache should move it to the front of
  // the LRU list without the need for a SendMsgPreflight call, since the
  // eventpair is still valid.
  std::optional<SocketAddress> first_entry;
  GetFirstEntryInserted(first_entry);
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(first_entry));
  ASSERT_FALSE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
  ASSERT_NO_FATAL_FAILURE(AssertEntryEvicted(first_entry, false));
}

TEST_F(DatagramSocketRouteCacheLruTest, MovedToFrontOfLruWhenEntryRefreshed) {
  ASSERT_NO_FATAL_FAILURE(FillCache());

  // Retrieving the oldest entry from the cache should move it to the front of
  // the LRU list after a successful SendMsgPreflight call, since the eventpair
  // was invalidated.
  ASSERT_NO_FATAL_FAILURE(server_.InvalidateClientCache());
  std::optional<SocketAddress> first_entry;
  GetFirstEntryInserted(first_entry);
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(first_entry));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
  ASSERT_NO_FATAL_FAILURE(AssertEntryEvicted(first_entry, false));
}

TEST_F(DatagramSocketRouteCacheLruTest, NotMovedToFrontOfLruWhenErrorSignaled) {
  ASSERT_NO_FATAL_FAILURE(FillCache());

  // Retrieving the oldest entry from the cache should *not* move it to the
  // front of the LRU list if an error was signaled on the socket.
  ASSERT_NO_FATAL_FAILURE(SignalErrorOnSocket());

  std::optional<SocketAddress> first_entry;
  GetFirstEntryInserted(first_entry);
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheReturnsSocketError(first_entry));
  ASSERT_NO_FATAL_FAILURE(ClearErrorOnSocket());

  ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
  ASSERT_NO_FATAL_FAILURE(AssertEntryEvicted(first_entry, true));
}

TEST_F(DatagramSocketRouteCacheLruTest, OnlyReturnedAddrMovedToFrontOfLru) {
  ASSERT_NO_FATAL_FAILURE(InsertNewEntry());

  // Connect to some address, and get the connected address so that it's added
  // to the cache.
  std::optional<SocketAddress> connected_addr;
  constexpr uint16_t kArbitraryPort = 44444;
  ASSERT_NO_FATAL_FAILURE(MakeSockAddrV4(kArbitraryPort, connected_addr));
  ASSERT_NO_FATAL_FAILURE(server_.SetConnectedAddress(connected_addr.value()));
  std::optional<SocketAddress> addr_nullopt;
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addr_nullopt));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  // Connect to a new address, but one that is already in the cache from before,
  // and get the connected address again. This address should be moved to the
  // front of the LRU list; the previously connected address should *not*.
  std::optional<SocketAddress> first_entry;
  GetFirstEntryInserted(first_entry);
  ASSERT_NO_FATAL_FAILURE(server_.SetConnectedAddress(first_entry.value()));
  ASSERT_NO_FATAL_FAILURE(AssertGetFromCacheSucceeds(addr_nullopt));
  ASSERT_TRUE(server_.TakeSendMsgPreflightCalled());

  // Insert enough entries to fill the cache and evict one entry.
  for (size_t i = 0; i < RouteCache::kMaxEntries - 1; i++) {
    ASSERT_NO_FATAL_FAILURE(InsertNewEntry());
  }
  ASSERT_NO_FATAL_FAILURE(AssertEntryEvicted(first_entry, false));
  ASSERT_NO_FATAL_FAILURE(AssertEntryEvicted(connected_addr, true));
}

class RawSocketTest : public zxtest::Test {
 public:
  void SetUp() final {
    ASSERT_OK(zx::eventpair::create(0u, &event_client_, &event_server_));

    zx::result server_end = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(server_end.status_value());

    fidl::BindServer(control_loop_.dispatcher(), std::move(*server_end), &server_);
    control_loop_.StartThread("control");
  }

  void Init() {
    ASSERT_OK(zxio_raw_socket_init(&storage_, TakeEventClient(), TakeClientEnd()));
    zxio_ = &storage_.io;
  }

  void TearDown() final {
    if (zxio_) {
      ASSERT_OK(zxio_close(zxio_));
      zxio_destroy(zxio_);
    }
    control_loop_.Shutdown();
  }

  zx::eventpair TakeEventClient() { return std::move(event_client_); }
  fidl::ClientEnd<fsocket_raw::Socket> TakeClientEnd() { return std::move(client_end_); }
  zxio_storage_t* storage() { return &storage_; }
  zxio_t* zxio() { return zxio_; }

 private:
  zxio_storage_t storage_;
  zxio_t* zxio_{nullptr};
  zx::eventpair event_client_, event_server_;
  fidl::ClientEnd<fsocket_raw::Socket> client_end_;
  zxio_tests::RawSocketServer server_{{}};
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(RawSocketTest, Basic) { Init(); }

TEST_F(RawSocketTest, Release) {
  Init();
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_release(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);

  EXPECT_OK(zx_handle_close(handle));
}

TEST_F(RawSocketTest, Borrow) {
  Init();
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_borrow(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);
}

TEST_F(RawSocketTest, CreateWithType) {
  ASSERT_OK(zxio_create_with_type(storage(), ZXIO_OBJECT_TYPE_RAW_SOCKET,
                                  TakeEventClient().release(),
                                  TakeClientEnd().TakeChannel().release()));
  ASSERT_OK(zxio_close(&storage()->io));
  zxio_destroy(&storage()->io);
}

TEST_F(RawSocketTest, Attr) {
  Init();

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(zxio(), &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_RAW_SOCKET, attr.object_type);
}

class PacketSocketTest : public zxtest::Test {
 public:
  void SetUp() final {
    ASSERT_OK(zx::eventpair::create(0u, &event_client_, &event_server_));

    zx::result server_end = fidl::CreateEndpoints(&client_end_);
    ASSERT_OK(server_end.status_value());

    fidl::BindServer(control_loop_.dispatcher(), std::move(*server_end), &server_);
    control_loop_.StartThread("control");
  }

  void Init() {
    ASSERT_OK(zxio_packet_socket_init(&storage_, TakeEventClient(), TakeClientEnd()));
    zxio_ = &storage_.io;
  }

  void TearDown() final {
    if (zxio_) {
      ASSERT_OK(zxio_close(zxio_));
      zxio_destroy(zxio_);
    }
    control_loop_.Shutdown();
  }

  zx::eventpair TakeEventClient() { return std::move(event_client_); }
  fidl::ClientEnd<fsocket_packet::Socket> TakeClientEnd() { return std::move(client_end_); }
  zxio_storage_t* storage() { return &storage_; }
  zxio_t* zxio() { return zxio_; }

 private:
  zxio_storage_t storage_;
  zxio_t* zxio_{nullptr};
  zx::eventpair event_client_, event_server_;
  fidl::ClientEnd<fsocket_packet::Socket> client_end_;
  zxio_tests::PacketSocketServer server_{{}};
  async::Loop control_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
};

TEST_F(PacketSocketTest, Basic) { Init(); }

TEST_F(PacketSocketTest, Release) {
  Init();
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_release(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);

  EXPECT_OK(zx_handle_close(handle));
}

TEST_F(PacketSocketTest, Borrow) {
  Init();
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_OK(zxio_borrow(zxio(), &handle));
  EXPECT_NE(handle, ZX_HANDLE_INVALID);
}

TEST_F(PacketSocketTest, CreateWithType) {
  ASSERT_OK(zxio_create_with_type(storage(), ZXIO_OBJECT_TYPE_PACKET_SOCKET,
                                  TakeEventClient().release(),
                                  TakeClientEnd().TakeChannel().release()));
  ASSERT_OK(zxio_close(&storage()->io));
  zxio_destroy(&storage()->io);
}

TEST_F(PacketSocketTest, Attr) {
  Init();

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(zxio(), &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_PACKET_SOCKET, attr.object_type);
}

}  // namespace
