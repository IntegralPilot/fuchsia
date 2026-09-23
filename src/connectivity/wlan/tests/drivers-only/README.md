# Fuchsia WLAN Platform Driver Tests

These tests exercise platform driver code and MLME/SME, but do not run any other WLAN components.
Simple tests will test a single instance of a platform driver. However, more complex test cases are
allowed as long as only platform driver code is under test.

Concretely, generally these tests use [`testcontroller-driver`](//src/connectivity/wlan/tests/helpers/testcontroller-driver) to create some number of platform
drivers and test them using the `fuchsia.wlan.sme/GenericSme` and
`fuchsia.wlan.fullmac/WlanFullmacImpl` or `fuchsia.wlan.softmac/WlanSoftmacBridge`
protocols.

The tests provide coverage for the following parts of the platform drivers:

  - Platform driver startup in various starting conditions, including conditions
    that should cause startup failure.
  - Platform driver shutdown in various normal and error conditions, verifying shutdown
    always completes.
  - Error handling code associated with each protocol served by the platform driver.

This coverage is tractable to achieve because *only* platform driver code is under test.
Failures in other more complex test cases outside of this directory should use these tests as
a reference for expected platform driver behavior.
