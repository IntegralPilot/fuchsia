// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On Aarch64 we're often going to be synchronizing with non-cache-coherent devices, so use the dsb
// variants. They also synchronize with cache flush operations. We use the full-system variations
// because some GPUs may not be in the outer-shareable domain.

// TODO(https://fxbug.dev/492132218) Share this code with adreno.

// Ensures that all writes before this call happen before any writes after this call.
pub fn write() {
    // SAFETY: We are calling this assembly correctly.
    unsafe {
        if cfg!(target_arch = "aarch64") {
            std::arch::asm!("dsb st");
        } else if cfg!(target_arch = "x86_64") {
            std::arch::asm!("sfence");
        } else {
            unimplemented!("write_barrier not implemented")
        }
    }
}
