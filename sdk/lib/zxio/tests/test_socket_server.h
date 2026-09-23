// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZXIO_TESTS_TEST_SOCKET_SERVER_H_
#define LIB_ZXIO_TESTS_TEST_SOCKET_SERVER_H_

#include <fidl/fuchsia.posix.socket.packet/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket.raw/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket/cpp/wire_test_base.h>

#include <zxtest/zxtest.h>

namespace zxio_tests {

class DatagramSocketServer final
    : public fidl::testing::WireTestBase<fuchsia_posix_socket::DatagramSocket> {
 public:
  explicit DatagramSocketServer(zx::socket socket) : socket_(std::move(socket)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
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
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket::wire::DatagramSocketDescribeResponse::Builder(alloc)
                        .socket(std::move(socket_))
                        .tx_meta_buf_size({})
                        .rx_meta_buf_size({})
                        .metadata_encoding_protocol_version({})
                        .Build());
  }

  zx::socket socket_;
};

class PacketSocketServer final
    : public fidl::testing::WireTestBase<fuchsia_posix_socket_packet::Socket> {
 public:
  explicit PacketSocketServer(zx::eventpair event) : event_(std::move(event)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) override {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket_packet::Socket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket_packet::wire::SocketDescribeResponse::Builder(alloc)
                        .event(std::move(event_))
                        .Build());
  }

  zx::eventpair event_;
};

class RawSocketServer final : public fidl::testing::WireTestBase<fuchsia_posix_socket_raw::Socket> {
 public:
  explicit RawSocketServer(zx::eventpair event) : event_(std::move(event)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) override {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket_raw::Socket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket_raw::wire::SocketDescribeResponse::Builder(alloc)
                        .event(std::move(event_))
                        .Build());
  }

  zx::eventpair event_;
};

class StreamSocketServer final
    : public fidl::testing::WireTestBase<fuchsia_posix_socket::StreamSocket> {
 public:
  explicit StreamSocketServer(zx::socket socket) : socket_(std::move(socket)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) override {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket::StreamSocket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    fidl::Arena alloc;
    completer.Reply(fuchsia_posix_socket::wire::StreamSocketDescribeResponse::Builder(alloc)
                        .socket(std::move(socket_))
                        .Build());
  }

  zx::socket socket_;
};

class SynchronousDatagramSocketServer final
    : public fidl::testing::WireTestBase<fuchsia_posix_socket::SynchronousDatagramSocket> {
 public:
  explicit SynchronousDatagramSocketServer(zx::eventpair event) : event_(std::move(event)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Clone(CloneRequestView request, CloneCompleter::Sync& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) final {
    completer.ReplySuccess();
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol =
        fidl::DiscoverableProtocolName<fuchsia_posix_socket::SynchronousDatagramSocket>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    fidl::Arena alloc;
    completer.Reply(
        fuchsia_posix_socket::wire::SynchronousDatagramSocketDescribeResponse::Builder(alloc)
            .event(std::move(event_))
            .Build());
  }

  zx::eventpair event_;
};

}  // namespace zxio_tests

#endif  // LIB_ZXIO_TESTS_TEST_SOCKET_SERVER_H_
