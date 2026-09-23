# Debugging lock dependency cycles

Lock dependency cycles are a common source of deadlocks. This guide provides
instructions for detecting, debugging, and resolving lock dependency cycles.

## Rust

Rust programs on Fuchsia can use [`fuchsia_sync`] for their locks to benefit
from additional runtime checks that detect access patterns that can deadlock.

These checks rely on the [`tracing_mutex`] crate to detect cycles between lock
acquisitions across different threads.

### Adopting fuchsia_sync

To start using `fuchsia_sync` in your code, follow these steps:

1. Add `//src/lib/fuchsia-sync` to your `deps`.
1. Replace `std::sync::Mutex` in your code with `fuchsia_sync::Mutex`.
1. Replace `std::sync::RwLock` with `fuchsia_sync::RwLock`.
1. Remove any error handling for poisoned locks, as `fuchsia_sync` does not
   support [lock poisoning].

### Enabling cycle checks

These checks are enabled in `fuchsia_sync` by default in debug builds.

You can manually enable them in balanced or release builds by setting a GN arg:

```sh
fx set ... --args=fuchsia_sync_detect_lock_cycles=true
```

Note: This instrumentation has significant performance and memory overhead
that will impact the usability of a device.

If a lock cycle is detected you will see a panic message like this:

```
thread 'main' (1) panicked at ../../third_party/rust_crates/forks/tracing-mutex-0.3.2/src/reporting.rs:
Found cycle in mutex dependency graph:
disabled backtrace

stack backtrace:
...
```

See the next section for instructions on how to enable backtraces.

### Printing cycle backtraces

`tracing-mutex` will always print a backtrace for the panicking thread that
would actually trigger a deadlock, but it is often useful to also know what
other lock acquisitions are a part of a cycle.

The instrumentation will collect and print these additional backtraces when the
`RUST_BACKTRACE` environment variable is set to `1`. Note that this comes with
a large performance overhead on top of the instrumentation's baseline overhead.

For an ELF component, include this shard in your component manifest to collect
backtraces for all lock acquisitions and print relevant ones when a deadlock
is detected:

```json5
{
  include: [ "//src/lib/fuchsia-sync/meta/enable_rust_backtrace.shard.cml" ],
  // ...
}
```

### Suppressing panics

You can suppress panics from lock cycles by calling

```rs
fuchsia_sync::suppress_lock_cycle_panics();
```

Warning: This should be used with caution as it hides real deadlocks.

## Ensuring consistent lock access order

This section lists some strategies that you can use to prevent deadlocks once
this instrumentation has identified a cycle.

### Example

Consider the following code:

```rs
fn do_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    let mut foo = foo.lock();
    let mut bar = bar.lock();
    foo.do_thing();
    bar.do_thing();
}

fn do_other_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    let mut bar = bar.lock();
    let mut foo = foo.lock();
    foo.do_other_thing();
    bar.do_other_thing();
}

fn main() {
    let foo = Mutex::new(...);
    let bar = Mutex::new(...);

    let first = std::thread::spawn(|| do_thing_to_both(foo, bar));
    let second = std::thread::spawn(|| do_other_thing_to_both(foo, bar));

    first.join().unwrap();
    second.join().unwrap();
}
```

This code will deadlock in scenarios where events occur in the following order:

1. `first` acquires `foo`
2. `second` acquires `bar`
3. `first` attempts to acquire `bar` but it is held by `second`
4. `second` attempts to acquire `foo` but it is held by `first`

Steps (3) and (4) will block without any thread able to wake them, leading to
a deadlock. `tracing-mutex` will panic with a message indicating that a cycle
has been detected.

Depending on the synchronization requirements of the locks in your use case, you
may be able to avoid this cycle in a couple of ways.

### Removing overlapping lock acquisitions

The simplest way to prevent a lock acquisition from participating in a cycle is
to release the lock before acquiring the next one. This is useful if the values
guarded by the two locks don't actually require their modifications to be
synchronized.

The above example can be fixed by updating the code as follows:

```rs
fn do_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    {
        let mut foo = foo.lock();
        foo.do_thing();
    }
    {
        let mut bar = bar.lock();
        bar.do_thing();
    }
}

fn do_other_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    {
        let mut bar = bar.lock();
        bar.do_other_thing();
    }
    {
        let mut foo = foo.lock();
        foo.do_other_thing();
    }
}

// ...
```

By releasing each lock before acquiring the next one, we ensure that no thread
can starve any other thread indefinitely.

This will allow modifications to the two variables to be interleaved but that is
acceptable in many situations.

### Aligning lock access order

In cases where it's important for accesses to two or more locks to be
synchronized, you must ensure that all threads acquire the locks in the exact
same order every time.

In the simplified example you could achieve this by swapping the order the locks
are acquired in `do_other_thing_to_both()`:

```rs
fn do_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    // This order is the same as the original example.
    let mut foo = foo.lock();
    let mut bar = bar.lock();
    foo.do_thing();
    bar.do_thing();
}

fn do_other_thing_to_both(foo: Mutex<...>, bar: Mutex<...>) {
    // Now the code acquires the locks in the same order as do_thing_to_both().
    let mut foo = foo.lock();
    let mut bar = bar.lock();
    foo.do_other_thing();
    bar.do_other_thing();
}

// ...
```

By always locking `foo` before locking `bar`, you ensure that all threads
acquire the locks in the same order and prevent them from forming a cycle and
deadlocking.

#### Asserting the correct acquisition order

When possible, acquire locks in their intended order early in their lifecycle.
This informs future readers and the cycle instrumentation of the correct
acquisition order, ensuring that panic messages have source locations pointing
to the callsite with incorrect usage.

Limit these extra lock acquisitions to builds with `debug_assertions` enabled
to avoid any performance penalty in release builds.

In the simple case of two locks, this means acquiring both locks in the desired
order shortly after they are created. For example:

```rs
fn main() {
    let foo = Mutex::new(...);
    let bar = Mutex::new(...);

    // foo should always be acquired before bar if they need to overlap.
    #[cfg(debug_assertions)]
    {
        let _foo = foo.lock();
        let _bar = bar.lock();
    }

    // ...
}
```

This will ensure that panics will come from code where `bar` is acquired before
`foo`, regardless of the exact order of the logic under test.

[`fuchsia_sync`]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_sync/index.html
[`tracing_mutex`]: https://fuchsia-docs.firebaseapp.com/rust/tracing_mutex/index.html
[lock poisoning]: https://doc.rust-lang.org/std/sync/struct.Mutex.html#poisoning
