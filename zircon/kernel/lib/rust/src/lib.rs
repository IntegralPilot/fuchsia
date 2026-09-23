// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// This is a stub crate_root file just to produce an rlib called
// "compiler_builtins" as rustc requires.  So far there is no need for it to
// define anything.  The compiler-generated references are already met by
// //zircon/kernel/lib/libc code.

#![feature(compiler_builtins)]
#![feature(linkage)]
#![feature(no_core)]
#![compiler_builtins]
#![no_builtins]
#![no_core]
#![no_std]
#![allow(unused_features)]
#![allow(internal_features)]
#![allow(stable_features)]
