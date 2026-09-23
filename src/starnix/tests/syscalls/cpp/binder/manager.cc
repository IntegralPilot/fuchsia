// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/binder/manager.h"

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

auto ManagerBehavior(std::string_view binder_dir, test_helper::Poker ready) {
  auto fd_and_mapping = OpenBinderAndMap(binder_dir);

  // Register as the context manager
  ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_SET_CONTEXT_MGR, 0), SyscallSucceeds());

  EnterLooper(fd_and_mapping.fd_);

  ready.poke();

  // This Binder context manager does only two things for the rest of its runtime:
  //   * accepts registration of the service
  //     * (failing if the service has ever been registered before)
  //   * hands out the handle to the service
  //     * (failing if the service has not before been registered)
  std::optional<uint32_t> service_provider_handle = std::nullopt;
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
        case BR_TRANSACTION: {
          struct binder_transaction_data& transaction_data =
              *(struct binder_transaction_data*)cursor;

          switch (transaction_data.code) {
            case kAddService: {
              ASSERT_EQ(transaction_data.offsets_size, sizeof(binder_size_t));
              ASSERT_EQ(transaction_data.data_size, sizeof(struct flat_binder_object));

              binder_size_t offset = *(binder_size_t*)(transaction_data.data.ptr.offsets);
              const struct flat_binder_object* obj =
                  (const struct flat_binder_object*)(transaction_data.data.ptr.buffer + offset);

              ASSERT_EQ(obj->hdr.type, BINDER_TYPE_HANDLE);
              ASSERT_EQ(service_provider_handle, std::nullopt);

              service_provider_handle = obj->handle;

              AcquireWriteBuffer acquire_write_buffer = {
                  .command = BC_ACQUIRE,
                  .handle = obj->handle,
              };
              struct binder_write_read acquire_write_read = {
                  .write_size = sizeof(acquire_write_buffer),
                  .write_buffer = (binder_uintptr_t)&acquire_write_buffer,
              };
              int acquire_result =
                  ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &acquire_write_read);
              ASSERT_THAT(acquire_result, SyscallSucceeds());

              ReplyWriteBuffer reply_write_buffer = {
                  .command = BC_REPLY,
                  .data =
                      {
                          .target = {.ptr = 0},
                          .cookie = transaction_data.cookie,
                          .code = transaction_data.code,
                          .flags = TF_STATUS_CODE,
                          .data_size = sizeof(int),
                          .offsets_size = 0,
                          .data =
                              {
                                  .ptr =
                                      {
                                          .buffer = (binder_uintptr_t)&acquire_result,
                                          .offsets = (binder_uintptr_t) nullptr,
                                      },
                              },
                      },
              };
              struct binder_write_read reply_write_read = {
                  .write_size = sizeof(reply_write_buffer),
                  .write_consumed = 0,
                  .write_buffer = (binder_uintptr_t)&reply_write_buffer,
                  .read_size = 0,
                  .read_consumed = 0,
                  .read_buffer = 0,
              };
              ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &reply_write_read),
                          SyscallSucceeds());
              break;
            }
            case kGetService: {
              ASSERT_TRUE(service_provider_handle.has_value());

              struct flat_binder_object send_handle_obj = {
                  .hdr =
                      {
                          .type = BINDER_TYPE_HANDLE,
                      },
                  .flags = 0x7f | FLAT_BINDER_FLAG_ACCEPTS_FDS,
                  .handle = *service_provider_handle,
                  .cookie = 0,
              };
              binder_size_t offset = 0;
              ReplyWriteBuffer reply_write_buffer = {
                  .command = BC_REPLY,
                  .data =
                      {
                          .target =
                              {
                                  .handle = transaction_data.target.handle,
                              },
                          .cookie = transaction_data.cookie,
                          .code = transaction_data.code,
                          .flags = TF_ACCEPT_FDS,
                          .data_size = sizeof(send_handle_obj),
                          .offsets_size = sizeof(offset),
                          .data =
                              {
                                  .ptr =
                                      {
                                          .buffer = (binder_uintptr_t)&send_handle_obj,
                                          .offsets = (binder_uintptr_t)&offset,
                                      },
                              },
                      },
              };
              struct binder_write_read reply_write_read = {
                  .write_size = sizeof(reply_write_buffer),
                  .write_buffer = (binder_uintptr_t)&reply_write_buffer,
              };
              ASSERT_THAT(ioctl(fd_and_mapping.fd_.get(), BINDER_WRITE_READ, &reply_write_read),
                          SyscallSucceeds());
              ASSERT_EQ(reply_write_read.read_consumed, 0ull);

              break;
            }
            case kServiceSendFd:
              // NOTE(https://fxbug.dev/441447806): control flow passes through here when the
              // client has permission to impersonate the provider and the invocation of the
              // impersonation transaction succeeds.
              break;
            default:
              FAIL() << "Unexpected transaction_data.code " << transaction_data.code << "!";
              break;
          }

          cursor += sizeof(transaction_data);
          break;
        }
        case BR_REPLY:
          FAIL() << "Unimplemented at this time; perhaps will be used in a future test.";
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

fit::deferred_action<fit::closure> ManagerProcess(
    std::string_view binder_dir,
    fit::function<pid_t(test_helper::ForkHelper&, fit::closure)> spawn_manager,
    test_helper::Poker ready) {
  std::unique_ptr<test_helper::ForkHelper> fork_helper =
      std::make_unique<test_helper::ForkHelper>();
  fork_helper->OnlyWaitForForkedChildren();

  pid_t pid = spawn_manager(*fork_helper, [binder_dir, ready = std::move(ready)]() mutable {
    ManagerBehavior(binder_dir, std::move(ready));
  });

  return fit::defer(fit::closure([pid, fork_helper = std::move(fork_helper)] {
    ASSERT_THAT(kill(pid, SIGKILL), SyscallSucceeds());
    fork_helper->ExpectSignal(SIGKILL);
    ASSERT_TRUE(fork_helper->WaitForChildren());
  }));
}

}  // namespace starnix_binder
