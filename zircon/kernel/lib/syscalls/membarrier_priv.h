// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SYSCALLS_MEMBARRIER_PRIV_H_
#define ZIRCON_KERNEL_LIB_SYSCALLS_MEMBARRIER_PRIV_H_

extern "C" {
void cpp_membarrier_data_barrier();
void cpp_membarrier_instruction_barrier();
}

#endif  // ZIRCON_KERNEL_LIB_SYSCALLS_MEMBARRIER_PRIV_H_
