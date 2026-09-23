# DFv1 to DFv2 driver migration

This playbook offers guidelines, best practices, examples, and reference
materials to help you migrate existing legacy DFv1 drivers, which are
stored in the Fuchsia source repository (`fuchsia.git`), to the new
[driver framework][driver-framework] (DFv2).

## Before you start {:#before-you-start}

Before you begin working on driver migration, review some of the
[key concepts][dfv2-concepts] related to DFv2 and familiarize yourself
with your DFv1 driver's unit tests and integration tests.

### Key differences between DFv1 and DFv2 {:#key-differences-between-dfv1-and-dfv2}

DFv2 enables Fuchsia drivers to be fully user-space
[components][components]. Like any other Fuchsia component, a DFv2 driver
exposes and receives [FIDL][fidl] capabilities to and from other components
and drivers in the system.

Notice the following key differences between DFv1 and DFv2:

- **DFv1**: Drivers are not components. The Banjo protocol is
  used for driver-to-driver communication. Driver host interfaces or the
  DDK ([Driver Development Kits][ddk]) wrapper are used to manage the life
  cycle of drivers.

- **DFv2**: Drivers are components. FIDL is used for all communication,
  including communication between drivers and non-drivers. The driver
  framework manages the life cycle of drivers.

### Expected outcome {:#expected-outcome}

Here is a list for the expected conditions of your driver after completing
the DFv2 migration:

- The driver can be registered with the [DFv2 driver manager][driver-manager].
- The driver can bind to a [device node][driver-node] in the system.
- Other Fuchsia components and drivers in the system can use the driver's
  capabilities.
- Fuchsia devices can be flashed with product images containing the driver.
- The driver passes all existing unit tests and integration tests.

## Driver migration playbook {:#driver-migration-playbook}

This playbook is designed to guide you through the migration tasks
in a linear manner, which are divided into the following two phases:

1. [**Migrate from DFv1 to DFv2**][migrate-from-dfv1-to-dfv2]: Update
   the driver's legacy DDK interfaces and other services to DFv2.
2. [**Migrate from Banjo to FIDL**][migrate-from-banjo-to-fidl]: Update
   the Banjo protocols used by the driver to FIDL to finish the
   migration.

However, keep in mind that depending on your driver's features or
settings, you may need to complete additional tasks that aren't covered
in this playbook.

## Extensions

The following guides are added to support tasks that were previously
missing in the playbook:

- [**Set up the compat device server in a DFv2 driver**][set-up-compat-device-server]:
  Set up the compat device server in a DFv2 driver and use it
  for communicating with DFv1 drivers.
- [**Connect and serve Banjo protocols in a DFv2 driver**][serve-banjo-protocols]:
  Serve Banjo protocols in a DFv2 driver and connect to
  its Banjo server from a DFv1 child driver.
- [**Set up services in a DFv2 driver**][set-up-services]: Set up services
  in a DFv2 driver so that the driver's services can be discovered
  by other Fuchsia components in the system.

<!-- Reference links -->

[driver-framework]: /docs/concepts/drivers/driver_framework.md
[components]: /docs/concepts/components/v2/README.md
[fidl]: /docs/concepts/fidl/overview.md
[driver-manager]: /docs/concepts/drivers/driver_framework.md#driver_manager
[driver-node]: /docs/concepts/drivers/drivers_and_nodes.md
[migrate-from-banjo-to-fidl]: /docs/development/drivers/migration/migrate-from-banjo-to-fidl/overview.md
[migrate-from-dfv1-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/overview.md
[set-up-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md
[serve-banjo-protocols]: /docs/development/drivers/migration/serve-banjo-protocols.md
[set-up-services]: /docs/concepts/drivers/driver_communication.md
[dfv2-concepts]: /docs/concepts/drivers/README.md
[ddk]: /docs/development/drivers/concepts/driver_development/using-ddktl.md
