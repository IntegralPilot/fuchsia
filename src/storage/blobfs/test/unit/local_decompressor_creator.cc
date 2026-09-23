// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "local_decompressor_creator.h"

#include <fidl/fuchsia.blobfs.internal/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/sync/completion.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <functional>
#include <memory>
#include <utility>

#include "src/storage/blobfs/compression/external_decompressor.h"

namespace blobfs {
namespace {

class LambdaConnector : public DecompressorCreatorConnector {
 public:
  using Callback =
      std::function<zx_status_t(fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator>)>;
  explicit LambdaConnector(Callback callback);

  // ExternalDecompressorCreatorConnector interface.
  zx_status_t ConnectToDecompressorCreator(
      fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> server_end) final;

 private:
  Callback callback_;
};

LambdaConnector::LambdaConnector(Callback callback) : callback_(std::move(callback)) {}

zx_status_t LambdaConnector::ConnectToDecompressorCreator(
    fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> server_end) {
  return callback_(std::move(server_end));
}

}  // namespace

zx::result<std::unique_ptr<LocalDecompressorCreator>> LocalDecompressorCreator::Create() {
  std::unique_ptr<LocalDecompressorCreator> decompressor(new LocalDecompressorCreator());
  decompressor->connector_ = std::make_unique<LambdaConnector>(
      [decompressor = decompressor.get()](
          fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> remote_channel) {
        return decompressor->RegisterChannel(std::move(remote_channel));
      });
  if (zx_status_t status = decompressor->loop_.StartThread(); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(decompressor));
}

LocalDecompressorCreator::~LocalDecompressorCreator() {
  sync_completion_t done;
  // Unbind everything from the server thread and prevent future bindings.
  ZX_ASSERT(ZX_OK == async::PostTask(loop_.dispatcher(), [this, &done]() {
              this->shutting_down_ = true;
              this->bindings_.RemoveAll();
              sync_completion_signal(&done);
            }));

  sync_completion_wait(&done, ZX_TIME_INFINITE);
}

zx_status_t LocalDecompressorCreator::RegisterChannel(
    fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> channel) {
  // Pushing binding management onto the server thread since the bindings are not thread safe.
  zx_status_t bind_status = ZX_OK;
  sync_completion_t done;
  zx_status_t post_status = async::PostTask(loop_.dispatcher(), [&]() mutable {
    if (this->shutting_down_) {
      bind_status = ZX_ERR_CANCELED;
    } else {
      this->RegisterChannelOnServerThread(std::move(channel));
    }
    sync_completion_signal(&done);
  });
  if (post_status != ZX_OK) {
    return post_status;
  }

  sync_completion_wait(&done, ZX_TIME_INFINITE);
  return bind_status;
}

void LocalDecompressorCreator::RegisterChannelOnServerThread(
    fidl::ServerEnd<fuchsia_blobfs_internal::DecompressorCreator> channel) {
  bindings_.AddBinding(loop_.dispatcher(), std::move(channel), &decompressor_,
                       fidl::kIgnoreBindingClosure);
}

}  // namespace blobfs
