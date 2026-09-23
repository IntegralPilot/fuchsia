// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::{c_char, c_int, c_uint};
use core::fmt::Write;
use core::panic::PanicInfo;

unsafe extern "C" {
    fn panic(fmt: *const c_char, ...) -> !;
}

/// Buffer a formatted panic message is rendered into.
///
/// Deliberately small: the panic handler runs on whatever stack the panicking
/// thread had left, which for a kernel thread is 8 KiB total and may be nearly
/// exhausted -- a stack overflow arrives here too. The extra byte is the null
/// terminator `StringBuffer` maintains.
type MessageBuffer = fbl::StringBuffer<257>;

#[panic_handler]
fn rust_panic(info: &PanicInfo<'_>) -> ! {
    // `as_str()` is `Some` only for a panic with a literal message; a formatted
    // one (integer overflow, bounds checks, `assert_eq!`, ...) has to be rendered.
    let mut buffer = MessageBuffer::new();
    let message = info.message();
    let (msg, truncated) = match message.as_str() {
        Some(literal) => (literal.as_bytes(), false),
        None => {
            // Cannot fail: `StringBuffer` drops what does not fit and reports
            // success, which is what we want -- an error would stop `fmt::write()`
            // and lose the rest of the message too.
            let _ = write!(&mut buffer, "{message}");
            let full = buffer.len() == buffer.capacity();
            // Truncation happens at a byte boundary, so a full buffer may end in a
            // partial character; keep only the longest prefix that is valid UTF-8.
            let len = core::str::from_utf8(&buffer).map_or_else(|e| e.valid_up_to(), str::len);
            (&buffer[..len], full)
        }
    };
    let suffix = if truncated { c"..." } else { c"" };

    if let Some(location) = info.location() {
        let file = location.file_as_c_str();
        let line = location.line();
        unsafe {
            panic(
                c"%s:%u: %.*s%s".as_ptr(),
                file.as_ptr(),
                line as c_uint,
                msg.len() as c_int,
                msg.as_ptr(),
                suffix.as_ptr(),
            );
        }
    } else {
        unsafe {
            panic(c"%.*s%s".as_ptr(), msg.len() as c_int, msg.as_ptr(), suffix.as_ptr());
        }
    }
}
