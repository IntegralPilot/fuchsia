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
TEXT runtime·vdsoCall_zx_channel_read(SB),NOSPLIT,$40-52
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVL handle+0(FP), DI
	MOVL options+4(FP), SI
	MOVQ bytes+8(FP), DX
	MOVQ handles+16(FP), CX
	MOVL num_bytes+24(FP), R8
	MOVL num_handles+28(FP), R9
	MOVQ actual_bytes+32(FP), R12
	MOVQ actual_handles+40(FP), R13
	MOVQ SP, BP   // BP is preserved across vsdo call by the x86-64 ABI
	ANDQ $~15, SP // stack alignment for x86-64 ABI
	PUSHQ R13
	PUSHQ R12
	MOVQ vdso_zx_channel_read(SB), AX
	CALL AX
	POPQ R12
	POPQ R13
	MOVQ BP, SP
	MOVL AX, ret+48(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_channel_write(handle uint32, options uint32, bytes unsafe.Pointer, num_bytes uint32, handles unsafe.Pointer, num_handles uint32) int32
TEXT runtime·vdsoCall_zx_channel_write(SB),NOSPLIT,$8-44
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVL handle+0(FP), DI
	MOVL options+4(FP), SI
	MOVQ bytes+8(FP), DX
	MOVL num_bytes+16(FP), CX
	MOVQ handles+24(FP), R8
	MOVL num_handles+32(FP), R9
	MOVQ vdso_zx_channel_write(SB), AX
	CALL AX
	MOVL AX, ret+40(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_clock_get_monotonic() uint64
TEXT runtime·vdsoCall_zx_clock_get_monotonic(SB),NOSPLIT,$8-8
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_clock_get_monotonic(SB), AX
	CALL AX
	MOVQ AX, ret+0(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_clock_get_monotonic_via_kernel() uint64
TEXT runtime·vdsoCall_zx_clock_get_monotonic_via_kernel(SB),NOSPLIT,$8-8
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_clock_get_monotonic_via_kernel(SB), AX
	CALL AX
	MOVQ AX, ret+0(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_handle_close_many(handles unsafe.Pointer, num_handles uint) int32
TEXT runtime·vdsoCall_zx_handle_close_many(SB),NOSPLIT,$8-20
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ handles+0(FP), DI
	MOVQ num_handles+8(FP), SI
	MOVQ vdso_zx_handle_close_many(SB), AX
	CALL AX
	MOVL AX, ret+16(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_ktrace_control(handle uint32, action uint32, options uint32, ptr unsafe.Pointer) int32
TEXT runtime·vdsoCall_zx_ktrace_control(SB),NOSPLIT,$8-28
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVL handle+0(FP), DI
	MOVL action+4(FP), SI
	MOVL options+8(FP), DX
	MOVQ ptr+16(FP), CX
	MOVQ vdso_zx_ktrace_control(SB), AX
	CALL AX
	MOVL AX, ret+24(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_nanosleep(deadline uint64) int32
TEXT runtime·vdsoCall_zx_nanosleep(SB),NOSPLIT,$8-12
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ deadline+0(FP), DI
	MOVQ vdso_zx_nanosleep(SB), AX
	CALL AX
	MOVL AX, ret+8(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_process_exit(retcode int64)
TEXT runtime·vdsoCall_zx_process_exit(SB),NOSPLIT,$8-8
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ retcode+0(FP), DI
	MOVQ vdso_zx_process_exit(SB), AX
	CALL AX
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_syscall_next()
TEXT runtime·vdsoCall_zx_syscall_next(SB),NOSPLIT,$8-0
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_syscall_next(SB), AX
	CALL AX
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_syscall_test0()
TEXT runtime·vdsoCall_zx_syscall_test0(SB),NOSPLIT,$8-0
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_syscall_test0(SB), AX
	CALL AX
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_syscall_test1()
TEXT runtime·vdsoCall_zx_syscall_test1(SB),NOSPLIT,$8-0
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_syscall_test1(SB), AX
	CALL AX
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_syscall_test2()
TEXT runtime·vdsoCall_zx_syscall_test2(SB),NOSPLIT,$8-0
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_syscall_test2(SB), AX
	CALL AX
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET

// func vdsoCall_zx_system_get_page_size() uint32
TEXT runtime·vdsoCall_zx_system_get_page_size(SB),NOSPLIT,$8-4
	GO_ARGS
	NO_LOCAL_POINTERS
	get_tls(CX)
	MOVQ g(CX), AX
	MOVQ g_m(AX), R14
	PUSHQ R14
	LEAQ ret+0(FP), DX
	MOVQ -8(DX), CX
	MOVQ CX, m_vdsoPC(R14)
	MOVQ DX, m_vdsoSP(R14)
	MOVQ vdso_zx_system_get_page_size(SB), AX
	CALL AX
	MOVL AX, ret+0(FP)
	POPQ R14
	MOVQ $0, m_vdsoSP(R14)
	RET
