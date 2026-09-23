// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

[[clang::noinline]] void f0() {
  // Generate an architectural exception (page fault) which will cause a restricted
  // exception exit to Starnix. Starnix's exception handling logic will then issue a
  // backtrace request exception.
  *reinterpret_cast<volatile char*>(0x0) = 1;
}

// Make sure we generate some thumb code as well, which the unwinder should also be able to handle.
// This is what the disassembly of this function looks like:
//   000109e0 <_Z2f1v>:
//     109e0: b580         	push	{r7, lr}
//     109e2: 466f         	mov	r7, sp
//     109e4: f7ff eff4    	blx	0x109d0 <_Z2f0v>        @ imm = #-0x18
//     109e8: bd80         	pop	{r7, pc}
//     109ea: d4d4         	bmi	0x10996 <__do_fini+0x56> @ imm = #-0x58
[[clang::noinline]] __attribute__((target("thumb"))) void f1() { f0(); }

// Normal arm32 ABI.
// The disassembly of this function looks like this:
//   000109ec <_Z2f2v>:
//     109ec: e92d4800     	push	{r11, lr}
//     109f0: e1a0b00d     	mov	r11, sp
//     109f4: fafffff9     	blx	0x109e0 <_Z2f1v>        @ imm = #-0x1c
//     109f8: e8bd8800     	pop	{r11, pc}
[[clang::noinline]] void f2() { f1(); }

[[clang::noinline]] __attribute__((target("thumb"))) void f3() { f2(); }

int main() { f3(); }
