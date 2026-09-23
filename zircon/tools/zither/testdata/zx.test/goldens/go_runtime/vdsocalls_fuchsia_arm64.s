// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library zx by zither, a Fuchsia platform
// tool.

#include "go_asm.h"
#include "go_tls.h"
#include "textflag.h"
#include "funcdata.h"


// func vdsoCall_zx_channel_read(handle uint32, options uint32, bytes unsafe.Pointer, handles unsafe.Pointer, num_bytes uint32, num_handles uint32, actual_bytes unsafe.Pointer, actual_handles unsafe.Pointer) int32
TEXT runtime·vdsoCall_zx_channel_read(SB),NOSPLIT,$0-52
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVW handle+0(FP), R0
	MOVW options+4(FP), R1
	MOVD bytes+8(FP), R2
	MOVD handles+16(FP), R3
	MOVW num_bytes+24(FP), R4
	MOVW num_handles+28(FP), R5
	MOVD actual_bytes+32(FP), R6
	MOVD actual_handles+40(FP), R7
	BL vdso_zx_channel_read(SB)
	MOVW R0, ret+48(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_channel_write(handle uint32, options uint32, bytes unsafe.Pointer, num_bytes uint32, handles unsafe.Pointer, num_handles uint32) int32
TEXT runtime·vdsoCall_zx_channel_write(SB),NOSPLIT,$0-44
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVW handle+0(FP), R0
	MOVW options+4(FP), R1
	MOVD bytes+8(FP), R2
	MOVW num_bytes+16(FP), R3
	MOVD handles+24(FP), R4
	MOVW num_handles+32(FP), R5
	BL vdso_zx_channel_write(SB)
	MOVW R0, ret+40(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_clock_get_monotonic() uint64
TEXT runtime·vdsoCall_zx_clock_get_monotonic(SB),NOSPLIT,$0-8
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_clock_get_monotonic(SB)
	MOVD R0, ret+0(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_clock_get_monotonic_via_kernel() uint64
TEXT runtime·vdsoCall_zx_clock_get_monotonic_via_kernel(SB),NOSPLIT,$0-8
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_clock_get_monotonic_via_kernel(SB)
	MOVD R0, ret+0(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_handle_close_many(handles unsafe.Pointer, num_handles uint) int32
TEXT runtime·vdsoCall_zx_handle_close_many(SB),NOSPLIT,$0-20
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVD handles+0(FP), R0
	MOVD num_handles+8(FP), R1
	BL vdso_zx_handle_close_many(SB)
	MOVW R0, ret+16(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_ktrace_control(handle uint32, action uint32, options uint32, ptr unsafe.Pointer) int32
TEXT runtime·vdsoCall_zx_ktrace_control(SB),NOSPLIT,$0-28
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVW handle+0(FP), R0
	MOVW action+4(FP), R1
	MOVW options+8(FP), R2
	MOVD ptr+16(FP), R3
	BL vdso_zx_ktrace_control(SB)
	MOVW R0, ret+24(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_nanosleep(deadline uint64) int32
TEXT runtime·vdsoCall_zx_nanosleep(SB),NOSPLIT,$0-12
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVD deadline+0(FP), R0
	BL vdso_zx_nanosleep(SB)
	MOVW R0, ret+8(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_process_exit(retcode int64)
TEXT runtime·vdsoCall_zx_process_exit(SB),NOSPLIT,$0-8
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	MOVD retcode+0(FP), R0
	BL vdso_zx_process_exit(SB)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_syscall_next()
TEXT runtime·vdsoCall_zx_syscall_next(SB),NOSPLIT,$0-0
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_syscall_next(SB)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_syscall_test0()
TEXT runtime·vdsoCall_zx_syscall_test0(SB),NOSPLIT,$0-0
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_syscall_test0(SB)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_syscall_test1()
TEXT runtime·vdsoCall_zx_syscall_test1(SB),NOSPLIT,$0-0
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_syscall_test1(SB)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_syscall_test2()
TEXT runtime·vdsoCall_zx_syscall_test2(SB),NOSPLIT,$0-0
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_syscall_test2(SB)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET

// func vdsoCall_zx_system_get_page_size() uint32
TEXT runtime·vdsoCall_zx_system_get_page_size(SB),NOSPLIT,$0-4
	GO_ARGS
	NO_LOCAL_POINTERS
	MOVD g_m(g), R21
	MOVD LR, m_vdsoPC(R21)
	DMB $0xe
	MOVD $ret-8(FP), R20 // caller's SP
	MOVD R20, m_vdsoSP(R21)
	BL vdso_zx_system_get_page_size(SB)
	MOVW R0, ret+0(FP)
	MOVD g_m(g), R21
	MOVD $0, m_vdsoSP(R21)
	RET
