# antlion

Collection of end-to-end Fuchsia WLAN tests.

[WLAN End-to-End Testing Docs] | [Report Bug] | [Request Feature]

[TOC]

[WLAN End-to-End Testing Docs]: http://go/antlion
[Report Bug]: http://go/conn-test-bug
[Request Feature]: http://b/issues/new?component=1182297&template=1680893

## End-to-end Testing

The following steps assume the following:

- Presence of a physical Fuchsia device as the default target for end-to-end
  tests.
- The Fuchsia device is flashed with a product bundle that removes a WLAN policy
  layer from the build. See [WLAN End-to-End Testing Docs] for instructions on
  flashing such a product bundle.

1. Add `//src/connectivity/wlan/tests/core:phy_existence_test` as a host label to your build.

2. Run the test:

```sh
fx test --build --e2e --output //src/connectivity/wlan/tests/core:phy_existence_test
```

## End-to-end Testing with an Access Point

The following steps assume the presence of two devices:

- Presence of a physical Fuchsia device as the default target for end-to-end tests.
- The Fuchsia device is flashed with a product bundle that removes a WLAN policy
  layer from the build. See [WLAN End-to-End Testing Docs] for instructions on
  flashing such a product bundle.
- Access point supported by antlion.

The steps for setting up an access point supported by antlion are outside the scope of
this guide. Consult the Fuchsia WLAN Team for more information.

1. Add `//src/connectivity/wlan/tests/core:connect_to_ap_test` as a host label to your build.

2. Run the test:

```sh
fx test --build --e2e --output //src/connectivity/wlan/tests/core:connect_to_ap_test -- \
  --ap-ip <IP address> --ap-ssh-port <port>
```

## Unit Testing

The following steps assume the presence of an emulated Fuchsia device for running unit tests.

1. Add `//src/testing/end_to_end/antlion:unit_tests` as a host label to your build.

2. Run the tests:

```sh
fx test --build --output //src/testing/end_to_end/antlion
```
