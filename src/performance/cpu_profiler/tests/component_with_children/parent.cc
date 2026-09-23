// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

void RepeatedlyCallClient(const fidl::Client<fuchsia_examples::Echo>& echo, size_t idx) {
  echo->EchoString({"ECHO!!"})
      .Then([&echo, idx](fidl::Result<fuchsia_examples::Echo::EchoString> res) {
        if (res.is_ok()) {
          RepeatedlyCallClient(echo, idx);
        }
      });
}

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  zx::result<fidl::ClientEnd<fuchsia_examples::Echo>> client_end1 =
      component::Connect<fuchsia_examples::Echo>("/svc/fuchsia.examples.Echo.1");
  if (client_end1.is_error()) {
    FX_LOGS(ERROR) << "Unable to connect to echo.1";
    return 1;
  }
  fidl::Client echo1{std::move(*client_end1), dispatcher};

  zx::result<fidl::ClientEnd<fuchsia_examples::Echo>> client_end2 =
      component::Connect<fuchsia_examples::Echo>("/svc/fuchsia.examples.Echo.2");
  if (client_end2.is_error()) {
    FX_LOGS(ERROR) << "Unable to connect to echo.2";
    return 1;
  }
  fidl::Client echo2{std::move(*client_end2), dispatcher};

  zx::result<fidl::ClientEnd<fuchsia_examples::Echo>> client_end3 =
      component::Connect<fuchsia_examples::Echo>("/svc/fuchsia.examples.Echo.3");
  if (client_end3.is_error()) {
    FX_LOGS(ERROR) << "Unable to connect to echo.3";
    return 1;
  }
  fidl::Client echo3{std::move(*client_end3), dispatcher};

  RepeatedlyCallClient(echo1, 1);
  RepeatedlyCallClient(echo2, 2);
  RepeatedlyCallClient(echo3, 3);
  loop.Run();
}
