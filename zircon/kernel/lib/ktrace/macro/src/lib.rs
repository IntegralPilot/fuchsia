// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

#[cfg(not(gcc))]
mod enabled;

// TODO(https://fxbug.dev/529516970): Tracing macros expand to no-ops when compiling the kernel with
// GCC to prevent unresolved symbol references and link errors.
#[cfg(gcc)]
mod disabled;
