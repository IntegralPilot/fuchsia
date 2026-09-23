# Deleting drivers

This page provides a list of the tasks to complete when you delete
an existing driver in the Fuchsia source tree.

When deleting a driver, do the following:

1. Get approval from at least one `OWNER` of the driver.

   If all of the `OWNERS` have abandoned the driver and are not
   responding, then an `OWNER` higher in the Fuchsia tree needs
   to approve the Gerrit change that deletes the driver.

1. Add an entry in the [`_drivers_epitaphs.yaml`][driver-epitaphs]
   file for each deleted driver.

   The following information must be provided:

   - `short_description`: A description of the deleted driver.
   - `deletion_reason`: The reason for the driver's deletion.
   - `gerrit_change_id`: The ID of the Gerrit change used to delete the driver.
   - `available_in_git`: A git hash of the `fuchsia.git` repository that
     contained the most up-to-date version of the driver before it was
     deleted.
   - `areas`: A list of areas for the driver from //build/drivers/areas.txt
   - `path`: The deleted driver's path in the `fuchsia.git`
     repository.

   An example entry:

   ```
   short_description: 'Qualcomm Peripheral Image Loading driver'
   deletion_reason: 'Hardware is not actively supported'
   gerrit_change_id: '506858'
   available_in_git: 'f441460db6b70ba38150c3437f42ff3d045d2b71'
   areas: ['Misc']
   path: '/src/devices/fw/drivers/qcom-pil'
   ```

<!-- Reference links -->

[driver-epitaphs]: https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/reference/hardware/_drivers_epitaphs.yaml
