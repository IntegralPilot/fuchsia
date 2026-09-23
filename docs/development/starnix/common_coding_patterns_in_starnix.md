# Common coding patterns in Starnix

This page provides a list of common coding patterns and best practices specific
to Starnix development.

[Starnix][starnix-concepts], which is essentially a kernel running in userspace,
runs Linux programs on Fuchsia. This creates specific considerations for
developers who need to ensure that their code produces the results expected by
Linux programs. This page aims to clarify some of these patterns and provide
best practices, covering topics like testing, managing userspace addresses,
handling error messages, and more.

The topics are:

- [Testing Starnix using Linux binaries](#testing-starnix-using-linux-binaries)
- [Representing userspace addresses in Starnix](#representing-userspace-addresses-in-starnix)
- [Handling syscall arguments in Starnix](#handling-syscall-arguments-in-starnix)
- [Safe error handling in Starnix](#safe-error-handling-in-starnix)
- [Creating errors in Starnix](#creating-errors-in-starnix)
- [Preventing arithmetic overflow in Starnix](#preventing-arithmetic-overflow-in-starnix)
- [Using FIDL proxies in Starnix](#using-fidl-proxies-in-starnix)
- [Spawning threads in Starnix](#spawning-threads-in-starnix)
- [Learning more](#learning-more)

## Testing Starnix using Linux binaries {:#testing-starnix-using-linux-binaries}

This section covers best practices for testing Starnix functionality, which
involves using binaries compiled for Linux.

Most of Starnix's test coverage comes from userspace binaries compiled for
Linux. The Fuchsia project runs these binaries on both Linux and Starnix to make
sure that Starnix matches Linux behavior.

Userspace unit tests verify the Linux UAPI, which is what Starnix implements.
Verifying Starnix behavior at this level gives us the freedom to refactor
Starnix's implementation with confidence.

In addition, Starnix kernel unit tests can be useful for verifying internal
invariants of the system. However, if you write kernel unit tests, be careful to
avoid "change-detector tests." In other words, ensure that you don't write tests
that fail when the implementation changes even if the changes are functionally
correct.

## Representing userspace addresses in Starnix {:#representing-userspace-addresses-in-starnix}

This section covers best practices for representing and validating addresses
within the userspace of Starnix.

The `UserAddress` and `UserRef` types are used to denote addresses that are in
"Linux userspace" in Starnix (that is, the
[restricted address space][restricted-mode]). Once it is determined that
a `UserAddress` points to an object of type `T`, convert it to a `UserRef<T>`.
This approach provides more type information, which makes it easier to read
and write to and from userspace.

Address validation is performed by Fuchsia's memory management subsystem. In
general, code outside of the memory manager should not perform any checks to
determine whether or not an address is valid before passing the address to
the memory manager (for example, checking that an address is non-null).

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  pub fn sys_something(current_task: &CurrentTask, user_events: UserRef<epoll_event>)
      -> Result <(), Errno> {
    let events = current_task.read_object(user_events)?;
    ...
  }
  ```

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  pub fn sys_something(current_task: &CurrentTask, user_events: UserRef<epoll_event>)
      -> Result <(), Errno> {
    if user_events.addr().is_null() {
      return error!(EFAULT);
    }

    let events = current_task.read_object(user_events)?;
    ...
  }
  ```

However, the most common exception to this rule is when a syscall needs to
return a specific error when a provided address is null, for example:

```cpp {:.devsite-disable-click-to-copy}
pub fn sys_something(current_task: &CurrentTask, user_events: UserRef<epoll_event>)
    -> Result <(), Errno> {
  if user_events.addr().is_null() {
    // The memory manager would never return ENOSYS for a read_object at a null
    // address, so an explicit check is required.
    return error!(ENOSYS);
  }

  let events = current_task.read_object(user_events)?;
  ...
}
```

## Handling syscall arguments in Starnix {:#handling-syscall-arguments-in-starnix}

This section focuses on best practices around `SyscallArg` , which is the
default type for all Starnix syscall arguments.

All arguments to Starnix syscall implementations start out as `SyscallArg`. This
type is then converted into specific syscall argument types using the `into()`
trait. For instance, when a syscall is being dispatched, it will be called as:

```cpp {:.devsite-disable-click-to-copy}
match syscall_nr {
  __NR_execve => {
    sys_execve(arg0.into(), arg1.into(), arg3.into())
  }
}
```

This gives the syscall implementation flexibility to use any type that
`SyscallArg` can be converted into.

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  fn sys_execve(
      user_path: UserCString,
      user_argv: UserRef<UserCString>,
      user_environ: UserRef<UserCString>,
  ) -> Result<(), Errno> {
    ...
  }
  ```

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  fn sys_execve(
      user_path: SyscallArg,
      user_argv: SyscallArg,
      user_environ: SyscallArg,
  ) -> Result<(), Errno> {
    ...
  }
  ```

## Safe error handling in Starnix  {:#safe-error-handling-in-starnix}

This section discusses the risks of using the `unwrap()` and `expect()` methods
within Starnix.

Panicking in Starnix is the equivalent of a kernel panic for the container it is
running. This means that if a syscall uses APIs like `unwrap()` or `expect()`,
it has the potential to panic, not only the process that caused the error, but
the entire container.

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  let value = option.ok_or_else(|| error!(EINVAL))?;
  ```

  This example is good practice because it uses `ok_or_else()` to handle
  error cases.

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  let value = option.unwrap();
  ```

  This example is bad practice because it has the potential to panic and
  bring down the entire container, not just the process that caused the invariant
  to be violated.

However, if an error is truly unrecoverable, it is acceptable to use `unwrap()`
or `expect()`. However, its use should contain a context string that describes
why a kernel panic is the only option.

## Creating errors in Starnix {:#creating-errors-in-starnix}

This section provides best practices for creating and translating errors.

Starnix uses a wrapper type for Linux error codes called `errno`. This type is
useful because it can capture the source location of the error, which is helpful
when debugging. When creating new errors, use the `error!()` macro.

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  if !name.entry.node.is_dir() {
    return error!(ENOTDIR, "Invalid path provided to sys_chroot");
  }
  ```

  This example is good practice because the `errno!()` macro provides more
  information, which helps debugging.

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  return error!(EINVAL);
  ```

  This example is bad practice because it returns an error code without
  any context or information about the error.

Plus, when translating one error into another, it may be convenient to use
`map_err()` with the `errno!()` macro.

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  let s = mm.read_c_string_to_vec(user_string, elem_limit).map_err(|e| {
    if e.code == ENAMETOOLONG {
      errno!(E2BIG)
    } else {
      e
    }
  })?;
  ```

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  let s = match mm.read_c_string_to_vec(user_string, elem_limit) {
    Err(e) if e.code == ENAMETOOLONG => {
      errno!(E2BIG)
    },
    Err(e) => {
      e
    },
    ok => ok,
  }?;
  ```

## Preventing arithmetic overflow in Starnix {:#preventing-arithmetic-overflow-in-starnix}

This section emphasizes the importance of using checked math operations when
dealing with numerical values originating from userspace.

Always use checked math (for example, `checked_add()` and `checked_mul()`) with
numerical values that come from userspace. This prevents bad values in userspace
from overflowing arithmetic in the kernel.

* {Good}

  ```cpp {:.devsite-disable-click-to-copy}
  pub fn sys_something(user_value: u32) -> Result<(), Errno> {
    let value = user_value.get()?;
    let result = value.checked_mul(2).ok_or_else(|| error!(EOVERFLOW))?;
    ...
  }
  ```

  This example is good practice because the code uses `checked_mul()` to
  perform multiplication on a value retrieved from userspace. This ensures that if
  the multiplication overflows, an `EOVERFLOW` error is returned instead of
  potentially causing unexpected behavior or crashes.

* {Bad}

  ```cpp {:.devsite-disable-click-to-copy}
  pub fn sys_something(user_value: u32) -> Result<(), Errno> {
    let value = user_value.get()?;
    let result = value * 2; // Potential overflow here!
    ...
  }
  ```

  This example is bad practice because the code directly multiplies the
  user-supplied value by 2 without checking for potential overflow. If the value
  is large enough, the multiplication could overflow, leading to incorrect results
  or a system crash.

## Using FIDL proxies in Starnix {:#using-fidl-proxies-in-starnix}

Note: The topic in this section is covered in more detail in
[IPC patterns in Starnix][ipc-patterns-in-starnix].

This section explains why Starnix, unlike some Fuchsia components, typically
uses synchronous proxies when interacting with FIDL protocols.

Starnix typically uses synchronous proxies because of its execution model.
Specifically, when servicing a Linux system call, Starnix code runs on the
thread of the user program that invoked the Linux system call.

Since the thread belongs to a Linux program, Starnix must perform the requested
work and then return control back to the Linux program.

This constraint means that the work Starnix is doing needs to be completed
synchronously before returning control.

Since the work needs to be completed before returning, a synchronous proxy
is the simplest solution. A synchronous proxy is also more performant, because
it avoids context switching to another thread and back (using an asynchronous
proxy would require a separate thread, for the asynchronous executor to use).

## Spawning threads in Starnix {:#spawning-threads-in-starnix}

**Avoid `std::thread::spawn`.**

`std::thread::spawn` creates a thread that becomes part of the *calling
process*. The lifetime of the thread will be less than or equal to that of the
calling process.

**What can go wrong?**

Say you call `std::thread::spawn` in the context of a Linux
process. That newly spawned thread is now tied to the lifetime of that Linux
process. When the process terminates, so does the thread. If the thread happens
to be holding a mutex (or similar resource) when the process is terminated then
the mutex will never be released and no task in the container will ever be able
to acquire that mutex.

**What is the alternative?**

[KernelThreads][kernel-threads]'s spawn methods are often the right choice.  These methods
create threads that become part of the *main Starnix process* (sometimes
referred to as the prototype process). The main Starnix process has no private
address space and will outlive all Linux processes in the container.

## Learning more {:#learning-more}

To learn more about the execution model for Starnix, please see
- [Making Linux syscalls in Fuchsia](/docs/concepts/starnix/syscalls.md)
- [RFC 0261: Fast and efficient user space kernel emulation](/docs/contribute/governance/rfcs/0261_fast_and_efficient_user_space_kernel_emulation.md)

<!-- Reference links -->

[starnix-concepts]: /docs/concepts/starnix/README.md
[restricted-mode]: /docs/concepts/starnix/syscalls.md#running-a-linux-program-in-restricted-mode
[ipc-patterns-in-starnix]: ipc_patterns_in_starnix.md
[kernel-threads]: https://fuchsia-docs.firebaseapp.com/rust/starnix_core/task/struct.KernelThreads.html
