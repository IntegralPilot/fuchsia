// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_SERVER_FAKE_SERVER_H_
#define SRC_STORAGE_LIB_BLOCK_SERVER_FAKE_SERVER_H_

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/vmo.h>

#include <memory>

#include "src/storage/lib/block_server/block_server.h"

namespace block_server {

/// Runs a BlockServer backed by a VMO.
class FakeServer {
 public:
  // Initialize a FakeServer. If `data` is set, it will be used as the initial contents of the
  // device, and the size of the VMO must agree with `info`.
  explicit FakeServer(const PartitionInfo& info, zx::vmo data = {});
  ~FakeServer();
  FakeServer(FakeServer&&);
  FakeServer& operator=(FakeServer&&);
  FakeServer(const FakeServer&) = delete;
  FakeServer& operator=(const FakeServer&) = delete;

  // Serves a new connection.  The FIDL handling is multiplexed onto a single per-server thread.
  void Serve(fidl::ServerEnd<fuchsia_storage_block::Block> request) {
    server_->Serve(std::move(request));
  }

 private:
  class FakeInterface;

  std::unique_ptr<FakeInterface> interface_;
  std::unique_ptr<BlockServer> server_;
};

}  // namespace block_server

#endif  // SRC_STORAGE_LIB_BLOCK_SERVER_FAKE_SERVER_H_
