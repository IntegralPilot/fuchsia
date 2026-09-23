
# Captive Thread Library

This is a Fuchsia C++ library for launching threads and controlling them using
Zircon kernel facilities: catching their exceptions, interrogating and mutating
their registers.

The library uses the `captive_thread` C++ namespace, and is built primarily
around the `captive_thread::CaptiveThread` class found in
[`<lib/captive-thread/captive-thread.h>`](include/lib/captive-thread/captive-thread.h).

## CaptiveThread

The `captive_thread::CaptiveThread` object represents a thread of execution,
and is constructed like `std::thread` to launch a thread running any callable
with its arguments.

### Forced Joining

Unlike `std::thread`, a `CaptiveThread` owns the thread.  When the
`CaptiveThread` is destroyed, the thread is destroyed.  That means that if the
thread has not exited by the time the `CaptiveThread` object is destroyed on
another thread, it's forced to exit.

The `CaptiveThread::ForceJoin()` method isn't like `std::thread::join()`.
Instead, it's what the destructor does: if the thread hasn't exited, make it
exit now.  When `ForceJoin()` returns (or the object is destroyed), the thread
is definitely no longer running.

If the thread was in the middle of any work, that work is abandoned.  This is
akin to a `longjmp` back to just before the thread's function was called.  But
harder: every single general register is restored to its original state,
including things never expected to change, like the thread pointer.  From a
low-level ABI perspective, it should safely recover from any condition the
thread might be in.

It will not run destructors for the objects inside functions as returning
normally would.  However, it _will_ run `thread_local` variable destructors.
So the code run in a captive thread should avoid allocating resources, taking
locks, or other kinds entanglements that require cleanup.  Especially avoid any
risk that some `thread_local` destructors (or equivalent C `tss_create` / POSIX
`pthread_key_create` callbacks) could go awry because of thread-local values in
undefined states.  Those cases, as well as clobbering libc's internal data
structures for the thread (e.g. memory stomps near the thread pointer), will
still crash the whole program, not just the captive thread.

### Exceptions and Suspension

When a `CaptiveThread` is running, it's mostly a normal `std::thread`.  But as
well as the forced joining option, any Zircon exceptions it hits can be caught.

The `WaitForException()` method waits until the thread hits an exception (or
exits).  Afterwards, `ExceptionReport()` gives the details and then
`ResolveException()` lets the thread continue running with the exception deemed
"handled"; `Resume()` lets it go on to the next exception handler (which in
most cases means crashing the whole process).

The `WaitForStop()` method works like `WaitForException()`, but will also
return if the thread becomes suspended.  The `Suspend()` method requests this,
and `Resume()` must be used after a non-exception stop.

### Cooperative Joining

If a `CaptiveThread` is running normally and can be trusted to exit
gracefully, then the `BlockUntilSuccess()` method can be used.  It just does
`std::thread::join()` on it, but after removing the exception-catching
machinery from the thread so that a crash is a crash.

### Register Access

The `Registers()` and `SetRegisters()` methods can be used on a thread while
it's stopped to fetch and/or mutate the thread's registers.  These methods take
an optional template parameter for the type of registers; the default is
`zx_thread_state_general_regs_t`.

The [`lib/captive-thread/registers.h`](include/lib/captive-thread/registers.h)
header provides some convenient APIs for picking apart the register data in
terms of each machine's ABI use of its registers.

### Single-Step

When the machine supports single-step, `CaptiveThread` makes it easy to use.
The `ResolveExceptionSingleStep()` and `ResumeSingleStep()` methods augment
their baseline counterparts by enabling single-step for the thread.  It will
soon report the expected exception so that the thread's state can be accessed
after a single machine instruction.  The `StepToException()` shorthand method
combines `ResolveExceptionSingleStep()` with `WaitForException()` for easy
repeated use.

## Testing Support

The additional [testing](testing) library provides gmock matchers and related
support for using `CaptiveThread` in tests using the gtest framework.
