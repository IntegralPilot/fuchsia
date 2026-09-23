// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

.data

msg:
    .ascii "hello starnix\n"

.text

.global _start
_start:
    addi a0, zero, 1 # write to stdout
    la a1, msg # pointer to msg
    addi a2, zero, 14 # length of msg
    addi a7, zero, 64 # linux write system call
    ecall

    addi a0, zero, 0 # return code
    addi a7, zero, 93 # linux exit call
    ecall
