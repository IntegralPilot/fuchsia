// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_TESTS_SYSMEM_SECURE_VMO_READ_TESTER_H_
#define SRC_SYSMEM_TESTS_SYSMEM_SECURE_VMO_READ_TESTER_H_

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <atomic>
#include <random>
#include <thread>

#include <zxtest/zxtest.h>

#include "common.h"

// Faulting on write to a mapping to the VMO can't be checked currently
// because maybe it goes into CPU cache without faulting because 34580?
class SecureVmoReadTester {
 public:
  explicit SecureVmoReadTester(zx::vmo secure_vmo);
  explicit SecureVmoReadTester(zx::unowned_vmo unowned_secure_vmo);
  ~SecureVmoReadTester();
  bool IsReadFromSecureAThing();
  // When we're trying to read from an actual secure VMO, expect_read_success is false.
  // When we're trying tor read from an aux VMO, expect_read_success is true.
  void AttemptReadFromSecure(bool expect_read_success = false);

 private:
  void Init();

  zx::vmo secure_vmo_to_delete_;
  zx::unowned_vmo unowned_secure_vmo_;
  zx::vmar child_vmar_;
  // volatile only so reads in the code actually read despite the value being
  // discarded.
  volatile uint8_t* map_addr_ = {};
  // This is set to true just before the attempt to read.
  std::atomic<bool> is_read_from_secure_attempted_ = false;
  std::atomic<bool> is_read_from_secure_a_thing_ = false;
  std::thread let_die_thread_;
  std::atomic<bool> is_let_die_started_ = false;

  std::uint64_t kSeed = 0;
  std::mt19937_64 prng_{kSeed};
  std::uniform_int_distribution<uint32_t> distribution_{0, std::numeric_limits<uint32_t>::max()};
};

#endif  // SRC_SYSMEM_TESTS_SYSMEM_SECURE_VMO_READ_TESTER_H_
