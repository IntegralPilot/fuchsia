// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::{EXIT_SUCCESS, ShellState};
use bstr::BString;
use std::io::{Read, Write};

pub fn builtin_times(
    _args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    _stderr: &mut dyn Write,
) -> i32 {
    let mut tms = libc::tms { tms_utime: 0, tms_stime: 0, tms_cutime: 0, tms_cstime: 0 };
    unsafe {
        libc::times(&mut tms);
    }

    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let clk_tck = if clk_tck > 0 { clk_tck as f64 } else { 100.0 };

    let user_m = (tms.tms_utime as f64 / clk_tck / 60.0) as i32;
    let user_s = tms.tms_utime as f64 / clk_tck;
    let sys_m = (tms.tms_stime as f64 / clk_tck / 60.0) as i32;
    let sys_s = tms.tms_stime as f64 / clk_tck;

    let chuser_m = (tms.tms_cutime as f64 / clk_tck / 60.0) as i32;
    let chuser_s = tms.tms_cutime as f64 / clk_tck;
    let chsys_m = (tms.tms_cstime as f64 / clk_tck / 60.0) as i32;
    let chsys_s = tms.tms_cstime as f64 / clk_tck;

    let _ = writeln!(
        stdout,
        "{}m{:.6}s {}m{:.6}s\n{}m{:.6}s {}m{:.6}s",
        user_m, user_s, sys_m, sys_s, chuser_m, chuser_s, chsys_m, chsys_s
    );

    EXIT_SUCCESS
}
