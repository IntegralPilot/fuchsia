// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.examples/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/outgoing/cpp/handlers.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/channel.h>
#include <lib/zx/handle.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <array>
#include <memory>
#include <thread>
#include <utility>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>
#include <src/lib/testing/predicates/status.h>
#include <src/storage/lib/vfs/cpp/managed_vfs.h>
#include <src/storage/lib/vfs/cpp/pseudo_dir.h>
#include <src/storage/lib/vfs/cpp/pseudo_file.h>

namespace {

// Expected path of directory hosting FIDL Services & Protocols.
constexpr char kSvcDirectoryPath[] = "svc";

constexpr char kTestString[] = "FizzBuzz";
constexpr char kTestStringReversed[] = "zzuBzziF";

class EchoImpl final : public fidl::WireServer<fuchsia_examples::Echo> {
 public:
  explicit EchoImpl(bool reversed) : reversed_(reversed) {}

  EchoImpl(bool reversed, fit::deferred_callback on_destruction)
      : reversed_(reversed), on_destruction_(std::move(on_destruction)) {}

  void SendString(SendStringRequestView request, SendStringCompleter::Sync& completer) override {}

  void EchoString(EchoStringRequestView request, EchoStringCompleter::Sync& completer) override {
    std::string value(request->value.get());
    if (reversed_) {
      std::reverse(value.begin(), value.end());
    }
    completer.Reply(fidl::StringView::FromExternal(value));
  }

 private:
  bool reversed_ = false;

  std::optional<fit::deferred_callback> on_destruction_ = std::nullopt;
};

class NaturalEchoImpl final : public fidl::Server<fuchsia_examples::Echo> {
 public:
  explicit NaturalEchoImpl(bool reversed) : reversed_(reversed) {}

  void SendString(SendStringRequest& request, SendStringCompleter::Sync& completer) override {}

  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    std::string value(request.value());
    if (reversed_) {
      std::reverse(value.begin(), value.end());
    }
    completer.Reply(value);
  }

 private:
  bool reversed_ = false;
};

class OutgoingDirectoryTest : public gtest::RealLoopFixture {
 public:
  void SetUp() override {
    outgoing_directory_ = std::make_unique<component::OutgoingDirectory>(dispatcher());
    zx::result server = fidl::CreateEndpoints(&client_end_);
    ASSERT_TRUE(server.is_ok()) << server.status_string();
    zx::result result = outgoing_directory_->Serve(std::move(server.value()));
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  component::OutgoingDirectory* GetOutgoingDirectory() { return outgoing_directory_.get(); }

  std::unique_ptr<component::OutgoingDirectory> TakeOutgoingDirectory() {
    return std::move(outgoing_directory_);
  }

  static fidl::ClientEnd<fuchsia_io::Directory> GetSvcClientEnd(
      const fidl::ClientEnd<fuchsia_io::Directory>& root,
      fidl::StringView path = kSvcDirectoryPath) {
    auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    fidl::OneWayStatus status = fidl::WireCall(root)->Open(
        path, fuchsia_io::wire::kPermReadable | fuchsia_io::wire::Flags::kProtocolDirectory, {},
        server_end.TakeChannel());
    EXPECT_TRUE(status.ok()) << status.FormatDescription();
    return std::move(client_end);
  }

  fidl::ClientEnd<fuchsia_io::Directory> TakeRootClientEnd() { return std::move(client_end_); }

  const fidl::ClientEnd<fuchsia_io::Directory>& GetRootClientEnd() { return client_end_; }

 protected:
  fidl::WireClient<fuchsia_examples::Echo> ConnectToServiceMember(
      fuchsia_examples::EchoService::ServiceClient& service, bool reversed) {
    zx::result connect_result =
        reversed ? service.connect_reversed_echo() : service.connect_regular_echo();
    EXPECT_TRUE(connect_result.is_ok()) << connect_result.status_string();
    return fidl::WireClient(std::move(connect_result.value()), dispatcher());
  }

  // Service handler that is pre-populated. This is only used for tests that
  // want to test failure paths.
  static fuchsia_examples::EchoService::InstanceHandler CreateNonEmptyServiceHandler() {
    return fuchsia_examples::EchoService::InstanceHandler({
        .regular_echo =
            [](fidl::ServerEnd<fuchsia_examples::Echo>) {
              // no op
            },
        .reversed_echo =
            [](fidl::ServerEnd<fuchsia_examples::Echo>) {
              // no op
            },
    });
  }

 private:
  std::unique_ptr<component::OutgoingDirectory> outgoing_directory_ = nullptr;
  fidl::ClientEnd<fuchsia_io::Directory> client_end_;
};

TEST_F(OutgoingDirectoryTest, MutualExclusionGuarantees_CheckOperations) {
  std::optional<component::OutgoingDirectory> outgoing_directory;
  outgoing_directory.emplace(dispatcher());

  // Cannot mutate it from a foreign thread.
  ASSERT_DEATH(
      {
        std::thread t([&] {
          EXPECT_TRUE(outgoing_directory.has_value());
          outgoing_directory.value()
              .AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(/*reversed=*/false))
              .status_value();
        });
        t.join();
      },
      "\\|component::OutgoingDirectory\\| is thread-unsafe\\.");

  // Cannot destroy it on a foreign thread.
  ASSERT_DEATH(
      {
        std::thread t([&] {
          EXPECT_TRUE(outgoing_directory.has_value());
          outgoing_directory.reset();
        });
        t.join();
      },
      "\\|component::OutgoingDirectory\\| is thread-unsafe\\.");

  // Properly destroy it on the main thread.
  outgoing_directory.reset();
}

TEST_F(OutgoingDirectoryTest, MutualExclusionGuarantees_CheckDispatcher) {
  component::OutgoingDirectory outgoing_directory{dispatcher()};
  ASSERT_DEATH(
      {
        std::thread t([&] { RunLoopUntilIdle(); });
        t.join();
      },
      "\\|component::OutgoingDirectory\\| is thread-unsafe\\.");
}

TEST_F(OutgoingDirectoryTest, CanBeMovedSafely) {
  component::OutgoingDirectory outgoing_directory(dispatcher());
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_EQ(outgoing_directory
                .AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(/*reversed=*/false))
                .status_value(),
            ZX_OK);

  zx::result result = outgoing_directory.Serve(std::move(endpoints->server));
  ASSERT_TRUE(result.is_ok()) << result.status_string();

  component::OutgoingDirectory moved_in_constructor(std::move(outgoing_directory));
  component::OutgoingDirectory moved_in_assignment = component::OutgoingDirectory(dispatcher());
  moved_in_assignment = std::move(moved_in_constructor);

  zx::result client_end =
      component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(endpoints->client));
  ASSERT_TRUE(client_end.is_ok()) << client_end.status_string();
  fidl::WireClient<fuchsia_examples::Echo> client(std::move(client_end.value()), dispatcher());

  std::string reply_received;
  client->EchoString(kTestString)
      .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                           fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& result) {
        ASSERT_TRUE(result.ok()) << "EchoString failed: " << result.error();
        auto* response = result.Unwrap();
        reply_received = std::string(response->response.data(), response->response.size());
        quit_loop();
      });
  RunLoop();

  EXPECT_EQ(reply_received, kTestString);
}

TEST_F(OutgoingDirectoryTest, AddProtocol) {
  // Setup fuchsia.examples.Echo server.
  ASSERT_EQ(
      GetOutgoingDirectory()
          ->AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(/*reversed=*/false))
          .status_value(),
      ZX_OK);

  // Setup fuchsia.examples.Echo client.
  zx::result client_end =
      component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(GetRootClientEnd()));
  ASSERT_EQ(client_end.status_value(), ZX_OK);
  fidl::WireClient<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher());

  std::string reply_received;
  client->EchoString(kTestString)
      .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                           fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& result) {
        ASSERT_TRUE(result.ok()) << "EchoString failed: " << result.error();
        auto* response = result.Unwrap();
        reply_received = std::string(response->response.data(), response->response.size());
        quit_loop();
      });
  RunLoop();

  EXPECT_EQ(reply_received, kTestString);
}

TEST_F(OutgoingDirectoryTest, AddProtocolNaturalServer) {
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddProtocol<fuchsia_examples::Echo>(
                    std::make_unique<NaturalEchoImpl>(/*reversed=*/false))
                .status_value(),
            ZX_OK);

  // Setup fuchsia.examples.Echo client.
  zx::result client_end =
      component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(GetRootClientEnd()));
  ASSERT_EQ(client_end.status_value(), ZX_OK);
  fidl::Client<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher());

  std::string reply_received;
  client->EchoString({kTestString})
      .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                           fidl::Result<fuchsia_examples::Echo::EchoString>& result) {
        ASSERT_TRUE(result.is_ok()) << "EchoString failed: " << result.error_value();
        reply_received = result->response();
        quit_loop();
      });
  RunLoop();

  EXPECT_EQ(reply_received, kTestString);
}

// Test that outgoing directory is able to serve multiple service members. In
// this case, the directory will host the `fuchsia.examples.EchoService` which
// contains two `fuchsia.examples.Echo` member. One regular, and one reversed.
TEST_F(OutgoingDirectoryTest, AddServiceServesAllMembers) {
  EchoImpl regular_impl(/*reversed=*/false);
  EchoImpl reversed_impl(/*reversed=*/true);
  {
    zx::result result = GetOutgoingDirectory()->AddService<fuchsia_examples::EchoService>(
        std::move(fuchsia_examples::EchoService::InstanceHandler({
            .regular_echo = regular_impl.bind_handler(dispatcher()),
            .reversed_echo = reversed_impl.bind_handler(dispatcher()),
        })));
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // Setup test client.
  zx::result open_result =
      component::OpenServiceAt<fuchsia_examples::EchoService>(GetSvcClientEnd(GetRootClientEnd()));
  ASSERT_TRUE(open_result.is_ok()) << open_result.status_string();

  fuchsia_examples::EchoService::ServiceClient service = std::move(open_result.value());

  // Assert that service is connected and that proper impl returns expected reply.
  for (bool reversed : {true, false}) {
    bool message_echoed = false;
    fidl::WireClient client = ConnectToServiceMember(service, reversed);
    std::string_view expected_reply = reversed ? kTestStringReversed : kTestString;
    client->EchoString(kTestString)
        .ThenExactlyOnce(
            [quit_loop = QuitLoopClosure(), &message_echoed, expected_reply = expected_reply](
                fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& reply) {
              EXPECT_TRUE(reply.ok()) << "Reply failed with: " << reply.error().status_string();
              EXPECT_EQ(reply.value().response.get(), expected_reply);
              message_echoed = true;
              quit_loop();
            });

    RunLoop();

    EXPECT_TRUE(message_echoed);
  }

  // Next, assert that after removing the service, the directory connection to
  // the directory housing the service members will be closed.
  zx::channel client, server;
  ASSERT_OK(zx::channel::create(0, &client, &server));
  zx::result result = component::internal::OpenNamedServiceAtRaw(
      GetSvcClientEnd(GetRootClientEnd()), fuchsia_examples::EchoService::Name,
      component::kDefaultInstance, std::move(server));
  ASSERT_OK(result.status_value());
  RunLoopUntilIdle();
  {
    zx_signals_t pending = ZX_SIGNAL_NONE;
    EXPECT_STATUS(ZX_ERR_TIMED_OUT,
                  client.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &pending));
  }
  {
    zx::result result = GetOutgoingDirectory()->RemoveService<fuchsia_examples::EchoService>();
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }
  RunLoopUntilIdle();
  {
    zx_signals_t pending = ZX_SIGNAL_NONE;
    EXPECT_OK(client.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(), &pending));
    EXPECT_EQ(pending, ZX_CHANNEL_PEER_CLOSED);
  }
}

// Test that serving a FIDL Protocol works as expected.
TEST_F(OutgoingDirectoryTest, AddProtocolCanServeMultipleProtocols) {
  constexpr static std::array<std::pair<bool, const char*>, 2> kIsReversedAndPaths = {
      {{false, "fuchsia.examples.Echo"}, {true, "fuchsia.examples.Ohce"}}};

  // Setup fuchsia.examples.Echo servers
  for (auto [reversed, path] : kIsReversedAndPaths) {
    ASSERT_EQ(GetOutgoingDirectory()
                  ->AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(reversed), path)
                  .status_value(),
              ZX_OK);
  }

  // Setup fuchsia.examples.Echo client
  for (auto [reversed, path] : kIsReversedAndPaths) {
    zx::result client_end =
        component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(GetRootClientEnd()), path);
    ASSERT_EQ(client_end.status_value(), ZX_OK);
    fidl::WireClient<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher());

    std::string reply_received;
    client->EchoString(kTestString)
        .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                             fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& result) {
          ASSERT_TRUE(result.ok()) << "EchoString failed: " << result.error();
          auto* response = result.Unwrap();
          reply_received = std::string(response->response.data(), response->response.size());
          quit_loop();
        });
    RunLoop();

    std::string_view expected_reply = reversed ? kTestStringReversed : kTestString;
    EXPECT_EQ(reply_received, expected_reply);
  }
}

// Test that serving a FIDL Protocol from a non-svc directory works as expected.
TEST_F(OutgoingDirectoryTest, AddProtocolAtServesProtocol) {
  constexpr static char kDirectory[] = "test";

  // Setup fuchsia.examples.Echo servers
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddProtocolAt<fuchsia_examples::Echo>(
                    kDirectory, std::make_unique<EchoImpl>(/*reversed=*/false))
                .status_value(),
            ZX_OK);

  zx::result client_end = component::ConnectAt<fuchsia_examples::Echo>(
      GetSvcClientEnd(GetRootClientEnd(), /*path=*/kDirectory));
  ASSERT_EQ(client_end.status_value(), ZX_OK);
  fidl::WireClient<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher());

  std::string reply_received;
  client->EchoString(kTestString)
      .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                           fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& result) {
        ASSERT_TRUE(result.ok()) << "EchoString failed: " << result.error().FormatDescription();
        auto* response = result.Unwrap();
        reply_received = std::string(response->response.data(), response->response.size());
        quit_loop();
      });
  RunLoop();

  EXPECT_EQ(reply_received, kTestString);
}

TEST_F(OutgoingDirectoryTest, AddServiceAtServesServiceInNonSvcDirectory) {
  constexpr static char kDirectory[] = "test";

  EchoImpl regular_impl(/*reversed=*/false);
  {
    zx::result result = GetOutgoingDirectory()->AddServiceAt<fuchsia_examples::EchoService>(
        std::move(fuchsia_examples::EchoService::InstanceHandler({
            .regular_echo = regular_impl.bind_handler(dispatcher()),
            .reversed_echo = nullptr,
        })),
        /*path=*/kDirectory);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // Setup test client.
  zx::result open_result = component::OpenServiceAt<fuchsia_examples::EchoService>(
      GetSvcClientEnd(GetRootClientEnd(), /*path=*/kDirectory));
  ASSERT_TRUE(open_result.is_ok()) << open_result.status_string();

  fuchsia_examples::EchoService::ServiceClient service = std::move(open_result.value());

  // Assert that service is connected and that proper impl returns expected reply.
  bool message_echoed = false;
  fidl::WireClient client = ConnectToServiceMember(service, /*reversed=*/false);
  std::string_view expected_reply = kTestString;
  client->EchoString(kTestString)
      .ThenExactlyOnce(
          [quit_loop = QuitLoopClosure(), &message_echoed, expected_reply = expected_reply](
              fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& reply) {
            EXPECT_TRUE(reply.ok()) << "Reply failed with: " << reply.error().status_string();
            EXPECT_EQ(reply.value().response.get(), expected_reply);
            message_echoed = true;
            quit_loop();
          });

  RunLoop();

  EXPECT_TRUE(message_echoed);
}

TEST_F(OutgoingDirectoryTest, AddServiceAtServesServiceInNestedPath) {
  constexpr static char kDirectory[] = "foo/bar";

  EchoImpl regular_impl(/*reversed=*/false);
  {
    zx::result result = GetOutgoingDirectory()->AddServiceAt<fuchsia_examples::EchoService>(
        std::move(fuchsia_examples::EchoService::InstanceHandler({
            .regular_echo = regular_impl.bind_handler(dispatcher()),
            .reversed_echo = nullptr,
        })),
        /*path=*/kDirectory);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // Setup test client.
  zx::result open_result = component::OpenServiceAt<fuchsia_examples::EchoService>(
      GetSvcClientEnd(GetRootClientEnd(), /*path=*/kDirectory));
  ASSERT_TRUE(open_result.is_ok()) << open_result.status_string();

  fuchsia_examples::EchoService::ServiceClient service = std::move(open_result.value());

  // Assert that service is connected and that proper impl returns expected reply.
  bool message_echoed = false;
  fidl::WireClient client = ConnectToServiceMember(service, /*reversed=*/false);
  std::string_view expected_reply = kTestString;
  client->EchoString(kTestString)
      .ThenExactlyOnce(
          [quit_loop = QuitLoopClosure(), &message_echoed, expected_reply = expected_reply](
              fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& reply) {
            EXPECT_TRUE(reply.ok()) << "Reply failed with: " << reply.error().status_string();
            EXPECT_EQ(reply.value().response.get(), expected_reply);
            message_echoed = true;
            quit_loop();
          });

  RunLoop();

  EXPECT_TRUE(message_echoed);
}

TEST_F(OutgoingDirectoryTest, AddDirectoryAtCanServeADirectory) {
  static constexpr char kTestPath[] = "root";
  static constexpr char kTestDirectory[] = "diagnostics";
  static constexpr char kTestFile[] = "sample.txt";
  static constexpr char kTestContent[] = "Hello World!";

  fs::ManagedVfs vfs(dispatcher());
  auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto text_file = fbl::MakeRefCounted<fs::BufferedPseudoFile>(
      /*read_handler=*/[](fbl::String* output) -> zx_status_t {
        *output = kTestContent;
        return ZX_OK;
      });
  auto diagnostics = fbl::MakeRefCounted<fs::PseudoDir>();
  diagnostics->AddEntry(kTestFile, text_file);
  vfs.ServeDirectory(diagnostics, std::move(endpoints.server));
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddDirectoryAt(std::move(endpoints.client), kTestPath, kTestDirectory)
                .status_value(),
            ZX_OK);

  std::thread([client_end = TakeRootClientEnd().TakeChannel().release(),
               quit_loop = QuitLoopClosure()]() {
    fbl::unique_fd root_fd;
    ASSERT_EQ(fdio_fd_create(client_end, root_fd.reset_and_get_address()), ZX_OK);

    std::string path = std::string(kTestPath) + "/" + std::string(kTestDirectory);
    fbl::unique_fd dir_fd(openat(root_fd.get(), path.c_str(), O_DIRECTORY));
    ASSERT_TRUE(dir_fd.is_valid()) << strerror(errno);

    fbl::unique_fd file_fd(openat(dir_fd.get(), kTestFile, O_RDONLY));
    ASSERT_TRUE(file_fd.is_valid()) << strerror(errno);
    static constexpr size_t kMaxBufferSize = 1024;
    static char kReadBuffer[kMaxBufferSize];
    ssize_t bytes_read = read(file_fd.get(), reinterpret_cast<void*>(kReadBuffer), kMaxBufferSize);
    ASSERT_GE(bytes_read, 0) << strerror(errno);

    std::string actual_content(kReadBuffer, bytes_read);
    EXPECT_EQ(actual_content, kTestContent);
    quit_loop();
  }).detach();

  RunLoop();

  vfs.Shutdown([quit_loop = QuitLoopClosure()](zx_status_t status) {
    ASSERT_EQ(status, ZX_OK);
    quit_loop();
  });
  RunLoop();

  EXPECT_EQ(GetOutgoingDirectory()->RemoveDirectory(kTestPath).status_value(), ZX_OK);
}

// Test that we can connect to the outgoing directory via multiple connections.
TEST_F(OutgoingDirectoryTest, ServeCanYieldMultipleConnections) {
  // Setup fuchsia.examples.Echo server
  ASSERT_EQ(
      GetOutgoingDirectory()
          ->AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(/*reversed=*/false))
          .status_value(),
      ZX_OK);

  // Setup fuchsia.examples.Echo client
  auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  // First |Serve| is invoked as part of test setup, so we'll assert that a
  // subsequent invocation is allowed.
  ASSERT_EQ(GetOutgoingDirectory()->Serve(std::move(endpoints.server)).status_value(), ZX_OK);

  std::vector<fidl::ClientEnd<fuchsia_io::Directory>> root_client_ends;
  // Take client end for channel used during invocation of |Serve| during setup.
  root_client_ends.emplace_back(TakeRootClientEnd());
  // Take client end for channel used during invocation of |Serve| in this function.
  root_client_ends.emplace_back(std::move(endpoints.client));

  while (!root_client_ends.empty()) {
    fidl::ClientEnd root = std::move(root_client_ends.back());
    root_client_ends.pop_back();

    zx::result client_end =
        component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(/*root=*/root));
    ASSERT_EQ(client_end.status_value(), ZX_OK);
    fidl::WireClient<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher());

    std::string reply_received;
    client->EchoString(kTestString)
        .ThenExactlyOnce([&reply_received, quit_loop = QuitLoopClosure()](
                             fidl::WireUnownedResult<fuchsia_examples::Echo::EchoString>& result) {
          ASSERT_TRUE(result.ok()) << "EchoString failed: " << result.error();
          auto* response = result.Unwrap();
          reply_received = std::string(response->response.data(), response->response.size());
          quit_loop();
        });
    RunLoop();

    EXPECT_EQ(reply_received, kTestString);
  }
}

class EchoEventHandler : public fidl::AsyncEventHandler<fuchsia_examples::Echo> {
 public:
  EchoEventHandler() = default;

  void on_fidl_error(fidl::UnbindInfo error) override { errors_.emplace_back(error); }

  std::vector<fidl::UnbindInfo> GetErrors() { return errors_; }

 private:
  std::vector<fidl::UnbindInfo> errors_;
};

// Test that after removing protocol, all clients are unable to make a call on
// the channel.
TEST_F(OutgoingDirectoryTest, RemoveProtocolClosesAllConnections) {
  // For this test case, 3 clients will connect to one Echo protocol.
  static constexpr size_t kNumClients = 3;

  EchoEventHandler event_handler;
  bool destroyed = false;
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(
                    /*reversed*/ false, /*on_destruction=*/fit::deferred_callback(
                        [&destroyed]() { destroyed = true; })))
                .status_value(),
            ZX_OK);

  fidl::ClientEnd<fuchsia_io::Directory> svc_directory = GetSvcClientEnd(GetRootClientEnd());
  std::vector<fidl::Client<fuchsia_examples::Echo>> clients = {};
  for (size_t i = 0; i < kNumClients; ++i) {
    zx::result client_end = component::ConnectAt<fuchsia_examples::Echo>(svc_directory.borrow());
    ASSERT_EQ(client_end.status_value(), ZX_OK);

    fidl::Client<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher(),
                                                &event_handler);
    client->EchoString(std::string(kTestString))
        .ThenExactlyOnce([quit_loop = QuitLoopClosure()](
                             fidl::Result<fuchsia_examples::Echo::EchoString>& result) {
          ASSERT_TRUE(result.is_ok());
          ASSERT_EQ(result->response(), kTestString);
          quit_loop();
        });
    RunLoop();

    clients.emplace_back(std::move(client));
  }

  ASSERT_EQ(GetOutgoingDirectory()->RemoveProtocol<fuchsia_examples::Echo>().status_value(), ZX_OK);
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler.GetErrors().size(), kNumClients);
  for (auto& error : event_handler.GetErrors()) {
    EXPECT_TRUE(error.is_peer_closed())
        << "Expected peer_closed. Got : " << error.FormatDescription();
  }
  ASSERT_TRUE(destroyed);
}

TEST_F(OutgoingDirectoryTest, OutgoingDirectoryDestructorClosesAllConnections) {
  bool destroyed = false;
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddProtocol<fuchsia_examples::Echo>(std::make_unique<EchoImpl>(
                    /*reversed*/ false, /*on_destruction=*/fit::deferred_callback(
                        [&destroyed]() { destroyed = true; })))
                .status_value(),
            ZX_OK);

  // Setup client
  zx::result client_end =
      component::ConnectAt<fuchsia_examples::Echo>(GetSvcClientEnd(TakeRootClientEnd()));
  ASSERT_EQ(client_end.status_value(), ZX_OK);

  EchoEventHandler event_handler;
  fidl::Client<fuchsia_examples::Echo> client(std::move(*client_end), dispatcher(), &event_handler);
  client->EchoString(std::string(kTestString))
      .ThenExactlyOnce([quit_loop = QuitLoopClosure()](
                           fidl::Result<fuchsia_examples::Echo::EchoString>& result) {
        ASSERT_TRUE(result.is_ok());
        ASSERT_EQ(result->response(), kTestString);
        quit_loop();
      });
  RunLoop();

  // We have to take the outgoing directory instead of creating a local one
  // because they would share a dispatcher and may fail synchronization checks.
  std::unique_ptr outgoing_directory = TakeOutgoingDirectory();
  // Destroy outgoing directory object.
  outgoing_directory.reset();
  RunLoopUntilIdle();

  ASSERT_EQ(event_handler.GetErrors().size(), 1u);
  for (auto& error : event_handler.GetErrors()) {
    EXPECT_TRUE(error.is_peer_closed())
        << "Expected peer_closed. Got : " << error.FormatDescription();
  }

  ASSERT_TRUE(destroyed);
}

TEST_F(OutgoingDirectoryTest, CreateFailsIfDispatcherIsNullptr) {
  ASSERT_DEATH({ component::OutgoingDirectory outgoing_directory(/*dispatcher=*/nullptr); }, "");
}

TEST_F(OutgoingDirectoryTest, ServeFailsIfHandleInvalid) {
  component::OutgoingDirectory outgoing_directory(dispatcher());
  auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  // Close server end in order to  invalidate channel.
  endpoints.server.reset();
  EXPECT_EQ(outgoing_directory.Serve(std::move(endpoints.server)).status_value(),
            ZX_ERR_BAD_HANDLE);
}

TEST_F(OutgoingDirectoryTest, AddServiceFailsIfInstanceNameIsEmpty) {
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddService<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(),
                                                            /*instance=*/"")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddServiceFailsIfEntryExists) {
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddService<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler())
                .status_value(),
            ZX_OK);

  EXPECT_EQ(GetOutgoingDirectory()
                ->AddService<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler())
                .status_value(),
            ZX_ERR_ALREADY_EXISTS);
}

TEST_F(OutgoingDirectoryTest, AddServiceFailsIfServiceHandlerEmpty) {
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddService<fuchsia_examples::EchoService>(
                    fuchsia_examples::EchoService::InstanceHandler())
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddServiceFailsIfServiceNameIsEmpty) {
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddService(CreateNonEmptyServiceHandler(), /*service=*/"",
                             /*instance=*/component::kDefaultInstance)
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddProtocolFailsIfImplIsNullptr) {
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddProtocol<fuchsia_examples::Echo>(
                    /*impl=*/static_cast<std::unique_ptr<fidl::WireServer<fuchsia_examples::Echo>>>(
                        nullptr))
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddProtocolAtFailsIfEntryExists) {
  constexpr std::string_view kProtocolName = "fuchsia.example.Echo";

  component::AnyHandler handler = [](zx::channel request) {};
  ASSERT_EQ(GetOutgoingDirectory()
                ->AddUnmanagedProtocolAt(std::move(handler), /*path=*/kSvcDirectoryPath,
                                         /*name=*/kProtocolName)
                .status_value(),
            ZX_OK);

  component::AnyHandler another_handler = [](zx::channel request) {};
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddUnmanagedProtocolAt(std::move(another_handler), /*path=*/kSvcDirectoryPath,
                                         /*name=*/kProtocolName)
                .status_value(),
            ZX_ERR_ALREADY_EXISTS);
}

TEST_F(OutgoingDirectoryTest, AddProtocolAtFailsIfNameIsEmpty) {
  component::AnyHandler handler = [](zx::channel request) {};
  EXPECT_EQ(
      GetOutgoingDirectory()
          ->AddUnmanagedProtocolAt(std::move(handler), /*path=*/kSvcDirectoryPath, /*name=*/"")
          .status_value(),
      ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddProtocolAtFailsIfDirectoryIsEmpty) {
  component::AnyHandler handler = [](zx::channel request) {};
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddUnmanagedProtocolAt(/*handler=*/std::move(handler), /*path=*/"",
                                         /*name=*/"fuchsia.examples.Echo")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddServiceAtFailsIfDirectoryIsEmpty) {
  component::AnyHandler handler = [](zx::channel request) {};
  EXPECT_EQ(
      GetOutgoingDirectory()
          ->AddServiceAt<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(), /*path=*/"")
          .status_value(),
      ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddServiceAtFailsIfPathMalformed) {
  component::AnyHandler handler = [](zx::channel request) {};
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddServiceAt<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(),
                                                              /*path=*/"/")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddServiceAt<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(),
                                                              /*path=*/"//")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddServiceAt<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(),
                                                              /*path=*/"foo//bar")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddServiceAt<fuchsia_examples::EchoService>(CreateNonEmptyServiceHandler(),
                                                              /*path=*/"foo/bar/")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddDirectoryFailsIfRemoteDirInvalid) {
  fidl::ClientEnd<fuchsia_io::Directory> dangling_client_end;
  ASSERT_FALSE(dangling_client_end.is_valid());

  EXPECT_EQ(GetOutgoingDirectory()
                ->AddDirectory(std::move(dangling_client_end), "AValidName")
                .status_value(),
            ZX_ERR_BAD_HANDLE);
}

TEST_F(OutgoingDirectoryTest, AddDirectoryFailsIfDirectoryNameIsEmpty) {
  auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();

  EXPECT_EQ(GetOutgoingDirectory()
                ->AddDirectory(std::move(endpoints.client), /*directory_name=*/"")
                .status_value(),
            ZX_ERR_INVALID_ARGS);
}

TEST_F(OutgoingDirectoryTest, AddDirectoryFailsIfEntryExists) {
  constexpr char kDirectoryName[] = "test";

  for (auto expected_status : {ZX_OK, ZX_ERR_ALREADY_EXISTS}) {
    zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
    ASSERT_TRUE(endpoints.is_ok()) << endpoints.status_string();
    EXPECT_EQ(GetOutgoingDirectory()
                  ->AddDirectory(std::move(endpoints->client), kDirectoryName)
                  .status_value(),
              expected_status);
  }
}

TEST_F(OutgoingDirectoryTest, AddDirectoryFailsIfNameUsedForAddProtocolAt) {
  constexpr char kDirectoryName[] = "diagnostics";

  ASSERT_EQ(GetOutgoingDirectory()
                ->AddProtocolAt<fuchsia_examples::Echo>(
                    kDirectoryName, std::make_unique<EchoImpl>(/*reversed*/ false))
                .status_value(),
            ZX_OK);

  auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  EXPECT_EQ(GetOutgoingDirectory()
                ->AddDirectory(std::move(endpoints.client), kDirectoryName)
                .status_value(),
            ZX_ERR_ALREADY_EXISTS);
}

TEST_F(OutgoingDirectoryTest, RemoveServiceFailsIfEntryDoesNotExist) {
  EXPECT_EQ(GetOutgoingDirectory()->RemoveService<fuchsia_examples::EchoService>().status_value(),
            ZX_ERR_NOT_FOUND);
}

TEST_F(OutgoingDirectoryTest, RemoveProtocolFailsIfEntryDoesNotExist) {
  EXPECT_EQ(GetOutgoingDirectory()->RemoveProtocol<fuchsia_examples::Echo>().status_value(),
            ZX_ERR_NOT_FOUND);
}

TEST_F(OutgoingDirectoryTest, RemoveDirectoryFailsIfEntryDoesNotExist) {
  EXPECT_EQ(GetOutgoingDirectory()->RemoveDirectory(/*directory_name=*/"test").status_value(),
            ZX_ERR_NOT_FOUND);
}

class OutgoingDirectoryPathParameterizedFixture
    : public testing::TestWithParam<std::pair<std::string, std::string>> {};

TEST_P(OutgoingDirectoryPathParameterizedFixture, BadServicePaths) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  component::OutgoingDirectory outgoing_directory(loop.dispatcher());
  zx::result endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  ASSERT_TRUE(endpoints.is_ok()) << endpoints.status_string();
  {
    zx::result result = outgoing_directory.Serve(std::move(endpoints->server));
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  auto [service_name, instance_name] = GetParam();
  EXPECT_EQ(
      outgoing_directory
          .AddService(fuchsia_examples::EchoService::InstanceHandler(), service_name, instance_name)
          .status_value(),
      ZX_ERR_INVALID_ARGS);
}

INSTANTIATE_TEST_SUITE_P(OutgoingDirectoryTestPathTest, OutgoingDirectoryPathParameterizedFixture,
                         testing::Values(std::make_pair("", component::kDefaultInstance),
                                         std::make_pair(".", component::kDefaultInstance),
                                         std::make_pair("fuchsia.examples.EchoService", ""),
                                         std::make_pair("fuchsia.examples.EchoService", "")));

}  // namespace
