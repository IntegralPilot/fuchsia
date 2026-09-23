// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_SBI_CALL_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_SBI_CALL_H_

// RISC-V Supervisor Binary Interface Specification
//
// https://github.com/riscv-non-isa/riscv-sbi-doc/blob/master/riscv-sbi.adoc

#include <lib/arch/riscv64/sbi.h>

#include <cstdint>

namespace arch {

// This provides generic arch::SbiCall<EID, FID> calls for any number of
// integer arguments.  Convenience functions using these are defined below in
// the arch::RiscvSbi:: namespace.

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall() {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a1 __asm__("a1");
  register uintptr_t a0 __asm__("a0");
  __asm__ volatile("ecall\n" : "=r"(a0), "=r"(a1) : "r"(a6), "r"(a7) : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a1 __asm__("a1");
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n" : "+r"(a0), "=r"(a1) : "r"(a6), "r"(a7) : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0, uintptr_t _a1) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a1 __asm__("a1") = _a1;
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n" : "+r"(a0), "+r"(a1) : "r"(a6), "r"(a7) : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0, uintptr_t _a1, uintptr_t _a2) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a2 __asm__("a2") = _a2;
  register uintptr_t a1 __asm__("a1") = _a1;
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a6), "r"(a7) : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0, uintptr_t _a1, uintptr_t _a2, uintptr_t _a3) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a3 __asm__("a3") = _a3;
  register uintptr_t a2 __asm__("a2") = _a2;
  register uintptr_t a1 __asm__("a1") = _a1;
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n" : "+r"(a0), "+r"(a1) : "r"(a2), "r"(a3), "r"(a6), "r"(a7) : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0, uintptr_t _a1, uintptr_t _a2, uintptr_t _a3,
                           uintptr_t _a4) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a4 __asm__("a4") = _a4;
  register uintptr_t a3 __asm__("a3") = _a3;
  register uintptr_t a2 __asm__("a2") = _a2;
  register uintptr_t a1 __asm__("a1") = _a1;
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n"
                   : "+r"(a0), "+r"(a1)
                   : "r"(a2), "r"(a3), "r"(a4), "r"(a6), "r"(a7)
                   : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

template <RiscvSbiEid Eid, auto Fid>
inline RiscvSbiRet SbiCall(uintptr_t _a0, uintptr_t _a1, uintptr_t _a2, uintptr_t _a3,
                           uintptr_t _a4, uintptr_t _a5) {
  register uintptr_t a7 __asm__("a7") = static_cast<uintptr_t>(Eid);
  register uintptr_t a6 __asm__("a6") = static_cast<uintptr_t>(Fid);
  register uintptr_t a5 __asm__("a5") = _a5;
  register uintptr_t a4 __asm__("a4") = _a4;
  register uintptr_t a3 __asm__("a3") = _a3;
  register uintptr_t a2 __asm__("a2") = _a2;
  register uintptr_t a1 __asm__("a1") = _a1;
  register uintptr_t a0 __asm__("a0") = _a0;
  __asm__ volatile("ecall\n"
                   : "+r"(a0), "+r"(a1)
                   : "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a6), "r"(a7)
                   : "memory");
  return RiscvSbiRet{.error = static_cast<RiscvSbiError>(a0), .value = static_cast<intptr_t>(a1)};
}

namespace RiscvSbi {

// Base Extension calls

inline RiscvSbiRet GetSpecVersion() {
  return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetSpecVersion>();
}

inline RiscvSbiRet GetImplId() { return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetImplId>(); }

inline RiscvSbiRet GetImplVersion() {
  return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetImplVersion>();
}

inline RiscvSbiRet ProbeExtension(RiscvSbiEid eid) {
  return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kProbeExtension>(static_cast<uintptr_t>(eid));
}

inline RiscvSbiRet GetMvendorid() {
  return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetMvendorid>();
}

inline RiscvSbiRet GetMarchid() { return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetMarchid>(); }

inline RiscvSbiRet GetMimpid() { return SbiCall<RiscvSbiEid::kBase, RiscvSbiBase::kGetMimpid>(); }

// Timer Extension calls

inline RiscvSbiRet SetTimer(uint64_t time) {
  return SbiCall<RiscvSbiEid::kTimer, RiscvSbiTimer::kSetTimer>(time);
}

// IPI Extension calls

inline RiscvSbiRet SendIpi(HartMask hart_mask, HartMaskBase hart_mask_base) {
  return SbiCall<RiscvSbiEid::kIpi, RiscvSbiIpi::kSendIpi>(hart_mask, hart_mask_base);
}

// System Reset Extension calls

inline RiscvSbiRet SystemReset(RiscvSbiResetType reset_type, RiscvSbiResetReason reset_reason) {
  return SbiCall<RiscvSbiEid::kSystemReset, RiscvSbiSystemReset::kSystemReset>(
      static_cast<uint32_t>(reset_type), static_cast<uint32_t>(reset_reason));
}

// Remote fence Extension calls

inline RiscvSbiRet RemoteFenceI(HartMask hart_mask, HartMaskBase hart_mask_base) {
  return SbiCall<RiscvSbiEid::kRfence, RiscvSbiRfence::kFenceI>(hart_mask, hart_mask_base);
}

inline RiscvSbiRet RemoteSfenceVma(HartMask hart_mask, HartMaskBase hart_mask_base,
                                   uintptr_t start_addr, uintptr_t size) {
  return SbiCall<RiscvSbiEid::kRfence, RiscvSbiRfence::kSfenceVma>(hart_mask, hart_mask_base,
                                                                   start_addr, size);
}

inline RiscvSbiRet RemoteSfenceVmaAsid(HartMask hart_mask, HartMaskBase hart_mask_base,
                                       uintptr_t start_addr, uintptr_t size, uint64_t asid) {
  return SbiCall<RiscvSbiEid::kRfence, RiscvSbiRfence::kSfenceVmaAsid>(hart_mask, hart_mask_base,
                                                                       start_addr, size, asid);
}

// Hart State Management Extension calls

inline RiscvSbiRet HartStart(HartId hart_id, uintptr_t start_addr, uint64_t opaque) {
  return SbiCall<RiscvSbiEid::kHart, RiscvSbiHart::kStart>(hart_id, start_addr, opaque);
}

inline RiscvSbiRet HartStop() { return SbiCall<RiscvSbiEid::kHart, RiscvSbiHart::kStop>(); }

inline RiscvSbiRet HartGetStatus(HartId hart_id) {
  return SbiCall<RiscvSbiEid::kHart, RiscvSbiHart::kGetStatus>(hart_id);
}

inline RiscvSbiRet HartSuspend(HartId hart_id, uintptr_t resume_addr, uint64_t opaque) {
  return SbiCall<RiscvSbiEid::kHart, RiscvSbiHart::kSuspend>(hart_id, resume_addr, opaque);
}

}  // namespace RiscvSbi

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_RISCV64_SBI_CALL_H_
