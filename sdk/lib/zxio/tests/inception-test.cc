// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pty/cpp/wire_test_base.h>
#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket.packet/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket.raw/cpp/wire_test_base.h>
#include <fidl/fuchsia.posix.socket/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <lib/zxio/cpp/inception.h>
#include <lib/zxio/types.h>
#include <lib/zxio/zxio.h>
#include <string.h>

#include <zxtest/zxtest.h>

#include "sdk/lib/zxio/tests/test_directory_server_base.h"
#include "sdk/lib/zxio/tests/test_file_server_base.h"
#include "sdk/lib/zxio/tests/test_socket_server.h"

namespace {

TEST(CreateWithAllocator, ErrorAllocator) {
  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    return ZX_ERR_INVALID_ARGS;
  };
  zx::socket socket0, socket1;
  ASSERT_OK(zx::socket::create(0u, &socket0, &socket1));
  void* context;
  ASSERT_STATUS(zxio_create_with_allocator(std::move(socket0), allocator, &context),
                ZX_ERR_NO_MEMORY);

  // Make sure that the handle is closed.
  zx_signals_t pending = 0;
  ASSERT_STATUS(socket1.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_CHANNEL_PEER_CLOSED, ZX_CHANNEL_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;
}

TEST(CreateWithAllocator, BadAllocator) {
  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    *out_storage = nullptr;
    return ZX_OK;
  };
  zx::socket socket0, socket1;
  ASSERT_OK(zx::socket::create(0u, &socket0, &socket1));
  void* context;
  ASSERT_STATUS(zxio_create_with_allocator(std::move(socket0), allocator, &context),
                ZX_ERR_NO_MEMORY);

  // Make sure that the handle is closed.
  zx_signals_t pending = 0;
  ASSERT_STATUS(socket1.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_CHANNEL_PEER_CLOSED, ZX_CHANNEL_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;
}

struct VmoWrapper {
  int32_t tag;
  zxio_storage_t storage;
};

TEST(CreateWithAllocator, Vmo) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(1024, 0u, &vmo));

  uint32_t data = 0x1a2a3a4a;
  ASSERT_OK(vmo.write(&data, 0u, sizeof(data)));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_VMO) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    auto* wrapper = new VmoWrapper{
        .tag = 0x42,
        .storage = {},
    };
    *out_storage = &(wrapper->storage);
    *out_context = wrapper;
    return ZX_OK;
  };

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(std::move(vmo), allocator, &context));
  ASSERT_NE(nullptr, context);
  std::unique_ptr<VmoWrapper> wrapper(static_cast<VmoWrapper*>(context));
  ASSERT_EQ(wrapper->tag, 0x42);

  zxio_t* io = &(wrapper->storage.io);

  uint32_t buffer = 0;
  size_t actual = 0;
  zxio_read(io, &buffer, sizeof(buffer), 0u, &actual);
  ASSERT_EQ(actual, sizeof(buffer));
  ASSERT_EQ(buffer, data);

  zxio_destroy(io);
}

namespace {

class TestDirectoryServer final : public zxio_tests::TestDirectoryServerBase {
  void Sync(SyncCompleter::Sync& completer) final { completer.ReplySuccess(); }
};

}  // namespace

TEST(CreateWithAllocator, Directory) {
  auto [dir_client, dir_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  auto representation = fuchsia_io::wire::Representation::WithDirectory({});

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_DIR) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop dir_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  TestDirectoryServer server;
  fidl::BindServer(dir_control_loop.dispatcher(), std::move(dir_server), &server);
  dir_control_loop.StartThread("dir_control_thread");

  void* context = nullptr;
  fidl::ClientEnd<fuchsia_io::Node> node_client(dir_client.TakeChannel());
  ASSERT_OK(
      zxio_create_with_allocator(std::move(node_client), representation, allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Check the zxio by sending a sync operation to the server.
  EXPECT_OK(zxio_sync(zxio));

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  dir_control_loop.Shutdown();
}

TEST(CreateWithAllocator, File) {
  auto [file_client, file_server] = fidl::Endpoints<fuchsia_io::File>::Create();

  zx::event file_event;
  ASSERT_OK(zx::event::create(0u, &file_event));
  fidl::Arena arena;
  auto file_info =
      fuchsia_io::wire::FileInfo::Builder(arena).observer(std::move(file_event)).Build();
  auto representation = fuchsia_io::wire::Representation::WithFile(
      fidl::ObjectView<decltype(file_info)>::FromExternal(&file_info));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_FILE) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop file_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::TestReadFileServer server;
  fidl::BindServer(file_control_loop.dispatcher(), std::move(file_server), &server);
  file_control_loop.StartThread("file_control_thread");

  void* context = nullptr;
  fidl::ClientEnd<fuchsia_io::Node> node_client(file_client.TakeChannel());
  ASSERT_OK(
      zxio_create_with_allocator(std::move(node_client), representation, allocator, &context));
  ASSERT_NE(context, nullptr);

  // The event in node_info should be consumed by zxio.
  EXPECT_FALSE(representation.file().observer().is_valid());

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Check the zxio by reading some test data from the server.
  char buffer[sizeof(zxio_tests::TestReadFileServer::kTestData)];
  size_t actual = 0u;

  ASSERT_OK(zxio_read(zxio, buffer, sizeof(buffer), 0u, &actual));

  EXPECT_EQ(sizeof(buffer), actual);
  EXPECT_BYTES_EQ(buffer, zxio_tests::TestReadFileServer::kTestData, sizeof(buffer));

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  file_control_loop.Shutdown();
}

class TestServiceNodeServer : public fidl::testing::WireTestBase<fuchsia_io::Node> {
 public:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) final {
    completer.ReplySuccess();
    // After the reply, we should close the connection.
    completer.Close(ZX_OK);
  }
};

TEST(CreateWithAllocator, Node) {
  auto [node_client, node_server] = fidl::Endpoints<fuchsia_io::Node>::Create();

  auto representation = fuchsia_io::wire::Representation::WithNode({});

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_NODE) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop service_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  TestServiceNodeServer server;
  fidl::BindServer(service_control_loop.dispatcher(), std::move(node_server), &server);
  service_control_loop.StartThread("service_control_thread");

  void* context = nullptr;
  ASSERT_OK(
      zxio_create_with_allocator(std::move(node_client), representation, allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  service_control_loop.Shutdown();
}

class TestTtyServer : public fidl::testing::WireTestBase<fuchsia_hardware_pty::Device> {
 public:
  explicit TestTtyServer(zx::eventpair event) : event_(std::move(event)) {}

 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ADD_FAILURE() << "unexpected message received: " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void Close(CloseCompleter::Sync& completer) final {
    completer.ReplySuccess();
    // After the reply, we should close the connection.
    completer.Close(ZX_OK);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol = fidl::DiscoverableProtocolName<fuchsia_hardware_pty::Device>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    fidl::Arena alloc;
    completer.Reply(fuchsia_hardware_pty::wire::DeviceDescribeResponse::Builder(alloc)
                        .event(std::move(event_))
                        .Build());
  }

  zx::eventpair event_;
};

TEST(CreateWithAllocator, Tty) {
  auto [node_client, node_server] = fidl::Endpoints<fuchsia_io::Node>::Create();

  zx::eventpair event0, event1;
  ASSERT_OK(zx::eventpair::create(0, &event0, &event1));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_TTY) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop tty_control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  TestTtyServer server(std::move(event1));
  fidl::ServerEnd<fuchsia_hardware_pty::Device> tty_server(node_server.TakeChannel());
  fidl::BindServer(tty_control_loop.dispatcher(), std::move(tty_server), &server);
  tty_control_loop.StartThread("tty_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(node_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Closing the zxio object should close our eventpair's peer event.
  zx_signals_t pending = 0;
  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_NE(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  tty_control_loop.Shutdown();
}

TEST(CreateWithAllocator, PacketSocket) {
  auto [socket_client, socket_server] =
      fidl::Endpoints<fuchsia_posix_socket_packet::Socket>::Create();

  zx::eventpair event0, event1;
  ASSERT_OK(zx::eventpair::create(0, &event0, &event1));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_PACKET_SOCKET) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::PacketSocketServer server(std::move(event1));
  fidl::BindServer(control_loop.dispatcher(), std::move(socket_server), &server);
  control_loop.StartThread("packet_socket_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(socket_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Closing the zxio object should close our eventpair's peer event.
  zx_signals_t pending = 0;
  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_NE(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  control_loop.Shutdown();
}

TEST(CreateWithAllocator, RawSocket) {
  auto [socket_client, socket_server] = fidl::Endpoints<fuchsia_posix_socket_raw::Socket>::Create();

  zx::eventpair event0, event1;
  ASSERT_OK(zx::eventpair::create(0, &event0, &event1));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_RAW_SOCKET) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::RawSocketServer server(std::move(event1));
  fidl::BindServer(control_loop.dispatcher(), std::move(socket_server), &server);
  control_loop.StartThread("raw_socket_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(socket_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Closing the zxio object should close our eventpair's peer event.
  zx_signals_t pending = 0;
  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_NE(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  control_loop.Shutdown();
}

TEST(CreateWithAllocator, SynchronousDatagramSocket) {
  auto [socket_client, socket_server] =
      fidl::Endpoints<fuchsia_posix_socket::SynchronousDatagramSocket>::Create();

  zx::eventpair event0, event1;
  ASSERT_OK(zx::eventpair::create(0, &event0, &event1));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_SYNCHRONOUS_DATAGRAM_SOCKET) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::SynchronousDatagramSocketServer server(std::move(event1));
  fidl::BindServer(control_loop.dispatcher(), std::move(socket_server), &server);
  control_loop.StartThread("synchronous_datagram_socket_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(socket_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  // Closing the zxio object should close our eventpair's peer event.
  zx_signals_t pending = 0;
  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_NE(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);

  ASSERT_STATUS(event0.wait_one(0u, zx::time::infinite_past(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED)
      << "pending is " << std::showbase << std::hex << pending;

  control_loop.Shutdown();
}

TEST(CreateWithAllocator, DatagramSocket) {
  auto [socket_client, socket_server] =
      fidl::Endpoints<fuchsia_posix_socket::DatagramSocket>::Create();

  zx::socket socket, peer;
  ASSERT_OK(zx::socket::create(ZX_SOCKET_DATAGRAM, &socket, &peer));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_DATAGRAM_SOCKET) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::DatagramSocketServer server(std::move(socket));
  fidl::BindServer(control_loop.dispatcher(), std::move(socket_server), &server);
  control_loop.StartThread("datagram_socket_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(socket_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);
  control_loop.Shutdown();
}

TEST(CreateWithAllocator, StreamSocket) {
  auto [socket_client, socket_server] =
      fidl::Endpoints<fuchsia_posix_socket::StreamSocket>::Create();

  zx::socket socket, peer;
  ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &socket, &peer));

  auto allocator = [](zxio_object_type_t type, zxio_storage_t** out_storage, void** out_context) {
    if (type != ZXIO_OBJECT_TYPE_STREAM_SOCKET) {
      return ZX_ERR_NOT_SUPPORTED;
    }
    *out_storage = new zxio_storage_t;
    *out_context = *out_storage;
    return ZX_OK;
  };

  async::Loop control_loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  zxio_tests::StreamSocketServer server(std::move(socket));
  fidl::BindServer(control_loop.dispatcher(), std::move(socket_server), &server);
  control_loop.StartThread("stream_socket_control_thread");

  void* context = nullptr;
  ASSERT_OK(zxio_create_with_allocator(socket_client.TakeChannel(), allocator, &context));
  ASSERT_NE(context, nullptr);

  std::unique_ptr<zxio_storage_t> storage(static_cast<zxio_storage_t*>(context));
  zxio_t* zxio = &(storage->io);

  ASSERT_OK(zxio_close(zxio));
  zxio_destroy(zxio);
  control_loop.Shutdown();
}

}  // namespace
