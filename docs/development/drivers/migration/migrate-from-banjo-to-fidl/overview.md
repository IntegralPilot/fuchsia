# Migrate from Banjo to FIDL

DFv1 drivers communicate with each other using the Banjo
protocol. In DFv2 all communications occur over
[FIDL (Fuchsia Interface Definition Language)][fidl] calls,
for both drivers and non-drivers. So if your DFv1 driver being
[migrated to DFv2][migrate-from-dfv1-to-dfv2] uses the Banjo
protocol, you'll need to update the driver to make only FIDL calls
to complete the migration.

In short, migrating a driver from Banjo to FIDL involves the
steps below:

1. Update the driver's `.fidl` file to create a new FIDL interface.
2. Update the driver's source code to use the new interface.
3. Build and test the driver using the new FIDL interface.

## Before you start {:#before-you-start}

Prior to starting migration tasks, first check out the
[**Frequently asked questions**][faq] page. This can help you identify
special conditions or edge cases that may apply to your driver.

## List of migration tasks {:#list-of-migration-tasks}

- [**Convert Banjo protocols to FIDL protocols**][convert-banjo-to-fidl]:
  Learn how to migrate a driver from using the Banjo protocol to FIDL.

  - [Update the DFv1 driver from Banjo to FIDL][update-banjo-to-fidl]
  - ([Optional) Update the DFv1 driver to use the driver runtime][update-driver-runtime]
  - ([Optional) Update the DFv1 driver to use non-default dispatchers][update-non-default-dispatchers]
  - ([Optional) Update the DFv1 driver to use two-way communication][update-two-way-communication]
  - [Update the DFv1 driver's unit tests to use FIDL][update-unit-tests]
  - [Additional resources][additional-resources]

<!-- Reference links -->

[fidl]: /docs/concepts/fidl/overview.md
[migrate-from-dfv1-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/overview.md
[faq]: /docs/development/drivers/migration/migrate-from-banjo-to-fidl/faq.md
[update-banjo-to-fidl]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-from-banjo-to-fidl
[update-driver-runtime]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-the-driver-runtime
[update-non-default-dispatchers]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-non-default-dispatchers
[update-two-way-communication]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-two-way-communication
[update-unit-tests]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-drivers-unit-tests-to-use-fidl
[additional-resources]: convert-banjo-protocols-to-fidl-protocols.md#additional-resources
[convert-banjo-to-fidl]: convert-banjo-protocols-to-fidl-protocols.md

