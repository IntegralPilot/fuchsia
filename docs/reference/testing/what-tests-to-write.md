# What Tests to Write

**Tests help uncover potential issues in code**, and the various types of tests offer different levels of coverage.

This document guides developers (component authors, driver authors, and API/ABI maintainers) on the essential tests for validating Fuchsia software.

The table below provides an overview of the types of tests categorized
by what needs that type of test.

|                                                            |Source Code|Components|Drivers|Protocols|
|------------------------------------------------------------|-----------|----------|-------|---------|
|[Unit](#unit-tests)                                         |All        |-         |-      |-        |
|[Integration](#integration-tests)                           |-          |All       |Some   |-        |
|[Compatibility (CTF)](#compatibility-tests)                 |-          |Some      |Some   |All (SDK)|
|[Spec Conformance](#spec-conformance-tests)                 |-          |Some      |All    |Some     |
|[Platform Expectation](#platform-expectation-tests)         |-          |Some      |Some   |Some     |
|[System Interaction (Product)](#system-interaction-tests)   |-          |Some      |Some   |Some     |
<!-- TODO(b/308191530): Fill out these sections
|[Microbenchmarks](#microbenchmarks)|All (performance critical)|-|-|-
|[Mezzobenchmarks](#mezzobenchmarks)|-|Some|Some|-
|[Macrobenchmarks](#macrobenchmarks)|-|Some|Some|-
-->

The following sections categorize types of tests in terms of the
following criteria:

* What needs it (what source code, packages, or components need
this kind of test)
* What does it test
* What are its key benefits
* Where to view coverage (how can you determine this test is working
and measure its impact)
* How to implement (what actions do you need to take as a developer
to get this coverage)
* Who writes this kind of test (who is responsible to provide this
coverage)
* Where are the sources stored (where do you place the test)
* When does Fuchsia run this type of test (when in the release
pipeline is this kind of test run)

## Unit Tests {#unit-tests}

* **What needs it:** All source code
* **What does it test for:** Code contracts for individual units of software
* **What are its key benefits:** Permits more effective refactoring,
optimization, and development.
* **Where to view coverage:** https://analysis.chromium.org/coverage/p/fuchsia
* **How to implement:** Use a test framework for your language of choice
* **Who writes this kind of test:** All component/driver owners
* **Where are the sources stored:**  Adjacent to the code being tested
* **When does Fuchsia run this type of test:** Commit-Queue and Continuous
Integration

All code should be covered by the smallest possible test that is
able to validate functionality. Many functions and classes in the
languages we use can have their contracts tested directly by small,
focused **unit tests**.

[Unit Testing](https://en.wikipedia.org/wiki/Unit_testing) is a
well understood area, and for the most part Fuchsia developers may
use any test framework for their language of choice, with a few
notable differences:

* Tests that run on a device (Device Tests) are distributed as
Fuchsia packages and run in an environment under the control of
Test Manager. By default these tests are completely hermetic and
isolated from the rest of the system, files output by tests cannot
be seen by any other test.
* Device tests may expose [custom
artifacts](/docs/development/testing/components/test_runner_framework.md?#custom-artifacts)
which are streamed to the host device by `ffx test`.
* Device tests compiled to produce coverage information have that
data streamed off of the device automatically. Tools like `ffx
coverage` help to automate the processing of the data. Coverage
information is surfaced in Gerrit and the Fuchsia Coverage dashboard
for in-tree tests, while OOT test executors will need their own
scripts to process the coverage output of tests.

Learn how to write driver unit tests in the
[Driver unit testing quick start](/docs/development/sdk/driver-testing/driver-unit-testing-quick-start.md).

The diagram below shows unit tests running in a Fuchsia system.
![Unit Tests](images/unit-test.png "Diagram shows unit tests  running in a Fuchsia system.")

## Integration Tests {#integration-tests}

Integration tests check that the interface and behavior of one component works alongside another component that calls it.
Validate that different components work together as a system and interact as expected.

The following scenarios are validated for the component under test:

* It can actually start up and respond to requests
* It responds as expected to requests
* It interacts as expected with its own dependencies
* If the driver is made up of multiple components, check the components are behaving correctly inside the driver

The recommendation is to run integration tests hermetically (in isolation) using [Test Realm Factory](/docs/development/testing/components/test_realm_factory.md), but they can be run non-hermetically if needed.

### Hermetic Integration Tests

* **What needs it:** All components and many drivers
* **What does it test for:** Runtime behavior and contracts
* **What are its key benefits:** Ensures that a component or driver can
start up, initialize itself, and interact with dependencies
* **Where to view coverage:** https://analysis.chromium.org/coverage/p/fuchsia.
Note that OOT use cases require build option changes to output
coverage information and process it.
* **How to implement:** [Test Realm
Factory](/docs/development/testing/components/test_realm_factory.md)
* **Who writes this kind of test:** All component authors, many driver developers
* **Where are the sources stored:**  Adjacent to the code being tested
* **When does Fuchsia run this type of test:** Commit-Queue and Continuous
Integration

While unit tests are small and focused on specific pieces of business
logic, **Integration Tests** are focused on testing the interactions
between software modules as a group.

[Integration testing](https://en.wikipedia.org/wiki/Integration_testing)
is another well understood area, but Fuchsia provides features for
testing that are radically different from what exists on other
operating systems. In particular, Fuchsia's Component Framework
enforces explicit usage and routing of "capabilities" that builds
on top of Zircon's [capability-based
security](https://en.wikipedia.org/wiki/Capability-based_security)
principles. The end result is that Fuchsia tests can be _provably
hermetic and [recursively
symmetric](/docs/contribute/contributing-to-cf/original_principles.md#sandboxing)_.
The implications of these properties have been referred to as
"Fuchsia's testing superpower." Tests may be perfectly isolated
from one another, and components may be nested arbitrarily. This
means that one or more entire Fuchsia subsystems may be run in
isolated test contexts (for example,
[DriverTestRealm](/docs/development/drivers/testing/driver_test_realm.md) and
[Test UI Stack](/docs/contribute/governance/rfcs/0180_test_ui_stack.md)).

The diagram below shows hermetic integration tests using the Test Realm pattern.
![Integration Tests](images/integration-test.png "Diagram shows hermetic integration tests using the Test Realm pattern.")

All tests on Fuchsia are hermetic by default, which means they
automatically benefit from provable hermeticity and the ability to
arbitrarily nest dependencies.

Hermetic Integration Tests simply build on top of this foundation
to run a component or driver in an isolated test environment and
interact with it using FIDL protocols. These tests cover the following
scenarios for a component/driver:

1. It can actually start up and respond to requests
1. It responds as expected to requests
1. It interacts as expected with its own dependencies

The recommended pattern for writing hermetic integration tests is
[Test Realm
Factory](/docs/development/testing/components/test_realm_factory.md)
(TRF), which prepares the test for reuse in different types of tests
below. TRF tests have three parts:

1. A Test Realm, an isolated instance of the component under test
and all of its dependencies.
1. A Test Suite, which interacts with a Test Realm and makes
assertions about its state.
1. A RealmFactory protocol, unique to each test suite, which is
used to construct Test Realms.

Each test case in the Test Suite calls methods on the RealmFactory
to create a Test Realm, interacts with that realm over the capabilities
it exposes, and asserts on the responses it receives. The [TRF
docs](/docs/development/testing/components/test_realm_factory.md)
provide instructions for using the testgen tool to automatically
create skeletons of the above to be filled out with details.

Hermetic Integration Tests using TRF forms the foundation for many
of the below types of tests.

**All components and many drivers require an integration test, and
those tests should use TRF.**


### Non-hermetic Integration Tests

* **What needs it:** Some components and drivers. Specifically those
that have dependencies that are difficult to mock or isolate (e.g. Vulkan).
* **What does it test for:** System behavior and contracts
* **What are its key benefits:** Ensures that a component or driver can
start up, initialize itself, and interact with dependencies, even
when some of those dependencies are system-wide and non-hermetic.
* **Where to view coverage:** https://analysis.chromium.org/coverage/p/fuchsia.
Note that OOT use cases require build option changes to output
coverage information and process it.
* **How to implement:** [Test Realm
Factory](/docs/development/testing/components/test_realm_factory.md)
* **Who writes this kind of test:** Some component and driver authors
* **Where are the sources stored:**  Adjacent to the code being tested
* **When does Fuchsia run this type of test:** Commit-Queue and Continuous
Integration

While Hermetic Integration Tests are what we should strive for,
certain tests are difficult to write hermetically. Often this is
because those tests have difficulty with dependencies that are not
yet written in a way that can be hermetically packaged. For instance,
we do not yet have a high-fidelity mock for Vulkan, so we allow
certain tests access to the system-wide Vulkan capabilities.

The diagram below shows non-hermetic integration tests with an outside system, component or driver interaction.
![Integration Tests](images/integration-test-non-hermetic.png "Diagram shows non-hermetic integration tests with an outside system, component or driver interaction.")

Tests that access system capabilities are called **Non-hermetic
Integration Tests**. While they are technically not hermetic, they
should still try to be as isolated as possible:

1. Depend on only the necessary capabilities to run the test.
1. Follow the Test Realm Factory style, albeit with dependent
capabilities from the system rather than isolated.
1. Prefer to depend on capabilities that make a best-effort to
provide isolation between clients (e.g. separate contexts or
sessions).
1. Clean up state when the test is done.

Non-hermetic integration tests must be run in a location of the
component topology that already has the required capabilities routed
to it. A list of existing locations and instructions for adding new
ones are
[here](/docs/development/testing/components/test_runner_framework.md?#non-hermetic_tests).

**Non-hermetic Integration Tests should be used sparingly and for
situations where no hermetic solution is practical.** It is preferred
that they are used as a stop-gap solution for a test that otherwise
would be made hermetic given appropriate mocks or isolation features.
**They _should not_ be used for tests that legitimately want to
assert on the behavior of a given system globally** (see instead
[On-device System Validation](#system-validation-tests) and [Host-driven
System Interaction Tests](#lacewing-tests)).


## Compatibility Tests (CTF) {#compatibility-tests}

* **What needs it:** Protocols exposed in the SDK, but is also applicable
to client libraries and tools.
* **What does it test for:** Platform ABI/API behavior consistency and
compatibility
* **What are its key benefits:** Ensures that the behavior of platform
protocols does not unexpectedly change in incompatible ways.
Especially important for platform stability.
* **Where to view coverage:** CTF coverage dashboard.
* **How to implement:** Write a Test Realm Factory (TRF) integration
test, [enable CTF
mode](/docs/development/testing/ctf/contributing_tests.md).
* **Who writes this kind of test:** All owners of SDK protocol
implementations
* **Where are the sources stored:**  fuchsia.git
* **When does Fuchsia run this type of test:** Commit-Queue and Continuous
Integration

Compatibility tests provide early warning
that a downstream breakage is possible due to a platform change,
and it is especially important to ensure that our platform ABI
remains stable. In general, every FIDL protocol's stable API exposed by the SDK
should have a compatibility test for its API levels.

These tests verify that _clients_ of the protocols, targeting a
stable API level and built with the SDK, will receive a platform
that is compatible with their expectations.

FIDL protocols evolve over time both in terms of their stated
interface as well as the behaviors they exhibit. A common error
arises when the output of some protocol changes in a way that differs
from previous expectations.

These errors are difficult to identify using only integration tests,
since the tests are often updated at the same time as the implementation
they are testing. Furthermore, we need to maintain a degree of
compatibility across the API revisions exposed in the SDK.

Compatibility Tests for Fuchsia (CTF) enable different versions
of a component implementation to be tested against different sets
of test expectations for compatibility. The TRF pattern is fully
integrated with CTF, and TRF tests may be nominated for compatibility
testing via a config change (no source change necessary).

The mechanism is as follows:

* For each milestone release (F#) of Fuchsia, retain the Test Suite
as it existed at that milestone.
* For each platform change, run each retained Test Suite against
the RealmFactory for each milestone release.

The end result is that the old expectations for behavior of exposed
protocols are maintained across future modifications. Failing to
provide this coverage means that subtle changes to the behavior or
interface of SDK protocols will cause downstream breakages that are
especially difficult to root cause.

The diagram below shows compatibility tests for using the Test Realm Factory (TRF) pattern and a frozen component fake.
![Compatibility Tests](images/compatibility-test.png "Diagram shows compatibility tests for using the Test Realm Factory (TRF) pattern and a frozen component fake.")

Enabling CTF mode for a TRF test is a simple configuration option,
and converting existing integration tests to TRF is straightforward
([examples](/docs/development/testing/components/test_realm_factory.md?#examples)).
Authors of components/drivers that implement SDK protocols should
prioritize converting their tests to TRF and enable CTF mode to
help stabilize the Fuchsia platform and save themselves ongoing
large scale change overhead.

**All components exposing protocols in the partner or public SDK
should have a CTF test.**

## Spec Conformance Tests {#spec-conformance-tests}

* **What needs it:** Out-of-tree (OOT) drivers, components undergoing
migration from one implementation to another, (some) framework
clients.
* **What does it test for:** Consistency between different implementations
of a protocol or library, to ensute they conform to the spec.
* **What are its key benefits:** Ensures that different implementations
of the same interface exhibit compatible behavior. This is especially
useful for drivers where there may be multiple implementers of the
same interface.
* **Where to view coverage:** TODO
* **How to implement:** Use parts of an existing TRF integration test
to create a new TRF test. Alternatively write this test in Lacewing
to interact with a complete system.
* **Who writes this kind of test:** Contract/protocol owners define a
test for requirements, and downstream users reuse or compose pieces
of that test to ensure their code meets the requirements.
* **Where are the sources stored:**  fuchsia.git or built via the SDK
in stand-alone repos
* **When does Fuchsia run this type of test:** Commit-Queue, out-of-tree
continuous integration.

It is common for the Fuchsia Platform to define a contract that
must be fulfilled by one or more implementations, some of which may
be defined outside of the fuchsia.git repository:

1. The Fuchsia SDK defines driver APIs which must be implemented
by drivers outside of the Fuchsia source tree (out-of-tree). The
drivers must produce results that are consistent with the expectations
the platform has for them.
1. Component rewrites consist of one existing and one in-progress
code base, which must remain consistent. For example, if we were
rewriting the filesystem storage stack, we want to make sure writing
and reading files produces results consistent with the original
implementation.
1. Client libraries like Inspect and logs are implemented in multiple
languages, and the implementation includes writing a binary format.
It is important that the action `LOG("Hello, world")` produces
binary-compatible output no matter the library producing it.

It is important to know that an implementation conforms to the
specification, and a Spec Conformance Test is used to validate this is
the case.


### Hermetic approach to spec conformance tests
We test driver conformance by exercising the driver and checking for expected behaviour - for drivers that control devices this will require running on hardware (recommended approach). This is run by driver developers using the SDK at their desk (on hardware), and in their CI/CQ

Spec Conformance Tests may build on top of TRF tests to have identical
structure to [Compatibility Tests](#compatibility-tests). In this scenario, the primary
difference is in how the different pieces of the TRF test are used.

The recommended pattern for Spec Conformance testing is to define a
RealmFactory (containing an implementation under test), a Test Suite
(validating the implementation of the specification) wherever the
contract is defined (e.g. fuchsia.git for SDK protocols), and the
[FIDL protocol for driving the
test](/docs/development/testing/components/test_realm_factory.md#test_topology)
(which is responsible for instantiating and interacting with a set
of components under test).  The Test Suite and FIDL protocol are
distributed to implementers (for example, through the SDK).  Developers
who implement the contract may use the distributed Test Suite and
implement their own RealmFactory that wraps their implementation
behind the FIDL protocol.  This means that the same exact set of
tests that define the contract are applied to each implementation.

The diagram below shows an approach for running spec conformance tests in a hermetic way.
![Spec conformance tests (Hermetic)](images/specification-conformance-test.png "Diagram shows an approach for running spec conformance tests in a hermetic way")

### Non-hermetic approach to spec conformance tests
The non-hermetic approach to test spec conformance is to run the test on a working system using Lacewing for (hardware-dependent implementation). This is run by developers in their CI/CQ against the product implementation.

The diagram below shows an approach for running spec conformance tests in a non-hermetic way.
![Spec conformance tests (non-hermetic)](images/specification-conformance-test-non-hermetic.png "Diagram shows an approach for running spec conformance tests in a non-hermetic way.")

### Host-driven approach to spec conformance tests
Alternatively, Spec Conformance tests may be written using Lacewing
and run as host-driven system interaction tests. This is particularly
useful when the implementer of the protocol is a driver or otherwise
depends on specific hardware. This is especially useful to assert that
product images including the driver both conform to the spec and
were appropriately assembled to support interacting with hardware.

The diagram below shows an approach for running spec conformance tests in a host-driven way.
![Spec conformance tests (Host-driven)](images/specification-conformance-test-host-driven.png "Diagram shows an approach for running spec conformance tests in a host-driven way.")

More concretely, we can solve the above examples as follows:

1. **Drivers:**
   Define driver FIDL in fuchsia.git. Create a TRF test with
   associated Test Suite and FIDL protocol; ship them in the SDK.
   In an out-of-tree driver repo, implement the driver, create a
   RealmFactory that wraps the driver and implements the test
   FIDL. Run the distributed Test Suite against that RealmFactory.
1. **Drivers (Lacewing):**
   Create a Lacewing test that interacts with the driver's FIDL
   protocols from the host. Ship this test as a binary artifact
   that can be run in out-of-tree driver repos or during product
   assembly.
1. **Component rewrite:**
   Create a new code base for the rewrite, create a skeleton TRF
   test implementing the test FIDL (following best practices to
   return non-failing
   [UNSUPPORTED](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.testing.harness/errors.fidl;l=49)
   values), with the Test Suite from the existing implementation.
   Incrementally rewrite the component, and as test cases begin
   pass set failures for those cases to blocking. When all test
   cases pass, the rewrite is done, delete the old implementation.
1. **Client libraries in multiple languages:**
   Define the operations that are possible in your library as a
   test FIDL. Create a TRF test for one implementation of the
   library, then use the Test Suite from that implementation for
   all other implementations. They will remain conformant.

**Interfaces that are expected to be implemented multiple times should ship
a spec conformance test for integrators to build on top of.**

## Platform Expectation Tests {#platform-expectation-tests}

* **What needs it:** Many platform components and drivers. For example,
drivers should ensure they interact with actual hardware on an
assembled system.
* **What does it test for:** Behavior of the component/driver on an
assembled product image
* **What are its key benefits:** Ensures that a platform component/driver
behaves as expected when installed in an actual product. Helps to
catch issues with capability routing and interactions with real
hardware.
* **Where to view coverage:** TODO
* **How to implement:** Write a Test Realm Factory (TRF) integration
test, create an execution realm, enable system validation mode (TODO). Take tests that assert on specifications and run these at every stage of the pipeline (reuse is highly encouraged). Leverage Platform Expectations Test Suit (PETS). <!-- do we want to talk about this? -->
* **Who writes this kind of test:** Owners of platform components and drivers
* **Where are the sources stored:**  fuchsia.git, shipped in the SDK to
out-of-tree repositories
* **When does Fuchsia run this type of test:** Commit-Queue, out-of-tree
continuous integration.

The diagram below shows platform expectation tests where the tests are shipped with the SDK.

![Platform expectation tests](images/platform-expectation-tests.png "Diagram shows platform expectation tests where the tests are shipped with the SDK.")

Hermetic integration tests ensure that a component performs correctly
in isolation, but it does not validate that an assembled system
image including that component works properly. System validation
tests are a special kind of non-hermetic integration test that
ensures the real component behaves as expected, subject to some
constraints.

Platform Expectation tests are typically based on hermetic TRF tests
consisting of a RealmFactory and Test Suite. Instead of using the
RealmFactory (which instantiates isolated components under test),
system validation tests use a stand-in component that provides
access to the real system capabilities.

For example, if you are testing `fuchsia.example.Echo`, your hermetic
TRF test will provide a RealmFactory that exposes
`fuchsia.example.test.EchoHarness` over which you can `CreateRealm()`
to obtain an isolated `fuchsia.example.Echo` connection. A system
validation test's stand-in component also implements `CreateRealm()`,
but provides a _real_ `fuchsia.example.Echo` connection from the
system itself.

This pattern allows you to use the exact same test code in hermetic
and non-hermetic cases, with incompatibilities handled by the
`UNSUPPORTED` return value.

To illustrate how this would work, consider system validation testing
with a harness that includes method `SetUpDependency()` in addition
to `CreateRealm()`. If it is not possible to set up the dependency
when running in a non-hermetic setting, that method simply returns
`UNSUPPORTED` and tests that depend on it are skipped. Consider
having test cases `read_any_data()` and `read_specific_data()` which
skip calling `SetUpDependency()` and do call it respectively. The
former case ensures that any data can be read in the correct format
(both hermetically and non-hermetically), while the latter case
ensures that specific data is returned (hermetically only).

To aid OOT system integrators, we may ship system validation test
suites in the SDK to be run against assembled product images OOT.
**This is a primary mechanism for validating the behavior of drivers
written OOT**.

**Platform components and drivers should have system validation
tests. The Fuchsia SDK should make a validation test suite available for
each driver expected to be implemented in a separate repository.**

Note: We differentiate between on-device and host-driven system
validation tests, recognizing that some system validation tasks
cannot happen within the context of a device (e.g. rebooting the
device as part of testing requires some process external to the
device to coordinate). Certain host-driven system interaction
tests (implemented as [System Interaction Tests](#system-interaction-tests)) can
provide the same coverage as Platform Expectation tests.

## System Interaction (Product) Tests  {#system-interaction-tests}
<!-- (Lacewing) -->

* **What needs it:** Some components and drivers; individual user
journeys (for instance, validating responses when the user touches
the screen).
* **What does it test for:** Behavioral regressions in system services
and individual user journeys.
* **What are its key benefits:** Has complete control over one or more
Fuchsia devices and non-Fuchsia peripherals. Covers cases requiring
multiple devices, rebooting devices, or simulating user interaction
with a device.
* **Where to view coverage:** TODO
* **How to implement:** Write a Python test using
[Mobly/Lacewing](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/testing/end_to_end/examples)
* **Who writes this kind of test:** Component/driver owners; product owners
* **Where are the sources stored:**  fuchsia.git or OOT
* **When does Fuchsia run this type of test:** Commit-Queue, OOT
Continuous Integration as needed

Fuchsia makes hermetic testing possible for a wide range of cases
that would be infeasible to write on other systems, but in some
cases there is just no replacement for physically controlling an
entire device. Instead of running these tests on the device under
test, the host system instead takes responsibility for controlling
one or more devices to exercise end-to-end code paths.

A host-driven system interaction test has the ability to fully
control a Fuchsia device using SDK tools and direct connection to
services on the device. They are written using the Lacewing framework
(build on Mobly).

Tests that use Lacewing can arbitrarily connect to services on a target
system. Some tests are written to target specific drivers or
subsystems (e.g. does the real-time clock save time across reboots?),
some are written to cover user journeys that require device-wide
configuration (e.g. can a logged-in user open a web browser and
navigate to a web page?), and some are written to control a number
of Fuchsia and non-Fuchsia devices in concert (e.g. can a Fuchsia
device pair with a bluetooth accessory?).

With the Lacewing Framework, interactions with the device are handled through
"affordances," which provide evolvable interfaces to interact with
specific device subsystems (e.g. Bluetooth affordance, WLAN affordance,
etc).

Diagram shows an approach for running system interaction tests using Lacewing.
![System Interaction Tests](images/system-interaction-test.png "Diagram shows an approach for running system interaction tests using Lacewing")

As with most end-to-end (E2E) tests, this kind of testing can be
expensive for several reasons:

* Tests often need to run on specific real hardware setups.
* E2E tests are less performant, which increases CI/CQ latency and
load.
* E2E tests are not isolated, which can increase their flakiness
if state is not managed appropriately.

E2E testing should be done sparingly for those reasons, but often
it is the last line of defense that covers one of the hardest testing
gaps: ensuring that a real user interacting with a system will see
the desired outputs.

**Some system components and drivers need this kind of test,** but
the main benefit of using Lacewing is to cover real-world device
interactions that cannot be covered by isolated on-device tests.
Choosing between system validation and Lacewing is often a judgement
call, but there is space for both kinds of testing in a complete
test strategy. Test authors should seek to get the coverage they
need for the lowest maintenance cost.