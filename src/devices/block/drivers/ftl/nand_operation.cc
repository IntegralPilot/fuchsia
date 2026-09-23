// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "nand_operation.h"

#include <lib/ddk/driver.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/status.h>

#include "lib/zx/result.h"

namespace ftl {

zx_status_t NandOperation::SetDataVmo(size_t num_bytes) {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return ZX_ERR_NO_MEMORY;
  }

  if (zx_status_t status = CreateVmoForCommand(num_bytes, operation->rw.command); status != ZX_OK) {
    return status;
  }

  operation->rw.data_vmo = vmo_.get();
  return ZX_OK;
}

zx_status_t NandOperation::SetOobVmo(size_t num_bytes) {
  nand_operation_t* operation = GetOperation();
  if (!operation) {
    return ZX_ERR_NO_MEMORY;
  }
  if (zx_status_t status = CreateVmoForCommand(num_bytes, operation->rw.command); status != ZX_OK) {
    return status;
  }

  operation->rw.oob_vmo = vmo_.get();
  return ZX_OK;
}

nand_operation_t* NandOperation::GetOperation() {
  if (!raw_buffer_) {
    CreateOperation();
  }
  return reinterpret_cast<nand_operation_t*>(raw_buffer_.get());
}

zx_status_t NandOperation::WaitForCompletion() {
  for (;;) {
    zx_status_t status = sync_completion_wait(&event_, ZX_SEC(60));
    switch (status) {
      case ZX_OK:
        sync_completion_reset(&event_);
        return status_;
      case ZX_ERR_TIMED_OUT:
        FDF_LOG(ERROR, "FTL: slow operation (%p), still waiting...", this);
        break;
      default:
        return status;
    }
  }
}

zx_status_t NandOperation::Execute(OobDoubler* parent) {
  parent->Queue(GetOperation(), OnCompletion, this);
  return WaitForCompletion();
}

// Static.
void NandOperation::OnCompletion(void* cookie, zx_status_t status, nand_operation_t* op) {
  NandOperation* operation = reinterpret_cast<NandOperation*>(cookie);
  operation->status_ = status;
  sync_completion_signal(&operation->event_);
}

zx_status_t NandOperation::CreateVmoForCommand(size_t num_bytes, nand_op_t command) {
  if (vmo_.is_valid()) {
    return ZX_OK;
  }
  if (zx_status_t status = zx::vmo::create(num_bytes, 0, &vmo_); status != ZX_OK) {
    return status;
  }
  if (command == NAND_OP_READ) {
    // Pre-commit pages only for read operations. The rawnand driver maps this VMO and memcpy's data
    // into it a page at a time. Pre-committing the pages allows the rawnand driver to map with
    // VM_MAP_RANGE which avoids page faulting on every page during the memcpy.
    if (zx_status_t status = vmo_.op_range(ZX_VMO_OP_COMMIT, 0, num_bytes, nullptr, 0);
        status != ZX_OK) {
      vmo_.reset();
      return status;
    }
  }
  return ZX_OK;
}

std::vector<zx::result<>> NandOperation::ExecuteBatch(
    OobDoubler* parent, cpp20::span<std::unique_ptr<NandOperation>> operations) {
  std::vector<zx::result<>> results(operations.size(), zx::ok());
  for (auto& operation : operations) {
    parent->Queue(operation->GetOperation(), &OnCompletion, static_cast<void*>(operation.get()));
  }

  for (size_t i = 0; i < operations.size(); ++i) {
    zx_status_t status = operations[i]->WaitForCompletion();
    results[i] = status == ZX_OK ? zx::result<>(zx::ok()) : zx::result<>(zx::error(status));
    if (results[i].is_ok()) {
      results[i] = operations[i]->status_ == ZX_OK
                       ? zx::result<>(zx::ok())
                       : zx::result<>(zx::error(operations[i]->status_));
    }
  }

  return results;
}

void NandOperation::CreateOperation() {
  ZX_DEBUG_ASSERT_MSG(op_size_ >= sizeof(nand_operation_t),
                      "op-size=%zu, sizeof(nand_operation_t)=%zu", op_size_,
                      sizeof(nand_operation_t));
  raw_buffer_.reset(new char[op_size_]);

  memset(raw_buffer_.get(), 0, op_size_);
}

}  // namespace ftl.
