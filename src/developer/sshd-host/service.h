// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_SSHD_HOST_SERVICE_H_
#define SRC_DEVELOPER_SSHD_HOST_SERVICE_H_

#include <fidl/fuchsia.boot/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.developer.console/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/async/wait.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/process.h>
#include <lib/zx/socket.h>
#include <sys/socket.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/lib/fsl/tasks/fd_waiter.h"

namespace sshd_host {

// Name of the collection that contains sshd shell child components.
inline constexpr std::string_view kShellCollection = "shell";
class Service;

// Service relies on the default async dispatcher and is not thread safe.
class Service {
 public:
  Service(async_dispatcher_t* dispatcher, uint16_t port);
  ~Service();

 private:
  struct Controller;

  void Wait();
  void Launch(fbl::unique_fd conn);
  void LaunchConsole(fbl::unique_fd conn);

  async_dispatcher_t* dispatcher_;
  fbl::unique_fd sock_;
  fsl::FDWaiter waiter_;
  uint64_t next_child_num_ = 0;
  zx::eventpair console_stopper_local_;
  zx::eventpair console_stopper_;

  struct LogRedirect {
    LogRedirect(async_dispatcher_t* dispatcher, zx::socket socket, uint64_t child_tag);
    ~LogRedirect();
    void OnLog(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
               const zx_packet_signal_t* signal);
    void Wait();
    async_dispatcher_t* dispatcher_;
    zx::socket socket_;
    uint64_t child_tag_;
    async::WaitMethod<LogRedirect, &LogRedirect::OnLog> waiter_;
    std::string buf_;
  };
};

}  // namespace sshd_host

#endif  // SRC_DEVELOPER_SSHD_HOST_SERVICE_H_
