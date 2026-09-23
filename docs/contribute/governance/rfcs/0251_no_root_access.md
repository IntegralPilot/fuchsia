<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0251" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

This RFC details how and why to create fine tuned resources rather than
accessing the root resource. An end goal is to completely eradicate the root
resource. Due to its pervasive use, we need to be thorough about how to remove
the resource effectively without breaking any of the myriad pieces that
currently depend upon it.

## Motivation

The root resource is a powerful capability that allows access to hardware
resources and exposes a large surface area of highly privileged access across
the system. The root resource allows access to its constituent resources.
Historically, it has been difficult to split up the root resource into its
constituents as only the kernel, board driver, or specified device drivers would
have the knowledge to specify valid ranges for the more specialized resources.
Although expedient, this pattern of passing the root resource via the
`component-manager` along to components in order to access specialized resources
does not follow the principle of least privilege. We should only send the
resources that the components require and completely remove the root resource.

## Stakeholders

*Facilitator:* leannogasawara@google.com

*Reviewers:*

*   Security (<pesk@google.com>)
*   Driver Framework (<surajmalhotra@google.com>)
*   Component Framework (<geb@google.com>)
*   Zircon (<maniscalco@google.com>)
*   Drivers (<cja@google.com>)

## Requirements

The root resource is being used in places where a finer tuned resource should
instead be exposed. Majority of call sites have been updated to access the
specialized resources. In places where this is not currently possible, there
must be new resources defined, routed, and exposed so that the system upholds
the principle of least privilege.

## Design

The `component-manager` can host a service per resource and route each resource
kind individually. This is already implemented for many resources but formally
should be the design for handling all resource kinds rather than making use of
the root resource to access specialized resources.

## Implementation

In places where the root resource is still being used, we should identify the
specific resource that the process needs and create a more fine-grained resource
with reduced privileges. The new resource would get routed to the components in
place of routing the root resource. The list of finer grain resources that
should exist is:

*   cpu resource: get and set CPU performance parameters
*   debug resource: for accessing various kernel debug syscalls
*   energy info resource: access energy consumption information
*   debuglog resource: enable reading and writing to a debuglog
*   framebuffer resource: getting and setting bootloader framebuffer
*   hypervisor resource: creating a virtual machine that can be run within the
    hypervisor
*   info resource: used for kernel stats
*   iommu resource: used by the iommu manager
*   ioport resource: gate access to x86 ioports
*   irq resource: gate access to interrupt objects
*   mexec resource: used for booting the system
*   mmio resource: gate access to mmio
*   msi resource: used to allocate MSI range on the PCI
*   power resource: manipulate CPU power
*   profile resource: used to create a scheduler profile
*   smc resource: used by the platform device protocol to access SMC ranges
*   vmex resource: mark VMOs as executable

These new resources will replace system calls that currently depend on the root
resource so that each call gets the least privilege necessary to work as
intended.

For each new resource identified:

*   If the resource corresponds to a ranged resource, it should have its own
    defined kind and no base. This includes ioport, irq, mmio, and smc
    resources.
*   For all other resources, the resource kind will be system resource with a
    newly defined base type.
*   Existing kernel syscalls will be updated to accept and validate the finer
    grained resource in addition to the root resource.
*   Component-manager will need to introduce new services to provide the handles
    to the new resources.
*   Call sites and corresponding cml files get updated to use more fine grained
    resources.

When all calls requiring access to the root resource have been replaced with a
more specialized resource, the `fuchsia.boot.RootResource` service can be
removed from `driver-manager` and `component-manager`. Its exposure can removed
from policy files so that security review is required should a future programmer
attempt to use the resource before it is completely eradicated.

At this stage, the root resource will only be created by the kernel and passed
on to user space as the first process of the system. The kernel can be changed
to stop creating the resource and update its syscalls to only validate against
the finer grained resources. The root resource shall no longer exist.

The new services hosted within component_manager should look like:

```


@discoverable
closed protocol IommuResource {
    strict Get() -> (resource struct {
        resource zx.Handle:RESOURCE; }
    );
};


```

## Performance

No change expected.

## Security considerations

This RFC improves the security of the system by reducing the surface area of the
root resource, and correspondingly its highly privileged access, to only the
kernel. The root resource was initially exposed because it was expedient to do
so. User space should not have access to the root resource especially when there
exists a straight forward implementation for how to use finer grained resources.
Downstream there are more resources to maintain but components should be
responsible for knowing specific resources that they require rather than using a
very powerful root resource.

## Privacy considerations

This RFC does not impact privacy.

## Testing

There exist tests for the already existing constituent resources in
`component-manager`. New resources can copy this example to ensure that they
work as intended. When removing routing of `fuchsia.boot.RootResource` protocol
from a cml, CICQ will break if there are still calls to the root resource. A
passing CICQ should be sufficient to test that the new resource is working and
that the root resource is no longer necessary.

## Documentation

As new resources are created, the [`resource documentation`] will need to be
updated as well. Many tests reference a fake resource that is simply called
`root_resource`. The variable names should also be updated to refer to the more
specific resource where applicable.

## Drawbacks, alternatives, and unknowns

### Alternative: Do nothing

The alternative to this RFC is to let sleeping dogs lie. The system works as is
and there are less resources to maintain. However, the status quo violates the
promise that a program will only have access to the resources that it needs.
Nothing in user space needs access to the root resource. The root resource is a
powerful capability that can do much more than is necessary. A finer tuned
resource routing makes it easier for contributors to ensure that their
components are only capable of accessing the least amount of privileges
required.

Another alternative is to use the root job. All other jobs descend from this
process so much like how the root resource can create all other resources.
Replacing the root resource with the root job is possible but does not solve the
problem of reducing access nor does it introduce a solution of finer grained
access.

## Future Work

As of today, the root resource is no longer being served by the component or
driver framework nor is it in core-tests. It is not being offered through policy
files. It is still being created by the kernel and validated in resource checks.
Removing the validation is the last step to eradicating the root resource.

[`resource documentation`]: /reference/kernel_objects/resource.md