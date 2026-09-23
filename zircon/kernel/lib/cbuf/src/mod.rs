// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::deadline::Deadline;
use crate::kernel::event::Event;
use core::ptr::NonNull;
use pin_init::PinInit;
use zx_status::Status;

/// Context returned along with a character read from the circular buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ReadContext {
    /// The character that was read.
    pub c: u8,
    /// Whether the buffer transitioned away from being completely full as a result of this read.
    pub transitioned_from_full: bool,
}

/// A thread-safe, lock-protected circular byte buffer.
#[ksync::guarded]
#[repr(C)]
pub struct Cbuf {
    #[guarded_by(lock)]
    head: u32,

    #[guarded_by(lock)]
    tail: u32,

    #[guarded_by(lock)]
    len_pow2: u32,

    #[guarded_by(lock)]
    buf: Option<NonNull<u8>>,

    #[pin]
    event: Event,

    #[mutex]
    lock: ksync::KMutex<ksync::RawSpinlock>,
}

// SAFETY: Cbuf is safe to send across thread boundaries because all mutable access
// to its fields is protected by the internal spinlock (`lock`).
unsafe impl Send for Cbuf {}

// SAFETY: Cbuf is safe to share across thread boundaries because all mutable access
// to its fields is protected by the internal spinlock (`lock`).
unsafe impl Sync for Cbuf {}

impl Cbuf {
    /// Returns a pin initializer for a new, uninitialized `Cbuf`.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init::pin_init!(Self {
            head: 0.into(),
            tail: 0.into(),
            len_pow2: 0.into(),
            buf: None.into(),
            event <- Event::init_unsignaled(),
            lock <- ksync::KMutex::init(),
        })
    }

    /// Initializes the circular buffer with the specified size and backing memory region.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `buf` points to a valid memory region of at least `len` bytes,
    /// and that this memory region remains valid for the lifetime of `Cbuf`.
    pub unsafe fn initialize(&self, len: usize, buf: *mut u8) -> Result<(), Status> {
        if len == 0 || !len.is_power_of_two() {
            return Err(Status::INVALID_ARGS);
        }
        let len_pow2 = len.trailing_zeros();

        ksync::lock!(let mut guard = self.lock_lock());
        let fields = guard.as_mut().fields_mut();
        *fields.len_pow2 = len_pow2;
        *fields.buf = NonNull::new(buf);
        *fields.head = 0;
        *fields.tail = 0;

        Ok(())
    }

    /// Returns `true` if the buffer is currently full.
    pub fn full(&self) -> bool {
        ksync::lock!(let guard = self.lock_lock());
        let fields = guard.fields();
        is_full(*fields.head, *fields.tail, *fields.len_pow2)
    }

    /// Writes a single character to the buffer if space is available.
    ///
    /// Returns `1` if the write succeeded, or `0` if the buffer was full.
    pub fn write_char(&self, c: u8) -> usize {
        let wrote = {
            ksync::lock!(let mut guard = self.lock_lock());
            let fields = guard.fields_mut();
            if is_full(*fields.head, *fields.tail, *fields.len_pow2) {
                0
            } else {
                if let Some(buf) = fields.buf {
                    // SAFETY: `initialize` caller guarantees that `buf` is valid.
                    unsafe {
                        buf.as_ptr().add(*fields.head as usize).write(c);
                    }
                    inc_pointer(fields.head, 1, *fields.len_pow2);
                    1
                } else {
                    0
                }
            }
        };

        if wrote > 0 {
            self.event.signal();
        }
        wrote
    }

    /// Reads a single character from the buffer, returning it along with the transition context.
    ///
    /// If `block` is true, this function blocks until a character is available to read.
    /// If `block` is false, it returns `Err(Status::SHOULD_WAIT)` if no character is available.
    pub fn read_char_with_context(&self, block: bool) -> Result<ReadContext, Status> {
        loop {
            {
                ksync::lock!(let mut guard = self.lock_lock());
                let fields = guard.fields_mut();
                if *fields.tail != *fields.head
                    && let Some(buf) = fields.buf
                {
                    // SAFETY: `initialize` caller guarantees that `buf` is valid.
                    let c = unsafe { buf.as_ptr().add(*fields.tail as usize).read() };
                    let transitioned_from_full =
                        is_full(*fields.head, *fields.tail, *fields.len_pow2);

                    inc_pointer(fields.tail, 1, *fields.len_pow2);
                    if *fields.tail == *fields.head {
                        let _ = self.event.unsignal();
                    }
                    return Ok(ReadContext { c, transitioned_from_full });
                }

                // Because the signal state does not 100% match the buffer state, it is critical
                // that the event is unsignaled when the buffer is found to be empty (not just when
                // it *transitions* to empty).
                let _ = self.event.unsignal();
            }

            if !block {
                return Err(Status::SHOULD_WAIT);
            }

            self.event.wait(&Deadline::infinite())?;
        }
    }

    /// Reads a single character from the buffer.
    ///
    /// If `block` is true, this function blocks until a character is available to read.
    /// If `block` is false, it returns `Err(Status::SHOULD_WAIT)` if no character is available.
    pub fn read_char(&self, block: bool) -> Result<u8, Status> {
        self.read_char_with_context(block).map(|ctx| ctx.c)
    }
}

#[inline]
fn inc_pointer(ptr: &mut u32, inc: u32, len_pow2: u32) {
    let mask = (1u32 << len_pow2) - 1;
    *ptr = ptr.wrapping_add(inc) & mask;
}

#[inline]
fn is_full(head: u32, tail: u32, len_pow2: u32) -> bool {
    if len_pow2 == 0 {
        return true;
    }
    let mask = (1u32 << len_pow2) - 1;
    let consumed = head.wrapping_sub(tail) & mask;
    let avail = (1u32 << len_pow2) - consumed - 1;
    avail == 0
}

/// Test suite for Rust cbuf implementation.
#[cfg(ktest)]
#[unittest::suite(name = "cbuf_rust")]
mod tests {
    use crate::platform_rs::timer::InstantMono;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicU32, Ordering};
    use pin_init::stack_pin_init;
    use unittest::{assert_eq, assert_ok, assert_true, unwrap_ok};
    use zx_status::Status;

    use crate::kernel::thread;

    const ZX_ERR_INTERNAL_INTR_KILLED: i32 = -502;

    /// Test that the cbuf constructor initializes it to full until initialized.
    #[test]
    fn constructor() {
        stack_pin_init!(let cbuf = Cbuf::init());
        assert_true!(cbuf.full());

        let mut buf = [0u8; 4];
        // SAFETY: `buf` is valid for cbuf lifetime.
        unsafe {
            unwrap_ok!(cbuf.initialize(buf.len(), buf.as_mut_ptr()));
        }
        assert_true!(!cbuf.full());
    }

    /// Test basic read and write operations.
    #[test]
    fn read_write() {
        stack_pin_init!(let cbuf = Cbuf::init());

        let mut buf = [0u8; 4];
        // SAFETY: `buf` is valid for cbuf lifetime.
        unsafe {
            unwrap_ok!(cbuf.initialize(buf.len(), buf.as_mut_ptr()));
        }

        assert_true!(!cbuf.full());

        // Nothing to read, don't wait.
        assert_true!(cbuf.read_char(false) == Err(Status::SHOULD_WAIT));

        // Write some characters.
        let data = b"ABC";
        for &c in data {
            assert_eq!(cbuf.write_char(c), 1);
        }
        assert_true!(cbuf.full());

        // Writing when full should return 0.
        assert_eq!(cbuf.write_char(b'D'), 0);

        // Read them back.
        for (i, &expected) in data.iter().enumerate() {
            let res = unwrap_ok!(cbuf.read_char_with_context(true));
            assert_eq!(res.transitioned_from_full, i == 0);
            assert_eq!(res.c, expected);
        }
        assert_true!(!cbuf.full());
    }

    extern "C" fn reader_thread_entry(arg: *mut c_void) -> i32 {
        // SAFETY: arg is a valid pointer to a Cbuf pinned on the parent thread's stack.
        let cbuf = unsafe { &*(arg as *const Cbuf) };
        loop {
            match cbuf.read_char(true) {
                Ok(_) => {}
                Err(status) => return status.into_raw(),
            }
        }
    }

    /// Test concurrent read and write operations to check for races.
    #[test]
    fn read_write_race() {
        stack_pin_init!(let cbuf = Cbuf::init());

        let mut buf = [0u8; 4];
        // SAFETY: `buf` is valid for cbuf lifetime.
        unsafe {
            unwrap_ok!(cbuf.initialize(buf.len(), buf.as_mut_ptr()));
        }

        let thread_name = c"cbuf_rust_race".as_ptr();
        let cbuf_ptr = &*cbuf as *const Cbuf as *mut c_void;

        // SAFETY: we pass reader_thread_entry and valid pointers. The thread is joined
        // before `cbuf` (and `buf`) goes out of scope.
        unsafe {
            let thread = unwrap_ok!(thread::spawn(thread_name, reader_thread_entry, cbuf_ptr));

            for _ in 0..1000 {
                while cbuf.write_char(b'A') == 0 {
                    thread::r#yield();
                }
            }

            thread.kill();

            let ret = unwrap_ok!(thread.join(InstantMono::INFINITE));
            assert_eq!(ret, ZX_ERR_INTERNAL_INTR_KILLED);
        }
    }

    /// Test initialization limits (size 0, non-power of two).
    #[test]
    fn init_limits() {
        stack_pin_init!(let cbuf = Cbuf::init());
        let mut buf = [0u8; 4];

        // Size 0 should fail.
        unsafe {
            assert_true!(cbuf.initialize(0, buf.as_mut_ptr()) == Err(Status::INVALID_ARGS));
        }

        // Non-power of two should fail.
        unsafe {
            assert_true!(cbuf.initialize(3, buf.as_mut_ptr()) == Err(Status::INVALID_ARGS));
            assert_true!(cbuf.initialize(5, buf.as_mut_ptr()) == Err(Status::INVALID_ARGS));
        }

        // Power of two should succeed.
        unsafe {
            assert_ok!(cbuf.initialize(4, buf.as_mut_ptr()));
        }
    }

    /// Test uninitialized cbuf operations.
    #[test]
    fn uninitialized() {
        stack_pin_init!(let cbuf = Cbuf::init());

        assert_true!(cbuf.full());
        assert_eq!(cbuf.write_char(b'A'), 0);
        assert_true!(cbuf.read_char(false) == Err(Status::SHOULD_WAIT));
    }

    /// Test buffer wrap around behavior.
    #[test]
    fn wrap_around() {
        stack_pin_init!(let cbuf = Cbuf::init());
        let mut buf = [0u8; 4];

        unsafe {
            assert_ok!(cbuf.initialize(buf.len(), buf.as_mut_ptr()));
        }

        // Write 3 chars (capacity is 3)
        assert_eq!(cbuf.write_char(b'A'), 1);
        assert_eq!(cbuf.write_char(b'B'), 1);
        assert_eq!(cbuf.write_char(b'C'), 1);

        assert_true!(cbuf.full());

        // Read 3 chars
        assert_eq!(unwrap_ok!(cbuf.read_char(false)), b'A');
        assert_eq!(unwrap_ok!(cbuf.read_char(false)), b'B');
        assert_eq!(unwrap_ok!(cbuf.read_char(false)), b'C');

        assert_true!(!cbuf.full());

        // Write 2 chars (wraps pointers)
        assert_eq!(cbuf.write_char(b'D'), 1);
        assert_eq!(cbuf.write_char(b'E'), 1);

        // Read 2 chars (wraps pointers)
        assert_eq!(unwrap_ok!(cbuf.read_char(false)), b'D');
        assert_eq!(unwrap_ok!(cbuf.read_char(false)), b'E');

        // Should be empty
        assert_true!(cbuf.read_char(false) == Err(Status::SHOULD_WAIT));
    }

    struct BlockingReadContext {
        cbuf: *mut Cbuf,
        state: *const AtomicU32, // 0: init, 1: about to read, 2: read done, 3: error
        read_char: *mut u8,
    }

    // SAFETY: We only pass valid pointers and don't share mutability unsafely.
    unsafe impl Send for BlockingReadContext {}

    extern "C" fn blocking_reader_entry(arg: *mut c_void) -> i32 {
        let ctx = unsafe { &*(arg as *const BlockingReadContext) };
        let cbuf = unsafe { &*ctx.cbuf };
        let state = unsafe { &*ctx.state };

        state.store(1, Ordering::SeqCst);
        let c = cbuf.read_char(true); // Should block until written.

        match c {
            Ok(val) => {
                unsafe { *ctx.read_char = val };
                state.store(2, Ordering::SeqCst);
                0
            }
            Err(status) => {
                state.store(3, Ordering::SeqCst); // error
                status.into_raw()
            }
        }
    }

    /// Test blocking read operation.
    #[test]
    fn blocking_read() {
        stack_pin_init!(let cbuf = Cbuf::init());

        let mut buf = [0u8; 4];
        // SAFETY: `buf` is valid for cbuf lifetime.
        unsafe {
            assert_ok!(cbuf.initialize(buf.len(), buf.as_mut_ptr()));
        }

        let state = AtomicU32::new(0);
        let mut read_char = 0u8;

        let mut ctx = BlockingReadContext {
            cbuf: &*cbuf as *const Cbuf as *mut Cbuf,
            state: &state,
            read_char: &mut read_char,
        };

        let thread_name = c"cbuf_blocking_read".as_ptr();
        let ctx_ptr = &mut ctx as *mut BlockingReadContext as *mut c_void;

        unsafe {
            let thread = unwrap_ok!(thread::spawn(thread_name, blocking_reader_entry, ctx_ptr));

            // Wait until the reader thread is about to read.
            while state.load(Ordering::SeqCst) < 1 {
                thread::r#yield();
            }

            // Wait until the reader thread is actually blocked.
            while !thread.is_blocked() {
                thread::r#yield();
                // If it failed and exited, break.
                if state.load(Ordering::SeqCst) == 3 {
                    break;
                }
            }

            if state.load(Ordering::SeqCst) == 3 {
                thread.join(InstantMono::INFINITE).ok();
                panic!("reader thread failed early");
            }

            // Double check it is indeed blocked and state is 1.
            assert_true!(thread.is_blocked());
            assert_eq!(state.load(Ordering::SeqCst), 1);

            // Now write a char. This should wake it up.
            assert_eq!(cbuf.write_char(b'X'), 1);

            // Wait for reader thread to complete.
            let ret = unwrap_ok!(thread.join(InstantMono::INFINITE));

            assert_ok!(Status::ok(ret));
            assert_eq!(state.load(Ordering::SeqCst), 2);
            assert_eq!(read_char, b'X');
        }
    }
}
