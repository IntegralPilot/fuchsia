# Migrate from DFv1 to DFv2

Migrating a DFv1 driver to DFv2 primarily involves updating the
driver's interfaces and services to DFv2.

However, if your target driver has descendant DFv1 drivers that haven't
yet migrated to DFv2, you need to use the [compatibility shim][compat-shim]
to enable your now-DFv2 driver to talk to other DFv1 drivers in the system.
For more information, see
[**Set up the compat device server in a DFv2 driver**][set-up-compat-device-server].

## Before you start {:#before-you-start}

Prior to starting migration tasks, first check out the
[**Frequently asked questions**][faq] page. This can help you identify
special conditions or edge cases that may apply to your driver.

## List of migration tasks {:#list-of-migration-tasks}

- [**Update DDK interfaces to DFv2**][update-ddk-interfaces-to-dfv2]:
  Learn how to migrate the legacy DFv1 driver interfaces to DFv2.

  - [Update dependencies from DDK to DFv2][update-dependencies]
  - [Update the driver interfaces from DFv1 to DFv2][update-driver-interfaces]
  - [Migrate other interfaces][migrate-other-interfaces]

- [**Update other services to DFv2**][update-other-services-to-dfv2]:
  Learn how to migrate a driver's services other than the DDK interfaces
  to DFv2.

  - [Use the DFv2 service discovery][use-service-discovery]
  - [Update component manifests of other drivers][update-component-manifests]
  - [Advertise a service from the DFv2 driver][driver-communication]
  - [Use dispatchers][use-dispatchers]
  - [Use the DFv2 inspect][use-dfv2-inspect]
  - ([Optional) Implement your own load_firmware method][implement-firmware]
  - ([Optional) Use the node properties generated from FIDL service offers][use-node-properties]
  - [Update unit tests to DFv2][update-unit-tests]
  - [Additional resources][additional-resources]

<!-- Reference links -->

[faq]: faq.md
[driver-interfaces]: update-ddk-interfaces-to-dfv2.md
[update-ddk-interfaces-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/update-ddk-interfaces-to-dfv2.md
[update-dependencies]: update-ddk-interfaces-to-dfv2.md#update-dependencies-from-ddk-to-dfv2
[update-driver-interfaces]: update-ddk-interfaces-to-dfv2.md#update-interfaces-from-ddk-to-dfv2
[migrate-other-interfaces]: update-ddk-interfaces-to-dfv2.md#migrating-other-interfaces
[update-other-services-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/update-other-services-to-dfv2.md
[use-service-discovery]: update-other-services-to-dfv2.md#use-the-dfv2-service-discovery
[update-component-manifests]: update-other-services-to-dfv2.md#update-component-manifests-of-other-drivers
[driver-communication]: /docs/concepts/drivers/driver_communication.md
[use-dispatchers]: update-other-services-to-dfv2.md#use-dispatchers
[use-dfv2-inspect]: update-other-services-to-dfv2.md#use-the-dfv2-inspect
[implement-firmware]: update-other-services-to-dfv2.md#implement-your-own-load-firmware-method
[use-node-properties]: update-other-services-to-dfv2.md#use-the-node-properties-generated-from-fidl-service-offers
[update-unit-tests]: update-other-services-to-dfv2.md#update-unit-tests-to-dfv2
[additional-resources]: update-other-services-to-dfv2.md#additional-resources
[set-up-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md
[compat-shim]: faq.md#what_is_the_compatibility_shim_and_when_is_it_necessary
