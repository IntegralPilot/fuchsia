// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/io_uring_helper.h"

#include <string.h>

#include <utility>

namespace io_uring_helper {

int io_uring_setup(uint32_t entries, io_uring_params* params) {
  return static_cast<int>(syscall(__NR_io_uring_setup, entries, params));
}

int io_uring_enter(int fd, int to_submit, int min_complete, int flags, sigset_t* sigset) {
  return static_cast<int>(
      syscall(__NR_io_uring_enter, fd, to_submit, min_complete, flags, sigset, sizeof(sigset_t)));
}

int io_uring_register(int fd, unsigned int opcode, void* arg, unsigned int nr_args) {
  return static_cast<int>(syscall(__NR_io_uring_register, fd, opcode, arg, nr_args));
}

fit::result<int, std::unique_ptr<IoUring>> IoUring::Create(uint32_t entries,
                                                           io_uring_params params) {
  fbl::unique_fd ring_fd(io_uring_setup(entries, &params));
  if (!ring_fd.is_valid()) {
    return fit::error(errno);
  }

  auto sq_result = test_helper::ScopedMMap::MMap(
      /*addr=*/nullptr, params.sq_off.array + params.sq_entries * sizeof(__u32),
      PROT_READ | PROT_WRITE, MAP_SHARED | MAP_POPULATE, ring_fd.get(), IORING_OFF_SQ_RING);
  if (sq_result.is_error()) {
    return fit::error(sq_result.error_value());
  }
  auto sq_mapping = std::move(sq_result.value());
  char* sq_ptr = static_cast<char*>(sq_mapping.mapping());

  auto cqe_result = test_helper::ScopedMMap::MMap(
      /*addr=*/nullptr, params.cq_off.cqes + params.cq_entries * sizeof(io_uring_cqe),
      PROT_READ | PROT_WRITE, MAP_SHARED | MAP_POPULATE, ring_fd.get(), IORING_OFF_CQ_RING);
  if (cqe_result.is_error()) {
    return fit::error(cqe_result.error_value());
  }
  auto cqe_mapping = std::move(cqe_result.value());
  char* cqe_ptr = static_cast<char*>(cqe_mapping.mapping());

  auto sqe_result = test_helper::ScopedMMap::MMap(
      /*addr=*/nullptr, params.sq_entries * sizeof(io_uring_sqe), PROT_READ | PROT_WRITE,
      MAP_SHARED | MAP_POPULATE, ring_fd.get(), IORING_OFF_SQES);
  if (sqe_result.is_error()) {
    return fit::error(sqe_result.error_value());
  }
  auto sqe_mapping = std::move(sqe_result.value());
  io_uring_sqe* sqes = reinterpret_cast<io_uring_sqe*>(sqe_mapping.mapping());

  uint32_t* sq_array = reinterpret_cast<uint32_t*>(sq_ptr + params.sq_off.array);
  std::atomic<uint32_t>* sq_tail_ptr =
      reinterpret_cast<std::atomic<uint32_t>*>(sq_ptr + params.sq_off.tail);

  std::atomic<uint32_t>* cq_head_ptr =
      reinterpret_cast<std::atomic<uint32_t>*>(cqe_ptr + params.cq_off.head);
  std::atomic<uint32_t>* cq_tail_ptr =
      reinterpret_cast<std::atomic<uint32_t>*>(cqe_ptr + params.cq_off.tail);
  io_uring_cqe* cqes_ptr = reinterpret_cast<io_uring_cqe*>(cqe_ptr + params.cq_off.cqes);

  return fit::ok(std::unique_ptr<IoUring>(new IoUring(
      std::move(ring_fd), params, std::move(sq_mapping), std::move(cqe_mapping),
      std::move(sqe_mapping), sqes, sq_array, sq_tail_ptr, cq_head_ptr, cq_tail_ptr, cqes_ptr)));
}

IoUring::IoUring(fbl::unique_fd ring_fd, io_uring_params params, test_helper::ScopedMMap sq_mapping,
                 test_helper::ScopedMMap cqe_mapping, test_helper::ScopedMMap sqe_mapping,
                 io_uring_sqe* sqes, uint32_t* sq_array, std::atomic<uint32_t>* sq_tail_ptr,
                 std::atomic<uint32_t>* cq_head_ptr, std::atomic<uint32_t>* cq_tail_ptr,
                 io_uring_cqe* cqes)
    : ring_fd_(std::move(ring_fd)),
      params_(params),
      sq_mapping_(std::move(sq_mapping)),
      cqe_mapping_(std::move(cqe_mapping)),
      sqe_mapping_(std::move(sqe_mapping)),
      sqes_(sqes),
      cqes_(cqes),
      sq_array_(sq_array),
      sq_tail_ptr_(sq_tail_ptr),
      cq_head_ptr_(cq_head_ptr),
      cq_tail_ptr_(cq_tail_ptr) {}

void IoUring::SubmitSqe(fit::function<void(Sqe*)> fill_request) {
  uint32_t tail = sq_tail_ptr_->load(std::memory_order_acquire);
  uint32_t index = tail & (params_.sq_entries - 1);
  Sqe* sqe = reinterpret_cast<Sqe*>(&sqes_[index]);
  memset(sqe, 0, sizeof(Sqe));
  fill_request(sqe);
  sq_array_[index] = index;
  sq_tail_ptr_->store(tail + 1, std::memory_order_release);
}

}  // namespace io_uring_helper
