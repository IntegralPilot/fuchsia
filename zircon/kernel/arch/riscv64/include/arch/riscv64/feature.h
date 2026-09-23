// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FEATURE_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FEATURE_H_

#include <stdbool.h>
#include <stdint.h>
#include <zircon/compiler.h>

#include <kernel/ffi.h>

__BEGIN_CDECLS

bool rust_riscv64_feature_has_vector();
bool rust_riscv64_feature_has_zicbom();
bool rust_riscv64_feature_has_zicboz();
bool rust_riscv64_feature_has_svpbmt();
bool rust_riscv64_feature_has_zicntr();
bool rust_riscv64_feature_has_sstc();
uint32_t rust_riscv64_feature_cbom_size();
uint32_t rust_riscv64_feature_cboz_size();
uint64_t rust_riscv64_feature_vlenb();

__END_CDECLS

#ifdef __cplusplus
#include <lib/arch/riscv64/feature.h>

inline bool riscv64_feature_has_vector() { return rust_riscv64_feature_has_vector(); }
inline bool riscv64_feature_has_zicbom() { return rust_riscv64_feature_has_zicbom(); }
inline bool riscv64_feature_has_zicboz() { return rust_riscv64_feature_has_zicboz(); }
inline bool riscv64_feature_has_svpbmt() { return rust_riscv64_feature_has_svpbmt(); }
inline bool riscv64_feature_has_zicntr() { return rust_riscv64_feature_has_zicntr(); }
inline bool riscv64_feature_has_sstc() { return rust_riscv64_feature_has_sstc(); }
inline uint32_t riscv_cbom_size() { return rust_riscv64_feature_cbom_size(); }
inline uint32_t riscv_cboz_size() { return rust_riscv64_feature_cboz_size(); }
inline uint64_t riscv_vlenb() { return rust_riscv64_feature_vlenb(); }

#endif  // __cplusplus

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_INCLUDE_ARCH_RISCV64_FEATURE_H_
