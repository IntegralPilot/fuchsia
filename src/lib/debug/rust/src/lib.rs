// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Issues a backtrace request to the system crash service.
#[inline]
pub fn backtrace_request_all_threads() {
    unsafe { ext::backtrace_request_all_threads_for_rust() };
}

#[inline]
pub fn backtrace_request_current_thread() {
    unsafe { ext::backtrace_request_current_thread_for_rust() };
}

#[inline]
pub fn backtrace_request_thread(koid: u64) {
    unsafe { ext::backtrace_request_thread_for_rust(koid) };
}

pub fn is_debugger_attached() -> bool {
    unsafe { ext::is_debugger_attached_for_rust() }
}

pub fn wait_for_debugger(seconds: u32) {
    unsafe { ext::wait_for_debugger_for_rust(seconds) };
}

mod ext {
    unsafe extern "C" {
        pub(crate) fn backtrace_request_all_threads_for_rust();
        pub(crate) fn backtrace_request_current_thread_for_rust();
        pub(crate) fn backtrace_request_thread_for_rust(koid: u64);
        pub(crate) fn is_debugger_attached_for_rust() -> bool;
        pub(crate) fn wait_for_debugger_for_rust(seconds: u32);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn call_backtrace() {
        super::backtrace_request_all_threads();
    }
}
