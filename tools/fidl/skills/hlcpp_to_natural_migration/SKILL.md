---
name: hlcpp_to_natural_migration
description: Migrate C++ components from HLCPP to Natural C++ FIDL bindings.
---

# FIDL HLCPP to Natural Bindings Migration

This skill guides you through migrating C++ components and tests from the legacy
HLCPP FIDL bindings to the modern Natural C++ bindings.

## When to use this skill

Use this skill when you are tasked with migrating a C++ component or test in
Fuchsia that uses HLCPP bindings (headers matching `<fuchsia/.../cpp/fidl.h>`)
to Natural C++ bindings (headers matching `<fidl/fuchsia.../cpp/fidl.h>`).

## Process

### 1. Research and Plan
1.  **Identify HLCPP Usage**: Look for includes like

    ```cpp
    #include <fuchsia/protocol/cpp/fidl.h>
    ```

    and dependencies ending in `_hlcpp` in `BUILD.gn`.
2.  **Identify FIDL Protocols**: Note the protocols used and implemented by the
    component.

### 2. Update BUILD.gn
1.  Replace dependencies on HLCPP bindings (e.g.,
    `//sdk/fidl/fuchsia.buildinfo:fuchsia.buildinfo_hlcpp`) with Natural C++
    bindings (e.g., `//sdk/fidl/fuchsia.buildinfo:fuchsia.buildinfo_cpp`).
2.  Update any test dependencies as well.

### 3. Update Headers and Implementation
1.  **Replace Includes**:
    - Change `#include <fuchsia/protocol/cpp/fidl.h>` to `#include
      <fidl/fuchsia.protocol/cpp/fidl.h>`.
2.  **Update Server Implementation**:
    - Inherit from `fidl::Server<FuchsiaProtocol>` instead of
      `fuchsia::protocol::Protocol`.
    - Update method signatures to use Natural C++ types and completers. E.g.,
      `void Method(MethodRequest& request, MethodCompleter::Sync& completer)`.
3.  **Update Client Usage**:
    - Use `fidl::Client<FuchsiaProtocol>` for async clients or
      `fidl::SyncClient<FuchsiaProtocol>` for sync clients.
4.  **Update Component Serving**:
    - Use `component::OutgoingDirectory` to serve protocols instead of legacy
      HLCPP patterns.
5.  **Events**:
    - Natural C++ event handlers use generated event types in their signature.
      E.g., `void OnEvent(fidl::Event<FuchsiaProtocol::OnEvent>& event)` instead
      of just `void OnEvent()`.

### Common Patterns and Gotchas
-   **Factory Methods**: Some types in Natural C++ (e.g., unions) require using
    factory methods like `WithField(...)` rather than default construction and
    setters.
-   **`AddUnmanagedProtocol`**: When hosting protocols in `OutgoingDirectory`
    using a lambda handler (instead of passing a `std::unique_ptr` to a server
    implementation), use `AddUnmanagedProtocol`.
-   **Self-Destruction in Callbacks**: If a connection's error or unbind handler
    removes the connection instance from a map (thus destroying it), doing so
    directly within the callback can cause crashes (e.g., re-entering allocator
    mutexes). Use `async::PostTask` to defer the destruction.
-   **Non-copyable `fidl::Result` in Tests**: In tests using
    `fidl::Client::Then`, the callback receives a `fidl::Result`. This type is
    typically non-copyable. Ensure the callback lambda takes it by reference
    (e.g., `[](auto& result)`) or by rvalue reference if you plan to move it.
-   **Table Initialization**: Natural C++ tables do not support designated
    initializers. Use setter methods instead, or construct a temporary and chain
    setters if they return a reference (e.g., `Table().field(value)`).
-   **`[[nodiscard]]`**: Many Natural C++ methods return values that are
    `[[nodiscard]]`. Instead of casting to `(void)` to ignore them, add checks
    for success (e.g., `EXPECT_TRUE(result.is_ok())` in tests).
-   **Removing `//sdk/lib/sys/cpp`**: Components that do not use HLCPP bindings
    should no longer depend on `//sdk/lib/sys/cpp`.
    -   Remove `//sdk/lib/sys/cpp` from `BUILD.gn`.
    -   Replace `sys::ComponentContext` with `component::OutgoingDirectory` for
        serving services.
    -   Replace `context->svc()->Connect<Protocol>()` with
        `component::Connect<Protocol>()` from
        `<lib/component/incoming/cpp/protocol.h>`.

### 4. Update Tests
1.  Migrate test fixtures to use Natural C++ clients.
2.  **Avoid Deadlocks**: If the test setup involves serving a protocol or VFS on
    a separate thread, ensure you don't create deadlocks by making synchronous
    calls from that thread to itself, or by blocking the main thread while it
    needs to serve requests.
3.  Prefer using `fidl::Client` and `RunLoopUntilIdle()` in tests rather than
    blocking `SyncClient` if the server is on the same thread.
4.  **Hybrid Test Environments**: If test utilities (e.g.,
    `ComponentContextProvider`) expect legacy HLCPP handlers, you can still
    implement fakes using Natural C++. Bridge them by converting the channel in
    the HLCPP handler to a `fidl::ServerEnd<Protocol>` and binding it to your
    Natural C++ server implementation.
5.  **Connecting to Fake Services**: When using `ComponentContextProvider` to
    host fake services, ensure Natural C++ clients connect to the *outgoing*
    directory (`public_service_directory()`) where the services are published.
6.  **Initial State Notifications**: Watch protocols may trigger an immediate
    notification if the fake service's initial state differs from the
    implementation's internal defaults. Remember to reset any "changed" flags in
    your test event handler after the initial synchronization to avoid false
    positives in subsequent assertions.

### 5. Verification
1.  Use `fx status` to check that modified targets are being build. Update
    `out/default/args.gn` to add them if they are missing.
2.  Run `fx build` to ensure everything compiles.
3.  Run the relevant tests with `fx test`.
4.  Run `fx format-code` before completing.

### 6. Cleanup
1.  If the target was listed in `build/cpp/hlcpp_visibility.gni`, remove it from
    the allowlist.
