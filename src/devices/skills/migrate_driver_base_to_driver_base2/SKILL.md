---
name: migrate-driver-base-to-driver-base2
description: >
  Migrate an already-DFv2 C++ driver from the older fdf::DriverBase to
  fdf::DriverBase2. Use when a driver compiles against driver_base.h, takes
  DriverStartArgs and an UnownedSynchronizedDispatcher in its constructor,
  uses FUCHSIA_DRIVER_EXPORT, or overrides Start()/PrepareStop()/Stop(), and
  must move to a default-constructible class, Start(DriverContext),
  FUCHSIA_DRIVER_EXPORT2, context.incoming()/take_incoming(),
  context.CreateInspector, and destructor-based sync cleanup. Assumes the
  driver is ALREADY DFv2 -- don't use for a DFv1/DDK driver still on
  ddk::Device or ZIRCON_DRIVER (use migrate-dfv1-to-dfv2).
---

# Migrate to DriverBase2

## Update Headers

Modify the include directive to use the new header:

```cpp
// Before
#include <lib/driver/component/cpp/driver_base.h>

// After
#include <lib/driver/component/cpp/driver_base2.h>
```

## Update Class Inheritance

Inherit from `fdf::DriverBase2` instead of `fdf::DriverBase`:

```cpp
// Before
class MyDriver : public fdf::DriverBase { ... };

// After
class MyDriver : public fdf::DriverBase2 { ... };
```

## Update Constructor

The constructor must take no arguments (be default constructible), but it still
needs to forward the driver name to `DriverBase2`. You no longer receive
`DriverContext` or `UnownedSynchronizedDispatcher` in the constructor; they are
provided in `Start()` or handled internally.

```cpp
// Before
MyDriver(fdf::DriverStartArgs start_args, fdf::UnownedSynchronizedDispatcher driver_dispatcher)
    : fdf::DriverBase("my_driver", std::move(start_args), std::move(driver_dispatcher)) {}

// After
MyDriver() : fdf::DriverBase2("my_driver") {}
```

## Update Export Macro

You must use `FUCHSIA_DRIVER_EXPORT2` instead of `FUCHSIA_DRIVER_EXPORT` for
`DriverBase2` drivers. This also requires including
`<lib/driver/component/cpp/driver_export2.h>`.

```cpp
// Before
#include <lib/driver/component/cpp/driver_export.h>
FUCHSIA_DRIVER_EXPORT(MyDriver);

// After
#include <lib/driver/component/cpp/driver_export2.h>
FUCHSIA_DRIVER_EXPORT2(MyDriver);
```

## Update Start Method

The `Start` method now receives a `DriverContext` by value. Many methods
previously available on `DriverBase` are now only accessible via this context
during `Start`.

```cpp
// Before
zx::result<> Start() override {
  incoming()->Connect(...);
  auto& offers = node_offers();
  ...
}

// After
zx::result<> Start(fdf::DriverContext context) override {
  context.incoming().Connect(...);
  auto& offers = context.node_offers();
  ...
}
```

### Handle Incoming Namespace

If you need to access the incoming namespace after the `Start` method completes,
you must take ownership of it from the context. Make sure that this is the last
call to the context where it might need to use this incoming namespace for its
own purposes. For example `CreateInspector` requires accessing the namespace, so
it must be called before taking away the incoming namespace.

> [!IMPORTANT]
> **Parameterization Rule**: "Rather than store incoming, pass it around in a parameter".
> If possible, avoid storing `incoming_` as a member variable in your driver class.
> Instead, pass `context.incoming()` (which returns `const Namespace&`) directly to
> synchronous initialization helpers. Only take ownership and store the namespace
> if it is strictly required for asynchronous OS-abstraction boundaries or downstream
> asynchronous calls.

> [!NOTE]
> Some SDK and internal APIs (like `compat::DeviceServer::Initialize` or `qualcomm::gpr::ServiceEnd`)
> still require a `std::shared_ptr<fdf::Namespace>`. You can create one by converting the
> unique pointer returned by `context.take_incoming()`:
> ```cpp
> auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
> ```
> This shared pointer can then be passed to constructors and stored if needed.

```cpp
// Example of retaining incoming namespace
zx::result<> Start(fdf::DriverContext context) override {
  incoming_ = context.take_incoming();
  ...
}
```

### Create Inspector

The `inspector()` method is removed. Use `context.CreateInspector` to create a
component inspector.

```cpp
// Before
inspector().Health().Ok();

// After
auto inspector = context.CreateInspector(this);
inspector.Health().Ok();
```

## Update Stop and Teardown

`fdf::DriverBase2` simplifies the stopping lifecycle.

* `PrepareStop(PrepareStopCompleter)` is renamed to `Stop(StopCompleter)`.
* The synchronous `Stop()` method is removed. Use the destructor for synchronous
  cleanup.

### Async Stop

```cpp
// Before
void PrepareStop(fdf::PrepareStopCompleter completer) override {
  // Do async cleanup
  completer(zx::ok());
}

// After
void Stop(fdf::StopCompleter completer) override {
  // Do async cleanup
  completer(zx::ok());
}
```

### Sync Stop

```cpp
// Before
void Stop() override {
  // Do sync cleanup
}

// After
~MyDriver() override {
  // Do sync cleanup in destructor
}
```

## Direct Driver Protocol Implementation

Some complex drivers or shims (like `CompatDriverServer`) do not use the
standard `FUCHSIA_DRIVER_EXPORT2` macro and factory shims. Instead, they
manually implement the `fuchsia_driver_framework::Driver` FIDL protocol.

In these cases:
1.  The wrapper server must manually instantiate `fdf::DriverContext` inside its
    `Start()` handler:
   ```cpp
   auto context = fdf::DriverContext(std::move(start_args));
   ```
   Note that `DriverContext` constructor in `DriverBase2` only takes
   `start_args` (it does not take `driver_dispatcher`).
2.  The wrapper must manually invoke the driver's custom `Start()` method,
    passing the context and start completer:
   ```cpp
   driver->Start(std::move(context), std::move(start_completer));
   ```
3.  The driver's custom `Start` method must call `DriverBaseInternalInit` to
    initialize base utilities (like dispatcher, name, and logger):
   ```cpp
   void MyDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
     DriverBaseInternalInit(context, fdf::UnownedSynchronizedDispatcher(driver_dispatcher_));
     ...
   }
   ```
   Note: `DriverBaseInternalInit` takes `driver_dispatcher` (usually
   `fdf::UnownedSynchronizedDispatcher`) which must be passed explicitly since
   it is not part of `DriverContext` anymore. If passing it from a member, use
   `fdf::UnownedSynchronizedDispatcher(driver_dispatcher_)` to explicitly copy
   it.

## Common Pitfalls

* **Constructor Logging**: Calling `fdf::info` or similar logging macros in the
  constructor will crash the driver. The logger is not initialized until
  `DriverBaseInternalInit` is called by the framework (which happens after
  construction but before `Start`). Perform any logging in `Start` instead.

* **Constructor Resource Binding (Timing Constraint)**: In `DriverBase2`,
  `node()`, `dispatcher()`, `incoming()`, and `logger()` are completely
  uninitialized and null during constructor execution. Any operations that bind
  Zircon channels (such as `device_.Bind({take_node(), dispatcher()})` or
  registering callbacks with the dispatcher) **must** be moved out of the
  constructor and placed at the top of `Start(context)`.

* **Move-Only Parent Node**: `node()` returns `const fidl::ClientEnd<Node>&` (a
  const reference). You cannot move from it using `std::move(node())` because
  moving from a const reference performs a copy, and `ClientEnd` is a move-only
  type with a deleted copy constructor. Always use `take_node()` instead of
  `std::move(node())` when taking ownership of the parent node channel.

* **Destructor Crashes on node_name()**: If you clean up loggers, telemetry, or
  deregister from lists in your destructor using `node_name()`, it will crash
  under `DriverBase2` because the `DriverContext` has already been destroyed.
  Instead, store `std::optional<std::string> node_name_` as a member variable
  during `Start()` and use this member variable in the destructor.

* **Missing Incoming Namespace**: Accessing `incoming()` after `Start` without
  having called `context.take_incoming()` will fail.
* **Constructor Typo**: Forgetting to remove `start_args` and
  `driver_dispatcher` from the base class initialization list.
* **Cleanup Order**: Relying on `Stop()` being called before destruction. In
  `DriverBase2`, synchronous cleanup belongs in the destructor.

## Further Reading

* [driver_base.h](/sdk/lib/driver/component/cpp/driver_base.h)
* [driver_base2.h](/sdk/lib/driver/component/cpp/driver_base2.h)

