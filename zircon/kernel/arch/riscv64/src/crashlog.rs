// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Architectural crashlog register formatting for RISC-V 64.

use core::fmt::Write;

/// Hardware crashlog registers capturing CPU exception state.
type CrashlogRegs = riscv64_crashlog_bindings::crashlog_regs_t;

const _: () = {
    assert!(core::mem::size_of::<CrashlogRegs>() == 24);
    assert!(core::mem::align_of::<CrashlogRegs>() == 8);
};

fn write_hex_reg<W: Write>(w: &mut W, name: &str, val: u64) -> core::fmt::Result {
    if val == 0 {
        writeln!(w, "{:>7}:                  0", name)
    } else {
        writeln!(w, "{:>7}: {:#18x}", name, val)
    }
}

/// Format crashlog registers into a string or writer.
///
/// # Safety
/// `regs.iframe` must either be null or point at a live `iframe_t` that outlives
/// the call. It is captured at the time of the crash, so the caller is
/// responsible for only rendering a crashlog whose frame is still mapped.
unsafe fn format_crashlog_registers<W: Write>(w: &mut W, regs: &CrashlogRegs) -> core::fmt::Result {
    if regs.iframe.is_null() {
        return writeln!(w, "missing");
    }

    // SAFETY: null was ruled out above, and the caller guarantees a non-null
    // `regs.iframe` points at a live `iframe_t` for the duration of the call.
    let frame = unsafe { &*regs.iframe };
    write_hex_reg(w, "pc", frame.regs.pc)?;
    write_hex_reg(w, "ra", frame.regs.ra)?;
    write_hex_reg(w, "sp", frame.regs.sp)?;
    write_hex_reg(w, "gp", frame.regs.gp)?;
    write_hex_reg(w, "tp", frame.regs.tp)?;
    write_hex_reg(w, "t0", frame.regs.t0)?;
    write_hex_reg(w, "t1", frame.regs.t1)?;
    write_hex_reg(w, "t2", frame.regs.t2)?;
    write_hex_reg(w, "s0", frame.regs.s0)?;
    write_hex_reg(w, "s1", frame.regs.s1)?;
    write_hex_reg(w, "a0", frame.regs.a0)?;
    write_hex_reg(w, "a1", frame.regs.a1)?;
    write_hex_reg(w, "a2", frame.regs.a2)?;
    write_hex_reg(w, "a3", frame.regs.a3)?;
    write_hex_reg(w, "a4", frame.regs.a4)?;
    write_hex_reg(w, "a5", frame.regs.a5)?;
    write_hex_reg(w, "a6", frame.regs.a6)?;
    write_hex_reg(w, "a7", frame.regs.a7)?;
    write_hex_reg(w, "s2", frame.regs.s2)?;
    write_hex_reg(w, "s3", frame.regs.s3)?;
    write_hex_reg(w, "s4", frame.regs.s4)?;
    write_hex_reg(w, "s5", frame.regs.s5)?;
    write_hex_reg(w, "s6", frame.regs.s6)?;
    write_hex_reg(w, "s7", frame.regs.s7)?;
    write_hex_reg(w, "s8", frame.regs.s8)?;
    write_hex_reg(w, "s9", frame.regs.s9)?;
    write_hex_reg(w, "s10", frame.regs.s10)?;
    write_hex_reg(w, "s11", frame.regs.s11)?;
    write_hex_reg(w, "t3", frame.regs.t3)?;
    write_hex_reg(w, "t4", frame.regs.t4)?;
    write_hex_reg(w, "t5", frame.regs.t5)?;
    write_hex_reg(w, "t6", frame.regs.t6)?;
    write_hex_reg(w, "status", frame.status)?;
    writeln!(w, "{:>7}: {:18}", "cause", regs.cause)?;
    write_hex_reg(w, "tval", regs.tval)
}

/// # Safety
/// Caller guarantees valid callbacks and pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_render_crashlog_registers(
    write_cb: unsafe extern "C" fn(*mut core::ffi::c_void, *const u8, usize),
    ctx: *mut core::ffi::c_void,
    regs: *const CrashlogRegs,
) {
    if regs.is_null() {
        return;
    }
    struct CbWriter {
        cb: unsafe extern "C" fn(*mut core::ffi::c_void, *const u8, usize),
        ctx: *mut core::ffi::c_void,
    }
    impl core::fmt::Write for CbWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            // SAFETY: Forwarding string chunk to provided callback.
            unsafe { (self.cb)(self.ctx, s.as_ptr(), s.len()) };
            Ok(())
        }
    }
    // SAFETY: null was ruled out above; the caller guarantees the remaining
    // pointer refers to a live `CrashlogRegs` for the duration of the call.
    let regs = unsafe { &*regs };
    let mut writer = CbWriter { cb: write_cb, ctx };
    // SAFETY: `regs.iframe` came from the caller under the same contract this
    // function documents, and `writer` only forwards to `write_cb`/`ctx`.
    let _ = unsafe { format_crashlog_registers(&mut writer, regs) };
}
