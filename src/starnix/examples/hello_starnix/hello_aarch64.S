// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

.data

msg:
    .ascii "hello starnix\n"

.text

.globl _start
_start:
    mov x0, #1 /* write to stdout */
    adrp x1, msg /* pointer to msg */
    add x1, x1, #:lo12:msg /* rest of msg pointer */
    mov x2, #14 /* length of msg */
    mov x8, #64
    svc #0

    mov x0, #0
    mov x8, #94
    svc #0


