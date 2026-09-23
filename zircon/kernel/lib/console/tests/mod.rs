// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::c_int;

#[repr(C)]
#[derive(Clone, Copy)]
struct File {
    write: unsafe extern "C" fn(
        *mut core::ffi::c_void,
        *const core::ffi::c_char,
        usize,
    ) -> core::ffi::c_int,
    ptr: *mut core::ffi::c_void,
}

unsafe extern "C" {
    static mut gStdout: File;
}

struct CapturerState {
    buf: *mut u8,
    buf_size: usize,
    len: usize,
    original_stdout: Option<File>,
}

struct SyncUnsafeCell<T>(core::cell::UnsafeCell<T>);

// SAFETY: This is safe because we only run tests sequentially in the kernel console test thread.
unsafe impl<T> Sync for SyncUnsafeCell<T> {}

static CAPTURER_STATE: SyncUnsafeCell<CapturerState> =
    SyncUnsafeCell(core::cell::UnsafeCell::new(CapturerState {
        buf: core::ptr::null_mut(),
        buf_size: 0,
        len: 0,
        original_stdout: None,
    }));

unsafe extern "C" fn write_callback(
    _ptr: *mut core::ffi::c_void,
    str_ptr: *const core::ffi::c_char,
    len: usize,
) -> core::ffi::c_int {
    let state = unsafe { &mut *CAPTURER_STATE.0.get() };
    if state.buf.is_null() || state.buf_size == 0 {
        return 0;
    }
    let to_copy = core::cmp::min(len, state.buf_size - 1 - state.len);
    if to_copy > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(str_ptr.cast::<u8>(), state.buf.add(state.len), to_copy);
        }
        state.len += to_copy;
        unsafe { *state.buf.add(state.len) = 0 };
    }
    len as core::ffi::c_int
}

unsafe fn test_capture_stdout_start(buf: *mut u8, size: usize) {
    let state = unsafe { &mut *CAPTURER_STATE.0.get() };
    state.buf = buf;
    state.buf_size = size;
    state.len = 0;
    unsafe { *state.buf = 0 };
    unsafe {
        state.original_stdout = Some(gStdout);
        gStdout = File { write: write_callback, ptr: core::ptr::null_mut() };
    }
}

unsafe fn test_capture_stdout_stop() {
    let state = unsafe { &mut *CAPTURER_STATE.0.get() };
    if let Some(orig) = state.original_stdout {
        unsafe {
            gStdout = orig;
        }
        state.original_stdout = None;
    }
    state.buf = core::ptr::null_mut();
    state.buf_size = 0;
    state.len = 0;
}

fn capture_output_helper(f: impl FnOnce() -> c_int) -> (c_int, &'static core::ffi::CStr) {
    static BUF: SyncUnsafeCell<[u8; 4096]> = SyncUnsafeCell(core::cell::UnsafeCell::new([0; 4096]));
    let buf_ptr = BUF.0.get() as *mut u8;
    unsafe {
        core::ptr::write_bytes(buf_ptr, 0, 4096);
        test_capture_stdout_start(buf_ptr, 4096);
    }
    let res = f();
    unsafe {
        test_capture_stdout_stop();
        (res, core::ffi::CStr::from_ptr(buf_ptr as *const core::ffi::c_char))
    }
}

/// Tests for the kernel console.
#[cfg(all(console_enabled, ktest))]
#[unittest::suite(name = "console_rust")]
mod console_tests {
    use crate::console_rust::console::{
        CMD_AVAIL_ALWAYS, CMD_AVAIL_NORMAL, CMD_AVAIL_PANIC, CMD_FLAG_PANIC, Cmd, CmdArgs, ECHO,
        EXIT_CONSOLE, console_run_script_locked, match_command, parse_bool, parse_c_style_int,
        static_command, tokenize_command,
    };
    use core::sync::atomic::{AtomicI32, Ordering};
    use unittest::{assert_eq, expect_eq, expect_false, expect_lt, expect_ne, expect_true};
    use zx_status::Status;

    // FFI imports.
    unsafe extern "C" {
        static __start_commands: Cmd;
        static __stop_commands: Cmd;
    }

    // Statically registered mock commands.
    static_command!(
        TEST_CMD,
        c"mock_success".as_ptr(),
        c"mock_success help".as_ptr(),
        mock_success_callback,
        CMD_AVAIL_NORMAL
    );

    static_command!(
        MOCK_FAILURE,
        c"mock_failure".as_ptr(),
        core::ptr::null(),
        mock_failure_callback,
        CMD_AVAIL_NORMAL
    );

    static_command!(
        CMD_MOCK_ALWAYS,
        c"mock_avail_always".as_ptr(),
        c"mock_avail_always help".as_ptr(),
        mock_success_callback,
        CMD_AVAIL_ALWAYS
    );

    static_command!(
        CMD_MOCK_PANIC,
        c"mock_avail_panic".as_ptr(),
        c"mock_avail_panic help".as_ptr(),
        mock_panic_callback,
        CMD_AVAIL_PANIC
    );

    static MOCK_CALL_COUNT: AtomicI32 = AtomicI32::new(0);

    unsafe extern "C" fn mock_success_callback(
        _argc: c_int,
        _argv: *const CmdArgs,
        _flags: u32,
    ) -> c_int {
        MOCK_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
        zx_status::sys::ZX_OK
    }

    unsafe extern "C" fn mock_failure_callback(
        _argc: c_int,
        _argv: *const CmdArgs,
        _flags: u32,
    ) -> c_int {
        MOCK_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
        Status::INVALID_ARGS.into_raw()
    }

    static MOCK_PANIC_FLAGS: AtomicI32 = AtomicI32::new(0);

    unsafe extern "C" fn mock_panic_callback(
        _argc: c_int,
        _argv: *const CmdArgs,
        flags: u32,
    ) -> c_int {
        MOCK_PANIC_FLAGS.store(flags as i32, Ordering::Relaxed);
        zx_status::sys::ZX_OK
    }

    fn contains_command_help(output: &[u8], name: &[u8], help: &[u8]) -> bool {
        output.split(|&b| b == b'\n').any(|line| {
            line.starts_with(b"\t") && line[1..].starts_with(name) && line.ends_with(help)
        })
    }

    fn find_command_index(output: &[u8], name: &[u8]) -> Option<usize> {
        for (idx, line) in output.split(|&b| b == b'\n').enumerate() {
            if line.starts_with(b"\t") && line[1..].starts_with(name) {
                return Some(idx);
            }
        }
        None
    }

    /// Verify size and alignment compatibility with C++ structs.
    #[test]
    fn console_abi_test() {
        assert_eq!(core::mem::size_of::<CmdArgs>(), 40);
        assert_eq!(core::mem::align_of::<CmdArgs>(), 8);
        assert_eq!(core::mem::size_of::<Cmd>(), 32);
        assert_eq!(core::mem::align_of::<Cmd>(), 8);
    }

    /// Verify CmdArgs and Cmd string helper methods.
    #[test]
    fn cmd_args_and_cmd_helpers_test() {
        // CmdArgs with null string
        let args_null = CmdArgs::default();
        expect_true!(args_null.as_str() == "");

        // CmdArgs with valid string
        let str_val = c"hello_args";
        let args_val = CmdArgs { arg_str: str_val.as_ptr(), ..Default::default() };
        expect_true!(args_val.as_str() == "hello_args");

        // Cmd with null pointers
        let cmd_null = Cmd {
            cmd_str: core::ptr::null(),
            help_str: core::ptr::null(),
            cmd_callback: mock_success_callback,
            availability_mask: CMD_AVAIL_NORMAL,
        };
        expect_true!(cmd_null.name() == "");
        expect_true!(cmd_null.help() == "");
    }

    /// Test echo command callback in Rust
    #[test]
    fn command_echo_test() {
        let original_echo = ECHO.load(Ordering::Relaxed);

        // Set echo setting to false.
        let res = console_run_script_locked("echo false");
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_false!(ECHO.load(Ordering::Relaxed));

        // Set echo setting to true.
        let res = console_run_script_locked("echo true");
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_true!(ECHO.load(Ordering::Relaxed));

        // Set echo setting using off and on keywords.
        console_run_script_locked("echo off");
        expect_false!(ECHO.load(Ordering::Relaxed));
        console_run_script_locked("echo on");
        expect_true!(ECHO.load(Ordering::Relaxed));

        // Set echo setting using case-insensitive keywords and integers.
        console_run_script_locked("echo OFF");
        expect_false!(ECHO.load(Ordering::Relaxed));
        console_run_script_locked("echo TRUE");
        expect_true!(ECHO.load(Ordering::Relaxed));
        console_run_script_locked("echo 0");
        expect_false!(ECHO.load(Ordering::Relaxed));
        console_run_script_locked("echo 1");
        expect_true!(ECHO.load(Ordering::Relaxed));
        console_run_script_locked("echo -1");
        expect_true!(ECHO.load(Ordering::Relaxed));

        // Invalid argument should not modify the echo setting and return an error.
        let res = console_run_script_locked("echo invalid_text");
        expect_eq!(res, Status::INVALID_ARGS.into_raw());
        expect_true!(ECHO.load(Ordering::Relaxed));

        // Missing argument should return an error.
        let res = console_run_script_locked("echo");
        expect_eq!(res, Status::INVALID_ARGS.into_raw());
        expect_true!(ECHO.load(Ordering::Relaxed));

        // Restore original.
        let cmd = if original_echo { "echo true" } else { "echo false" };
        console_run_script_locked(cmd);
    }

    /// Test exit command callback in Rust
    #[test]
    fn command_exit_test() {
        let original_exit = EXIT_CONSOLE.load(Ordering::Relaxed);

        EXIT_CONSOLE.store(false, Ordering::Relaxed);
        let res = console_run_script_locked("exit");
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_false!(EXIT_CONSOLE.load(Ordering::Relaxed));

        // Restore.
        EXIT_CONSOLE.store(original_exit, Ordering::Relaxed);
    }

    /// Test `test` command callback in Rust
    #[test]
    fn command_test_cmd_test() {
        let res = console_run_script_locked("test foo 123 0x456 true");
        expect_eq!(res, zx_status::sys::ZX_OK);
    }

    /// Test boot-test-success command callback in Rust
    #[test]
    fn command_boot_test_success_test() {
        // Test success.
        console_run_script_locked("mock_success");
        let res = console_run_script_locked("boot-test-success");
        expect_eq!(res, zx_status::sys::ZX_OK);

        // Test failure.
        let some_failure = console_run_script_locked("mock_failure");
        let res = console_run_script_locked("boot-test-success");
        expect_eq!(res, some_failure);

        // Restore to success state.
        console_run_script_locked("mock_success");
    }

    /// Test and command callback in Rust
    #[test]
    fn command_and_test() {
        // If lastresult != zx_status::sys::ZX_OK, it should return lastresult immediately.
        let some_failure = console_run_script_locked("mock_failure");
        let res = console_run_script_locked("and mock_success");
        expect_eq!(res, some_failure);

        // If lastresult == zx_status::sys::ZX_OK, it should execute the command.
        console_run_script_locked("mock_success");
        let res = console_run_script_locked("and mock_success");
        expect_eq!(res, zx_status::sys::ZX_OK);

        // Invalid args (< 2)
        console_run_script_locked("mock_success");
        let res = console_run_script_locked("and");
        expect_eq!(res, Status::INVALID_ARGS.into_raw());

        // Command not found
        console_run_script_locked("mock_success");
        let res = console_run_script_locked("and nonexistent_cmd_xyz");
        expect_eq!(res, Status::NOT_FOUND.into_raw());
    }

    /// Test repeat command callback in Rust
    #[test]
    fn command_repeat_test() {
        MOCK_CALL_COUNT.store(0, Ordering::Relaxed);

        // Repeat mock_success.
        let res = console_run_script_locked("repeat 3 mock_success");
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_eq!(MOCK_CALL_COUNT.load(Ordering::Relaxed), 3);

        MOCK_CALL_COUNT.store(0, Ordering::Relaxed);

        // Repeat with early failure.
        let res = console_run_script_locked("repeat 3 mock_failure");
        expect_ne!(res, zx_status::sys::ZX_OK);
        expect_eq!(MOCK_CALL_COUNT.load(Ordering::Relaxed), 1);

        // Invalid args (< 3)
        let res = console_run_script_locked("repeat 3");
        expect_eq!(res, Status::INVALID_ARGS.into_raw());

        // Command not found
        let res = console_run_script_locked("repeat 3 nonexistent_cmd_xyz");
        expect_eq!(res, Status::NOT_FOUND.into_raw());
    }

    /// Test help command callback in Rust (normal)
    #[test]
    fn command_help_normal_test() {
        let cmd = match_command("help", 0xff);
        expect_true!(cmd.is_some());
        let cb = cmd.unwrap().cmd_callback;

        // Statically allocate the capture buffer to avoid kernel stack overflows.
        static BUF: SyncUnsafeCell<[u8; 4096]> =
            SyncUnsafeCell(core::cell::UnsafeCell::new([0; 4096]));
        let buf_ptr = BUF.0.get() as *mut u8;

        // Reset buffer and start capturing stdout.
        unsafe {
            core::ptr::write_bytes(buf_ptr, 0, 4096);
            test_capture_stdout_start(buf_ptr, 4096);
        }
        // Execute command and stop capturing stdout.
        let res = unsafe { cb(1, core::ptr::null(), 0) };
        unsafe {
            test_capture_stdout_stop();
        }

        expect_eq!(res, zx_status::sys::ZX_OK);

        // Slice the buffer up to the null terminator.
        let output = unsafe { core::slice::from_raw_parts(buf_ptr, 4096) };
        let len = output.iter().position(|&b| b == b'\0').unwrap_or(output.len());
        let output = &output[..len];

        expect_true!(contains_command_help(
            output,
            b"mock_avail_always",
            b"mock_avail_always help"
        ));
        expect_true!(contains_command_help(output, b"mock_success", b"mock_success help"));

        // Verify alphabetical sorting: mock_avail_always (a) comes before mock_success (s).
        let always_idx = find_command_index(output, b"mock_avail_always");
        let success_idx = find_command_index(output, b"mock_success");

        match (always_idx, success_idx) {
            (Some(a), Some(s)) => {
                expect_lt!(a, s);
            }
            _ => {
                record_failure!();
            }
        }
    }

    /// Test help command callback in Rust (panic)
    #[test]
    fn command_help_panic_test() {
        let cmd = match_command("help", 0xff);
        expect_true!(cmd.is_some());
        let cb = cmd.unwrap().cmd_callback;

        // Statically allocate the capture buffer to avoid kernel stack overflows.
        static BUF: SyncUnsafeCell<[u8; 4096]> =
            SyncUnsafeCell(core::cell::UnsafeCell::new([0; 4096]));
        let buf_ptr = BUF.0.get() as *mut u8;

        // Reset buffer and start capturing stdout.
        unsafe {
            core::ptr::write_bytes(buf_ptr, 0, 4096);
            test_capture_stdout_start(buf_ptr, 4096);
        }
        // Execute command and stop capturing stdout.
        let res = unsafe { cb(1, core::ptr::null(), CMD_FLAG_PANIC) };
        unsafe {
            test_capture_stdout_stop();
        }

        expect_eq!(res, zx_status::sys::ZX_OK);

        // Slice the buffer up to the null terminator.
        let output = unsafe { core::slice::from_raw_parts(buf_ptr, 4096) };
        let len = output.iter().position(|&b| b == b'\0').unwrap_or(output.len());
        let output = &output[..len];

        // mock_avail_always is still printed.
        expect_true!(contains_command_help(
            output,
            b"mock_avail_always",
            b"mock_avail_always help"
        ));

        // mock_success (CMD_AVAIL_NORMAL) should NOT be printed in panic mode.
        expect_false!(contains_command_help(output, b"mock_success", b"mock_success help"));
    }

    /// Test parse_c_style_int for decimal, hex, and octal integer formats
    #[test]
    fn parse_c_style_int_test() {
        // Decimal numbers
        expect_true!(parse_c_style_int("0") == Ok((0, 0)));
        expect_true!(parse_c_style_int("42") == Ok((42, 42)));
        expect_true!(parse_c_style_int("+42") == Ok((42, 42)));
        expect_true!(parse_c_style_int("-42") == Ok((0, -42)));

        // Hexadecimal numbers
        expect_true!(parse_c_style_int("0x10") == Ok((16, 16)));
        expect_true!(parse_c_style_int("0X10") == Ok((16, 16)));
        expect_true!(parse_c_style_int("0x2a") == Ok((42, 42)));
        expect_true!(parse_c_style_int("-0x10") == Ok((0, -16)));

        // Octal numbers
        expect_true!(parse_c_style_int("077") == Ok((63, 63)));
        expect_true!(parse_c_style_int("-077") == Ok((0, -63)));

        // Whitespace trimmed
        expect_true!(parse_c_style_int("  100  ") == Ok((100, 100)));

        // Minimum and maximum 64-bit signed integers
        expect_true!(parse_c_style_int("-9223372036854775808") == Ok((0, i64::MIN)));
        expect_true!(parse_c_style_int("9223372036854775807") == Ok((i64::MAX as u64, i64::MAX)));

        // Upper-half 64-bit kernel pointers and u64::MAX
        expect_true!(
            parse_c_style_int("0xffffffff80100000") == Ok((0xffffffff80100000, -2146435072))
        );
        expect_true!(parse_c_style_int("0xffffffffffffffff") == Ok((u64::MAX, -1)));
        expect_true!(parse_c_style_int("18446744073709551615") == Ok((u64::MAX, -1)));

        // Overflow beyond 64-bit bounds
        expect_true!(parse_c_style_int("-9223372036854775809") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("-0x8000000000000001") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("18446744073709551616") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("0x10000000000000000") == Err(Status::INVALID_ARGS));

        // Invalid inputs
        expect_true!(parse_c_style_int("") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("-") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("+") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("0x") == Err(Status::INVALID_ARGS));
        expect_true!(parse_c_style_int("invalid") == Err(Status::INVALID_ARGS));
    }

    /// Test parse_bool for true/false/on/off keywords and integer fallbacks
    #[test]
    fn parse_bool_test() {
        // Direct string keywords (case-insensitive)
        expect_true!(parse_bool("true", Err(Status::INVALID_ARGS)) == Ok(true));
        expect_true!(parse_bool("TRUE", Err(Status::INVALID_ARGS)) == Ok(true));
        expect_true!(parse_bool("on", Err(Status::INVALID_ARGS)) == Ok(true));
        expect_true!(parse_bool("ON", Err(Status::INVALID_ARGS)) == Ok(true));
        expect_true!(parse_bool("false", Err(Status::INVALID_ARGS)) == Ok(false));
        expect_true!(parse_bool("FALSE", Err(Status::INVALID_ARGS)) == Ok(false));
        expect_true!(parse_bool("off", Err(Status::INVALID_ARGS)) == Ok(false));
        expect_true!(parse_bool("OFF", Err(Status::INVALID_ARGS)) == Ok(false));

        // Numeric fallbacks (positive and negative integers)
        expect_true!(parse_bool("1", Ok((1, 1))) == Ok(true));
        expect_true!(parse_bool("42", Ok((42, 42))) == Ok(true));
        expect_true!(parse_bool("-1", Ok((0, -1))) == Ok(true));
        expect_true!(parse_bool("-100", Ok((0, -100))) == Ok(true));
        expect_true!(parse_bool("0", Ok((0, 0))) == Ok(false));

        // Invalid
        expect_true!(parse_bool("bad", Err(Status::INVALID_ARGS)) == Err(Status::INVALID_ARGS));
    }

    /// Test tokenize_command for plain tokens, quotes, variables, and semicolons
    #[test]
    fn tokenize_command_test() {
        let mut args =
            unsafe { kalloc::Box::<[CmdArgs; 16]>::try_new_zeroed().unwrap().assume_init() };
        let mut buf = unsafe { kalloc::Box::<[u8; 1024]>::try_new_zeroed().unwrap().assume_init() };
        let mut continue_slice: Option<&[u8]> = None;

        // Plain whitespace tokenization
        let input = b"cmd foo bar 123";
        let argc = tokenize_command(input, &mut continue_slice, &mut *buf, &mut *args).unwrap();
        expect_eq!(argc, 4);
        expect_true!(args[0].as_str() == "cmd");
        expect_true!(args[1].as_str() == "foo");
        expect_true!(args[2].as_str() == "bar");
        expect_true!(args[3].as_str() == "123");
        expect_eq!(args[3].arg_int, 123);
        expect_true!(continue_slice.is_none());

        // Quoted string tokens
        let input = b"cmd \"hello world\" arg3";
        let argc = tokenize_command(input, &mut continue_slice, &mut *buf, &mut *args).unwrap();
        expect_eq!(argc, 3);
        expect_true!(args[0].as_str() == "cmd");
        expect_true!(args[1].as_str() == "hello world");
        expect_true!(args[2].as_str() == "arg3");

        // Unterminated quote returns error
        let input = b"cmd \"unterminated quote";
        let res = tokenize_command(input, &mut continue_slice, &mut *buf, &mut *args);
        expect_true!(res.is_err());

        // Variable expansion to "0"
        let input = b"cmd $var_name";
        let argc = tokenize_command(input, &mut continue_slice, &mut *buf, &mut *args).unwrap();
        expect_eq!(argc, 2);
        expect_true!(args[1].as_str() == "0");

        // Semicolon separation
        let input = b"first_cmd arg1; second_cmd arg2";
        let argc = tokenize_command(input, &mut continue_slice, &mut *buf, &mut *args).unwrap();
        expect_eq!(argc, 2);
        expect_true!(args[0].as_str() == "first_cmd");
        expect_true!(args[1].as_str() == "arg1");
        expect_true!(continue_slice.is_some());
        expect_true!(continue_slice.unwrap() == b" second_cmd arg2");

        // Empty buffer safely returns 0
        let mut empty_buf = [0u8; 0];
        let mut args_empty = [CmdArgs::default(); 4];
        let res = tokenize_command(b"test", &mut continue_slice, &mut empty_buf, &mut args_empty);
        expect_true!(res == Ok(0));

        // Tight buffer guarantees null termination without overrun
        let mut tight_buf = [0xffu8; 6];
        let mut args_tight = [CmdArgs::default(); 4];
        let argc =
            tokenize_command(b"hello world", &mut continue_slice, &mut tight_buf, &mut args_tight)
                .unwrap();
        expect_true!(argc >= 1);
        expect_true!(tight_buf[tight_buf.len() - 1] == 0);
        // Reading as_str() on the truncated token is safe and properly null-terminated
        let _ = args_tight[0].as_str();
    }

    /// Test executing multiple commands across semicolons and newlines, including quoted semicolons
    #[test]
    fn console_run_script_multiline_test() {
        MOCK_CALL_COUNT.store(0, Ordering::Relaxed);
        let res =
            console_run_script_locked("mock_success \"arg1;arg2\"; mock_success\nmock_success");
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_eq!(MOCK_CALL_COUNT.load(Ordering::Relaxed), 3);
    }

    /// Test panic command availability filtering
    #[test]
    fn panic_command_availability_test() {
        // In normal mode:
        expect_true!(match_command("mock_success", CMD_AVAIL_NORMAL).is_some());
        expect_true!(match_command("mock_avail_always", CMD_AVAIL_NORMAL).is_some());
        expect_true!(match_command("mock_avail_panic", CMD_AVAIL_NORMAL).is_none());

        // In panic mode:
        expect_true!(match_command("mock_success", CMD_AVAIL_PANIC).is_none());
        expect_true!(match_command("mock_avail_always", CMD_AVAIL_PANIC).is_some());
        expect_true!(match_command("mock_avail_panic", CMD_AVAIL_PANIC).is_some());
    }

    /// Test panic command callback execution with CMD_FLAG_PANIC
    #[test]
    fn panic_command_execution_test() {
        MOCK_PANIC_FLAGS.store(0, Ordering::Relaxed);
        let cmd = match_command("mock_avail_panic", CMD_AVAIL_PANIC);
        expect_true!(cmd.is_some());
        let cb = cmd.unwrap().cmd_callback;
        let res = unsafe { cb(1, core::ptr::null(), CMD_FLAG_PANIC) };
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_eq!(MOCK_PANIC_FLAGS.load(Ordering::Relaxed), CMD_FLAG_PANIC as i32);
    }

    // Verifies that the test command is registered correctly from the Rust side.
    // We must expose this via FFI to ensure the C++ tests runner links this file.
    // The alternative would be to move these tests directly into
    // `zircon/kernel/lib/console/rust/src/lib.rs`, but keeping them in a separate
    // test crate avoids cluttering the main library with mocks and helpers.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn command_visibility_from_rust_test() -> bool {
        unsafe {
            let start = &__start_commands as *const Cmd;
            let stop = &__stop_commands as *const Cmd;
            let len = stop.offset_from(start) as usize;
            let commands = core::slice::from_raw_parts(start, len);

            for cmd in commands {
                if cmd.name() == "mock_success" {
                    if cmd.help() != "mock_success help" {
                        return false;
                    }
                    if cmd.availability_mask != CMD_AVAIL_NORMAL {
                        return false;
                    }
                    return true;
                }
            }
        }
        false
    }
}

/// Tests for the kernel console with history enabled.
#[cfg(all(console_enabled, ktest))]
#[cfg(feature = "console_enable_history")]
#[unittest::suite(name = "console_rust_history_enabled")]
mod console_history_enabled_tests {
    use crate::console_rust::console::{
        add_history, console_init_history, match_command, next_history, prev_history,
    };
    use unittest::{expect_eq, expect_true};

    /// Test history by running back and forth through it.
    #[test]
    fn history_test() {
        console_init_history();
        add_history("test line 1");
        add_history("test line 2");
        add_history("test line 3");

        let mut buf = [0u8; 128];

        // Test the cursor by moving backwards.
        let mut cursor = None;

        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"test line 3");

        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"test line 2");

        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"test line 1");

        // Now go forward.
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"test line 2");

        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"test line 3");

        // Next should hit the end (returns empty string).
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"");

        // Now test the `history` command callback.
        let cmd = match_command("history", 0xff);
        expect_true!(cmd.is_some());
        let cb = cmd.unwrap().cmd_callback;

        let (res, output) = capture_output_helper(|| unsafe { cb(1, core::ptr::null(), 0) });
        expect_eq!(res, zx_status::sys::ZX_OK);
        expect_true!(output == c"command history:\n\ttest line 3\n\ttest line 2\n\ttest line 1\n");
    }

    /// Test history duplicate suppression and empty string rejection
    #[test]
    fn history_dedup_and_empty_test() {
        console_init_history();
        add_history("");
        add_history("cmd_unique_1");
        add_history("cmd_unique_1"); // Consecutive duplicate should be rejected
        add_history("cmd_unique_2");

        let mut buf = [0u8; 128];
        let mut cursor = None;
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_unique_2");
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_unique_1");
        // Head reached
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_unique_1");
    }

    /// Test history ring buffer wraparound when adding more than 16 entries
    #[test]
    fn history_ring_buffer_overflow_test() {
        console_init_history();
        add_history("line 1");
        add_history("line 2");
        add_history("line 3");
        add_history("line 4");
        add_history("line 5");
        add_history("line 6");
        add_history("line 7");
        add_history("line 8");
        add_history("line 9");
        add_history("line 10");
        add_history("line 11");
        add_history("line 12");
        add_history("line 13");
        add_history("line 14");
        add_history("line 15");
        add_history("line 16");
        add_history("line 17");
        add_history("line 18");

        let expected_lines: [&[u8]; 16] = [
            b"line 18", b"line 17", b"line 16", b"line 15", b"line 14", b"line 13", b"line 12",
            b"line 11", b"line 10", b"line 9", b"line 8", b"line 7", b"line 6", b"line 5",
            b"line 4", b"line 3",
        ];
        let mut buf = [0u8; 128];
        let mut cursor = None;
        for expected in expected_lines {
            let len = prev_history(&mut cursor, &mut buf);
            expect_true!(&buf[..len] == expected);
        }

        // Further UP keypresses at the oldest entry must remain clamped at "line 3" without wrapping
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"line 3");
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"line 3");

        // Test max length history line clamped to LINE_LEN - 1 (127 bytes)
        let long_line = "1234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890";
        add_history(long_line);
        let mut cursor = None;
        let len = prev_history(&mut cursor, &mut buf);
        expect_eq!(len, 127);
        expect_true!(buf[..len] == long_line.as_bytes()[..127]);
    }

    /// Test alternating UP and DOWN arrow keys immediately visits adjacent entries.
    #[test]
    fn history_direction_switch_test() {
        console_init_history();
        add_history("cmd_1");
        add_history("cmd_2");
        add_history("cmd_3");

        let mut buf = [0u8; 128];
        let mut cursor = None;

        // DOWN at prompt returns empty
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"");

        // UP moves to latest
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_3");
        // UP moves to older
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_2");

        // Switching direction to DOWN immediately returns newer without duplicate
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_3");

        // Switching direction to UP immediately returns older without duplicate
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_2");

        // UP moves to oldest
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_1");
        // UP at oldest stays at oldest
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_1");

        // DOWN moves towards newer
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_2");
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_3");

        // DOWN at newest returns to prompt (empty)
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"");
        // DOWN at prompt stays at prompt
        let len = next_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"");

        // UP from prompt immediately returns newest
        let len = prev_history(&mut cursor, &mut buf);
        expect_true!(&buf[..len] == b"cmd_3");
    }
}

/// Tests for the kernel console with history disabled.
#[cfg(all(console_enabled, ktest))]
#[cfg(not(feature = "console_enable_history"))]
#[unittest::suite(name = "console_rust_history_disabled")]
mod console_history_disabled_tests {
    use crate::console_rust::console::match_command;
    use unittest::expect_true;

    /// Test history safety and graceful handling when history is disabled at build time.
    #[test]
    fn history_disabled_test() {
        // Executing match_command for "history" should return None when disabled.
        let cmd = match_command("history", 0xff);
        expect_true!(cmd.is_none());
    }
}
