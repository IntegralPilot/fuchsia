// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/binder_helper.h"

#include <fcntl.h>
#include <sys/ioctl.h>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace starnix_binder {

fbl::unique_fd OpenBinder(std::string_view dir) {
  return fbl::unique_fd(open((std::string(dir) + "/binder").c_str(), O_RDWR | O_CLOEXEC));
}

ParsedMessage ParseMessage(const binder_uintptr_t start, const binder_size_t length) {
  // This function is based on the code of `printReturnCommand`:
  // https://cs.android.com/android/platform/superproject/+/master:frameworks/native/libs/binder/IPCThreadState.cpp;drc=bf14463e0c2309f04d0ba25cf951dcea3c47858e;l=153

  ParsedMessage m;

  const binder_uintptr_t end = start + length;

  binder_uintptr_t ptr = start;
  while (ptr < end) {
    binder_driver_return_protocol returned = *(binder_driver_return_protocol*)ptr;
    m.returns_.push_back(returned);
    ptr += sizeof(binder_driver_return_protocol);
    switch (returned) {
      case BR_TRANSACTION_SEC_CTX:
        ptr += sizeof(binder_transaction_data_secctx);
        break;
      case BR_TRANSACTION:
      case BR_REPLY:
        ptr += sizeof(binder_transaction_data);
        break;
      case BR_ACQUIRE_RESULT:
        ptr += sizeof(uint32_t);
        break;
      case BR_INCREFS:
      case BR_ACQUIRE:
      case BR_RELEASE:
      case BR_DECREFS:
        ptr += sizeof(uint32_t) * 2;
        break;
      case BR_ATTEMPT_ACQUIRE:
        ptr += sizeof(uint32_t) * 3;
        break;
      case BR_DEAD_BINDER:
      case BR_CLEAR_DEATH_NOTIFICATION_DONE:
        ptr += sizeof(uint32_t);
        break;
      case BR_OK:
      case BR_DEAD_REPLY:
      case BR_TRANSACTION_COMPLETE:
      case BR_FINISHED:
      case BR_NOOP:
      case BR_FAILED_REPLY:
        break;
      case BR_ERROR:
        ptr += sizeof(int32_t);
        break;
      default:
        break;
    }
  }
  return m;
}

void EnterLooper(const fbl::unique_fd& binder_fd) {
  EnterLooperWriteBuffer write_buffer;
  struct binder_write_read write_read = {
      .write_size = sizeof(write_buffer),
      .write_consumed = 0,
      .write_buffer = (binder_uintptr_t)&write_buffer,
  };

  ASSERT_THAT(ioctl(binder_fd.get(), BINDER_WRITE_READ, &write_read), SyscallSucceeds());
}

FdTransaction::FdTransaction(uint32_t target_handle, uint32_t code, int fd)
    : fd_object{
          .hdr = {.type = BINDER_TYPE_FD},
          .pad_flags = 0x7f | FLAT_BINDER_FLAG_ACCEPTS_FDS,
          .fd = static_cast<uint32_t>(fd),
      },
      offset(0),
      write_buffer{.command = BC_TRANSACTION,
                   .data = {.target = {.handle = target_handle},
                            .code = code,
                            .flags = TF_ACCEPT_FDS,
                            .data_size = sizeof(struct binder_fd_object),
                            .offsets_size = sizeof(binder_size_t),
                            .data = {.ptr = {
                                         .buffer = (binder_uintptr_t)&fd_object,
                                         .offsets = (binder_uintptr_t)&offset,
                                     }}}} {}

}  // namespace starnix_binder
