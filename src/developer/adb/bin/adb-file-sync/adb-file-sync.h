// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_ADB_BIN_ADB_FILE_SYNC_ADB_FILE_SYNC_H_
#define SRC_DEVELOPER_ADB_BIN_ADB_FILE_SYNC_ADB_FILE_SYNC_H_

#include <fidl/fuchsia.hardware.adb/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/zx/result.h>

#include "src/developer/adb/bin/adb-file-sync/adb_file_sync_config.h"
#include "src/developer/adb/third_party/adb-file-sync/adb-file-sync-base.h"

namespace adb_file_sync {

class AdbFileSync : public AdbFileSyncBase,
                    public fidl::WireServer<fuchsia_hardware_adb::Provider> {
 public:
  AdbFileSync(adb_file_sync_config::Config config, async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher),
        context_(std::make_unique<sys::ComponentContext>(
            sys::ServiceDirectory::CreateFromNamespace(), dispatcher)),
        config_(std::move(config)) {}

  static zx_status_t StartService(adb_file_sync_config::Config config);
  void OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<fuchsia_hardware_adb::Provider> server_end);

  void ConnectToService(fuchsia_hardware_adb::wire::ProviderConnectToServiceRequest* request,
                        ConnectToServiceCompleter::Sync& completer) override;

  zx::result<zx::channel> ConnectToComponent(std::string name,
                                             std::vector<std::string>* out_path) override;

  async_dispatcher_t* dispatcher() { return dispatcher_; }

 private:
  friend class AdbFileSyncTest;

  zx_status_t ConnectToAdbDevice(zx::channel chan);

  async_dispatcher_t* dispatcher_;
  std::unique_ptr<sys::ComponentContext> context_;
  std::optional<fidl::ServerBindingRef<fuchsia_hardware_adb::Provider>> binding_ref_;
  adb_file_sync_config::Config config_;
  fidl::SyncClient<fuchsia_sys2::RealmQuery> realm_query_;
  fidl::SyncClient<fuchsia_sys2::LifecycleController> lifecycle_;
};

}  // namespace adb_file_sync

#endif  // SRC_DEVELOPER_ADB_BIN_ADB_FILE_SYNC_ADB_FILE_SYNC_H_
