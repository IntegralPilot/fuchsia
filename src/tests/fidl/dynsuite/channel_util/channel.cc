// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/tests/fidl/dynsuite/channel_util/channel.h"

#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <cstring>

namespace channel_util {

static bool compare_bytes(const char* log_prefix, const std::vector<uint8_t>& actual,
                          const Bytes& expected, bool ignore_txid_bytes) {
  bool ok = true;
  if (actual.size() != expected.size()) {
    ok = false;
    printf("%s: compare_bytes: actual size (%zu bytes) != expected size (%zu bytes)\n", log_prefix,
           expected.size(), actual.size());
  }
  for (size_t i = 0; i < std::min(expected.size(), actual.size()); i++) {
    auto txid_offset = offsetof(fidl_message_header_t, txid);
    if (ignore_txid_bytes && i >= txid_offset &&
        i < txid_offset + sizeof(fidl_message_header_t::txid)) {
      continue;
    }
    if (actual[i] != expected.data()[i]) {
      ok = false;
      printf("%s: compare_bytes: actual[%zu] != expected[%zu]: 0x%x != 0x%x\n", log_prefix, i, i,
             actual[i], expected.data()[i]);
    }
  }
  return ok;
}

static bool compare_handles(const char* log_prefix, const std::vector<zx_handle_info_t>& actual,
                            const std::vector<ExpectedHandle>& expected) {
  bool ok = true;
  if (actual.size() != expected.size()) {
    ok = false;
    printf("%s: compare_handles: actual size (%zu handles) != expected size (%zu handles)\n",
           log_prefix, expected.size(), actual.size());
  }
  for (size_t i = 0; i < std::min(expected.size(), actual.size()); i++) {
    // These should always be true for a handle received on a channel.
    ZX_ASSERT(actual[i].handle != ZX_HANDLE_INVALID);
    ZX_ASSERT(actual[i].unused == 0);
    if (expected[i].koid != ZX_KOID_INVALID) {
      zx_info_handle_basic_t info;
      zx_status_t status = zx_object_get_info(actual[i].handle, ZX_INFO_HANDLE_BASIC, &info,
                                              sizeof info, nullptr, nullptr);
      if (status != ZX_OK) {
        ok = false;
        printf("%s: compare_handles: zx_object_get_info() returned status: %s\n", log_prefix,
               zx_status_get_string(status));
      } else if (info.koid != expected[i].koid) {
        ok = false;
        printf("%s: compare_handles: actual[%zu].koid (%lx) != expected[%zu].koid (%lx)\n",
               log_prefix, i, i, info.koid, expected[i].koid);
      }
    }
    if (actual[i].type != expected[i].type) {
      ok = false;
      printf("%s: compare_handles: actual[%zu].type != expected[%zu].type: 0x%x != 0x%x\n",
             log_prefix, i, i, actual[i].type, expected[i].type);
    }
    if (actual[i].rights != expected[i].rights) {
      ok = false;
      printf("%s: compare_handles: actual[%zu].rights != expected[%zu].rights: 0x%x != 0x%x\n",
             log_prefix, i, i, actual[i].rights, expected[i].rights);
    }
  }
  return ok;
}

zx_status_t Channel::write(const Message& message) {
  ZX_ASSERT_MSG(message.bytes.size() % FIDL_ALIGNMENT == 0, "bytes must be 8-byte aligned");
  std::vector<zx_handle_disposition_t> handles;
  for (auto h : message.handles) {
    handles.push_back(zx_handle_disposition_t{
        .operation = ZX_HANDLE_OP_MOVE,
        .handle = h.handle,
        .type = h.type,
        .rights = h.rights,
        .result = ZX_OK,
    });
  }
  return channel_.write_etc(0, message.bytes.data(), static_cast<uint32_t>(message.bytes.size()),
                            handles.data(), static_cast<uint32_t>(handles.size()));
}

zx_status_t Channel::read_and_check_impl(const char* log_prefix, const ExpectedMessage& expected,
                                         zx_txid_t* out_unknown_txid) {
  ZX_ASSERT_MSG(expected.bytes.size() % 8 == 0, "bytes must be 8-byte aligned");
  std::vector<uint8_t> bytes(ZX_CHANNEL_MAX_MSG_BYTES);
  std::vector<zx_handle_info_t> handles(ZX_CHANNEL_MAX_MSG_HANDLES);
  uint32_t actual_bytes;
  uint32_t actual_handles;
  zx_status_t status;
  status = channel_.wait_one(ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED,
                             zx::deadline_after(kWaitTimeout), nullptr);
  if (status != ZX_OK) {
    printf("read_and_check: object_wait_one() returned status: %s\n", zx_status_get_string(status));
    return status;
  }
  status = channel_.read_etc(0, bytes.data(), handles.data(), static_cast<uint32_t>(bytes.size()),
                             static_cast<uint32_t>(handles.size()), &actual_bytes, &actual_handles);
  if (status != ZX_OK) {
    printf("%s: channel_read_etc() returned status: %s\n", log_prefix,
           zx_status_get_string(status));
    return status;
  }
  bytes.resize(actual_bytes);
  handles.resize(actual_handles);
  if (out_unknown_txid != nullptr) {
    // If out_unknown_txid is non-null, we need to retrieve the txid.
    if (bytes.size() < sizeof(fidl_message_header_t)) {
      printf("%s: message body is smaller than FIDL message header\n", log_prefix);
      return ZX_ERR_INVALID_ARGS;
    }
    fidl_message_header_t hdr;
    memcpy(&hdr, bytes.data(), sizeof hdr);
    *out_unknown_txid = hdr.txid;
  }
  if (!compare_bytes(log_prefix, bytes, expected.bytes, out_unknown_txid != nullptr)) {
    status = ZX_ERR_INVALID_ARGS;
  }
  if (!compare_handles(log_prefix, handles, expected.handles)) {
    status = ZX_ERR_INVALID_ARGS;
  }
  return status;
}

}  // namespace channel_util
