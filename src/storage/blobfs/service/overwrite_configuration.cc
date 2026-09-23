// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/service/overwrite_configuration.h"

#include <lib/syslog/cpp/macros.h>

#include "src/storage/blobfs/blobfs.h"

namespace blobfs {

OverwriteConfigurationService::OverwriteConfigurationService(async_dispatcher_t* dispatcher,
                                                             Blobfs& blobfs)
    : fs::Service([this, dispatcher](
                      fidl::ServerEnd<fuchsia_storage_blobfs::OverwriteConfiguration> server_end) {
        bindings_.AddBinding(dispatcher, std::move(server_end), this, fidl::kIgnoreBindingClosure);
        return ZX_OK;
      }),
      blobfs_(blobfs) {}

void OverwriteConfigurationService::Set(SetRequestView request, SetCompleter::Sync& completer) {
  BlobOverwriteConfig config;
  switch (request->overwrite_format) {
    case fuchsia_storage_blobfs::wire::OverwriteFormat::kNoOverwrite:
      config = BlobOverwriteConfig::kNoOverwrite;
      break;
    case fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToCompact:
      config = BlobOverwriteConfig::kOverwriteToCompact;
      break;
    case fuchsia_storage_blobfs::wire::OverwriteFormat::kOverwriteToPadded:
      config = BlobOverwriteConfig::kOverwriteToPadded;
      break;
    default:
      // Unknown format.
      completer.ReplyError(ZX_ERR_INVALID_ARGS);
      return;
  }
  blobfs_.SetOverwriteConfig(config);
  completer.ReplySuccess();
}

void OverwriteConfigurationService::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_storage_blobfs::OverwriteConfiguration> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

}  // namespace blobfs
