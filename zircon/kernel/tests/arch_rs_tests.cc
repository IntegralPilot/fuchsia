// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>
#include <stdint.h>

#include <arch/ops.h>

extern "C" {  // Defined in Rust

bool rust_arch_rs_tests_interrupt_ops();
uint32_t rust_arch_rs_tests_curr_cpu_num();
uint32_t rust_arch_rs_tests_max_num_cpus();

}  // extern "C"

namespace {

bool rust_interrupt_ops_test() {
  BEGIN_TEST;
  EXPECT_TRUE(rust_arch_rs_tests_interrupt_ops());
  END_TEST;
}

bool rust_curr_cpu_num_test() {
  BEGIN_TEST;
  EXPECT_EQ(rust_arch_rs_tests_curr_cpu_num(), arch_curr_cpu_num());
  END_TEST;
}

bool rust_max_num_cpus_test() {
  BEGIN_TEST;
  EXPECT_EQ(rust_arch_rs_tests_max_num_cpus(), arch_max_num_cpus());
  END_TEST;
}

UNITTEST_START_TESTCASE(arch_rs_tests)
UNITTEST("test Rust interrupt enable/disable/disabled ops", rust_interrupt_ops_test)
UNITTEST("test Rust current CPU number", rust_curr_cpu_num_test)
UNITTEST("test Rust max num CPUs", rust_max_num_cpus_test)
UNITTEST_END_TESTCASE(arch_rs_tests, "arch_rs", "Tests for arch_rs")

}  // namespace
