// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_SERVICE_OTA_HEALTH_CHECK_H_
#define SRC_STORAGE_BLOBFS_SERVICE_OTA_HEALTH_CHECK_H_

#ifndef __Fuchsia__
#error Fuchsia-only Header
#endif

#include <fidl/fuchsia.update.verify/cpp/wire.h>
#include <lib/async/dispatcher.h>

#include <fbl/ref_ptr.h>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/lib/vfs/cpp/service.h"

namespace blobfs {

// OtaHealthCheckService is a service which clients can use to ask blobfs to perform basic
// self-checks of runtime behaviour (such as reading, writing files).
class OtaHealthCheckService
    : public fidl::WireServer<fuchsia_update_verify::ComponentOtaHealthCheck>,
      public fs::Service {
 public:
  // fuchsia.update.verify.ComponentOtaHealthCheck interface
  void GetHealthStatus(GetHealthStatusCompleter::Sync& completer) final;

 protected:
  friend fbl::internal::MakeRefCountedHelper<OtaHealthCheckService>;
  friend fbl::RefPtr<OtaHealthCheckService>;

  OtaHealthCheckService(async_dispatcher_t* dispatcher, Blobfs& blobfs);

  ~OtaHealthCheckService() override = default;

 private:
  Blobfs& blobfs_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_SERVICE_OTA_HEALTH_CHECK_H_
