---
name: editing-fuchsia-controller
description: >
  Understand Fuchsia Controller's architecture and async mechanism. Guide
  agents working on Fuchsia Controller internals.
---

## Overview

Fuchsia Controller is a host-side Python library used to interact with Fuchsia
devices. It allows Python scripts to speak FIDL directly to the device.

## Architecture

Fuchsia Controller is implemented as a hybrid Python/C++/Rust library located at
[src/developer/ffx/lib/fuchsia-controller/](/src/developer/ffx/lib/fuchsia-controller/).

Python Code (fuchsia_controller_py, fidl_bindings, async wraps) -> C++ ABI layer
implementing a Python C Extension (fuchsia_controller_internal) -> Rust Backend
statically linked, exposing a C ABI (Runs LocalExecutor in a dedicated command
thread)

1.  **Python Frontend**: Located in `python/`. It provides the user-facing API
    (`Context`, `Channel`, `Socket`, etc.) and the FIDL binding implementation
    (`python/fidl/`).
2.  **C++ ABI Layer**: Located in `cpp/fuchsia_controller_internal/`. It acts as
    the bridge, exposing a C API to Python using standard Python C extension
    mechanisms. It forwards calls to the Rust backend.
3.  **Rust Backend**: Located in `src/`. It compiles to a static library
    (`rustc_staticlib` in `BUILD.gn`). It contains the actual implementation for
    connecting to devices, derived from `ffx` libraries.

## Async Synchronization Mechanism

Fuchsia Controller is primarily designed for use with async Python (`asyncio`).
Because the Rust backend runs in its own dedicated thread with its own
`LocalExecutor`, a synchronization mechanism is required to bridge Rust async
events to Python `asyncio` coroutines without blocking the Python event loop.

This is achieved using a Unix domain socket pair (or pipe) and Python's
`add_reader` API.

### Detailed Flow

1.  **Initialization**:
    * When the Rust `LibContext` is created, it initializes a `LibNotifier`
      ([src/lib_context.rs](/src/developer/ffx/lib/fuchsia-controller/src/lib_context.rs)).
    * `LibNotifier` creates a socket pair using `UnixStream::pair()`.
    * The receiver end of the socket is kept as a `RawFd`.

2.  **Python Registration**:
    * In Python, when an async operation needs to wait for a handle (e.g., a
      Channel or Socket) to be ready, it uses a `GlobalHandleWaker`
      ([python/fidl/_ipc.py](/src/developer/ffx/lib/fuchsia-controller/python/fidl/_ipc.py)).
    * The waker calls `fc.connect_handle_notifier()`, which calls into the Rust
      backend via the C ABI (`ffx_connect_handle_notifier`) to retrieve the
      `RawFd` of the socket pair.
    * Python registers a reader on this FD with the current async event loop:
        ```python
        asyncio.get_running_loop().add_reader(
            notification_fd,
            enqueue_ready_zx_handle_from_fd,
            notification_fd,
            self._handle_ready_queues,
        )
        ```

3.  **Rust Notification**:
    * When the Rust backend detects an event on a Zircon handle (managed via
      FDomain), it sends the handle number to the `LibNotifier`'s sender
      channel.
    * A background task in Rust reads from this channel and writes the 4-byte
      handle ID (little-endian) to the socket stream:
        ```rust
        // src/lib_context.rs
        while let Ok(raw_handle) = rx.recv().await {
            stream_tx.write_u32_le(raw_handle).await;
        }
        ```

4.  **Python Wakeup**:
    * The data written by Rust makes the `notification_fd` readable in Python.
    * `asyncio` triggers the callback `enqueue_ready_zx_handle_from_fd`.
    * The callback reads the 4-byte handle ID from the socket:
        ```python
        # python/fidl/_ipc.py
        s = socket.fromfd(fd, socket.AF_UNIX, socket.SOCK_STREAM)
        handle_no = int.from_bytes(s.recv(4), "little")
        ```
    * It then pushes this handle ID into a per-handle `asyncio.Queue`:
        ```python
        queue = handle_ready_queues.get(handle_no)
        queue.put_nowait(handle_no)
        ```
    * Any Python coroutine awaiting on `wait_ready(handle)` (which awaits on
      this queue) is then woken up to process the event (e.g., read data from
      the channel).

## Key Files

- **BUILD.gn**: Defines the targets, including the Rust staticlib and Python
  libraries.
- **src/lib.rs**: C ABI implementation in Rust.
- **src/lib_context.rs**: Manages the Rust background thread, executor, and
  `LibNotifier`.
- **cpp/fuchsia_controller_internal/fuchsia_controller.h**: C ABI declarations.
- **python/fuchsia_controller_py/__init__.py**: Python base bindings wrapping
  the C extension.
- **python/fidl/_ipc.py**: Async I/O handling, `GlobalHandleWaker`, and the
  socket reader integration.

## Testing

To run the tests for Fuchsia Controller and its related components, you need to
ensure they are included in your build configuration. Since these are host-side
tests, use `fx add-host-test`.

### Fuchsia Controller Test Suite

The Fuchsia Controller test suite includes unit tests, conformance tests, and
other Python host tests.

1.  **Add tests to build configuration:**
    ```bash
    fx add-host-test //src/developer/ffx/lib/fuchsia-controller:tests
    ```

2.  **Run the tests:** You can run all the tests matching `fuchsia_controller_`
    using `fx test`:
    ```bash
    fx test fuchsia_controller_
    ```
    This will execute around 10 tests, including the static conformance tests
    (`fuchsia_controller_static_conformance_tests`).

### fidlgen_python Test Suite

The `fidlgen_python` tests ensure the Python FIDL bindings generator works
correctly.

1.  **Add tests to build configuration:**
    ```bash
    fx add-host-test //tools/fidl/fidlgen_python:tests
    ```

2.  **Run the tests:** You can run the Python test scripts using `fx test`:
    ```bash
    fx test fidlgen_python
    ```
    This will execute the test scripts (around 8 tests).

3.  **Golden Tests and Examples:** The `fidlgen_python` target also includes
    golden tests and examples that are validated during the build step. To
    trigger these, perform a standard build after adding the host tests:
    ```bash
    fx build
    ```
