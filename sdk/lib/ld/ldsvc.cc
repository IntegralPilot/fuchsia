// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/zircon.h>

#include <ldmsg/ldmsg.h>

#include "startup-diagnostics.h"
#include "zircon.h"

namespace ld {

namespace {

struct LdsvcOp {
  uint64_t ordinal;
  std::string_view call;
  bool return_vmo = false;
};

constexpr LdsvcOp kLoadObject = {
    .ordinal = LDMSG_OP_LOAD_OBJECT, .call = "LoadObject", .return_vmo = true};
constexpr LdsvcOp kConfig = {.ordinal = LDMSG_OP_CONFIG, .call = "Config"};

zx::vmo LoaderServiceRpc(Diagnostics& diag, const zx::channel& ldsvc, const LdsvcOp& type,
                         std::string_view name) {
  union {
    ldmsg_req_t req;
    ldmsg_rsp_t rsp;
  } ldmsg_req_rsp;
  ldmsg_req_t* req = &ldmsg_req_rsp.req;
  size_t req_len;
  zx_status_t status = ldmsg_req_encode(type.ordinal, req, &req_len,
                                        static_cast<const char*>(name.data()), name.size());
  if (status != ZX_OK) {
    diag.SystemError("message of ", name.size(), "bytes too large for loader service protocol");
    return {};
  }

  ldmsg_rsp_t* rsp = &ldmsg_req_rsp.rsp;
  zx::vmo vmo;
  const zx_channel_call_args_t call = {
      .wr_bytes = req,
      .rd_bytes = rsp,
      .rd_handles = vmo.reset_and_get_address(),
      .wr_num_bytes = static_cast<uint32_t>(req_len),
      .rd_num_bytes = sizeof(*rsp),
      .rd_num_handles = type.return_vmo ? 1u : 0u,
  };

  uint32_t reply_size;
  uint32_t handle_count;
  status = ldsvc.call(0, zx::time::infinite(), &call, &reply_size, &handle_count);
  if (status != ZX_OK) {
    diag.SystemError("zx_channel_call of ", call.wr_num_bytes,
                     "bytes to loader service: ", elfldltl::ZirconError{status});
    return {};
  }

  size_t expected_reply_size = ldmsg_rsp_get_size(rsp);
  if (reply_size != expected_reply_size) [[unlikely]] {
    diag.SystemError("loader service reply ", reply_size, " bytes != ", expected_reply_size);
    return {};
  }
  if (rsp->header.ordinal != type.ordinal) [[unlikely]] {
    diag.SystemError("loader service reply opcode ", rsp->header.ordinal, "!= ", type.ordinal);
    return {};
  }
  if (rsp->rv != ZX_OK) [[unlikely]] {
    if (rsp->rv != ZX_ERR_NOT_FOUND) {
      diag.SystemError("fuchsia.ldsvc/", type.call, "(\"", name, "\") ",
                       elfldltl::ZirconError{rsp->rv});
    }
    return {};
  }
  if (!type.return_vmo) {
    return {};
  }
  if (!vmo) [[unlikely]] {
    diag.SystemError("fuchsia.ldsvc/", type.call, "(\"", name, "\") returned invalid VMO handle.");
    return {};
  }
  return vmo;
}

}  // namespace

zx::vmo StartupData::GetLibraryVmo(Diagnostics& diag, std::string_view name) const {
  return LoaderServiceRpc(diag, ldsvc, kLoadObject, name);
}

void StartupData::ConfigLdsvc(Diagnostics& diag, std::string_view name) const {
  LoaderServiceRpc(diag, ldsvc, kConfig, name);
}

}  // namespace ld
