// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

[[noreturn]] [[clang::always_inline]] void f0() { __builtin_trap(); }

[[clang::noinline]] void f1() { f0(); }

[[clang::always_inline]] void f2() { f1(); }

[[clang::noinline]] void f3() { f2(); }

int main() { f3(); }
