// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use console_env as _;

#[cfg(console_enabled)]
pub mod console {
    use console_env::BOOT_TEST_SUCCESS_STRING;
    use core::ffi::{c_char, c_int, c_void};
    use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    #[cfg(feature = "console_enable_history")]
    use core::sync::atomic::{AtomicU8, AtomicUsize};
    use kprint::{kprint, kprintln};
    use zx_status::Status;

    pub const CMD_AVAIL_NORMAL: u8 = 1 << 0;
    pub const CMD_AVAIL_PANIC: u8 = 1 << 1;
    pub const CMD_AVAIL_ALWAYS: u8 = CMD_AVAIL_NORMAL | CMD_AVAIL_PANIC;

    // Command is happening at crash time.
    pub const CMD_FLAG_PANIC: u32 = 1 << 0;

    pub static ECHO: AtomicBool = AtomicBool::new(true);
    pub static EXIT_CONSOLE: AtomicBool = AtomicBool::new(false);
    pub static LAST_RESULT: AtomicI32 = AtomicI32::new(0);

    const BACKSPACE_DEL: u8 = 0x7f;
    const BACKSPACE_BS: u8 = 0x08;
    const ESCAPE: u8 = 0x1b;
    const RIGHT_ARROW: u8 = 0x43;
    const LEFT_ARROW: u8 = 0x44;
    #[cfg(feature = "console_enable_history")]
    const UP_ARROW: u8 = 0x41;
    #[cfg(feature = "console_enable_history")]
    const DOWN_ARROW: u8 = 0x42;

    #[cfg(feature = "console_enable_history")]
    const HISTORY_LEN: usize = 16;
    const LINE_LEN: usize = 128;
    const MAX_NUM_ARGS: usize = 16;
    const PANIC_LINE_LEN: usize = 32;

    #[cfg(feature = "console_enable_history")]
    const HISTORY_BUF_SIZE: usize = HISTORY_LEN * LINE_LEN;
    #[cfg(feature = "console_enable_history")]
    static HISTORY_BUF: [AtomicU8; HISTORY_BUF_SIZE] =
        [const { AtomicU8::new(0) }; HISTORY_BUF_SIZE];
    #[cfg(feature = "console_enable_history")]
    static HISTORY_NEXT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct CmdArgs {
        pub arg_str: *const c_char,
        pub arg_uint: u64,
        pub arg_ptr: *mut c_void,
        pub arg_int: i64,
        pub arg_bool: bool,
    }

    impl CmdArgs {
        pub fn as_str(&self) -> &str {
            if self.arg_str.is_null() {
                return "";
            }
            unsafe { core::ffi::CStr::from_ptr(self.arg_str) }.to_str().unwrap_or("")
        }
    }

    pub type CmdCallback =
        unsafe extern "C" fn(argc: c_int, argv: *const CmdArgs, flags: u32) -> c_int;

    #[repr(C)]
    pub struct Cmd {
        pub cmd_str: *const c_char,
        pub help_str: *const c_char,
        pub cmd_callback: CmdCallback,
        pub availability_mask: u8,
    }

    impl Cmd {
        pub fn name(&self) -> &str {
            if self.cmd_str.is_null() {
                return "";
            }
            unsafe { core::ffi::CStr::from_ptr(self.cmd_str) }.to_str().unwrap_or("")
        }

        pub fn help(&self) -> &str {
            if self.help_str.is_null() {
                return "";
            }
            unsafe { core::ffi::CStr::from_ptr(self.help_str) }.to_str().unwrap_or("")
        }
    }

    // Safety: Cmd structures placed in the commands section are read-only
    // after boot and safe to share between threads.
    unsafe impl Sync for Cmd {}

    // Verify size and alignment compatibility with C++ structs.
    zr::static_assert!(core::mem::size_of::<CmdArgs>() == 40);
    zr::static_assert!(core::mem::align_of::<CmdArgs>() == 8);
    zr::static_assert!(core::mem::size_of::<Cmd>() == 32);
    zr::static_assert!(core::mem::align_of::<Cmd>() == 8);

    #[macro_export]
    macro_rules! commands_section {
        () => {
            ".data.rel.ro.commands"
        };
    }
    pub(crate) use commands_section;

    #[macro_export]
    macro_rules! static_command {
        ($var_name:ident, $cmd:expr, $help:expr, $func:expr, $mask:expr) => {
            #[used]
            #[unsafe(link_section = $crate::console_rust::console::commands_section!())]
            pub static $var_name: $crate::console_rust::console::Cmd =
                $crate::console_rust::console::Cmd {
                    cmd_str: $cmd,
                    help_str: $help,
                    cmd_callback: $func,
                    availability_mask: $mask,
                };
        };
    }
    pub(crate) use static_command;

    // Helpers.
    #[cfg(feature = "console_enable_history")]
    #[inline]
    fn ptrnext(ptr: usize) -> usize {
        (ptr + 1) % HISTORY_LEN
    }

    #[cfg(feature = "console_enable_history")]
    #[inline]
    fn ptrprev(ptr: usize) -> usize {
        (ptr + HISTORY_LEN - 1) % HISTORY_LEN
    }

    /// Loads the history entry at `line_idx` into `out` and returns its length.
    #[cfg(feature = "console_enable_history")]
    #[inline]
    fn load_history_line(line_idx: usize, out: &mut [u8; LINE_LEN]) -> usize {
        let start = line_idx * LINE_LEN;
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = HISTORY_BUF[start + i].load(Ordering::Relaxed);
        }
        out.iter().position(|&b| b == 0).unwrap_or(LINE_LEN)
    }

    fn cgetchar() -> Option<u8> {
        let mut character: c_char = 0;
        let read_status = crate::platform_rs::debug::platform_dgetc(&mut character, true);
        if read_status < 0 { None } else { Some(character as u8) }
    }

    fn cputchar(character: u8) {
        crate::debuglog_rs::dlog_serial_write(core::slice::from_ref(&character));
    }

    fn cputs(string: &str) {
        crate::debuglog_rs::dlog_serial_write(string.as_bytes());
    }

    fn panic_puts(string: &str) {
        for &byte in string.as_bytes() {
            crate::platform_rs::debug::platform_pputc(byte);
        }
    }

    fn panic_getc() -> Option<u8> {
        let mut character: core::ffi::c_char = 0;
        if crate::platform_rs::debug::platform_pgetc(&mut character) < 0 {
            None
        } else {
            #[allow(clippy::unnecessary_cast)]
            Some(character as u8)
        }
    }

    // FFI imports.
    unsafe extern "C" {
        fn cpp_console_lock();
        fn cpp_console_unlock();
    }

    // Statically registered commands.
    static_command!(CMD_ECHO, c"echo".as_ptr(), core::ptr::null(), cmd_echo, CMD_AVAIL_ALWAYS);

    static_command!(
        CMD_EXIT,
        c"exit".as_ptr(),
        c"exit the command processor".as_ptr(),
        cmd_exit,
        CMD_AVAIL_NORMAL
    );

    static_command!(
        CMD_TEST,
        c"test".as_ptr(),
        c"test the command processor".as_ptr(),
        cmd_test,
        CMD_AVAIL_ALWAYS
    );

    static_command!(
        CMD_GRACEFUL_SHUTDOWN,
        c"graceful-shutdown".as_ptr(),
        c"shut the system down gracefully".as_ptr(),
        cmd_graceful_shutdown,
        CMD_AVAIL_ALWAYS
    );

    static_command!(
        CMD_BOOT_TEST_SUCCESS,
        c"boot-test-success".as_ptr(),
        c"report boot-test success".as_ptr(),
        cmd_boot_test_success,
        CMD_AVAIL_ALWAYS
    );

    static_command!(
        CMD_AND,
        c"and".as_ptr(),
        c"execute command if last command succeeded".as_ptr(),
        cmd_and,
        CMD_AVAIL_ALWAYS
    );

    static_command!(
        CMD_REPEAT,
        c"repeat".as_ptr(),
        c"execute command in a loop for N loops or until error".as_ptr(),
        cmd_repeat,
        CMD_AVAIL_ALWAYS
    );

    static_command!(CMD_HELP, c"help".as_ptr(), c"this list".as_ptr(), cmd_help, CMD_AVAIL_ALWAYS);

    #[cfg(feature = "console_enable_history")]
    static_command!(
        CMD_HISTORY,
        c"history".as_ptr(),
        c"command history".as_ptr(),
        cmd_history,
        CMD_AVAIL_ALWAYS
    );

    // Callback implementations.
    unsafe extern "C" fn cmd_echo(argc: c_int, argv: *const CmdArgs, _flags: u32) -> c_int {
        if argc < 2 {
            kprintln!("Usage: echo <true|false|on|off>");
            return Status::INVALID_ARGS.into_raw();
        }

        // SAFETY: `argv` points to `argc` initialized `CmdArgs` structures prepared by `tokenize_command`.
        let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
        let arg_str = args[1].as_str();
        let int_res = parse_c_style_int(arg_str);
        match parse_bool(arg_str, int_res) {
            Ok(val) => {
                ECHO.store(val, Ordering::Relaxed);
                zx_status::sys::ZX_OK
            }
            Err(_) => {
                kprintln!("echo: invalid argument \"{:s}\", expected boolean", arg_str);
                Status::INVALID_ARGS.into_raw()
            }
        }
    }

    unsafe extern "C" fn cmd_exit(_argc: c_int, _argv: *const CmdArgs, _flags: u32) -> c_int {
        EXIT_CONSOLE.store(true, Ordering::Relaxed);
        zx_status::sys::ZX_OK
    }

    unsafe extern "C" fn cmd_test(argc: c_int, argv: *const CmdArgs, _flags: u32) -> c_int {
        // SAFETY: `argv` points to `argc` initialized `CmdArgs` structures prepared by `tokenize_command`.
        let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
        kprintln!("argc {}, argv {:p}", argc, argv);
        for (i, arg) in args.iter().enumerate() {
            kprintln!(
                "\t{}: str '{:s}', int {}, uint {:#x}, ptr {:p}, bool {}",
                i as c_int,
                arg.as_str(),
                arg.arg_int,
                arg.arg_uint,
                arg.arg_ptr,
                arg.arg_bool,
            );
        }
        zx_status::sys::ZX_OK
    }

    unsafe extern "C" fn cmd_graceful_shutdown(
        _argc: c_int,
        _argv: *const CmdArgs,
        _flags: u32,
    ) -> c_int {
        kprintln!("*** Performing graceful shutdown from kernel shell... ***");
        const ZX_SEC_10: i64 = 10 * 1_000_000_000;
        let dlog_deadline = crate::platform_rs::timer::current_mono_time() + ZX_SEC_10;
        if let Err(status) = crate::debuglog_rs::dlog_shutdown(dlog_deadline) {
            kprintln!("debuglog shutdown failed: {}", status.into_raw());
            // Proceed to platform_halt() even if debuglog_rs::dlog_shutdown() fails.
        }
        // Does not return.
        crate::platform_rs::power::platform_halt(
            crate::platform_rs::power::PlatformHaltAction::Shutdown,
            crate::platform_rs::power::ZirconCrashReason::NoCrash,
        );
    }

    unsafe extern "C" fn cmd_boot_test_success(
        _argc: c_int,
        _argv: *const CmdArgs,
        _flags: u32,
    ) -> c_int {
        let last = LAST_RESULT.load(Ordering::Relaxed);
        kprintln!("*** Last script command result: {} ***", last);
        if last == 0 {
            kprintln!("{:s}", BOOT_TEST_SUCCESS_STRING);
        }
        last
    }

    unsafe fn get_commands() -> &'static [Cmd] {
        unsafe extern "C" {
            #[link_name = "__start_commands"]
            static __start_commands: Cmd;
            #[link_name = "__stop_commands"]
            static __stop_commands: Cmd;
        }
        let start = unsafe { &__start_commands as *const Cmd };
        let stop = unsafe { &__stop_commands as *const Cmd };
        let count = (stop as usize - start as usize) / core::mem::size_of::<Cmd>();
        // SAFETY: The linker script defines `__start_commands` and `__stop_commands` bounding a contiguous array of `Cmd` structs.
        unsafe { core::slice::from_raw_parts(start, count) }
    }

    pub fn match_command(name: &str, availability_mask: u8) -> Option<&'static Cmd> {
        // SAFETY: `get_commands()` returns a static slice of registered commands from the linker section.
        let commands = unsafe { get_commands() };
        commands
            .iter()
            .find(|&cmd| (availability_mask & cmd.availability_mask) != 0 && cmd.name() == name)
    }

    unsafe extern "C" fn cmd_and(argc: c_int, argv: *const CmdArgs, flags: u32) -> c_int {
        if argc < 2 {
            kprintln!("Usage: and COMMAND...");
            return Status::INVALID_ARGS.into_raw();
        }

        let last = LAST_RESULT.load(Ordering::Relaxed);
        if last != zx_status::sys::ZX_OK {
            return last;
        }

        // SAFETY: `argv` points to `argc` initialized `CmdArgs` structures.
        let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
        let cmd = match match_command(args[1].as_str(), CMD_AVAIL_NORMAL) {
            Some(cmd) => cmd,
            None => {
                kprintln!("command \"{:s}\" not found", args[1].as_str());
                return Status::NOT_FOUND.into_raw();
            }
        };

        // SAFETY: `argv.add(1)` points to `argc - 1` remaining `CmdArgs` elements, valid for the sub-command callback.
        unsafe { (cmd.cmd_callback)(argc - 1, argv.add(1), flags) }
    }

    unsafe extern "C" fn cmd_repeat(argc: c_int, argv: *const CmdArgs, flags: u32) -> c_int {
        const MIN_ARGS: c_int = 3;
        if argc < MIN_ARGS {
            kprintln!("Usage: repeat <iterations | -1> COMMAND...");
            return Status::INVALID_ARGS.into_raw();
        }

        // SAFETY: `argv` points to `argc` initialized `CmdArgs` structures.
        let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
        let cmd = match match_command(args[2].as_str(), CMD_AVAIL_NORMAL) {
            Some(cmd) => cmd,
            None => {
                kprintln!("command \"{:s}\" not found", args[2].as_str());
                return Status::NOT_FOUND.into_raw();
            }
        };

        // Negative arguments will cause it to effectively loop forever
        let iterations = if args[1].arg_int >= 0 { args[1].arg_uint as usize } else { usize::MAX };
        for i in 0..iterations {
            if iterations == usize::MAX {
                kprint!("repeat ({}): {:s}", i + 1, args[2].as_str());
            } else {
                kprint!("repeat ({}/{}): {:s}", i + 1, iterations, args[2].as_str());
            }
            for arg in MIN_ARGS..argc {
                kprint!(" {:s}", args[arg as usize].as_str());
            }
            kprintln!("");

            // SAFETY: `argv.add(2)` points to `argc - 2` remaining `CmdArgs` elements, valid for the repeated callback.
            let err = unsafe { (cmd.cmd_callback)(argc - 2, argv.add(2), flags) };
            if err != zx_status::sys::ZX_OK {
                kprintln!("stopping repeat due to nonzero status {}", err);
                return err;
            }
        }

        zx_status::sys::ZX_OK
    }

    unsafe extern "C" fn cmd_help(_argc: c_int, _argv: *const CmdArgs, flags: u32) -> c_int {
        let commands = unsafe { get_commands() };
        let count = commands.len();

        // Filter out commands based on if we're called at normal or panic time.
        let availability_mask =
            if (flags & CMD_FLAG_PANIC) != 0 { CMD_AVAIL_PANIC } else { CMD_AVAIL_NORMAL };

        kprintln!("command list:");

        // If we're not panicking (and are free to allocate memory), sort the
        // commands alphabetically before printing.
        if (flags & CMD_FLAG_PANIC) != 0 {
            for cmd in commands {
                if (availability_mask & cmd.availability_mask) != 0 && !cmd.help_str.is_null() {
                    kprintln!("\t{:<16s}: {:s}", cmd.name(), cmd.help());
                }
            }
        } else {
            let mut ptrs_slice = match kalloc::Box::<[*const Cmd]>::try_new_zeroed_slice(count) {
                Ok(s) => s,
                Err(_) => return Status::NO_MEMORY.into_raw(),
            };
            for (i, cmd) in commands.iter().enumerate() {
                ptrs_slice[i] = cmd;
            }

            ptrs_slice.sort_unstable_by(|&a, &b| {
                let a_cmd = unsafe { &*a };
                let b_cmd = unsafe { &*b };
                a_cmd.name().cmp(b_cmd.name())
            });

            for &cmd_ptr in ptrs_slice.iter() {
                let cmd = unsafe { &*cmd_ptr };
                if (availability_mask & cmd.availability_mask) != 0 && !cmd.help_str.is_null() {
                    kprintln!("\t{:<16s}: {:s}", cmd.name(), cmd.help());
                }
            }
        }

        zx_status::sys::ZX_OK
    }

    #[cfg(feature = "console_enable_history")]
    unsafe extern "C" fn cmd_history(_argc: c_int, _argv: *const CmdArgs, _flags: u32) -> c_int {
        kprintln!("command history:");

        // In C++, the history buffer was accessed using raw pointer arithmetic. In Rust, we
        // iterate through the ring buffer backwards starting from the latest entry, loading
        // each line into a stack buffer and printing non-empty entries.
        let history_next = HISTORY_NEXT.load(Ordering::Relaxed);
        let mut ptr = ptrprev(history_next);
        let mut line_buf = match kalloc::Box::<[u8; LINE_LEN]>::try_new_zeroed() {
            Ok(b) => unsafe { b.assume_init() },
            Err(_) => return Status::NO_MEMORY.into_raw(),
        };
        for _ in 0..HISTORY_LEN {
            let len = load_history_line(ptr, &mut line_buf);
            if len > 0
                && let Ok(s) = core::str::from_utf8(&line_buf[..len])
            {
                kprintln!("\t{:s}", s);
            }
            ptr = ptrprev(ptr);
        }

        zx_status::sys::ZX_OK
    }

    /// # Concurrency Notes
    ///
    /// While `HISTORY_BUF` and `HISTORY_NEXT` use atomic operations to ensure safe visibility
    /// across threads without data races, individual byte writes in `add_history` are performed
    /// with `Ordering::Relaxed` without external locks. This preserves the 1:1 concurrency model
    /// of the original console, which assumes a single active interactive console loop or serial
    /// input stream. If concurrent multi-writer console lines are introduced in the future,
    /// writing to and traversing history would require external synchronization.
    #[cfg(feature = "console_enable_history")]
    pub fn add_history(line: &str) {
        // Reject empty lines.
        if line.is_empty() {
            return;
        }

        let history_next = HISTORY_NEXT.load(Ordering::Relaxed);
        let last = ptrprev(history_next);

        let mut prev_buf = match kalloc::Box::<[u8; LINE_LEN]>::try_new_zeroed() {
            Ok(b) => unsafe { b.assume_init() },
            Err(_) => return, // if OOM, just give up saving history
        };
        let prev_len = load_history_line(last, &mut prev_buf);
        if &prev_buf[..prev_len] == line.as_bytes() {
            // Don't store duplicate commands in history.
            return;
        }

        // Compute next line start index and store.
        let next_start = history_next * LINE_LEN;
        let bytes = line.as_bytes();
        let copy_len = core::cmp::min(bytes.len(), LINE_LEN - 1);
        for i in 0..copy_len {
            HISTORY_BUF[next_start + i].store(bytes[i], Ordering::Relaxed);
        }
        HISTORY_BUF[next_start + copy_len].store(0, Ordering::Relaxed);

        HISTORY_NEXT.store(ptrnext(history_next), Ordering::Relaxed);
    }

    /// Move to the next (newer) command in history.
    ///
    /// If the cursor is already at the prompt (`None`), returns empty (0).
    /// When stepping past the newest entry, resets the cursor to `None` (the prompt) and returns empty (0).
    #[cfg(feature = "console_enable_history")]
    pub fn next_history(cursor: &mut Option<usize>, out: &mut [u8; LINE_LEN]) -> usize {
        let Some(curr) = *cursor else {
            // Already at the prompt / bottom.
            return 0;
        };

        let history_next = HISTORY_NEXT.load(Ordering::Relaxed);
        let next = ptrnext(curr % HISTORY_LEN);

        // If we reach the head or an empty slot, return to the prompt.
        if next == history_next || HISTORY_BUF[next * LINE_LEN].load(Ordering::Relaxed) == 0 {
            *cursor = None;
            0
        } else {
            *cursor = Some(next);
            load_history_line(next, out)
        }
    }

    /// Move to the previous (older) command in history.
    ///
    /// If starting from the prompt (`None`), steps to the newest active command.
    /// Subsequent calls step backward until reaching the oldest entry, where it remains.
    #[cfg(feature = "console_enable_history")]
    pub fn prev_history(cursor: &mut Option<usize>, out: &mut [u8; LINE_LEN]) -> usize {
        let history_next = HISTORY_NEXT.load(Ordering::Relaxed);
        let target = match *cursor {
            None => {
                // Starting from the prompt: load the newest active entry.
                let last = ptrprev(history_next);
                if HISTORY_BUF[last * LINE_LEN].load(Ordering::Relaxed) == 0 {
                    // History is empty.
                    return 0;
                }
                last
            }
            Some(curr) => {
                // Step back one entry in the ring buffer if not already at the oldest entry.
                let curr_idx = curr % HISTORY_LEN;
                let prev = ptrprev(curr_idx);
                if curr_idx == history_next
                    || HISTORY_BUF[prev * LINE_LEN].load(Ordering::Relaxed) == 0
                {
                    // Reached the oldest entry, so keep current.
                    curr_idx
                } else {
                    prev
                }
            }
        };

        *cursor = Some(target);
        load_history_line(target, out)
    }

    pub fn parse_c_style_int(input_str: &str) -> Result<(u64, i64), zx_status::Status> {
        let input_str = input_str.trim();
        let is_neg = input_str.starts_with('-');
        let abs_str = if is_neg {
            input_str.strip_prefix('-').unwrap_or(input_str)
        } else {
            input_str.strip_prefix('+').unwrap_or(input_str)
        };

        let (base, digits) =
            if let Some(hex) = abs_str.strip_prefix("0x").or_else(|| abs_str.strip_prefix("0X")) {
                (16, hex)
            } else if abs_str.starts_with('0') && abs_str.len() > 1 {
                (8, &abs_str[1..])
            } else {
                (10, abs_str)
            };

        if digits.is_empty() || digits.starts_with('+') || digits.starts_with('-') {
            return Err(zx_status::Status::INVALID_ARGS);
        }

        let (unsigned_val, signed_val) = if is_neg {
            let val =
                u64::from_str_radix(digits, base).map_err(|_| zx_status::Status::INVALID_ARGS)?;
            if val > (1u64 << 63) {
                return Err(zx_status::Status::INVALID_ARGS);
            }
            (0, val.wrapping_neg() as i64)
        } else {
            let val =
                u64::from_str_radix(digits, base).map_err(|_| zx_status::Status::INVALID_ARGS)?;
            (val, val as i64)
        };

        Ok((unsigned_val, signed_val))
    }

    pub fn parse_bool(
        input_str: &str,
        int_result: Result<(u64, i64), zx_status::Status>,
    ) -> Result<bool, zx_status::Status> {
        if input_str.eq_ignore_ascii_case("true") || input_str.eq_ignore_ascii_case("on") {
            Ok(true)
        } else if input_str.eq_ignore_ascii_case("false") || input_str.eq_ignore_ascii_case("off") {
            Ok(false)
        } else {
            match int_result {
                Ok((unsigned_val, signed_val)) => Ok(unsigned_val != 0 || signed_val != 0),
                Err(e) => Err(e),
            }
        }
    }

    #[allow(clippy::result_unit_err)]
    pub fn tokenize_command<'a>(
        in_str: &'a [u8],
        continue_slice: &mut Option<&'a [u8]>,
        buf: &mut [u8],
        args: &mut [CmdArgs],
    ) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut in_pos = 0;
        let mut out_pos = 0;
        let mut arg_index = 0;

        #[derive(PartialEq)]
        enum State {
            Initial,
            NextField,
            Space,
            InSpace,
            Token,
            InToken,
            QuotedToken,
            InQuotedToken,
            Var,
            InVar,
            CommandSep,
        }

        let mut state = State::Initial;
        *continue_slice = None;

        let arg_count = args.len();
        // Leave space for null terminator.
        let max_payload_len = buf.len() - 1;

        while in_pos <= in_str.len() {
            let byte = if in_pos < in_str.len() { in_str[in_pos] } else { 0 };

            match state {
                State::Initial | State::NextField => {
                    if byte == 0 {
                        break;
                    }
                    if byte.is_ascii_whitespace() {
                        state = State::Space;
                    } else if byte == b';' {
                        state = State::CommandSep;
                    } else {
                        state = State::Token;
                    }
                }
                State::Space => {
                    state = State::InSpace;
                }
                State::InSpace => {
                    if byte == 0 {
                        break;
                    }
                    if byte == b';' {
                        state = State::CommandSep;
                    } else if !byte.is_ascii_whitespace() {
                        state = State::Token;
                    } else {
                        in_pos += 1;
                    }
                }
                State::Token => {
                    if byte == b'"' {
                        state = State::QuotedToken;
                    } else if byte == b'$' {
                        state = State::Var;
                    } else {
                        state = State::InToken;
                        if arg_index < arg_count && out_pos < max_payload_len {
                            args[arg_index].arg_str = buf[out_pos..].as_ptr() as *const c_char;
                        }
                    }
                }
                State::InToken => {
                    if byte == 0 {
                        arg_index += 1;
                        break;
                    }
                    if byte.is_ascii_whitespace() || byte == b';' {
                        arg_index += 1;
                        if out_pos < buf.len() {
                            buf[out_pos] = 0;
                            out_pos += 1;
                        }
                        if arg_index == arg_count {
                            break;
                        }
                        state = State::NextField;
                    } else {
                        if out_pos < max_payload_len {
                            buf[out_pos] = byte;
                            out_pos += 1;
                        }
                        in_pos += 1;
                    }
                }
                State::QuotedToken => {
                    state = State::InQuotedToken;
                    if arg_index < arg_count && out_pos < max_payload_len {
                        args[arg_index].arg_str = buf[out_pos..].as_ptr() as *const c_char;
                    }
                    in_pos += 1;
                }
                State::InQuotedToken => {
                    if byte == 0 {
                        return Err(());
                    }
                    if byte == b'"' {
                        arg_index += 1;
                        if out_pos < buf.len() {
                            buf[out_pos] = 0;
                            out_pos += 1;
                        }
                        if arg_index == arg_count {
                            break;
                        }
                        state = State::NextField;
                        in_pos += 1;
                    } else {
                        if out_pos < max_payload_len {
                            buf[out_pos] = byte;
                            out_pos += 1;
                        }
                        in_pos += 1;
                    }
                }
                State::Var => {
                    state = State::InVar;
                    if arg_index < arg_count && out_pos < max_payload_len {
                        args[arg_index].arg_str = buf[out_pos..].as_ptr() as *const c_char;
                    }
                    in_pos += 1;
                }
                State::InVar => {
                    if byte == 0 || byte.is_ascii_whitespace() || byte == b';' {
                        if out_pos < max_payload_len {
                            buf[out_pos] = b'0';
                            out_pos += 1;
                        }
                        if out_pos < buf.len() {
                            buf[out_pos] = 0;
                            out_pos += 1;
                        }
                        arg_index += 1;
                        if arg_index == arg_count {
                            break;
                        }
                        state = State::NextField;
                    } else {
                        in_pos += 1;
                    }
                }
                State::CommandSep => {
                    in_pos += 1;
                    *continue_slice = Some(&in_str[in_pos..]);
                    break;
                }
            }
        }

        if out_pos < buf.len() {
            buf[out_pos] = 0;
        }
        if let Some(last) = buf.last_mut() {
            // Null terminator.
            *last = 0;
        }

        convert_args(args, arg_index);

        Ok(arg_index)
    }

    fn convert_args(args: &mut [CmdArgs], count: usize) {
        for i in 0..count {
            if i >= args.len() {
                break;
            }
            if args[i].arg_str.is_null() {
                continue;
            }
            let arg_str = args[i].as_str();

            let int_result = parse_c_style_int(arg_str);
            let bool_val = parse_bool(arg_str, int_result).unwrap_or(false);
            let (unsigned_val, signed_val) = int_result.unwrap_or((0, 0));

            args[i].arg_uint = unsigned_val;
            args[i].arg_ptr =
                core::ptr::with_exposed_provenance_mut::<c_void>(unsigned_val as usize);
            args[i].arg_int = signed_val;
            args[i].arg_bool = bool_val;
        }
    }

    pub fn command_loop<F>(
        mut get_line: Option<F>,
        showprompt: bool,
        locked: bool,
    ) -> Result<(), zx_status::Status>
    where
        F: FnMut(&mut [u8; LINE_LEN]) -> Option<usize>,
    {
        let mut exit = false;
        let mut ret = Ok(());

        // Allocate large buffers on the heap to avoid kernel stack overflows.
        const MAX_NUM_ARGS: usize = 16;
        let mut args = match kalloc::Box::<[CmdArgs; MAX_NUM_ARGS]>::try_new_zeroed() {
            Ok(b) => unsafe { b.assume_init() },
            Err(_) => return Err(zx_status::Status::NO_MEMORY),
        };

        const OUTBUFLEN: usize = 1024;
        let mut outbuf = match kalloc::Box::<[u8; OUTBUFLEN]>::try_new_zeroed() {
            Ok(b) => unsafe { b.assume_init() },
            Err(_) => return Err(zx_status::Status::NO_MEMORY),
        };

        let mut continue_offset: Option<usize> = None;
        let mut current_len = 0;

        let mut line_buf = match kalloc::Box::<[u8; LINE_LEN]>::try_new_zeroed() {
            Ok(b) => unsafe { b.assume_init() },
            Err(_) => return Err(zx_status::Status::NO_MEMORY),
        };

        while !exit {
            let buffer_slice: &[u8] = if let Some(offset) = continue_offset {
                &line_buf[offset..current_len]
            } else {
                if showprompt {
                    // Drain any pending debuglog output before drawing the prompt.
                    crate::debuglog_rs::dlog_sync();
                    cputs("] ");
                }

                if let Some(ref mut gl) = get_line {
                    if let Some(len) = gl(&mut line_buf) {
                        if len == 0 {
                            continue;
                        }
                        current_len = len;
                        &line_buf[..len]
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            };

            let mut continue_slice: Option<&[u8]> = None;
            let argc_res =
                tokenize_command(buffer_slice, &mut continue_slice, &mut *outbuf, &mut *args);

            if let Some(slice) = continue_slice {
                let offset = slice.as_ptr() as usize - line_buf.as_ptr() as usize;
                continue_offset = Some(offset);
            } else {
                continue_offset = None;
            }

            let argc = match argc_res {
                Ok(c) => c,
                Err(_) => {
                    if showprompt {
                        kprintln!("syntax error");
                    }
                    continue;
                }
            };

            if argc == 0 {
                continue;
            }

            let Some(command) = match_command(args[0].as_str(), CMD_AVAIL_NORMAL) else {
                kprintln!("command \"{:s}\" not found", args[0].as_str());
                LAST_RESULT.store(Status::NOT_FOUND.into_raw(), Ordering::Relaxed);
                continue;
            };

            if !locked {
                // SAFETY: FFI call to acquire the kernel command lock before running the command callback.
                unsafe { cpp_console_lock() };
            }

            EXIT_CONSOLE.store(false, Ordering::Relaxed);

            // SAFETY: `args.as_mut_ptr()` points to an array of `argc` valid `CmdArgs` elements initialized by `tokenize_command`.
            let last = unsafe { (command.cmd_callback)(argc as c_int, args.as_mut_ptr(), 0) };
            LAST_RESULT.store(last, Ordering::Relaxed);

            if EXIT_CONSOLE.load(Ordering::Relaxed) {
                exit = true;
                EXIT_CONSOLE.store(false, Ordering::Relaxed);
                ret = Err(zx_status::Status::CANCELED);
            }

            if !locked {
                // SAFETY: FFI call to release the kernel command lock after running the command callback.
                unsafe { cpp_console_unlock() };
            }
        }

        ret
    }

    struct LineReadStruct<'a> {
        script_bytes: &'a [u8],
        pos: usize,
    }

    fn fetch_next_line(
        line_read: &mut LineReadStruct<'_>,
        buffer: &mut [u8; LINE_LEN],
    ) -> Option<usize> {
        if line_read.pos >= line_read.script_bytes.len() {
            return None;
        }

        let mut len = 0;
        while line_read.pos < line_read.script_bytes.len() {
            let byte = line_read.script_bytes[line_read.pos];
            // Semicolon splitting is handled by tokenize_command() to support quoted semicolons.
            if byte == 0 || byte == b'\n' {
                line_read.pos += 1;
                break;
            }
            buffer[len] = byte;
            len += 1;
            line_read.pos += 1;
            if len >= LINE_LEN - 1 {
                break;
            }
        }

        buffer[len] = 0;
        #[cfg(feature = "console_enable_history")]
        {
            if let Ok(line_str) = core::str::from_utf8(&buffer[..len]) {
                add_history(line_str);
            }
        }

        Some(len)
    }

    fn console_run_script_etc(script: &str, locked: bool) -> i32 {
        let mut line_read = LineReadStruct { script_bytes: script.as_bytes(), pos: 0 };

        let _ = command_loop(
            Some(|buf: &mut [u8; LINE_LEN]| fetch_next_line(&mut line_read, buf)),
            false,
            locked,
        );

        LAST_RESULT.load(Ordering::Relaxed)
    }

    pub fn console_run_script(script: &str) -> i32 {
        console_run_script_etc(script, false)
    }

    pub fn console_run_script_locked(script: &str) -> i32 {
        console_run_script_etc(script, true)
    }

    pub fn read_debug_line(buffer: &mut [u8; LINE_LEN]) -> Option<usize> {
        let mut pos = 0;
        let mut escape_level = 0;
        #[cfg(feature = "console_enable_history")]
        let mut history_cursor: Option<usize> = None;

        loop {
            let Some(character) = cgetchar() else {
                continue;
            };

            let echo_enabled = ECHO.load(Ordering::Relaxed);

            if escape_level == 0 {
                match character {
                    b'\r' | b'\n' => {
                        if echo_enabled {
                            cputchar(b'\n');
                        }
                        break;
                    }
                    BACKSPACE_DEL | BACKSPACE_BS => {
                        if pos > 0 {
                            pos -= 1;
                            cputs("\x08 \x08");
                        }
                    }
                    ESCAPE => {
                        escape_level += 1;
                    }
                    _ => {
                        buffer[pos] = character;
                        pos += 1;
                        if echo_enabled {
                            cputchar(character);
                        }
                    }
                }
            } else if escape_level == 1 {
                if character == b'[' {
                    escape_level += 1;
                } else {
                    escape_level = 0;
                }
            } else {
                match character {
                    RIGHT_ARROW => {
                        buffer[pos] = b' ';
                        pos += 1;
                        if echo_enabled {
                            cputchar(b' ');
                        }
                    }
                    LEFT_ARROW if pos > 0 => {
                        pos -= 1;
                        if echo_enabled {
                            cputs("\x08 \x08");
                        }
                    }
                    #[cfg(feature = "console_enable_history")]
                    UP_ARROW | DOWN_ARROW => {
                        while pos > 0 {
                            pos -= 1;
                            if echo_enabled {
                                cputs("\x08 \x08");
                            }
                        }

                        pos = if character == UP_ARROW {
                            prev_history(&mut history_cursor, buffer)
                        } else {
                            next_history(&mut history_cursor, buffer)
                        };

                        pos = core::cmp::min(pos, LINE_LEN - 1);
                        buffer[pos] = 0;

                        if echo_enabled && let Ok(s) = core::str::from_utf8(&buffer[..pos]) {
                            cputs(s);
                        }
                    }
                    _ => {}
                }
                escape_level = 0;
            }

            // Buffer holds up to LINE_LEN - 1 characters plus null terminator.
            if pos == LINE_LEN {
                cputs("\nerror: line too long\n");
                pos = 0;
                break;
            }
        }

        buffer[pos] = 0;
        #[cfg(feature = "console_enable_history")]
        {
            if let Ok(line_str) = core::str::from_utf8(&buffer[..pos]) {
                add_history(line_str);
            }
        }

        Some(pos)
    }

    fn read_line_panic(buffer: &mut [u8]) -> &mut [u8] {
        let len = buffer.len();
        let mut pos = 0;
        loop {
            let Some(character) = panic_getc() else {
                continue;
            };

            match character {
                b'\r' | b'\n' => {
                    crate::platform_rs::debug::platform_pputc(b'\n');
                    break;
                }
                BACKSPACE_DEL | BACKSPACE_BS => {
                    if pos > 0 {
                        pos -= 1;
                        panic_puts("\x08 \x08");
                    }
                }
                _ => {
                    buffer[pos] = character;
                    pos += 1;
                    crate::platform_rs::debug::platform_pputc(character);
                }
            }

            // Buffer holds up to len - 1 characters plus null terminator.
            if pos == len {
                panic_puts("\nerror: line too long\n");
                pos = 0;
                break;
            }
        }

        buffer[pos] = 0;
        &mut buffer[..pos]
    }

    #[cfg(feature = "console_enable_history")]
    pub fn console_init_history() {
        for byte in &HISTORY_BUF {
            byte.store(0, Ordering::Relaxed);
        }
        HISTORY_NEXT.store(0, Ordering::Relaxed);
    }

    #[cfg(not(feature = "console_enable_history"))]
    pub fn console_init_history() {}

    pub fn console_start() {
        kprintln!("entering main console loop\n");
        while command_loop(Some(|buf: &mut [u8; LINE_LEN]| read_debug_line(buf)), true, false)
            == Ok(())
        {}
        kprintln!("exiting main console loop\n");
    }

    pub fn kernel_shell_init() {
        let boot_options = boot_options::BootOptions::get();
        let script_bytes = &boot_options.shell_script;
        if script_bytes[0] != 0
            && let Ok(c_str) = core::ffi::CStr::from_bytes_until_nul(script_bytes)
        {
            let bytes = c_str.to_bytes();
            if let Ok(b) = kalloc::Box::<[u8; LINE_LEN]>::try_new_zeroed() {
                let mut buffer = unsafe { b.assume_init() };
                let len = core::cmp::min(bytes.len(), LINE_LEN - 1);
                buffer[..len].copy_from_slice(&bytes[..len]);
                for b in &mut buffer[..len] {
                    if *b == b'+' {
                        *b = b' ';
                    }
                }
                buffer[len] = 0;

                if let Ok(script_str) = core::str::from_utf8(&buffer[..len]) {
                    console_run_script(script_str);
                }
            }
        }

        if boot_options.shell {
            console_start();
        }
    }

    pub fn panic_shell_start() {
        kprintln!("entering panic shell loop\n");

        crate::arch_rs::set_blocking_disallowed(false);

        let mut input_buffer = [0u8; PANIC_LINE_LEN];
        let mut args = [CmdArgs::default(); MAX_NUM_ARGS];

        loop {
            panic_puts("! ");
            let current_slice = read_line_panic(&mut input_buffer);

            let mut argc = 0;
            let mut in_token = false;

            for byte in current_slice.iter_mut() {
                if byte.is_ascii_whitespace() {
                    *byte = 0;
                    in_token = false;
                } else if !in_token {
                    if argc >= MAX_NUM_ARGS {
                        break;
                    }
                    args[argc].arg_str = byte as *mut u8 as *const c_char;
                    argc += 1;
                    in_token = true;
                }
            }

            if argc == 0 {
                continue;
            }

            convert_args(&mut args, argc);

            if let Some(command) = match_command(args[0].as_str(), CMD_AVAIL_PANIC) {
                let cmd_callback = command.cmd_callback;
                // SAFETY: `args.as_mut_ptr()` points to `argc` initialized `CmdArgs` elements.
                unsafe { cmd_callback(argc as c_int, args.as_mut_ptr(), CMD_FLAG_PANIC) };
            } else {
                panic_puts("command not found\n");
            }
        }
    }

    // FFI exports.

    /// # Safety
    ///
    /// This function should only be called once during initialization.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rust_console_init_history() {
        console_init_history();
    }

    /// # Safety
    ///
    /// Caller must ensure `string` is a valid null-terminated C string.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rust_console_run_script(string: *const c_char) -> c_int {
        if string.is_null() {
            return Status::INVALID_ARGS.into_raw();
        }
        let cstr = unsafe { core::ffi::CStr::from_ptr(string) };
        let Ok(s) = cstr.to_str() else {
            return Status::INVALID_ARGS.into_raw();
        };
        console_run_script(s)
    }

    /// # Safety
    ///
    /// Caller must ensure `string` is a valid null-terminated C string.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rust_console_run_script_locked(string: *const c_char) -> c_int {
        if string.is_null() {
            return Status::INVALID_ARGS.into_raw();
        }
        let cstr = unsafe { core::ffi::CStr::from_ptr(string) };
        let Ok(s) = cstr.to_str() else {
            return Status::INVALID_ARGS.into_raw();
        };
        console_run_script_locked(s)
    }

    /// # Safety
    ///
    /// No special safety requirements, but marked unsafe to match C ABI export.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rust_kernel_shell_init() {
        kernel_shell_init();
    }

    /// # Safety
    ///
    /// This is unsafe because it calls FFI methods.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn rust_panic_shell_start() {
        panic_shell_start();
    }
}

#[cfg(not(console_enabled))]
pub mod console {
    pub fn console_run_script(_script: &str) -> i32 {
        0
    }
    pub fn console_run_script_locked(_script: &str) -> i32 {
        0
    }
    pub fn panic_shell_start() {}
    pub fn kernel_shell_init() {}
}
