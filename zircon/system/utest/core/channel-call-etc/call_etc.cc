// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/event.h>
#include <lib/zx/port.h>
#include <lib/zx/socket.h>
#include <lib/zx/vmo.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <thread>

// Needed to test API coverage of null params in GCC.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wnonnull"
#include <lib/zx/channel.h>
#pragma GCC diagnostic pop

#include <zxtest/zxtest.h>

namespace {

template <size_t NBufBytes = 65536, size_t NBufHandles = 64>
class EchoServer {
 public:
  EchoServer() {
    zx::channel server_end;
    ASSERT_OK(zx::channel::create(0, &client_end, &server_end));
    thread = std::thread(ServerThread, std::move(server_end));
  }

  ~EchoServer() { thread.join(); }

  zx::channel ClientEnd() { return std::move(client_end); }

 private:
  static void ServerThread(zx::channel server_end) {
    uint32_t actual_bytes;
    uint32_t actual_handles;
    uint8_t bytes[NBufBytes];
    zx_handle_t handles[NBufHandles];
    zx_signals_t pending = 0;
    zx_status_t status = server_end.wait_one(ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED,
                                             zx::time::infinite(), &pending);
    if (status != ZX_OK || (pending & ZX_CHANNEL_READABLE) == 0) {
      return;
    }
    ASSERT_EQ(ZX_OK, server_end.read(0, bytes, handles, NBufBytes, NBufHandles, &actual_bytes,
                                     &actual_handles));
    ASSERT_EQ(ZX_OK, server_end.wait_one(ZX_CHANNEL_WRITABLE, zx::time::infinite(), nullptr));
    ASSERT_EQ(ZX_OK, server_end.write(0, bytes, actual_bytes, handles, actual_handles));
  }

  zx::channel client_end;
  std::thread thread;
};

TEST(ChannelCallEtcTest, BytesOnlySuccessCase) {
  EchoServer echo_server;
  zx::channel client_end = echo_server.ClientEnd();

  constexpr size_t message_size = 512;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  for (size_t i = 0; i < message_size; i++) {
    request_bytes[i] = static_cast<uint8_t>(i % 256);
  }

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = nullptr,
      .rd_bytes = response_bytes,
      .rd_handles = nullptr,
      .wr_num_bytes = message_size,
      .wr_num_handles = 0,
      .rd_num_bytes = message_size,
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_OK,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  EXPECT_EQ(message_size, actual_bytes);
  EXPECT_EQ(0, actual_handles);
  // The first four bytes are overwritten by zx_channel_call with the
  // txid.
  EXPECT_BYTES_EQ(request_bytes + 4, response_bytes + 4, message_size - 4);
}

TEST(ChannelCallEtcTest, HandlesSuccessCase) {
  EchoServer echo_server;
  zx::channel client_end = echo_server.ClientEnd();

  constexpr size_t message_size = 4;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  zx::port port0;
  zx::port port1;
  ASSERT_EQ(ZX_OK, zx::port::create(0, &port0));
  ASSERT_EQ(ZX_OK, zx::port::create(0, &port1));

  zx_info_handle_basic_t info0;
  zx_info_handle_basic_t info1;
  ASSERT_EQ(ZX_OK, zx_object_get_info(port0.get(), ZX_INFO_HANDLE_BASIC, &info0, sizeof(info0),
                                      nullptr, nullptr));
  ASSERT_EQ(ZX_OK, zx_object_get_info(port1.get(), ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1),
                                      nullptr, nullptr));

  constexpr size_t handles_size = 2;
  zx_handle_disposition_t request_handles[handles_size] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = port0.release(),
          .type = ZX_OBJ_TYPE_NONE,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = port1.release(),
          .type = ZX_OBJ_TYPE_NONE,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };
  zx_handle_info_t response_handles[handles_size] = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = request_handles,
      .rd_bytes = response_bytes,
      .rd_handles = response_handles,
      .wr_num_bytes = message_size,
      .wr_num_handles = handles_size,
      .rd_num_bytes = message_size,
      .rd_num_handles = handles_size,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_OK,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  ASSERT_EQ(message_size, actual_bytes);
  ASSERT_EQ(handles_size, actual_handles);
  EXPECT_NE(0, response_handles[0].handle);
  EXPECT_EQ(info0.type, response_handles[0].type);
  EXPECT_EQ(info0.rights, response_handles[0].rights);
  EXPECT_NE(0, response_handles[1].handle);
  EXPECT_EQ(info1.type, response_handles[1].type);
  EXPECT_EQ(info1.rights, response_handles[1].rights);
  zx_handle_close(response_handles[0].handle);
  zx_handle_close(response_handles[1].handle);
}

TEST(ChannelCallEtcTest, ReducedRightsSuccessCase) {
  EchoServer echo_server;
  zx::channel client_end = echo_server.ClientEnd();

  constexpr size_t message_size = 4;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  zx::port port0;
  ASSERT_EQ(ZX_OK, zx::port::create(0, &port0));

  constexpr size_t handles_size = 1;
  zx_handle_disposition_t request_handles[handles_size] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = port0.release(),
          .type = ZX_OBJ_TYPE_PORT,
          .rights = ZX_RIGHT_TRANSFER,
          .result = ZX_OK,
      },
  };
  zx_handle_info_t response_handles[handles_size] = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = request_handles,
      .rd_bytes = response_bytes,
      .rd_handles = response_handles,
      .wr_num_bytes = message_size,
      .wr_num_handles = handles_size,
      .rd_num_bytes = message_size,
      .rd_num_handles = handles_size,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_OK,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  ASSERT_EQ(message_size, actual_bytes);
  ASSERT_EQ(handles_size, actual_handles);
  EXPECT_NE(0, response_handles[0].handle);
  EXPECT_EQ(ZX_OBJ_TYPE_PORT, response_handles[0].type);
  EXPECT_EQ(ZX_RIGHT_TRANSFER, response_handles[0].rights);
  zx_handle_close(response_handles[0].handle);
}

TEST(ChannelCallEtcTest, IncreasedRightsFailureCase) {
  zx::channel client_end;
  zx::channel server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  constexpr size_t message_size = 4;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  zx::port port0;
  ASSERT_EQ(ZX_OK, zx::port::create(0, &port0));

  constexpr size_t handles_size = 1;
  zx_handle_disposition_t request_handles[handles_size] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = port0.release(),
          .type = ZX_OBJ_TYPE_PORT,
          .rights = ZX_RIGHT_TRANSFER | ZX_RIGHT_MANAGE_PROCESS,
          .result = ZX_OK,
      },
  };
  zx_handle_info_t response_handles[handles_size] = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = request_handles,
      .rd_bytes = response_bytes,
      .rd_handles = response_handles,
      .wr_num_bytes = message_size,
      .wr_num_handles = handles_size,
      .rd_num_bytes = message_size,
      .rd_num_handles = handles_size,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  zx_handle_close(request_handles[0].handle);
}

TEST(ChannelCallEtcTest, WrongObjectTypeFailureCase) {
  zx::channel client_end;
  zx::channel server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  constexpr size_t message_size = 4;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  zx::port port0;
  ASSERT_EQ(ZX_OK, zx::port::create(0, &port0));

  constexpr size_t handles_size = 1;
  zx_handle_disposition_t request_handles[handles_size] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = port0.release(),
          .type = ZX_OBJ_TYPE_VMO,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };
  zx_handle_info_t response_handles[handles_size] = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = request_handles,
      .rd_bytes = response_bytes,
      .rd_handles = response_handles,
      .wr_num_bytes = message_size,
      .wr_num_handles = handles_size,
      .rd_num_bytes = message_size,
      .rd_num_handles = handles_size,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_ERR_WRONG_TYPE,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  zx_handle_close(request_handles[0].handle);
}

TEST(ChannelCallEtcTest, BadChannelFailureCase) {
  zx::channel client_end;

  constexpr size_t message_size = 4;
  uint8_t request_bytes[message_size];
  uint8_t response_bytes[message_size];

  zx_channel_call_etc_args_t args = {
      .wr_bytes = request_bytes,
      .wr_handles = nullptr,
      .rd_bytes = response_bytes,
      .rd_handles = nullptr,
      .wr_num_bytes = message_size,
      .wr_num_handles = 0,
      .rd_num_bytes = message_size,
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE,
            client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
}

TEST(ChannelCallEtcTest, NullArgsReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(), nullptr,
                                &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelCallEtcTest, BadArgsPointerReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(),
                                reinterpret_cast<zx_channel_call_etc_args_t*>(1), &actual_bytes,
                                &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelCallEtcTest, NullActualBytesReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), nullptr, 0));
  });

  zx_txid_t txid = 0;
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(), &args, nullptr,
                                &actual_handles),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, NullActualHandlesReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), nullptr, 0));
  });

  zx_txid_t txid = 0;
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(), &args,
                                &actual_bytes, nullptr),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, BadActualBytesPointerReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), nullptr, 0));
  });

  zx_txid_t txid = 0;
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(), &args,
                                reinterpret_cast<uint32_t*>(1), &actual_handles),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, BadActualHandlesPointerReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), nullptr, 0));
  });

  zx_txid_t txid = 0;
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  EXPECT_EQ(zx_channel_call_etc(client_end.get(), 0, zx::time::infinite().get(), &args,
                                &actual_bytes, reinterpret_cast<uint32_t*>(1)),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, InvalidOptionsReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0xff, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

TEST(ChannelCallEtcTest, WithoutReadRightReturnsAccessDenied) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  zx::channel client_no_read;
  ASSERT_OK(client_end.replace(ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_READ, &client_no_read));

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_no_read.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_ACCESS_DENIED);
}

TEST(ChannelCallEtcTest, WithoutWriteRightReturnsAccessDenied) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  zx::channel client_no_write;
  ASSERT_OK(client_end.replace(ZX_DEFAULT_CHANNEL_RIGHTS & ~ZX_RIGHT_WRITE, &client_no_write));

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(
      client_no_write.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
      ZX_ERR_ACCESS_DENIED);
}

TEST(ChannelCallEtcTest, FromWrongObjectTypeReturnsWrongType) {
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = nullptr,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };
  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(zx_channel_call_etc(event.get(), 0, zx::time::infinite().get(), &args, &actual_bytes,
                                &actual_handles),
            ZX_ERR_WRONG_TYPE);
}

TEST(ChannelCallEtcTest, WithDuplicateOpHandleSucceeds) {
  EchoServer echo_server;
  zx::channel client_end = echo_server.ClientEnd();

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  char req[4] = {0};
  char rep[4] = {0};
  zx_handle_disposition_t wr_disp = {
      .operation = ZX_HANDLE_OP_DUPLICATE,
      .handle = event.get(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_RIGHT_SAME_RIGHTS,
      .result = ZX_OK,
  };
  zx_handle_info_t rd_info = {};

  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = &wr_disp,
      .rd_bytes = rep,
      .rd_handles = &rd_info,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 1,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 1,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, sizeof(req));
  EXPECT_EQ(actual_handles, 1u);
  EXPECT_EQ(wr_disp.result, ZX_OK);

  // Original handle is still valid in caller
  EXPECT_OK(zx_handle_check_valid(event.get()));

  // Reply handle is valid duplicate
  EXPECT_NE(rd_info.handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(rd_info.type, ZX_OBJ_TYPE_EVENT);
  EXPECT_OK(zx_handle_close(rd_info.handle));
}

TEST(ChannelCallEtcTest, DispositionFailureSetsResultFieldAndReturnsFirstError) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  zx::socket socket_s, socket_c;
  ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &socket_s, &socket_c));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  zx_handle_disposition_t dispositions[3] = {
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = socket_s.release(),
          .type = ZX_OBJ_TYPE_SOCKET,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event.release(),
          .type = ZX_OBJ_TYPE_VMO,  // Wrong type: event is not VMO
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = 0b100,  // Bad handle
          .type = ZX_OBJ_TYPE_NONE,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = dispositions,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 3,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(dispositions[0].result, ZX_OK);
  EXPECT_EQ(dispositions[1].result, ZX_ERR_WRONG_TYPE);
  EXPECT_EQ(dispositions[2].result, ZX_ERR_BAD_HANDLE);
}

TEST(ChannelCallEtcTest, MixedMoveAndDuplicateOnFailure) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  zx::event event_dup;
  ASSERT_OK(zx::event::create(0, &event_dup));
  zx::event event_move;
  ASSERT_OK(zx::event::create(0, &event_move));

  zx_handle_t dup_raw = event_dup.get();
  zx_handle_t move_raw = event_move.get();

  zx_handle_disposition_t dispositions[3] = {
      {
          .operation = ZX_HANDLE_OP_DUPLICATE,
          .handle = dup_raw,
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = event_move.release(),
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
      {
          .operation = ZX_HANDLE_OP_MOVE,
          .handle = 0b100,  // Bad handle
          .type = ZX_OBJ_TYPE_EVENT,
          .rights = ZX_RIGHT_SAME_RIGHTS,
          .result = ZX_OK,
      },
  };

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = dispositions,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 3,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_BAD_HANDLE);
  EXPECT_EQ(dispositions[0].result, ZX_OK);
  EXPECT_EQ(dispositions[1].result, ZX_OK);
  EXPECT_EQ(dispositions[2].result, ZX_ERR_BAD_HANDLE);

  // Duplicate handle remains valid
  EXPECT_OK(zx_handle_check_valid(dup_raw));
  // Moved handle was consumed / closed
  EXPECT_EQ(zx_handle_check_valid(move_raw), ZX_ERR_NOT_FOUND);
}

TEST(ChannelCallEtcTest, SelfHandleTransferReturnsNotSupported) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_DUPLICATE,
      .handle = client_end.get(),
      .type = ZX_OBJ_TYPE_CHANNEL,
      .rights = ZX_RIGHT_SAME_RIGHTS,
      .result = ZX_OK,
  };

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = &disp,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 1,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_NOT_SUPPORTED);
  EXPECT_EQ(disp.result, ZX_ERR_NOT_SUPPORTED);
  EXPECT_OK(zx_handle_check_valid(client_end.get()));
}

TEST(ChannelCallEtcTest, PeerClosedBeforeCallReturnsPeerClosed) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));
  server_end.reset();  // Close remote

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  zx_handle_t event_raw = event.get();

  zx_handle_disposition_t disp = {
      .operation = ZX_HANDLE_OP_MOVE,
      .handle = event.release(),
      .type = ZX_OBJ_TYPE_EVENT,
      .rights = ZX_RIGHT_SAME_RIGHTS,
      .result = ZX_OK,
  };

  char req[4] = {0};
  char rep[4] = {0};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = req,
      .wr_handles = &disp,
      .rd_bytes = rep,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(req),
      .wr_num_handles = 1,
      .rd_num_bytes = sizeof(rep),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_PEER_CLOSED);
  EXPECT_EQ(zx_handle_check_valid(event_raw), ZX_ERR_NOT_FOUND);
}

TEST(ChannelCallEtcTest, ResponseTooLargeForRdNumBytesReturnsBufferTooSmall) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));

    uint8_t reply_payload[32] = {};
    memcpy(reply_payload, &txid, sizeof(txid));
    ASSERT_OK(server_end.write(0, reply_payload, sizeof(reply_payload), nullptr, 0));
  });

  zx_txid_t txid = 0;
  uint8_t rd_buf[8] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),  // 8 bytes < 32 bytes reply
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_BUFFER_TOO_SMALL);
  server_thread.join();
}

TEST(ChannelCallEtcTest, ResponseTooLargeForRdNumHandlesReturnsBufferTooSmall) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));

    zx::event e1, e2;
    ASSERT_OK(zx::event::create(0, &e1));
    ASSERT_OK(zx::event::create(0, &e2));
    zx_handle_t handles[2] = {e1.release(), e2.release()};
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), handles, 2));
  });

  zx_txid_t txid = 0;
  uint8_t rd_buf[8] = {};
  zx_handle_info_t rd_handles[1] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = rd_handles,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 1,  // only room for 1 handle, reply has 2
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_BUFFER_TOO_SMALL);
  server_thread.join();
}

TEST(ChannelCallEtcTest, BadRdBytesBufferReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), nullptr, 0));
  });

  zx_txid_t txid = 0;
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = reinterpret_cast<void*>(1),
      .rd_handles = nullptr,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(txid),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, BadRdHandlesBufferReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));

    zx::event e;
    ASSERT_OK(zx::event::create(0, &e));
    zx_handle_t h = e.release();
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), &h, 1));
  });

  zx_txid_t txid = 0;
  uint8_t rd_buf[8] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = reinterpret_cast<zx_handle_info_t*>(1),
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 1,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
  server_thread.join();
}

TEST(ChannelCallEtcTest, HandlesInfoValidation) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  std::thread server_thread([server_end = std::move(server_end)]() {
    ASSERT_OK(server_end.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), nullptr));
    zx_txid_t txid = 0;
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;
    ASSERT_OK(server_end.read(0, &txid, nullptr, sizeof(txid), 0, &actual_bytes, &actual_handles));

    zx::event event;
    ASSERT_OK(zx::event::create(0, &event));
    zx::socket socket_s, socket_c;
    ASSERT_OK(zx::socket::create(ZX_SOCKET_STREAM, &socket_s, &socket_c));
    zx::port port;
    ASSERT_OK(zx::port::create(0, &port));

    zx_handle_t reply_handles[3] = {event.release(), socket_s.release(), port.release()};
    ASSERT_OK(server_end.write(0, &txid, sizeof(txid), reply_handles, 3));
  });

  zx_txid_t txid = 0;
  uint8_t rd_buf[8] = {};
  zx_handle_info_t rd_handles[3] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = &txid,
      .wr_handles = nullptr,
      .rd_bytes = rd_buf,
      .rd_handles = rd_handles,
      .wr_num_bytes = sizeof(txid),
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(rd_buf),
      .rd_num_handles = 3,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(client_end.call_etc(0, zx::time::infinite(), &args, &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, sizeof(txid));
  EXPECT_EQ(actual_handles, 3u);

  EXPECT_NE(rd_handles[0].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(rd_handles[0].type, ZX_OBJ_TYPE_EVENT);
  EXPECT_EQ(rd_handles[0].rights, ZX_DEFAULT_EVENT_RIGHTS);
  EXPECT_EQ(rd_handles[0].unused, 0u);
  EXPECT_OK(zx_handle_close(rd_handles[0].handle));

  EXPECT_NE(rd_handles[1].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(rd_handles[1].type, ZX_OBJ_TYPE_SOCKET);
  EXPECT_EQ(rd_handles[1].rights, ZX_DEFAULT_SOCKET_RIGHTS);
  EXPECT_EQ(rd_handles[1].unused, 0u);
  EXPECT_OK(zx_handle_close(rd_handles[1].handle));

  EXPECT_NE(rd_handles[2].handle, ZX_HANDLE_INVALID);
  EXPECT_EQ(rd_handles[2].type, ZX_OBJ_TYPE_PORT);
  EXPECT_EQ(rd_handles[2].rights, ZX_DEFAULT_PORT_RIGHTS);
  EXPECT_EQ(rd_handles[2].unused, 0u);
  EXPECT_OK(zx_handle_close(rd_handles[2].handle));

  server_thread.join();
}

TEST(ChannelCallEtcTest, IovecSuccessCase) {
  EchoServer echo_server;
  zx::channel client_end = echo_server.ClientEnd();

  char chunk1[4] = {'a', 'b', 'c', 'd'};
  char chunk2[4] = {'e', 'f', 'g', 'h'};
  zx_channel_iovec_t iovecs[2] = {
      {
          .buffer = chunk1,
          .capacity = sizeof(chunk1),
          .reserved = 0,
      },
      {
          .buffer = chunk2,
          .capacity = sizeof(chunk2),
          .reserved = 0,
      },
  };

  char response_bytes[8] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = iovecs,
      .wr_handles = nullptr,
      .rd_bytes = response_bytes,
      .rd_handles = nullptr,
      .wr_num_bytes = 2,
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(response_bytes),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  ASSERT_OK(client_end.call_etc(ZX_CHANNEL_WRITE_USE_IOVEC, zx::time::infinite(), &args,
                                &actual_bytes, &actual_handles));
  EXPECT_EQ(actual_bytes, 8u);
  EXPECT_EQ(actual_handles, 0u);
  EXPECT_BYTES_EQ(chunk2, response_bytes + 4, 4);
}

TEST(ChannelCallEtcTest, IovecTotalCapacityLessThanTxidSizeReturnsInvalidArgs) {
  zx::channel client_end, server_end;
  ASSERT_OK(zx::channel::create(0, &client_end, &server_end));

  char tiny[2] = {'a', 'b'};
  zx_channel_iovec_t iovecs[1] = {
      {
          .buffer = tiny,
          .capacity = sizeof(tiny),
          .reserved = 0,
      },
  };

  char response[4] = {};
  zx_channel_call_etc_args_t args = {
      .wr_bytes = iovecs,
      .wr_handles = nullptr,
      .rd_bytes = response,
      .rd_handles = nullptr,
      .wr_num_bytes = 1,
      .wr_num_handles = 0,
      .rd_num_bytes = sizeof(response),
      .rd_num_handles = 0,
  };

  uint32_t actual_bytes = 0;
  uint32_t actual_handles = 0;
  EXPECT_EQ(client_end.call_etc(ZX_CHANNEL_WRITE_USE_IOVEC, zx::time::infinite(), &args,
                                &actual_bytes, &actual_handles),
            ZX_ERR_INVALID_ARGS);
}

}  // namespace
