// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.test.echo/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/wait.h>
#include <lib/zx/channel.h>
#include <zircon/status.h>

#include <runtests-utils/service-proxy-dir.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace fio = fuchsia_io;

namespace {

class Echo : public fidl::WireServer<fidl_test_echo::Echo> {
 public:
  explicit Echo(std::string response) : response_(std::move(response)) {}

  void EchoString(EchoStringRequestView request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(fidl::StringView::FromExternal(response_));
  }

 private:
  std::string response_;
};

constexpr char kTestString[] = "test";
constexpr char kEchoString[] = "echo";
constexpr char kProxyEchoString[] = "proxy_echo";

TEST(ServiceProxyDirTest, Simple) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::unique_ptr<fs::SynchronousVfs> vfs;

  vfs = std::make_unique<fs::SynchronousVfs>(loop.dispatcher());

  Echo echo(kEchoString);
  auto dir = fbl::MakeRefCounted<fs::PseudoDir>();
  dir->AddEntry(
      kEchoString,
      fbl::MakeRefCounted<fs::Service>(
          [&echo, dispatcher = loop.dispatcher()](fidl::ServerEnd<fidl_test_echo::Echo> request) {
            fidl::BindServer(dispatcher, std::move(request), &echo);
            return ZX_OK;
          }));
  ASSERT_OK(loop.StartThread());

  auto [client, server] = fidl::Endpoints<fio::Directory>::Create();

  ASSERT_OK(vfs->ServeDirectory(std::move(dir), std::move(server)));
  ASSERT_OK(loop.StartThread());

  Echo proxy_echo(kProxyEchoString);
  auto proxy_dir = fbl::MakeRefCounted<runtests::ServiceProxyDir>(std::move(client));
  proxy_dir->AddEntry(
      kProxyEchoString,
      fbl::MakeRefCounted<fs::Service>([&proxy_echo, dispatcher = loop.dispatcher()](
                                           fidl::ServerEnd<fidl_test_echo::Echo> request) {
        fidl::BindServer(dispatcher, std::move(request), &proxy_echo);
        return ZX_OK;
      }));
  ASSERT_OK(loop.StartThread());

  auto [proxy_client, proxy_server] = fidl::Endpoints<fio::Directory>::Create();

  ASSERT_OK(vfs->ServeDirectory(std::move(proxy_dir), std::move(proxy_server)));
  ASSERT_OK(loop.StartThread());

  // First check the service served directly by the proxy.
  {
    auto [client, server] = fidl::Endpoints<fidl_test_echo::Echo>::Create();
    ASSERT_OK(fidl::WireCall(proxy_client)
                  ->Open(fidl::StringView(kProxyEchoString), fio::Flags::kProtocolService, {},
                         server.TakeChannel())
                  .status());
    const fidl::WireResult result = fidl::WireCall(client)->EchoString(kTestString);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_EQ(response.response.get(), kProxyEchoString);
  }

  // Second check the service that's being proxied by the proxy.
  {
    auto [client, server] = fidl::Endpoints<fidl_test_echo::Echo>::Create();
    ASSERT_OK(fidl::WireCall(proxy_client)
                  ->Open(fidl::StringView(kEchoString), fio::Flags::kProtocolService, {},
                         server.TakeChannel())
                  .status());
    const fidl::WireResult result = fidl::WireCall(client)->EchoString(kTestString);
    ASSERT_OK(result.status());
    const fidl::WireResponse response = result.value();
    ASSERT_EQ(response.response.get(), kEchoString);
  }

  loop.Shutdown();
}

}  // anonymous namespace
