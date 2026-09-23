// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::HandleValue;
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};
use syscalls_macro::syscall;
use zx_status::Status;

#[syscall]
pub fn sys_syscall_test_rust_0() -> Result<(), Status> {
    Ok(())
}

#[syscall]
pub fn sys_syscall_test_rust_1(a: i32) -> Result<(), Status> {
    Status::ok(a)
}

#[syscall]
pub fn sys_syscall_test_rust_2(a: i32, b: i32) -> Result<(), Status> {
    Status::ok(a.wrapping_add(b))
}

#[syscall]
pub fn sys_syscall_test_rust_3(a: i32, b: i32, c: i32) -> Result<(), Status> {
    Status::ok(a.wrapping_add(b).wrapping_add(c))
}

#[syscall]
pub fn sys_syscall_test_rust_4(a: i32, b: i32, c: i32, d: i32) -> Result<(), Status> {
    Status::ok(a.wrapping_add(b).wrapping_add(c).wrapping_add(d))
}

#[syscall]
pub fn sys_syscall_test_rust_5(a: i32, b: i32, c: i32, d: i32, e: i32) -> Result<(), Status> {
    Status::ok(a.wrapping_add(b).wrapping_add(c).wrapping_add(d).wrapping_add(e))
}

#[syscall]
pub fn sys_syscall_test_rust_6(
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    e: i32,
    f: i32,
) -> Result<(), Status> {
    Status::ok(a.wrapping_add(b).wrapping_add(c).wrapping_add(d).wrapping_add(e).wrapping_add(f))
}

#[syscall]
pub fn sys_syscall_test_rust_7(
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    e: i32,
    f: i32,
    g: i32,
) -> Result<(), Status> {
    Status::ok(
        a.wrapping_add(b)
            .wrapping_add(c)
            .wrapping_add(d)
            .wrapping_add(e)
            .wrapping_add(f)
            .wrapping_add(g),
    )
}

#[syscall]
pub fn sys_syscall_test_rust_8(
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    e: i32,
    f: i32,
    g: i32,
    h: i32,
) -> Result<(), Status> {
    Status::ok(
        a.wrapping_add(b)
            .wrapping_add(c)
            .wrapping_add(d)
            .wrapping_add(e)
            .wrapping_add(f)
            .wrapping_add(g)
            .wrapping_add(h),
    )
}

#[syscall]
pub fn sys_syscall_test_rust_wrapper(a: i32, b: i32, c: i32) -> Result<(), Status> {
    if a < 0 || b < 0 || c < 0 {
        return Err(Status::INVALID_ARGS);
    }
    let ret = a.wrapping_add(b).wrapping_add(c);
    if ret > 50 { Err(Status::OUT_OF_RANGE) } else { Status::ok(ret) }
}

#[syscall]
pub fn sys_syscall_test_rust_inptr(
    ptr: UserInPtr<i32>,
    value: UserOutPtr<i32>,
) -> Result<(), Status> {
    if ptr.is_null() || value.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let val = ptr.read()?;
    value.write(val)?;
    Ok(())
}

#[syscall]
pub fn sys_syscall_test_rust_outptr(value: i32, ptr: UserOutPtr<i32>) -> Result<(), Status> {
    if ptr.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    ptr.write(value)?;
    Ok(())
}

#[syscall]
pub fn sys_syscall_test_rust_inoutptr(ptr: UserInOutPtr<i32>) -> Result<(), Status> {
    if ptr.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    let val = ptr.read()?;
    ptr.write(val.wrapping_add(val))?;
    Ok(())
}

#[syscall]
pub fn sys_syscall_test_rust_handle(
    handle: HandleValue,
    value: UserOutPtr<u32>,
) -> Result<(), Status> {
    if value.is_null() {
        return Err(Status::INVALID_ARGS);
    }
    value.write(handle.raw_value())?;
    Ok(())
}
