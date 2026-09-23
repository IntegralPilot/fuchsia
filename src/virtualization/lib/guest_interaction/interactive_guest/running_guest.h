// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_RUNNING_GUEST_H_
#define SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_RUNNING_GUEST_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.virtualization.guest.interaction/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>

#include <string>

#include <fbl/unique_fd.h>

#include "src/virtualization/lib/guest_interaction/client/client_impl.h"

namespace interactive_guest {

using FileTransferCallback = fit::function<void(zx_status_t status)>;

class RunningGuest {
 public:
  RunningGuest(std::string name, fbl::unique_fd vsock_fd, async_dispatcher_t* dispatcher,
               fidl::SyncClient<fuchsia_virtualization::Guest> guest_sync);
  ~RunningGuest();
  void PutFile(fidl::ClientEnd<fuchsia_io::File> host_source, const std::string& guest_dest,
               FileTransferCallback result);
  void GetFile(const std::string& guest_source, fidl::ClientEnd<fuchsia_io::File> host_dest,
               FileTransferCallback result);
  void Execute(std::string command, std::map<std::string, std::string> env_variables,
               zx::socket std_in, zx::socket std_out, zx::socket std_err,
               fidl::ServerEnd<fuchsia_virtualization_guest_interaction::CommandListener> listener);
  void Shutdown();
  const char* Name();

 private:
  std::string name_;
  thrd_t guest_interaction_service_thread_;
  async_dispatcher_t* dispatcher_;
  std::unique_ptr<ClientImpl<PosixPlatform>> interaction_client_;

  // Unused, but retained for lifecycle management.
  fidl::SyncClient<fuchsia_virtualization::Guest> guest_sync_;
};

}  // namespace interactive_guest

#endif  // SRC_VIRTUALIZATION_LIB_GUEST_INTERACTION_INTERACTIVE_GUEST_RUNNING_GUEST_H_
