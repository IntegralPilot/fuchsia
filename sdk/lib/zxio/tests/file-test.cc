// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sync/completion.h>
#include <lib/zxio/ops.h>

#include <atomic>
#include <memory>

#include <zxtest/zxtest.h>

#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/tests/file_test_suite.h"
#include "sdk/lib/zxio/tests/test_file_server_base.h"

namespace {

namespace fio = fuchsia_io;

class CloseCountingFileServer : public zxio_tests::TestFileServerBase {
 public:
  CloseCountingFileServer() = default;
  ~CloseCountingFileServer() override = default;

  virtual void Init() {}

  // Exercised by |zxio_close|.
  void Close(CloseCompleter::Sync& completer) final {
    num_close_.fetch_add(1);
    zxio_tests::TestFileServerBase::Close(completer);
  }

  void Query(QueryCompleter::Sync& completer) final {
    const std::string_view kProtocol = fidl::DiscoverableProtocolName<fuchsia_io::File>;
    // TODO(https://fxbug.dev/42052765): avoid the const cast.
    uint8_t* data = reinterpret_cast<uint8_t*>(const_cast<char*>(kProtocol.data()));
    completer.Reply(fidl::VectorView<uint8_t>::FromExternal(data, kProtocol.size()));
  }

  void Describe(DescribeCompleter::Sync& completer) override { completer.Reply({}); }

  uint32_t num_close() const { return num_close_.load(); }

  void ForceErrorAfterNCalls(uint8_t n, zx_status_t status) {
    EXPECT_EQ(forced_error_after_n_calls_, std::nullopt);
    forced_error_after_n_calls_ = std::make_pair(n, status);
  }

 protected:
  zx_status_t CheckForcedError() {
    if (forced_error_after_n_calls_) {
      auto& [n, status] = forced_error_after_n_calls_.value();
      if (n == 0) {
        forced_error_after_n_calls_ = std::nullopt;
        return status;
      }

      --n;
    }

    return ZX_OK;
  }

 private:
  std::atomic<uint32_t> num_close_ = 0;
  std::optional<std::pair<uint8_t, zx_status_t>> forced_error_after_n_calls_ = std::nullopt;
};

class File : public zxtest::Test {
 public:
  ~File() override { binding_->Unbind(); }

  template <typename ServerImpl>
  void StartServer() {
    server_ = std::make_unique<ServerImpl>();
    ASSERT_NO_FATAL_FAILURE(server_->Init());
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);
    ASSERT_OK(loop_->StartThread("fake-filesystem"));
  }

  template <typename ServerImpl>
  void StartAndGetServer(ServerImpl** out_server) {
    ASSERT_NO_FATAL_FAILURE(StartServer<ServerImpl>());
    *out_server = static_cast<ServerImpl*>(server_.get());
  }

  zx::result<fidl::ClientEnd<fio::File>> OpenConnection() {
    zx::result ends = fidl::CreateEndpoints<fio::File>();
    if (ends.is_error()) {
      return ends.take_error();
    }
    auto binding = fidl::BindServer(loop_->dispatcher(), std::move(ends->server), server_.get());
    binding_ = std::make_unique<fidl::ServerBindingRef<fio::File>>(std::move(binding));
    return zx::ok(std::move(ends->client));
  }

  zx_status_t OpenFile() {
    zx::result client_end = OpenConnection();
    if (client_end.is_error()) {
      return client_end.status_value();
    }
    fidl::WireResult result = fidl::WireCall(client_end.value())->Describe();
    if (result.status() != ZX_OK) {
      return result.status();
    }
    fio::wire::FileInfo& file = result.value();
    return zxio_file_init(&file_, file.has_observer() ? std::move(file.observer()) : zx::event{},
                          file.has_stream() ? std::move(file.stream()) : zx::stream{},
                          std::move(client_end.value()));
  }

  void TearDown() override {
    ASSERT_EQ(0, server_->num_close());
    ASSERT_OK(zxio_close(&file_.io));
    zxio_destroy(&file_.io);
    ASSERT_EQ(1, server_->num_close());
  }

 protected:
  zxio_storage_t file_;
  std::unique_ptr<CloseCountingFileServer> server_;
  std::unique_ptr<fidl::ServerBindingRef<fio::File>> binding_;
  std::unique_ptr<async::Loop> loop_;
};

class TestServerEvent final : public CloseCountingFileServer {
 public:
  void Init() override { ASSERT_OK(zx::event::create(0, &event_)); }

  const zx::event& event() const { return event_; }

  void Describe(DescribeCompleter::Sync& completer) final {
    zx::event event;
    if (zx_status_t status = event_.duplicate(ZX_RIGHTS_BASIC, &event); status != ZX_OK) {
      completer.Close(ZX_ERR_INTERNAL);
      return;
    }
    fidl::Arena alloc;
    completer.Reply(fio::wire::FileInfo::Builder(alloc).observer(std::move(event)).Build());
  }

 private:
  zx::event event_;
};

class GetAttributesTestServer final : public CloseCountingFileServer {
 public:
  void GetAttributes(GetAttributesRequestView, GetAttributesCompleter::Sync& completer) final {
    fuchsia_io::ImmutableNodeAttributes immutable_attrs;
    immutable_attrs.protocols() = fuchsia_io::NodeProtocolKinds::kFile;
    fidl::Arena arena;
    completer.ReplySuccess(/*mutable_attrs*/ {}, fidl::ToWire(arena, immutable_attrs));
  }
};

TEST_F(File, Open) {
  ASSERT_NO_FAILURES(StartServer<GetAttributesTestServer>());
  ASSERT_NO_FAILURES(OpenFile());

  zxio_node_attributes_t attr = {.has = {.object_type = true}};
  ASSERT_OK(zxio_attr_get(&file_.io, &attr));
  EXPECT_EQ(ZXIO_OBJECT_TYPE_FILE, attr.object_type);
}

TEST_F(File, WaitTimeOut) {
  ASSERT_NO_FAILURES(StartServer<TestServerEvent>());
  ASSERT_NO_FAILURES(OpenFile());

  zxio_signals_t observed = ZX_SIGNAL_NONE;
  ASSERT_STATUS(zxio_wait_one(&file_.io, ZXIO_SIGNAL_ALL, ZX_TIME_INFINITE_PAST, &observed),
                ZX_ERR_TIMED_OUT);
  EXPECT_EQ(ZXIO_SIGNAL_NONE, observed);
}

TEST_F(File, WaitForReadable) {
  TestServerEvent* server;
  ASSERT_NO_FAILURES(StartAndGetServer<TestServerEvent>(&server));
  ASSERT_NO_FAILURES(OpenFile());

  zxio_signals_t observed = ZX_SIGNAL_NONE;
  ASSERT_OK(server->event().signal(
      ZX_SIGNAL_NONE, static_cast<zx_signals_t>(fuchsia_io::wire::FileSignal::kReadable)));
  ASSERT_OK(zxio_wait_one(&file_.io, ZXIO_SIGNAL_READABLE, ZX_TIME_INFINITE_PAST, &observed));
  EXPECT_EQ(ZXIO_SIGNAL_READABLE, observed);
}

TEST_F(File, WaitForWritable) {
  TestServerEvent* server;
  ASSERT_NO_FAILURES(StartAndGetServer<TestServerEvent>(&server));
  ASSERT_NO_FAILURES(OpenFile());

  zxio_signals_t observed = ZX_SIGNAL_NONE;
  ASSERT_OK(server->event().signal(
      ZX_SIGNAL_NONE, static_cast<zx_signals_t>(fuchsia_io::wire::FileSignal::kWritable)));
  ASSERT_OK(zxio_wait_one(&file_.io, ZXIO_SIGNAL_WRITABLE, ZX_TIME_INFINITE_PAST, &observed));
  EXPECT_EQ(ZXIO_SIGNAL_WRITABLE, observed);
}

TEST_F(File, GetVmoPropagatesError) {
  // Positive error codes are protocol-specific errors, and will not
  // occur in the system.
  static constexpr zx_status_t kGetAttrError = 1;
  static constexpr zx_status_t kGetBufferError = 2;

  class TestServer : public CloseCountingFileServer {
   public:
    void GetAttributes(GetAttributesRequestView, GetAttributesCompleter::Sync& completer) final {
      completer.ReplyError(kGetAttrError);
    }

    void GetBackingMemory(GetBackingMemoryRequestView request,
                          GetBackingMemoryCompleter::Sync& completer) final {
      completer.ReplyError(kGetBufferError);
    }
  };
  ASSERT_NO_FAILURES(StartServer<TestServer>());
  ASSERT_NO_FAILURES(OpenFile());

  zx::vmo vmo;
  ASSERT_STATUS(kGetBufferError, zxio_vmo_get_clone(&file_.io, vmo.reset_and_get_address()));
  ASSERT_STATUS(kGetBufferError, zxio_vmo_get_exact(&file_.io, vmo.reset_and_get_address()));
  ASSERT_STATUS(kGetAttrError, zxio_vmo_get_copy(&file_.io, vmo.reset_and_get_address()));
}

class TestServerChannel final : public CloseCountingFileServer {
 public:
  void Init() override {
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &store_));
    const size_t kZero = 0u;
    ASSERT_OK(store_.set_property(ZX_PROP_VMO_CONTENT_SIZE, &kZero, sizeof(kZero)));
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, store_, 0, &stream_));
  }

  void Read(ReadRequestView request, ReadCompleter::Sync& completer) override {
    zx_status_t status = CheckForcedError();
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    if (request->count > fio::wire::kMaxBuf) {
      completer.Close(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    uint8_t buffer[fio::wire::kMaxBuf];
    zx_iovec_t vec = {
        .buffer = buffer,
        .capacity = request->count,
    };
    size_t actual = 0u;
    status = stream_.readv(0, &vec, 1, &actual);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(buffer, actual));
  }

  void ReadAt(ReadAtRequestView request, ReadAtCompleter::Sync& completer) override {
    zx_status_t status = CheckForcedError();
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    if (request->count > fio::wire::kMaxBuf) {
      completer.Close(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    uint8_t buffer[fio::wire::kMaxBuf];
    zx_iovec_t vec = {
        .buffer = buffer,
        .capacity = request->count,
    };
    size_t actual = 0u;
    status = stream_.readv_at(0, request->offset, &vec, 1, &actual);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(buffer, actual));
  }

  void Write(WriteRequestView request, WriteCompleter::Sync& completer) override {
    zx_status_t status = CheckForcedError();
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    if (request->data.size() > fio::wire::kMaxBuf) {
      completer.Close(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    zx_iovec_t vec = {
        .buffer = request->data.data(),
        .capacity = request->data.size(),
    };
    size_t actual = 0u;
    status = stream_.writev(0, &vec, 1, &actual);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    completer.ReplySuccess(actual);
  }

  void WriteAt(WriteAtRequestView request, WriteAtCompleter::Sync& completer) override {
    zx_status_t status = CheckForcedError();
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }

    if (request->data.size() > fio::wire::kMaxBuf) {
      completer.Close(ZX_ERR_OUT_OF_RANGE);
      return;
    }
    zx_iovec_t vec = {
        .buffer = request->data.data(),
        .capacity = request->data.size(),
    };
    size_t actual = 0u;
    status = stream_.writev_at(0, request->offset, &vec, 1, &actual);
    if (status != ZX_OK) {
      completer.ReplyError(status);
      return;
    }
    completer.ReplySuccess(actual);
  }

  void Seek(SeekRequestView request, SeekCompleter::Sync& completer) override {
    zx_off_t seek;
    if (zx_status_t status = stream_.seek(static_cast<zx_stream_seek_origin_t>(request->origin),
                                          request->offset, &seek);
        status != ZX_OK) {
      completer.ReplyError(status);
    } else {
      completer.ReplySuccess(seek);
    }
  }

 private:
  zx::vmo store_;
  zx::stream stream_;
};

TEST_F(File, ReadWriteChannel) {
  ASSERT_NO_FAILURES(StartServer<TestServerChannel>());
  ASSERT_OK(OpenFile());
  ASSERT_NO_FAILURES(FileTestSuite::ReadWrite(&file_.io));
}

TEST_F(File, ReadvWritevChannel) {
  ASSERT_NO_FAILURES(StartServer<TestServerChannel>());
  ASSERT_OK(OpenFile());

  auto check_io =
      [&](zx_status_t (*zxio_fn)(zxio_t*, const zx_iovec_t*, size_t, zxio_flags_t, size_t*),
          void* buf, size_t buflen, zx_status_t status) {
        char random_unused_buf[1];
        const zx_iovec_t iov[2] = {
            {
                .buffer = buf,
                .capacity = buflen,
            },
            {
                .buffer = reinterpret_cast<void*>(random_unused_buf),
                .capacity = sizeof(random_unused_buf),
            },
        };

        size_t actual = 0u;
        server_->ForceErrorAfterNCalls(0, status);
        ASSERT_NOT_OK(zxio_fn(&file_.io, reinterpret_cast<const zx_iovec_t*>(iov), 2, 0, &actual),
                      status);
        server_->ForceErrorAfterNCalls(1, status);
        ASSERT_OK(zxio_fn(&file_.io, reinterpret_cast<const zx_iovec_t*>(iov), 2, 0, &actual));
        EXPECT_EQ(actual, buflen);
      };

  char write_buf[] = "abcd";
  ASSERT_NO_FAILURES(check_io(zxio_writev, write_buf, sizeof(write_buf), ZX_ERR_IO));

  size_t seek = 0;
  ASSERT_OK(zxio_seek(&file_.io, ZXIO_SEEK_ORIGIN_START, 0, &seek));
  EXPECT_EQ(seek, 0);

  char read_buf[sizeof(write_buf) - 2] = {};
  ASSERT_NO_FAILURES(check_io(zxio_readv, read_buf, sizeof(read_buf), ZX_ERR_NOT_FOUND));
  ASSERT_EQ(strncmp(write_buf, read_buf, sizeof(read_buf)), 0);
}

class TestServerStream final : public CloseCountingFileServer {
 public:
  void Init() override {
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &store_));
    const size_t kZero = 0u;
    ASSERT_OK(store_.set_property(ZX_PROP_VMO_CONTENT_SIZE, &kZero, sizeof(kZero)));
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, store_, 0, &stream_));
  }

  void Describe(DescribeCompleter::Sync& completer) final {
    zx::stream stream;
    if (zx_status_t status = stream_.duplicate(ZX_RIGHT_SAME_RIGHTS, &stream); status != ZX_OK) {
      completer.Close(ZX_ERR_INTERNAL);
      return;
    }
    fidl::Arena alloc;
    completer.Reply(fio::wire::FileInfo::Builder(alloc).stream(std::move(stream)).Build());
  }

 private:
  zx::vmo store_;
  zx::stream stream_;
};

TEST_F(File, ReadWriteStream) {
  ASSERT_NO_FAILURES(StartServer<TestServerStream>());
  ASSERT_OK(OpenFile());
  ASSERT_NO_FAILURES(FileTestSuite::ReadWrite(&file_.io));
}

}  // namespace
