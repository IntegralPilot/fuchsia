// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/feedback_data.h"

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/fdio/spawn.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <memory>
#include <optional>
#include <utility>

namespace forensics::feedback {

FeedbackData::FeedbackData(async_dispatcher_t* dispatcher,
                           std::shared_ptr<sys::ServiceDirectory> services,
                           timekeeper::Clock* clock, inspect::Node* inspect_root,
                           cobalt::Logger* cobalt, RedactorBase* redactor,
                           feedback::AnnotationManager* annotation_manager, Options options)
    : dispatcher_(dispatcher),
      services_(services),
      clock_(clock),
      cobalt_(cobalt),
      inspect_node_manager_(inspect_root),
      inspect_data_budget_(options.limit_inspect_data, &inspect_node_manager_, cobalt_),
      attachment_providers_(dispatcher, services, SpawnSystemLogRecorder(),
                            options.delete_previous_boot_logs_time, clock, redactor,
                            &inspect_data_budget_, options.snapshot_config.attachment_allowlist,
                            cobalt_, std::move(options.root_job)),
      data_provider_(dispatcher_, services_, clock_, redactor, options.is_first_instance,
                     options.snapshot_config.default_annotations,
                     options.snapshot_config.attachment_allowlist, cobalt_, annotation_manager,
                     attachment_providers_.GetAttachmentManager(), &inspect_data_budget_) {}

feedback_data::DataProvider* FeedbackData::DataProvider() { return &data_provider_; }

void FeedbackData::ShutdownImminent(::fit::deferred_callback stop_respond) {
  system_log_recorder_lifecycle_.set_error_handler(
      [stop_respond = std::move(stop_respond)](const zx_status_t status) mutable {
        if (status != ZX_OK) {
          FX_PLOGS(WARNING, status) << "Lost connection to system log recorder";
        }

        // |stop_respond| must explicitly be called otherwise it won't run until the error
        // handler is destroyed (which doesn't happen).
        stop_respond.call();
      });
  system_log_recorder_lifecycle_->Stop();
}

std::shared_ptr<sys::ServiceDirectory> FeedbackData::SpawnSystemLogRecorder() {
  zx::channel lifecycle_client, lifecycle_server;
  if (const zx_status_t status = zx::channel::create(0, &lifecycle_client, &lifecycle_server);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status)
        << "Failed to create system log recorder lifecycle channel, logs will not be persisted";
    return nullptr;
  }

  zx::channel directory_client, directory_server;
  if (const zx_status_t status =
          zx::channel::create(/*flags=*/0, &directory_client, &directory_server);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status)
        << "Failed to create system log recorder directory channel, logs will not be persisted";
    return nullptr;
  }

  const std::array<const char*, 2> argv = {
      "system_log_recorder" /* process name */,
      nullptr,
  };
  const std::array actions = {
      fdio_spawn_action_t{
          .action = FDIO_SPAWN_ACTION_ADD_HANDLE,
          .h =
              {
                  .id = PA_HND(PA_USER0, 0),
                  .handle = lifecycle_server.release(),
              },
      },
      fdio_spawn_action_t{
          .action = FDIO_SPAWN_ACTION_ADD_HANDLE,
          .h =
              {
                  .id = PA_HND(PA_DIRECTORY_REQUEST, 0),
                  .handle = directory_server.release(),
              },
      },
  };

  zx_handle_t process;
  char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH] = {};
  if (const zx_status_t status = fdio_spawn_etc(
          ZX_HANDLE_INVALID, FDIO_SPAWN_CLONE_ALL, "/pkg/bin/system_log_recorder", argv.data(),
          /*environ=*/nullptr, actions.size(), actions.data(), &process, err_msg);
      status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Failed to spawn system log recorder, logs will not be persisted: "
                            << err_msg;
    return nullptr;
  }

  system_log_recorder_lifecycle_.Bind(std::move(lifecycle_client), dispatcher_);

  zx::result svc_client = component::OpenDirectoryAt(
      fidl::UnownedClientEnd<fuchsia_io::Directory>(directory_client.get()), "svc");
  if (svc_client.is_error()) {
    FX_PLOGS(ERROR, svc_client.status_value())
        << "Failed to open /svc in system log recorder outgoing directory";
    return nullptr;
  }

  return std::make_shared<sys::ServiceDirectory>(svc_client->TakeChannel());
}

}  // namespace forensics::feedback
