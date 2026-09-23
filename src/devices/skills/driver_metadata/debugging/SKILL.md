---
name: driver-metadata-debugging
description: >
  Debug driver metadata delivery failures between a sender and receiver DFv2
  driver. Use when fdf_metadata::GetMetadata returns ZX_ERR_NOT_FOUND, the
  receiver gets stale or default values, or the metadata service is missing
  from /ns/svc -- diagnose with ffx component explore, verifying the sender's
  capabilities/expose, the receiver's use, a missing CreateOffer, populate-
  before-AddChild ordering, and that the service name equals the fully
  qualified FIDL type (not fuchsia.driver.metadata.Service). For generic FIDL
  routing not involving metadata, use the FIDL client debugging skill.
---

# Driver Metadata Debugging (DFv2)

## Verification Steps

### 1. Verify the Service in the Receiver's Namespace
The most direct way to see if metadata is reaching the receiver driver is to
explore its incoming namespace.

Run the following command to enter the component's sandbox:
```bash
ffx component explore <receiver-driver-moniker>
```

Once inside the shell, check the `/ns/svc` directory for the fully qualified
FIDL service name:
```bash
ls /ns/svc
```
* **Expected**: A service name matching the FIDL type (e.g.,
  `fuchsia.examples.metadata.Metadata`).

If the service is not listed, proceed to the troubleshooting section below.

## Troubleshoot Common Issues

### Handle Failed Retrieval (ZX_ERR_NOT_FOUND)

* **Missing Capability Routing in Manifest**:
  * Check the sender's `.cml` file. It must have a `capabilities` entry and an
    `expose` entry for the service.
  * Check the receiver's `.cml` file. It must have a `use` entry for the
    service.
* **Missing Offer in Code**:
  * The sender driver must generate a capability offer for the metadata and
    include that offer when creating the child node.
* **Service Name Mismatch**:
  * The service name in the `.cml` file must match the fully qualified FIDL type
    name (e.g., `fuchsia.examples.metadata.Metadata`). A common mistake is using
    a generic service name instead of the specific FIDL type.

### Handle Stale or Default Values

* **Order of Operations**:
    * Ensure the sender driver populates the metadata and serves it **before**
      creating the child node or at least before the child attempts to read it.

## Further Reading

* [Driver Metadata Implementation Skill
  (C++)](/src/devices/skills/driver_metadata/implementation/cpp/SKILL.md)
* [Driver Metadata Testing Skill
  (C++)](/src/devices/skills/driver_metadata/testing/cpp/SKILL.md)
* [/docs/development/drivers/tutorials/metadata-tutorial.md](/docs/development/drivers/tutorials/metadata-tutorial.md)
* [Driver Metadata Examples](/examples/drivers/metadata/)
