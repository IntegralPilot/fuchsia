# Driver testing

## Integration tests

Driver authors should use the [driver_test_realm](/docs/development/drivers/testing/driver_test_realm.md)
for integration tests.

## Device enumeration tests

Driver authors should use
[device enumeration tests](/docs/development/drivers/testing/device_enumeration.md)
to verify system-level topology integration across the full board tree during driver development.

## Unit tests
Drivers authors should use the [driver testing library](/docs/development/sdk/driver-testing/driver-unit-testing-quick-start.md)
library for unit tests.

There are a number of helpful mock libraries:

* [fake-platform-device](/sdk/lib/driver/fake-platform-device/cpp/fake-pdev.h) - Creates info for a fake pdev parent
* [fake-mmio-reg](/sdk/lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h) Faking MMIO registers
* [mock-mmiog](/sdk/lib/driver/mock-mmio/cpp/region.h) Mocking MMIO registers
* [fake-object](/sdk/lib/driver/fake-object/cpp/README.md) - fake userspace versions of kernel objects
