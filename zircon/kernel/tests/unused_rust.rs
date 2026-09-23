// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Temporary dependencies on kernel Rust crates to build them while we are
// working on bringing up more functionality in Rust.  At the moment, this
// code is not actually used by the kernel.

use console_env as _;
use debug as _;
use ksync as _;
