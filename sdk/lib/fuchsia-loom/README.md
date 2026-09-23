# fuchsia-loom

This library provides concurrency primitives through two versions of the same
library with the same API surface:

- A re-export of standard library and common third-party concurrency primitives
- Versions of those same concurrency primitives instrumented with loom

This makes adding support for concurrency testing through [loom] much easier.
Because loom concurrency primitives occasionally have slightly different APIs
compared to the standard library, wrapper types are occasionally used to provide
a unified API surface.

[loom]: https://docs.rs/loom

## Your code must run on host

Concurrency tests can only be run on host, and so any library or binary that
needs concurrency testing must also compile and run on host. You may need to
mock Fuchsia-specific systems, disable Fuchisa-specific code, and disable
Fuchsia-specific dependencies.

### Consider running your tests through MIRI

If your target contains `unsafe` code, then running on host will also allow you
to run your tests through MIRI to detect Undefined Behavior. MIRI can detect
Undefined Behavior even if it wouldn't trigger a sanitizer like ASAN or UBSAN,
and provides detailed backtraces to aid in debugging and fixing unsoundness.

## Adding concurrency testing to an existing library

Let's say you have an existing library target like this:

```
rustc_library("foo") {
    edition = ".."
    sources = [ .. ]
    deps = [ .. ]
}
```

Which uses concurrency primitives like these:

```
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Sender, Receiver, channel};
```

### Step 1: Make your library build on host

You can build the host-specific version of your library by adding the host
toolchain to the end of the target label (e.g.
`//src/lib/foo(//build/toolchain:host_x64)`).

### Step 2: Make a duplicate version of your target

We're going to have two versions of the target which we build: a production one
that uses the standard library concurrency primitives, and a testing one that
uses loom concurrency primitives. You can move your common build configuration
into a shared object, and then add separate fuchsia-loom dependencies for each:

```
_common = {
    edition = ".."
    sources = [ .. ]
    deps = [ .. ]
}

rustc_library("foo") {
    forward_variables_from(_common, "*")

    deps += [ "//sdk/lib/fuchsia-loom" ]
}

if (is_host) {
    rustc_library("foo_loom) {
        forward_variables_from(_common, "*")

        configs += [ ":loom" ]
        deps += [ "//sdk/lib/fuchisa-loom:loom" ]
        testonly = true
    }
}

config("loom") {
  # The loom crate documentation recommends compiling with optimizations since
  # the number of iterations can be large enough to make tests unreasonably slow
  # otherwise.
  configs = [ "//build/config:optimize_moderate" ]
}
```

### Step 3: Replace concurrency primitives

You'll need to replace your concurrency primitives with ones from
`fuchsia-loom`:

- `Arc`: use `fuchisa_loom::sync::Arc`
- `Mutex`: use `fuchsia_loom::sync::Mutex`
- `Atomic{Usize, Bool, ..}`: use `fuchsia_loom::sync::atomic::Atomic{..}`
- `UnsafeCell`: use `fuchsia_loom::cell::UnsafeCell` and replace `get()` with
  `.with()` or `.with_mut()` depending on whether you need a mutable pointer.
- `AtomicWaker`: use `fuchsia_loom::future::AtomicWaker`
- `unreachable_unchecked()`: use `fuchsia_loom::hint::unreachable_unchecked`

Some of the API surfaces for these primitives differ from the standard library
API surfaces, so you may have to change some of your code to adapt.

### Step 4: Write concurrency tests

Concurrency tests can be added to a `tests/loom.rs` file. You'll need to make a
`loom::model::Builder` and call `check(..)` on some test code that exercises
your concurrent code. Some basic tips:

- Loom exhaustively verifies every possible interleaving of your code by
  default, which can take a very long time for especially complex code. You can
  speed up verification significantly by limiting the maximum number of thread
  pre-emptions; the loom docs suggest that a thread pre-emption bound of 2 or 3
  is typically enough. You can configure this by setting the `preemption_bound`
  field on a `loom::model::Builder`.
- Loom only verifies traditional multithreaded code, so you'll need to replace
  any `spawn(..)` calls with `loom::thread::spawn(loom::future::block_on(..))`.
  This effectively transforms each task into a separate "thread", though they're
  not literally modeled as threads in loom.
- Loom can only verify that your code behaves correctly if you provide it
  suitably complex tests. In general, your loom tests should specifically set up
  races between different threads. If you don't call `loom::thread::spawn`, then
  you won't actually verify anything!

### Step 5: Add a test target for your concurrency tests

The last thing you need to do is actually write a test target for your
concurrency tests:

```
if (is_host) {
    # ...

    rustc_test("foo_loom_tests") {
        edition = ".."
        source_root = "tests/loom.rs"
        sources = [ "tests/loom.rs" ]
        deps = [ ":foo_loom" ]
    }
}
```

You can add this to your `tests` group as usual, just make sure to specify that
it uses the host toolchain:

```
group("tests") {
    testonly = true
    deps = [ ":foo_loom_tests($host_toolchain)" ]
}
```
