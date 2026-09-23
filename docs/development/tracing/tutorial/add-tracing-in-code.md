# Add tracing in your code

This page describes how to add tracing to your Fuchsia component's code.

## Prerequisites

Before you begin, make sure you have completed the following tasks:

* [Familiarize yourself with the Fuchsia tracing system][fuchsia-tracing-system].
* [Register your component as a tracing provider][register-a-trace-provider].

Also, make sure to use the relevant library in your code:

<<../_common/_tracing_headers.md>>

## Use tracing macros in your code {:#use-tracing-macros-in-your-code}

Once your component is registered as a [trace provider][register-a-trace-provider],
you can add tracing in your component's code.

The following actions are often useful and can easily be added in the code
using the tracing macros:

* [Trace an instant event](#trace-an-instant-event).
* [Time an event](#time-an-event).
* [Disable tracing](#disable-tracing).
* [Determine if tracing is on](#determine-if-tracing-is-on).

For the list of all available tracing macros, see

* [Rust Tracing macros][rust-macros].
* [C/C++ Tracing macros][c-cpp-macros].

### Trace an instant event {:#trace-an-instant-event}

The following example writes an instant event representing a single moment in time:

* {Rust}

  ```rust
  fuchsia_trace::instant!(c"helloworld", c"hello_world_test", fuchsia_trace::Scope::Process, "message" => "Hello, World!");
  ```

  This example specifies a category of `helloworld`, a name of `hello_world_test`,
  a scope of `TRACE_SCOPE_PROCESS`, and a key and value pair.

  For more information on the `instant!` macro, see
  [`instant!`][trace-instant-rust].

* {C++}

  ```cpp
  TRACE_INSTANT("helloworld", "hello_world_test", TRACE_SCOPE_PROCESS, "message", "Hello, World!");
  ```

  This example specifies a category of `helloworld`, a name of `hello_world_test`,
  a scope of `TRACE_SCOPE_PROCESS`, and a key and value pair.

  For more information on the `TRACE_INSTANT` macro, see
  [`TRACE_INSTANT`][trace-instant].

* {C }

  ```c
  TRACE_INSTANT("helloworld", "hello_world_test", TRACE_SCOPE_PROCESS, "message", TA_STRING("Hello, World!"));
  ```

  This example specifies a category of `helloworld`, a name of `hello_world_test`,
  a scope of `TRACE_SCOPE_PROCESS`, and a key and value pair.

  For more information on the `TRACE_INSTANT` macro, see
  [`TRACE_INSTANT`][trace-instant].

### Time an event {:#time-an-event transformation="converted"}

This example shows you how to time a function or procedure:

Note: This example is based on a a [`blobfs`][blobfs-cc] vnode constructor.

* {Rust}

  ```rust
  fn InitCompressed() {
      fuchsia_trace::duration!(c"helloworld", c"hello_world_test", fuchsia_trace::Scope::Process, "message" => "Hello, World!");
      ...
      // Duration ends due to RAII
  }
  ```

  This example records the length of time spent in the constructor,
  along with the size and number of blocks.

  For more information on the `duration!` macro, see
  [`duration!`][trace-duration-rust].

* {C++}

  ```cpp
  zx_status_t VnodeBlob::InitCompressed() {
      TRACE_DURATION("blobfs", "Blobfs::InitCompressed", "size", inode_.blob_size,
                     "blocks", inode_.num_blocks);
      ...
      // Duration ends due to RAII
  }
  ```

  This example records the length of time spent in the constructor,
  along with the size and number of blocks. Since this is a C++ example,
  the compiler can infer the data types.

  For more information on the `TRACE_DURATION` macro, see
  [`TRACE_DURATION`][trace-duration].

* {C }

  ```c
  zx_status_t VnodeBlob_InitCompressed(inode_t inode) {
      TRACE_DURATION_BEGIN("blobfs", "Blobfs_InitCompressed", "size", inode.blob_size, "blocks",
      inode.num_blocks);
      ...
      TRACE_DURATION_END("blobfs", "Blobfs_InitCompressed");
  }
  ```

  This example records the length of time spent in the constructor,
  along with the size and number of blocks.

  For more information on the `TRACE_DURATION_BEGIN` and `TRACE_DURATION_END` macros, see
  [`TRACE_DURATION_BEGIN`][trace-duration-begin] and [`TRACE_DURATION_END`][trace-duration-end].

### Disable tracing {:#disable-tracing}

There are cases where you may wish to entirely disable tracing (for
instance, when you are about to release the component into production).
If the `NTRACE` macro is added in your code, the tracing macros do not
generate any code.

The following example (for C and C++) shows the `NTRACE` macro:

```c
#define NTRACE  // disable tracing
#include <lib/trace/event.h>
```

Make sure that you define the `NTRACE` macro before the `#include`statement.

In the example below, the `rx_count` and `tx_count` fields are used only with
tracing, so if `NTRACE` is asserted, which indicates that tracing is disabled,
the fields do not take up space in the `my_statistics_t` structure.

```c
typedef struct {
#ifndef NTRACE  // reads as "if tracing is not disabled"
    uint64_t    rx_count;
    uint64_t    tx_count;
#endif
    uint64_t    npackets;
} my_statistics_t;
```

However, if you do need to conditionally compile the code for managing
the recording of the statistics, you can use the `TRACE_INSTANT` macro:

```c
#ifndef NTRACE
    status.tx_count++;
    TRACE_INSTANT("bandwidth", "txpackets", TRACE_SCOPE_PROCESS,
                  "count", TA_UINT64(status.tx_count));
#endif  // NTRACE
```

For more information on the `NTRACE` macro, see [`NTRACE`][ntrace].

### Determine if tracing is on {:#determine-if-tracing-is-on}

In some cases, you may need to determine if tracing is on at runtime.

* {Rust}

  ```rust
  if fuchsia_trace::category_enabled!(c"my_category") {
    let v = do_something_expensive();
    fuchsia_trace::instant!(c"my_category", ...
  }
  ```

  If you need to check if *any* tracing is enabled without checking a specific
  category, you can use `fuchsia_trace::is_enabled()`, but be aware this
  may have higher overhead as it is not cached as category checking is.

  The rust trace bindings don't support compile time checking if tracing is disabled.

  For more information, see [`category_enabled`][trace-category-enabled-rust] and
  [`is_enabled`][trace-enabled-rust].

* {C++}

  If tracing is compiled in your code because `NTRACE` is not defined,
  you can check if a specific category is enabled using `TRACE_CATEGORY_ENABLED()`.

  ```cpp
  #ifndef NTRACE
      if (TRACE_CATEGORY_ENABLED("my_category")) {
          int v = do_something_expensive();
          TRACE_INSTANT("my_category", ...
      }
  #endif  // NTRACE
  ```

  If you need to check if *any* tracing is enabled without checking a specific
  category, you can use `TRACE_ENABLED()`, but be aware this
  may have higher overhead as it is not cached as category checking is.

  The example above uses `#ifndef NTRACE` because the function
  `do_something_expensive()` might only be compiled in when tracing is enabled.

  For more information, see [`TRACE_CATEGORY_ENABLED`][trace-category-enabled] and
  [`TRACE_ENABLED`][trace-enabled].

* {C }

  If tracing is compiled in your code because `NTRACE` is not defined,
  you can check if a specific category is enabled using `TRACE_CATEGORY_ENABLED()`.

  ```c
  #ifndef NTRACE
      if (TRACE_CATEGORY_ENABLED("my_category")) {
          int v = do_something_expensive();
          TRACE_INSTANT("my_category", ...
      }
  #endif  // NTRACE
  ```

  If you need to check if *any* tracing is enabled without checking a specific
  category, you can use `TRACE_ENABLED()`, but be aware this
  may have higher overhead as it is not cached as category checking is.

  The example above uses `#ifndef NTRACE` because the function
  `do_something_expensive()` might only be compiled in when tracing is enabled.

  For more information, see [`TRACE_CATEGORY_ENABLED`][trace-category-enabled] and
  [`TRACE_ENABLED`][trace-enabled].

Once you have added tracing code to your component, you can now collect a
trace from the component. For more information, see the next
[Record and visualize a trace][record-and-visualize-a-trace] page.

<!-- Reference links -->

[fuchsia-tracing-system]: /docs/concepts/kernel/tracing-system.md
[register-a-trace-provider]: /docs/development/tracing/tutorial/register-a-trace-provider.md
[c-cpp-macros]: /docs/reference/tracing/c_cpp_macros.md
[rust-macros]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/index.html
[trace-instant]: /docs/reference/tracing/c_cpp_macros.md#TRACE_INSTANT
[trace-instant-rust]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/macro.instant.html
[ntrace]: /docs/reference/tracing/c_cpp_macros.md#NTRACE
[trace-enabled]: /docs/reference/tracing/c_cpp_macros.md#TRACE_ENABLED
[trace-category-enabled]: /docs/reference/tracing/c_cpp_macros.md#TRACE_CATEGORY_ENABLED
[trace-enabled-rust]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/fn.is_enabled.html
[trace-category-enabled-rust]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/fn.category_enabled.html
[blobfs-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/src/storage/blobfs/blobfs.cc
[trace-duration]: /docs/reference/tracing/c_cpp_macros.md#TRACE_DURATION
[trace-duration-begin]: /docs/reference/tracing/c_cpp_macros.md#TRACE_DURATION_BEGIN
[trace-duration-end]: /docs/reference/tracing/c_cpp_macros.md#TRACE_DURATION_END
[trace-duration-rust]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/fn.duration.html
[record-and-visualize-a-trace]: /docs/development/tracing/tutorial/record-and-visualize-a-trace.md
