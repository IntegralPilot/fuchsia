// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>
#include <sys/stat.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

#include <fbl/ref_ptr.h>

#include "src/developer/vsock-sshd-host/data_dir.h"
#include "src/developer/vsock-sshd-host/service.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/vfs_types.h"
#include "src/storage/lib/vfs/cpp/vnode.h"

namespace {
const uint16_t kPort = 22;

class DevNullVnode : public fs::Vnode {
 public:
  explicit DevNullVnode() = default;

  zx_status_t Read(void* data, size_t len, size_t off, size_t* out_actual) override {
    *out_actual = 0;
    return ZX_OK;
  }

  zx_status_t Write(const void* data, size_t len, size_t off, size_t* out_actual) override {
    *out_actual = len;
    return ZX_OK;
  }

  zx_status_t Truncate(size_t len) override { return ZX_OK; }

  zx::result<fs::VnodeAttributes> GetAttributes() const override {
    return zx::ok(fs::VnodeAttributes{
        .mode = S_IFCHR | S_IRUSR | S_IWUSR,
    });
  }

  fuchsia_io::NodeProtocolKinds GetProtocols() const override {
    return fuchsia_io::NodeProtocolKinds::kFile;
  }
};

}  // namespace

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"sshd-host"}).BuildAndInitialize();

  FX_LOG_KV(INFO, "sshd-host starting up");

  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fs::ManagedVfs vfs(loop.dispatcher());

  auto root = fbl::MakeRefCounted<fs::PseudoDir>();
  root->AddEntry("data", BuildDataDir());
  auto dev = fbl::MakeRefCounted<fs::PseudoDir>();
  dev->AddEntry("null", fbl::MakeRefCounted<DevNullVnode>());
  root->AddEntry("dev", std::move(dev));

  // Serve outgoing directory
  auto outgoing_request = fidl::ServerEnd<fuchsia_io::Directory>(
      zx::channel((zx_take_startup_handle(PA_DIRECTORY_REQUEST))));
  if (zx_status_t status = vfs.ServeDirectory(std::move(root), std::move(outgoing_request));
      status != ZX_OK) {
    FX_LOGS(FATAL) << "Failed to host outgoing directory " << status;
  }

  uint16_t port = kPort;
  if (argc > 1) {
    int arg = atoi(argv[1]);
    if (arg <= 0) {
      FX_LOG_KV(ERROR, "Invalid port", FX_KV("argv[1]", argv[1]));
      return -1;
    }
    port = static_cast<uint16_t>(arg);
  }
  sshd_host::Service service(loop.dispatcher(), port);

  if (zx_status_t status = loop.Run(); status != ZX_OK) {
    FX_LOG_KV(FATAL, "Failed to run loop", FX_KV("status", zx_status_get_string(status)));
  }

  return 0;
}
