// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpp_connection.h"

#include <lib/magma/util/macros.h>

#include "cpp_buffer.h"
#include "cpp_context.h"

namespace msd {

static void cpp_connection_killed(void* context) {
  reinterpret_cast<CppConnection*>(context)->ContextKilled();
}

std::unique_ptr<CppConnection> CppConnection::Create(struct MsdDevice* device, uint64_t client_id) {
  auto connection = std::make_unique<CppConnection>();

  struct MsdConnection* msd_connection =
      msd_device_create_connection(device, client_id,
                                   MsdConnectionCallbacks{
                                       .token = connection.get(),
                                       .context_killed = cpp_connection_killed,
                                   });
  if (!msd_connection) {
    return MAGMA_DRETP(nullptr, "msd_device_create_connection failed");
  }
  connection->connection_ = msd_connection;

  return connection;
}

void CppConnection::ContextKilled() {
  if (notification_handler_) {
    notification_handler_->ContextKilled();
  } else {
    MAGMA_LOG(WARNING, "CppConnection Context Killed but no notification handler");
  }
}

CppConnection::~CppConnection() { msd_connection_release(connection_); }

magma_status_t CppConnection::MsdMapBuffer(msd::Buffer& buffer, uint64_t gpu_va, uint64_t offset,
                                           uint64_t length, uint64_t flags) {
  auto& msd_buffer = static_cast<CppBuffer&>(buffer);

  return msd_connection_map_buffer(connection_, msd_buffer.buffer(), gpu_va, offset, length, flags);
}

void CppConnection::MsdReleaseBuffer(msd::Buffer& buffer, bool shutting_down) {
  auto& msd_buffer = static_cast<CppBuffer&>(buffer);

  msd_connection_release_buffer2(connection_, msd_buffer.buffer(), shutting_down);
}

std::unique_ptr<msd::Context> CppConnection::MsdCreateContext() {
  struct MsdContext* msd_context = msd_connection_create_context(connection_);
  if (!msd_context)
    return MAGMA_DRETP(nullptr, "msd_connection_create_context failed");

  return std::make_unique<CppContext>(msd_context);
}

std::unique_ptr<msd::Context> CppConnection::MsdCreateContext2(uint64_t priority) {
  struct MsdContext* msd_context = msd_connection_create_context2(connection_, priority);
  if (!msd_context)
    return MAGMA_DRETP(nullptr, "msd_connection_create_context2 failed");

  return std::make_unique<CppContext>(msd_context);
}

}  // namespace msd
