# Honeydew affordances

[TOC]

This page will cover some of the common topics associated with Honeydew
affordances

## What is a Honeydew affordance?

Honeydew affordances are a way to abstract away the details of how to interact
with specific actions supported by Fuchsia device (such as `trace`, `wlan`,
`bluetooth` etc.).

They provide a high-level interface that allows users to perform common tasks
associated with specific Fuchsia components (such as `connect` operation in
`wlan` component) without having to worry about the underlying implementation
details.

For example, instead of having to know how to set up a WLAN connection, a user
can simply call the `connect()` method on the `Wlan` affordance.

This makes it easier to group
* all of the tasks associated with specific Fuchsia components into one
  affordance
* all of the affordances supported by Fuchsia device into one FuchsiaDevice
  class using Python Composition

```py
class WLAN:
  def scan(self, ...):
    ...

  def connect(self, ...):
    ...

  ...

class Bluetooth:
  def scan(self, ...):
    ...

  def connect(self, ...):
    ...

  ...

class Trace:
  def start(self, ...):
    ...

  def stop(self, ...):
    ...

  ...

class FuchsiaDevice:
  @property
  def wlan(self):
    return WLAN()

  @property
  def trace(self):
    return Trace()

  @property
  def bluetooth(self):
    return Bluetooth()

  ...
```

## Create a new affordance

The following sections walk through the development process for a Honeydew
affordance by creating `HelloWorld` affordance.

1. Start by creating a [directory](../honeydew/affordances/hello_world/) for
   `HelloWorld` affordance in `honeydew` code base at
   `$FUCHSIA_DIR/src/testing/end_to_end/honeydew/honeydew/affordances/hello_world/`

2. Create [OWNERS](../honeydew/affordances/hello_world/OWNERS) and
   [METADATA.textproto](../honeydew/affordances/hello_world/METADATA.textproto)
   files with appropriate information (associated with the team who is writing
   the affordance)

3. Create `__init.py__`, `hello_world.py` and `hello_world_impl.py` files where:
    * [`__init__.py`](../honeydew/affordances/hello_world/__init__.py)  file in
      Python serves to mark this directory as a package, enabling it to be
      imported as a module
    * [hello_world.py](../honeydew/affordances/hello_world/hello_world.py) file
      is an abstract base class (ABC) which will contain the contract for
      `HelloWorld` affordance
    * [hello_world_using_ffx.py](../honeydew/affordances/hello_world/hello_world_using_ffx.py)
      file which will contain the implementation for `HelloWorld` affordance
      (in this case `HelloWorldUsingFfx`)

4. In addition to these, here are few other files to consider that may be useful
   for some affordances:
    * [errors.py](../honeydew/affordances/hello_world/errors.py) file to
      include all of the errors raised by `HelloWorld` affordance
    * [types.py](../honeydew/affordances/connectivity/wlan/utils/types.py) file
      to include all of the custom types associated with the  `HelloWorld`
      affordance

5. Make sure, HelloWorld affordance ABC contract
   ([HelloWorld](../honeydew/affordances/hello_world/hello_world.py)) inherits
   [Affordance](../honeydew/affordances/affordance.py) ABC contract

6. Make sure, `__init__()` method in HelloWorld affordance implementation
   ([HelloWorldUsingFfx](../honeydew/affordances/hello_world/hello_world_using_ffx.py))
   calls `self.verify_supported()`. This is to ensure if `HelloWorld` affordance
   in invoked on any FuchsiaDevice that does not support this affordance,
   it will raise `NotSupportedError`

7. Create [tests](../honeydew/affordances/hello_world/tests) folder under
   `$FUCHSIA_DIR/src/testing/end_to_end/honeydew/honeydew/affordances/hello_world/`
   with the following subfolders:
    * [unit_tests](../honeydew/affordances/hello_world/tests/unit_tests/) folder
      to add the unit tests associated with `HelloWorld` affordance
      implementation (in this case
      [hello_world_using_ffx.py](../honeydew/affordances/hello_world/hello_world_using_ffx.py))
    * [functional_tests](../honeydew/affordances/hello_world/tests/functional_tests/)
      folder to add the functional tests associated with `HelloWorld` affordance
      implementation
      * Every single method in ([HelloWorld](../honeydew/affordances/hello_world/hello_world.py))
        ABC class should be exercised at least once in this functional test.
        This is to ensure affordance methods are actually working as intended
        when run on an actual Fuchsia device.

8. For [unit_tests](../honeydew/affordances/hello_world/tests/unit_tests/), do
   the following steps to be included to run these unit tests as part of CQ
    *  Create `group("tests") {` defined in unit_tests's
      [BUILD.gn](../honeydew/affordances/hello_world/tests/unit_tests/BUILD.gn)
    * Update `group("unit_tests") {` defined in Honeydew's [BUILD.gn](../BUILD.gn)
      with HelloWorld affordance's unit tests (`//src/testing/end_to_end/honeydew/affordances/hello_world/tests/unit_tests:tests`)
    * Verify that the following commands run the newly created unit tests
      successfully:
      ```
      $ fx set core.x64 --with-host //src/testing/end_to_end/honeydew:unit_tests
      $ fx test //src/testing/end_to_end/honeydew --host --output
      ```

9. For [functional_tests](../honeydew/affordances/hello_world/tests/functional_tests/),
   follow these steps:
    *  Create `group("tests") {` defined in functional_tests's
      [BUILD.gn](../honeydew/affordances/hello_world/tests/functional_tests/BUILD.gn)
    * Update `group("functional_tests") {` defined in Honeydew's [BUILD.gn](../BUILD.gn)
      with HelloWorld affordance's unit tests (`//src/testing/end_to_end/honeydew/affordances/hello_world/tests/functional_tests:tests`)
    * Verify by running below commands, these newly created functional tests are
      run successfully using a locally connected Fuchsia device (such as `vim3`)
      or Fuchsia emulator
      ```
      $ fx set core.x64 --with-host //src/testing/end_to_end/honeydew:functional_tests
      $ fx test //src/testing/end_to_end/honeydew/honeydew/affordances/hello_world/tests/functional_tests:hello_world_test --e2e --output
      ```

10. For these newly created
   [functional_tests](../honeydew/affordances/hello_world/tests/functional_tests/),
   follow these steps to be included to run these functional tests as part
   of infra:
    * Identify which all Fuchsia builds where this affordance can be run
    * For all the platform specific Fuchsia builds (`workbench.[x64|vim3]` etc),
      update the corresponding tets groups in [BUILD.gn](../../BUILD.gn)
    * For all the internal product specific Fuchsia builds, update the
      corresponding tets groups in `//v/g/bundles/buildbot/<PRODUCT>/<BOARD>/BUILD.gn`
      file (this is usually done as a followup CL in tqr/, once `HelloWorld`
      affordance CL has been merged)
    * Goal is to run these functional tests on all of the different Fuchsia
      builds where we commonly expect users to write e2e tests using Lacewing

11. Update `python_library("honeydew_no_testonly") {` defined in Honeydew's
   [BUILD.gn](../BUILD.gn) with all of the HelloWorld affordance related files
   (excluding [tests](../honeydew/affordances/hello_world/tests) folder contents)

12. Update [FuchsiaDevice](../honeydew/fuchsia_device/fuchsia_device.py) ABC
    class to add `HelloWorld` as an affordance of this `FuchsiaDevice` class

13. Update [FuchsiaDevice](../honeydew/fuchsia_device/fuchsia_device.py)
    implementation class to return `HelloWorldUsingFfx` as implementation for
    `HelloWorld` affordance

You can use https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1272407 as
a reference while creating a new affordance.
