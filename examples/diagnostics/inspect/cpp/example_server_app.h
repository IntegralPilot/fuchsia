// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DIAGNOSTICS_INSPECT_CPP_EXAMPLE_SERVER_APP_H_
#define EXAMPLES_DIAGNOSTICS_INSPECT_CPP_EXAMPLE_SERVER_APP_H_

#include <lib/async-loop/cpp/loop.h>
// [START inspect_imports]
#include <lib/inspect/component/cpp/component.h>
// [END inspect_imports]
#include "echo_connection.h"

namespace example {

class ExampleServerApp {
 public:
  explicit ExampleServerApp();

 private:
  ExampleServerApp(const ExampleServerApp&) = delete;
  ExampleServerApp& operator=(const ExampleServerApp&) = delete;

  std::unique_ptr<inspect::ComponentInspector> inspector_;
  std::shared_ptr<EchoConnectionStats> echo_stats_;
  // Must hang on to the outgoing directory handle so the services are not closed.
  std::optional<component::OutgoingDirectory> outgoing_directory_;
};

}  // namespace example

#endif  // EXAMPLES_DIAGNOSTICS_INSPECT_CPP_EXAMPLE_SERVER_APP_H_
