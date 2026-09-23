// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// x86_64 doesn't depend on vDSO for sigreturn.
pub const VDSO_SIGRETURN_NAME: Option<&'static [u8]> = None;
