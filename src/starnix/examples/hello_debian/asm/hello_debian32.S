// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

.text
.globl _start
_start:
IN:
    ldr.w    sl, [pc, #0x68]
    ldr.w    r4, [pc, #0x68]
    mov      r0, sp
    bl _say_hello

_say_hello:
    mov r0, #1 /* fd = stdout (1) */
    adr r1, msg /* address to msg */
    mov r2, #15 /* msg length */
    mov r7, #4 /* write */
    svc #0

    // bkpt

    mov r0, #0 /* exit status = 0 */
    mov r7, #0
    add r7, r7, #248 /* exit_group */
    svc #0

msg:
  .ascii "hello debian32\n"
