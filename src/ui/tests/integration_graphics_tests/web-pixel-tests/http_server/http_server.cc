// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <arpa/inet.h>
#include <lib/stdcompat/string_view.h>
#include <lib/syslog/cpp/macros.h>

#include <cstddef>
#include <string>
#include <thread>

#include <fbl/unique_fd.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/ui/tests/integration_graphics_tests/web-pixel-tests/constants.h"

namespace {

constexpr int kMaxBufferSize = 2048;

}  // namespace

// This is a simple HTTP server that accepts the connection, reads once, sends reply and closes.
class HttpServer {
 public:
  // Constr.
  explicit HttpServer() {}

  ~HttpServer() {
    if (server_fd_.is_valid()) {
      shutdown(server_fd_.get(), SHUT_RDWR);
      server_fd_.reset();
    }
  }

  // Binds the server to |kPort| and listens for incoming connections.
  void Serve() {
    FX_CHECK(!server_fd_.is_valid());

    server_fd_ = fbl::unique_fd(socket(AF_INET, SOCK_STREAM, 0));
    FX_CHECK(server_fd_.is_valid());

    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port = htons(kPort);
    socklen_t addrlen = sizeof(addr);
    FX_CHECK(bind(server_fd_.get(), reinterpret_cast<sockaddr*>(&addr), addrlen) == 0);
    FX_CHECK(getsockname(server_fd_.get(), reinterpret_cast<sockaddr*>(&addr), &addrlen) == 0);
    FX_CHECK(listen(server_fd_.get(), 1) == 0);
  }

  // Listen for incoming requests and send a response.
  void Run() {
    while (true) {
      sockaddr_in addr{};
      socklen_t addrlen = sizeof(addr);
      fbl::unique_fd conn(accept(server_fd_.get(), reinterpret_cast<sockaddr*>(&addr), &addrlen));
      if (!conn.is_valid()) {
        break;
      }

      // Read the incoming request.
      std::string buf(kMaxBufferSize, 0);
      ssize_t ret = read(conn.get(), buf.data(), buf.size());
      FX_CHECK(ret >= 0);
      buf.resize(ret);

      std::string file;
      if (cpp23::contains(buf, kStaticHtml)) {
        file = fxl::StringPrintf("/pkg/data/%s", kStaticHtml);
      } else if (cpp23::contains(buf, kDynamicHtml)) {
        file = fxl::StringPrintf("/pkg/data/%s", kDynamicHtml);
      } else if (cpp23::contains(buf, kFourColorsVideo)) {
        file = fxl::StringPrintf("/pkg/data/%s", kFourColorsVideo);
      } else if (cpp23::contains(buf, kVideoHtml)) {
        file = fxl::StringPrintf("/pkg/data/%s", kVideoHtml);
      } else {
        FX_NOTIMPLEMENTED();
      }

      std::string content;
      FX_CHECK(files::ReadFileToString(file, &content));

      // Send the response to the client.
      std::ostringstream response;
      response << "HTTP/1.1 200 OK\r\n"
               << "Content-Length: " << content.size() << "\r\n\r\n"
               << content;
      ssize_t wret = write(conn.get(), response.str().data(), response.str().size());
      FX_CHECK(wret == static_cast<ssize_t>(response.str().size()));
    }
  }

 private:
  fbl::unique_fd server_fd_;
};

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "Http Server started ....";
  HttpServer server;
  server.Serve();
  server.Run();
}
