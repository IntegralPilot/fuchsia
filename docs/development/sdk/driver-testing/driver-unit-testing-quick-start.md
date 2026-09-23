# Driver unit testing quick start

Follow this quick start to write a driver unit test based on the
[simple unit test code example](https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/tests/test.cc):

## Include library dependencies

Include this library dependency, as well as the gtest dependency:

```cpp
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>
```

The library provides two classes that can be used in tests.
In the example we use `ForegroundDriverTest`, but there is also a
`BackgroundDriverTest` available.
See [Foreground vs. Background](#foreground-vs-background) below for details.

## Create configuration class

Tests define a configuration class to pass into the library
through a template parameter.
The class takes care of managing the individual parts of the unit test,
making sure they are run in the correct dispatcher context.

This configuration class must define two types, one for the driver, the other
for the environment dependencies of the driver which we show in the next
section.

Here is an example of a configuration class from the example:

```cpp
class TestConfig final {
 public:
  using DriverType = simple::SimpleDriver;
  using EnvironmentType = SimpleDriverTestEnvironment;
};
```

## Define environment type class

The `EnvironmentType` must be an isolated class
that provides your driver’s custom dependencies.
It does not need to provide framework dependencies (except for `compat::DeviceServer`),
as the library does that already.

If no extra dependencies are needed, use `fdf_testing::MinimalCompatEnvironment`
which provides a default `compat::DeviceServer` (note this is only available
in-tree as the compat protocol is not in the SDK).

Here is our environment from the example:

```cpp
class SimpleDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    // Perform any additional initialization here, such as setting up compat device servers
    // and FIDL servers.
    return zx::ok();
  }
};
```

## Define the test

Now we can put it all together and make our test. Here is what that looks like
in our example:

```cpp
class SimpleDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_F(SimpleDriverTest, VerifyChildNode) {
  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(1u, node.children().size());
    EXPECT_TRUE(node.children().count("simple_child"));
  });
}
```

## Run unit tests

Driver unit tests are executed from within the test folder of the driver itself.
For example, execute the following command to run the driver tests
for the iwlwifi driver:

```posix-terminal
tools/bazel test third_party/iwlwifi/test:iwlwifi_test_pkg
```

## Configuration arguments

### DriverType

The type of the driver under test that will be provided back through the
`driver()` and `RunInDriverContext()` functions.

By default this is *NOT* used for driver lifecycle management
(ie. starting/stopping the driver).
That happens through the driver registration symbol
created by the `FUCHSIA_DRIVER_EXPORT` macro call from the driver.

When using a custom test-specific driver in `DriverType` (for example to
provide test-specific functions), add a static `GetDriverRegistration`
function as shown below. This will override the global registration symbol.

```cpp
static DriverRegistration GetDriverRegistration()
```

### EnvironmentType
A class that contains custom dependencies for the driver under test.
The environment will always live on a background dispatcher.

It must be default constructible, derive from the `fdf_testing::Environment class`,
and override the following function:
`zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override;`

The function is called automatically on the background environment dispatcher
when starting the driver-under-test.
It must add its parts to the provided `fdf::OutgoingDirectory object`,
generally done through the `AddService` method.
The `OutgoingDirectory` backs the driver's incoming namespace, hence its name,
`to_driver_vfs`.

Here is what a custom environment that provides the compat protocol and a
custom test-defined FIDL server looks like:

```cpp
class MyFidlServer : public fidl::WireServer<fuchsia_examples_gizmo::Proto> {...};

class CustomEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    device_server_.Initialize(component::kDefaultInstance);
    EXPECT_EQ(ZX_OK, device_server_.Serve(
    fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));

    EXPECT_EQ(ZX_OK, to_driver_vfs.AddService<fuchsia_examples_gizmo::Service::Proto>(
      custom_server_.CreateInstanceHandler()).status_value());

    return zx::ok();
  }

 private:
  compat::DeviceServer device_server_;
  MyFidlServer custom_server_;
};
```

## Foreground vs. Background

The choice between foreground and background driver tests lies in how the test plans to
communicate with the driver-under-test. If the test will be calling public methods on the driver
a lot, the foreground driver test should be chosen. If the test will be calling through the
driver's exposed FIDL more often, then the background driver test should be chosen.

When using the foreground version, the test can access the driver under test
using a `driver()` method and directly make calls into it,
but sync client tasks send to driver-provided FIDL must go through a
`RunOnBackgroundDispatcherSync()`.

When using the background version the test can make sync FIDL calls into
driver-provided FIDL, but must go through a `RunInDriverContext()` when
accessing the driver instance.

## The driver_test()

As seen in the example test above, there is a `driver_test()` getter that the
test created to return a reference to the library class. This object provides
all of the controls that a test can use to do various operations for their test,
like starting the driver, connecting to it, and running tasks.
There are some methods available under both foreground and background tests,
and some that are specific to the threading mode. See below for the available
methods.

## Methods available on foreground tests

### driver

This can be used to access the driver directly from the test.
Since the driver is on the foreground it is safe to access this
on the main test thread.

### RunOnBackgroundDispatcherSync

Runs a task on a background dispatcher, separate from the driver.
This is done to avoid deadlocking with the driver when making sync client calls
into a driver that is on the foreground.

## Methods available on background tests

### RunInDriverContext

This can be used to run a callback on the driver under test.
The callback input will have a reference to the driver.
All accesses to the driver must go through this as it is unsafe to touch the driver
on the main test thread when it is on the background.


## Methods available on both

### runtime

Access the driver runtime object.
This can be used to create new background dispatchers or
to run the foreground dispatcher.
The user does not need to explicitly create dispatchers for the environment or
the driver as the library takes care of that.

### StartDriver

This can be used to start the driver under test. Waits for the start to
complete before returning the result.

### StartDriverWithCustomStartArgs

Same as StartDriver but can modify the driver start arguments before sending
it to the driver.

### StopDriver

Stops the driver under test. This calls PrepareStop on the DriverBase
implementation and waits for the completion. This must be called if StartDriver
succeeded. It can also be called if StartDriver failed, but to match the
behavior of the driver host, it will be a no-op.

### ShutdownAndDestroyDriver

Shuts down the driver dispatchers belonging to the driver-under-test, and then
call the driver's destroy hook. This happens automatically on the destruction
of the test, but can also be called manually by a test if it needs to happen
earlier for some validation or to
[start the driver again](#starting-the-driver-multiple-times-in-a-single-test)

### Connect

Connects to an instance of a service member that the driver under test provides.
This can be either a driver transport or a zircon channel transport based service.

### ConnectThroughDevfs

Connects to a protocol that the driver has exported through `devfs`.
This can be given the node_name of the `devfs` node,
or a list of node names to traverse before reaching the `devfs` node.

### RunInEnvironmentTypeContext

Runs a task on the `EnvironmentType` instance that the test is using.

### RunInNodeContext

Runs a task on the `fdf_testing::TestNode` instance that the test is using.
This can be used to validate the driver’s interactions with the driver framework node
(like checking how many children have been added).

## Starting the driver multiple times in a single test

To start/stop the driver multiple times in a test without changing the
environment, ensure to go through all 3 steps:
 - StartDriver/StartDriverWithCustomStartArgs
 - StopDriver
 - ShutdownAndDestroyDriver

## Run* functions warning

Be careful when using the Run* functions
(`RunInDriverContext`, `RunOnBackgroundDispatcherSync`, `RunInEnvironmentTypeContext`, `RunInNodeContext`).
These tasks run on specific dispatchers, so it might be unsafe to:

* Pass raw pointers into them from another context
(main thread or a different Run* kind) to use in the function
* Return a raw pointer (through a captured ref or return type) out of them
to use on the main thread or to capture/use in another Run* function
(except for a Run* function of the same kind).

## Examples

* [Simple driver unit test](https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/tests/test.cc)
* [fxr/997660](http://fxr/997660)
* [fxr/1001356](http://fxr/1001356)
* [fxr/996165](http://fxr/996165)