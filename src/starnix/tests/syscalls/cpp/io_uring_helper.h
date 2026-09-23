// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_IO_URING_HELPER_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_IO_URING_HELPER_H_

#include <lib/fit/function.h>
#include <lib/fit/result.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <atomic>
#include <memory>

#include <fbl/unique_fd.h>
#include <linux/io_uring.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

#ifndef IORING_SETUP_COOP_TASKRUN
#define IORING_SETUP_COOP_TASKRUN (1U << 8)
#endif

#ifndef IORING_SETUP_SINGLE_ISSUER
#define IORING_SETUP_SINGLE_ISSUER (1U << 12)
#endif

#ifndef IORING_SETUP_DEFER_TASKRUN
#define IORING_SETUP_DEFER_TASKRUN (1U << 13)
#endif

#ifndef IORING_OP_URING_CMD
#define IORING_OP_URING_CMD 46
#endif

#ifndef IORING_REGISTER_PERSONALITY
#define IORING_REGISTER_PERSONALITY 9
#endif

namespace io_uring_helper {

int io_uring_setup(uint32_t entries, io_uring_params* params);
int io_uring_enter(int fd, int to_submit, int min_complete, int flags, sigset_t* sigset);
int io_uring_register(int fd, unsigned int opcode, void* arg, unsigned int nr_args);

// The sysroot that is used to build tests does not have an up-to-date definition of the
// io_uring_sqe struct. Specifically, it is missing the personality field, so we use this local
// definition derived from the following man page:
// https://man7.org/linux/man-pages/man7/io_uring.7.html
struct Sqe {
  __u8 opcode;
  __u8 flags;
  __u16 ioprio;
  __s32 fd;
  union {
    __u64 off;
    __u64 addr2;
    struct {
      __u32 cmd_op;
      __u32 __pad1;
    };
  };
  union {
    __u64 addr;
    __u64 splice_off_in;
    struct {
      __u32 level;
      __u32 optname;
    };
  };
  __u32 len;
  union {
    __kernel_rwf_t rw_flags;
    __u32 fsync_flags;
    __u16 poll_events;
    __u32 poll32_events;
    __u32 sync_range_flags;
    __u32 msg_flags;
    __u32 timeout_flags;
    __u32 accept_flags;
    __u32 cancel_flags;
    __u32 open_flags;
    __u32 statx_flags;
    __u32 fadvise_advice;
    __u32 splice_flags;
    __u32 rename_flags;
    __u32 unlink_flags;
    __u32 hardlink_flags;
    __u32 xattr_flags;
    __u32 msg_ring_flags;
    __u32 uring_cmd_flags;
    __u32 waitid_flags;
    __u32 futex_flags;
    __u32 install_fd_flags;
    __u32 nop_flags;
  };
  __u64 user_data;
  union {
    __u16 buf_index;
    __u16 buf_group;
  } __attribute__((packed));
  __u16 personality;
  union {
    __s32 splice_fd_in;
    __u32 file_index;
    __u32 optlen;
    struct {
      __u16 addr_len;
      __u16 __pad3[1];
    };
  };
  union {
    struct {
      __u64 addr3;
      __u64 __pad2[1];
    };
    __u64 optval;
    __u8 cmd[0];
  };
};

// An RAII helper class that manages the lifetime and memory mappings of an io_uring instance.
class IoUring {
 public:
  // Returns the instance or the system errno on failure.
  static fit::result<int, std::unique_ptr<IoUring>> Create(uint32_t entries,
                                                           io_uring_params params = {});

  IoUring(const IoUring&) = delete;
  IoUring& operator=(const IoUring&) = delete;
  IoUring(IoUring&&) = delete;
  IoUring& operator=(IoUring&&) = delete;

  int fd() const { return ring_fd_.get(); }
  const io_uring_params& params() const { return params_; }

  io_uring_sqe* sqes() const { return sqes_; }
  io_uring_cqe* cqes() const { return cqes_; }
  uint32_t* sq_array() const { return sq_array_; }
  std::atomic<uint32_t>* sq_tail_ptr() const { return sq_tail_ptr_; }
  std::atomic<uint32_t>* cq_head_ptr() const { return cq_head_ptr_; }
  std::atomic<uint32_t>* cq_tail_ptr() const { return cq_tail_ptr_; }

  // Prepares and submits an SQE using the provided callback to populate it.
  void SubmitSqe(fit::function<void(Sqe*)> fill_request);

 private:
  IoUring(fbl::unique_fd ring_fd, io_uring_params params, test_helper::ScopedMMap sq_mapping,
          test_helper::ScopedMMap cqe_mapping, test_helper::ScopedMMap sqe_mapping,
          io_uring_sqe* sqes, uint32_t* sq_array, std::atomic<uint32_t>* sq_tail_ptr,
          std::atomic<uint32_t>* cq_head_ptr, std::atomic<uint32_t>* cq_tail_ptr,
          io_uring_cqe* cqes);

  fbl::unique_fd ring_fd_;
  io_uring_params params_;
  test_helper::ScopedMMap sq_mapping_;
  test_helper::ScopedMMap cqe_mapping_;
  test_helper::ScopedMMap sqe_mapping_;
  io_uring_sqe* sqes_ = nullptr;
  io_uring_cqe* cqes_ = nullptr;
  uint32_t* sq_array_ = nullptr;
  std::atomic<uint32_t>* sq_tail_ptr_ = nullptr;
  std::atomic<uint32_t>* cq_head_ptr_ = nullptr;
  std::atomic<uint32_t>* cq_tail_ptr_ = nullptr;
};

}  // namespace io_uring_helper

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_IO_URING_HELPER_H_
