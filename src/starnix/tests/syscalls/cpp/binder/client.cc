// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/binder/client.h"

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>

#include <queue>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/android/binder.h>

#include "src/starnix/tests/syscalls/cpp/binder/common.h"
#include "src/starnix/tests/syscalls/cpp/binder_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace starnix_binder {

namespace {

struct WriteReadIoctl {
  const binder_size_t size;
  const union {
    const AcquireWriteBuffer acquire;
    const TransactionWriteBuffer transaction;
  } write_buffer;
  const bool read_buffer;
  const int fd;
};

void ClientBehavior(std::string_view binder_dir, test_helper::Poker completed) {
  auto fd_and_mapping = OpenBinderAndMap(binder_dir);

  const TransactionWriteBuffer get_handle_write_buffer = {
      .command = BC_TRANSACTION,
      .data =
          {
              .target =
                  {
                      .handle = kServiceManagerHandle,
                  },
              .cookie = 0,
              .code = kGetService,
              .flags = TF_ACCEPT_FDS,
              .data_size = 0,
              .offsets_size = 0,
              .data =
                  {
                      .ptr =
                          {
                              .buffer = (binder_uintptr_t) nullptr,
                              .offsets = (binder_uintptr_t) nullptr,
                          },
                  },
          },
  };

  std::optional<uint32_t> service_provider_handle = std::nullopt;
  std::optional<uint32_t> service_provider_binder_fd = std::nullopt;
  int8_t transactions_remaining = 3;
  std::queue<WriteReadIoctl> queue = {};

  queue.push({
      .size = sizeof(get_handle_write_buffer),
      .write_buffer = {.transaction = get_handle_write_buffer},
      .read_buffer = true,
      .fd = fd_and_mapping.fd_.get(),
  });

  while (0 < transactions_remaining) {
    std::array<uint32_t, 32> read_buffer = {};
    struct binder_write_read write_read;
    int fd_for_ioctl;
    if (queue.empty()) {
      write_read = {
          .write_size = 0,
          .write_consumed = 0,
          .write_buffer = (binder_uintptr_t) nullptr,
          .read_size = sizeof(read_buffer),
          .read_consumed = 0,
          .read_buffer = (binder_uintptr_t)read_buffer.data(),
      };
      fd_for_ioctl = fd_and_mapping.fd_.get();
    } else {
      WriteReadIoctl& client_ioctl = queue.front();
      write_read = {
          .write_size = client_ioctl.size,
          .write_consumed = 0,
          .write_buffer = (binder_uintptr_t)(&(client_ioctl.write_buffer)),
          .read_size = client_ioctl.read_buffer ? sizeof(read_buffer) : 0,
          .read_consumed = 0,
          .read_buffer = (binder_uintptr_t)read_buffer.data(),
      };
      fd_for_ioctl = client_ioctl.fd;
    }

    ASSERT_THAT(ioctl(fd_for_ioctl, BINDER_WRITE_READ, &write_read), SyscallSucceeds());

    queue.pop();

    binder_uintptr_t cursor = (binder_uintptr_t)read_buffer.data();
    binder_uintptr_t limit = cursor + write_read.read_consumed;
    while (cursor < limit) {
      binder_driver_return_protocol returned = *(binder_driver_return_protocol*)(cursor);
      cursor += sizeof(binder_driver_return_protocol);

      switch (returned) {
        case BR_NOOP:
          break;
        case BR_TRANSACTION_COMPLETE:
          transactions_remaining--;
          break;
        case BR_INCREFS:
        case BR_ACQUIRE:
        case BR_RELEASE:
        case BR_DECREFS:
          cursor += sizeof(struct binder_ptr_cookie);
          break;
        case BR_TRANSACTION:
          cursor += sizeof(struct binder_transaction_data);
          break;
        case BR_REPLY: {
          struct binder_transaction_data& received_transaction_data =
              *(struct binder_transaction_data*)cursor;

          if (received_transaction_data.code == kGetService) {
            ASSERT_EQ(received_transaction_data.offsets_size, sizeof(binder_size_t));
            ASSERT_EQ(received_transaction_data.data_size, sizeof(struct flat_binder_object));

            binder_size_t offset = *(binder_size_t*)(received_transaction_data.data.ptr.offsets);
            const struct flat_binder_object& obj =
                *(const struct flat_binder_object*)(received_transaction_data.data.ptr.buffer +
                                                    offset);

            ASSERT_EQ(obj.hdr.type, BINDER_TYPE_HANDLE);
            ASSERT_EQ(service_provider_handle, std::nullopt);

            AcquireWriteBuffer acquire_write_buffer = {
                .command = BC_ACQUIRE,
                .handle = obj.handle,
            };
            queue.push({
                .size = sizeof(acquire_write_buffer),
                .write_buffer =
                    {
                        .acquire = acquire_write_buffer,
                    },
                .read_buffer = false,
                .fd = fd_and_mapping.fd_.get(),
            });

            service_provider_handle = obj.handle;

            TransactionWriteBuffer send_fd_write_buffer = {
                .command = BC_TRANSACTION,
                .data =
                    {
                        .target =
                            {
                                .handle = obj.handle,
                            },
                        .cookie = 0,
                        .code = kServiceSendFd,
                        .flags = TF_ACCEPT_FDS,
                        .data_size = 0,
                        .offsets_size = 0,
                        .data =
                            {
                                .ptr =
                                    {
                                        .buffer = (binder_uintptr_t) nullptr,
                                        .offsets = (binder_uintptr_t) nullptr,
                                    },
                            },
                    },
            };

            queue.push({
                .size = sizeof(send_fd_write_buffer),
                .write_buffer =
                    {
                        .transaction = send_fd_write_buffer,
                    },
                .read_buffer = true,
                .fd = fd_and_mapping.fd_.get(),
            });
          } else if (received_transaction_data.code == kServiceSendFd) {
            ASSERT_EQ(received_transaction_data.offsets_size, sizeof(binder_size_t));
            ASSERT_EQ(received_transaction_data.data_size, sizeof(struct binder_fd_object));

            binder_size_t received_offset =
                *(binder_size_t*)(binder_uintptr_t)received_transaction_data.data.ptr.offsets;

            const struct binder_fd_object& binder_fd_obj =
                *(struct binder_fd_object*)(((binder_uintptr_t)
                                                 received_transaction_data.data.ptr.buffer) +
                                            received_offset);

            ASSERT_EQ(binder_fd_obj.hdr.type, BINDER_TYPE_FD);
            struct stat stat_buffer;
            ASSERT_THAT(fstat(binder_fd_obj.fd, &stat_buffer), SyscallSucceeds());

            const TransactionWriteBuffer impersonate = {
                .command = BC_TRANSACTION,
                .data =
                    {
                        .target =
                            {
                                .handle = received_transaction_data.target.handle,
                            },
                        .cookie = received_transaction_data.cookie,
                        .code = received_transaction_data.code,
                        .flags = TF_ONE_WAY,
                        .data_size = 0,
                        .offsets_size = 0,
                        .data =
                            {
                                .ptr =
                                    {
                                        .buffer = (binder_uintptr_t) nullptr,
                                        .offsets = (binder_uintptr_t) nullptr,
                                    },
                            },
                    },
            };

            queue.push({
                .size = sizeof(impersonate),
                .write_buffer =
                    {
                        .transaction = impersonate,
                    },
                .read_buffer = true,
                .fd = static_cast<int>(binder_fd_obj.fd),
            });

            service_provider_binder_fd = binder_fd_obj.fd;
          }

          cursor += sizeof(received_transaction_data);
          break;
        }
        case BR_DEAD_BINDER:
          FAIL() << "Unimplemented BR_DEAD_BINDER case - maybe we will need this in a future test?";
          break;
        case BR_FAILED_REPLY:
          FAIL()
              << "Unimplemented BR_FAILED_REPLY case - maybe we will need this in a future test?";
          break;
        case BR_DEAD_REPLY:
          FAIL() << "Unimplemented BR_DEAD_REPLY case - maybe we will need this in a future test?";
          break;
        case BR_ERROR:
          cursor += sizeof(uint32_t);
          break;
        default:
          FAIL() << "Unexpected binder_driver_return_protocol value " << returned << "!";
          break;
      }
    }
  }

  completed.poke();
}

}  // namespace

fit::deferred_action<fit::closure> ClientProcess(
    std::string_view binder_dir,
    fit::function<void(test_helper::ForkHelper&, fit::closure)> spawn_client,
    test_helper::Poker completed) {
  std::unique_ptr<test_helper::ForkHelper> fork_helper =
      std::make_unique<test_helper::ForkHelper>();
  fork_helper->OnlyWaitForForkedChildren();

  spawn_client(*fork_helper, [binder_dir, completed = std::move(completed)]() mutable {
    ClientBehavior(binder_dir, std::move(completed));
  });

  return fit::defer(fit::closure(
      [fork_helper = std::move(fork_helper)] { ASSERT_TRUE(fork_helper->WaitForChildren()); }));
}

}  // namespace starnix_binder
