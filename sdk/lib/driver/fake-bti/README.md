# fake-bti

This library provides fake replacements for the BTI and PMT syscalls for the
purpose of testing driver code in an unprivileged environment.  It works by
defining strong symbols over system calls which interact with BTIs, e.g.:

- **zx_object_get_info**()
- **zx_bti_pin**()
- **zx_bti_release_quarantine**().
- **zx_pmt_unpin**()

The C++ and Rust libraries exposes methods for creating fake BTI "handles" that
are compatible with Zircon system calls.  Generally, it is safe to use any
system call on a fake BTI that is compatible with a regular BTI handle.
