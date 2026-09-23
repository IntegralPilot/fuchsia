// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/binder/provider.h"

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/android/binder.h>

#include "src/starnix/tests/syscalls/cpp/binder/common.h"
#include "src/starnix/tests/syscalls/cpp/binder_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace starnix_binder {

namespace {

void ProviderBehavior(
    std::string_view binder_dir, test_helper::Poker ready,
    fit::function<void(std::string_view)> validate_client_secctx_seen_by_provider) {
  auto fd_and_mapping = OpenBinderAndMap(binder_dir);

  struct flat_binder_object add_service_object = {
      .hdr =
          {
              .type = BINDER_TYPE_BINDER,
          },
      .flags = 0x7f | FLAT_BINDER_FLAG_ACCEPTS_FDS | FLAT_BINDER_FLAG_TXN_SECURITY_CTX,
      .binder = (binder_uintptr_t) nullptr,
      .cookie = 0,
  };
  binder_size_t add_service_offset = 0;
  TransactionWriteBuffer add_service_write_buffer = {
      .command = BC_TRANSACTION,
      .data =
          {
              .target =
                  {
                      .handle = kServiceManagerHandle,
                  },
              .cookie = 0,
              .code = kAddService,
              .flags = TF_ROOT_OBJECT,
              .data_size = sizeof(add_service_object),
              .offsets_size = sizeof(add_service_offset),
              .data =
                  {
                      .ptr =
                          {
                              .buffer = (binder_uintptr_t)&add_service_object,
                              .offsets = (binder_uintptr_t)&add_service_offset,
                          },
                  },
          },
  };
  std::array<int32_t, 32> add_service_read_buffer = {};

  struct binder_write_read add_service_write_read = {
      .write_size = sizeof(add_service_write_buffer),
      .write_consumed = 0,
      .write_buffer = (binder_uintptr_t)&add_service_write_buffer,
      .read_size = sizeof(add_service_read_buffer),
      .read_consumed = 0,
      .read_buffer = (binder_uintptr_t)add_service_read_buffer.data(),
  };

  ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &add_service_write_read),
              SyscallSucceeds());

  // TODO: https://fxbug.dev/441451502 - it looks like Linux and Starnix may differ in what
  // they populate into add_service_read_buffer at this point; Linux looks like it puts in
  // BR_NOOP, BR_INCREFS, BR_ACQUIRE, and BR_TRANSACTION_COMPLETE, but Starnix looks like it
  // puts in just BR_ACQUIRE. This is worth a deeper look.

  EnterLooper(fd_and_mapping.fd_);

  ready.poke();

  while (true) {
    std::array<uint32_t, 32> read_buffer = {};
    struct binder_write_read write_read = {
        .read_size = sizeof(read_buffer),
        .read_consumed = 0,
        .read_buffer = (binder_uintptr_t)read_buffer.data(),
    };
    ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &write_read), SyscallSucceeds());

    binder_uintptr_t cursor = (binder_uintptr_t)read_buffer.data();
    binder_uintptr_t limit = cursor + write_read.read_consumed;
    while (cursor < limit) {
      binder_driver_return_protocol returned = *(binder_driver_return_protocol*)(cursor);
      cursor += sizeof(binder_driver_return_protocol);

      switch (returned) {
        case BR_NOOP:
        case BR_TRANSACTION_COMPLETE:
          break;
        case BR_INCREFS:
        case BR_ACQUIRE:
        case BR_RELEASE:
        case BR_DECREFS:
          cursor += sizeof(struct binder_ptr_cookie);
          break;
        case BR_TRANSACTION:
          FAIL() << "Unimplemented - maybe we will need this in a future test?";
          break;
        case BR_TRANSACTION_SEC_CTX: {
          const struct binder_transaction_data_secctx& transaction_data_secctx =
              *(struct binder_transaction_data_secctx*)cursor;
          validate_client_secctx_seen_by_provider((const char*)transaction_data_secctx.secctx);

          if (transaction_data_secctx.transaction_data.code == kServiceSendFd &&
              transaction_data_secctx.transaction_data.flags != TF_ONE_WAY) {
            struct binder_fd_object send_fd_obj = {
                .hdr =
                    {
                        .type = BINDER_TYPE_FD,
                    },
                .pad_flags =
                    0x7f | FLAT_BINDER_FLAG_ACCEPTS_FDS | FLAT_BINDER_FLAG_TXN_SECURITY_CTX,
                .fd = static_cast<uint32_t>(fd_and_mapping.fd_.get()),
                .cookie = transaction_data_secctx.transaction_data.cookie,
            };
            binder_size_t send_fd_offset = 0;
            ReplyWriteBuffer reply_write_buffer = {
                .command = BC_REPLY,
                .data =
                    {
                        .target =
                            {
                                .handle = transaction_data_secctx.transaction_data.target.handle,
                            },
                        .cookie = transaction_data_secctx.transaction_data.cookie,
                        .code = transaction_data_secctx.transaction_data.code,
                        .flags = TF_ACCEPT_FDS,
                        .data_size = sizeof(send_fd_obj),
                        .offsets_size = sizeof(send_fd_offset),
                        .data =
                            {
                                .ptr =
                                    {
                                        .buffer = (binder_uintptr_t)&send_fd_obj,
                                        .offsets = (binder_uintptr_t)&send_fd_offset,
                                    },
                            },
                    },
            };
            struct binder_write_read send_fd_write_read = {
                .write_size = sizeof(reply_write_buffer),
                .write_buffer = (binder_uintptr_t)&reply_write_buffer,
            };

            ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &send_fd_write_read),
                        SyscallSucceeds());
          }

          cursor += sizeof(transaction_data_secctx);
          break;
        }
        case BR_REPLY:
          cursor += sizeof(struct binder_transaction_data);
          break;
        case BR_DEAD_BINDER:
        case BR_FAILED_REPLY:
        case BR_DEAD_REPLY:
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
}

}  // namespace

fit::deferred_action<fit::closure> ProviderProcess(
    std::string_view binder_dir,
    fit::function<pid_t(test_helper::ForkHelper&, fit::closure)> spawn_provider,
    test_helper::Poker ready,
    fit::function<void(std::string_view)> validate_client_secctx_seen_by_provider) {
  std::unique_ptr<test_helper::ForkHelper> fork_helper =
      std::make_unique<test_helper::ForkHelper>();
  fork_helper->OnlyWaitForForkedChildren();

  pid_t pid = spawn_provider(*fork_helper,
                             [binder_dir, ready = std::move(ready),
                              validate_client_secctx_seen_by_provider =
                                  std::move(validate_client_secctx_seen_by_provider)]() mutable {
                               ProviderBehavior(binder_dir, std::move(ready),
                                                std::move(validate_client_secctx_seen_by_provider));
                             });

  return fit::defer(fit::closure([pid, fork_helper = std::move(fork_helper)] {
    ASSERT_THAT(kill(pid, SIGKILL), SyscallSucceeds());
    fork_helper->ExpectSignal(SIGKILL);
    ASSERT_TRUE(fork_helper->WaitForChildren());
  }));
}

}  // namespace starnix_binder
