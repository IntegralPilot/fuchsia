// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_HELPER_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_HELPER_H_

#include <fcntl.h>
#include <sys/mount.h>
#include <sys/types.h>

#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/android/binder.h>

namespace starnix_binder {

constexpr int kServiceManagerHandle = 0;
const size_t kBinderMMapSize = sysconf(_SC_PAGESIZE);

// Opens the binder
fbl::unique_fd OpenBinder(std::string_view dir);

struct ParsedMessage {
  std::vector<binder_driver_return_protocol> returns_;
};

ParsedMessage ParseMessage(binder_uintptr_t start, binder_size_t length);

struct __attribute__((packed)) TransactionWriteBuffer {
  const binder_driver_command_protocol command = BC_TRANSACTION;
  const struct binder_transaction_data data;
};

struct __attribute__((packed)) EnterLooperWriteBuffer {
  const binder_driver_command_protocol command = BC_ENTER_LOOPER;
};

struct __attribute((packed)) AcquireWriteBuffer {
  const binder_driver_command_protocol command = BC_ACQUIRE;
  const uint32_t handle;
};

struct __attribute__((packed)) ReplyWriteBuffer {
  const binder_driver_command_protocol command = BC_REPLY;
  const struct binder_transaction_data data;
};

void EnterLooper(const fbl::unique_fd& binder_fd);

// A helper helper to prepare a transaction that sends a single file descriptor.
struct FdTransaction {
  struct binder_fd_object fd_object;
  binder_size_t offset;
  TransactionWriteBuffer write_buffer;

  FdTransaction(uint32_t target_handle, uint32_t code, int fd);
  FdTransaction(const FdTransaction&) = delete;
  FdTransaction(FdTransaction&&) = delete;
};

}  // namespace starnix_binder

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_HELPER_H_
