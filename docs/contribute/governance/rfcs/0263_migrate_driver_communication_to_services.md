<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0263" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Currently almost all driver access by non-drivers is mediated by the Device
File System (devfs). Unfortunately, devfs has several drawbacks:

 - It's difficult to restrict clients of devfs to specific protocols.
 - The API makes it difficult for drivers to expose more than one protocol.
 - Topological paths and hardcoded class paths expose clients to internal
   implementation details and can result in unintentional breakages.
 - Since devfs connections are unknown to the component manager, they cannot be
   used for dependency tracking
Furthermore, while it is currently possible for any driver and client to
migrate from devfs to aggregated services, there are currently significant
challenges that driver authors face when performing such a migration.


## Summary

We propose to shift the way drivers are accessed by non-drivers by migrating
such access to use aggregated services that are mediated by the component
manager. This migration will include both topological paths (/dev/sys/) and
class paths (/dev/class/). As part of this process, any remaining access to
drivers by clients using the fuchsia.device/Controller protocol will also be
removed. This RFC will cover the challenges faced in migrating to aggregated
services and propose solutions to ensure driver authors are not unduly
impacted. We also hope to provide more standardization for driver access, to
improve code health and reduce the difficulty of writing drivers and clients.

## Stakeholders

_Facilitator:_

_Reviewers:_

 - cja@
 - suraj@
 - csuter@
 - jfsulliv@
 - The component framework team


_Consulted:_

 - The driver team / driver authors

_Socialization:_

This RFC went through a design review with the Driver Framework team.

## Background
Up until recently, drivers in fuchsia were not components and the only way that
a non-driver could communicate with a driver was through devfs.
The driver manager maintains devfs and populates two branches that are
relevant here:  /dev/sys/ and /dev/class.

 * /dev/sys/ is populated starting at the board driver as root, and represents
   the hierarchy of parent-child binding relationships in the driver manager.
   Paths in this branch are referred to as "Topological paths". Topological
   paths can be opened by components with access to the "dev-topological"
   directory capability.
 * /dev/class/ is populated by a driver specifying a class name when adding a
   child device. Driver manager then adds an entry to /dev/class/<class-name>/
   which corresponds to the driver. This allows drivers with similar outputs
   to be grouped together, for example all cameras may have an entry in
   /dev/class/camera. Paths in this branch are referred to as "Class paths".
   Class paths can be opened by components with access to the "dev-class"
   directory capability, which is often further restricted to access of a
   specific class folder.

The driver entries in either branch allow two types of access to a driver:
device_controller and device.

 * device_controller provides a fuchsia.device/Controller interface, which
   allows components to query, bind and rebind the driver.
 * device provides a driver specific interface to the driver, that is in
   practice associated with the class name that a driver specifies when adding
   a child device. For example, it so happens that all the drivers that
   currently add a device with the class name "camera"  serve a
   fuchsia.hardware.camera interface, and connecting to /dev/class/camera/000
   (if it exists) will instantiate a connection to that driver's
   fuchsia.hardware.camera interface.

It has been a major effort for the last few years to isolate and remove uses of
the fuchsia.device/Controller interface, and the removal of those uses will be
considered out of scope for this RFC. It is mentioned here for completeness,
and to note that no similar interface will be made available through aggregated
services.

## Motivation

The primary goal of this migration is to remove the devfs system of accessing
drivers and replace it with a more standardized service-based connection model.
This approach has a number of benefits, many of which are related to the
deficiencies of devfs:

 * Increased visibility of dependencies and connections. By having clients
   connect through a service capability, the component manager can track
   connections and maintain accurate dependency graphs. This is not possible
   in using devfs, as its structure is largely opaque to the component manager.
 * Services provide a better documented and more flexible way to export
   capabilities from a driver. With devfs, only one protocol can be offered
   per device instance, and the protocol type is never explicitly defined.
   With Aggregated services, multiple protocols can be offered, and each is
   explicitly listed in the service definition.
 * More explicit and tightly controlled access. A component must specify which
   type of service it is requesting, in contrast to topological paths in devfs,
   which can allow a component access to any driver.
 * Remove dependency on topological paths. Topological paths provide
   visibility into how each driver is mounted by the driver manager.
   Hardcoding a topological path therefore represents a dependency on inner
   workings of the driver framework, which we hope to avoid.

Furthermore, devfs access is accomplished in various ways, both synchronously
and asynchronous, and by class path and topological path. We hope to provide
a single method for connecting to the services offered by drivers and
non-drivers alike, which should improve code health and reduce the difficulty
of writing drivers and clients.


## Requirements

The migration must provide solutions for all current users of devfs,
so no functionality is lost.
Further, drivers and clients must be able to perform soft migrations,
adding services and connecting to services without requiring that all
clients and drivers who provide a service migrate simultaneously.

## Design

### Driver framework work
Each migration of a client or driver away from devfs will have to be
addressed individually. However, some work will be required of the
driver framework to assist in addressing the issues listed above.

#### class name <-> protocol / service mapping
To aid in conversions, a mapping between class name, protocol name
and service name will be collated in a c++ header. Currently no explicit mapping
exists - there is merely an implicit agreement between clients and
drivers that each class corresponds to a specific fidl protocol.
Making this mapping explicit can enable the following steps:

 * Ensure that clients and servers of a `/dev/class/<classname>`  agree on a
   set protocol
 * Allow clients to migrate from connecting to `/dev/class/<classname>`  to
   `fuchsia.hardware.<protocolname>`.

This mapping will require making a service for every class name
(there are currently ~200 possible class names)  Additional entries
in this mapping may also need to be created for services that are
currently only accessed by topological path. This mapping is intended to last
only for the duration of the migration. Once each class is migrated, its entry
in the mapping can be removed.

#### Propagation of Bus information
With the removal of topological paths, the driver manager will take
on the responsibility of propagating bus information from bus drivers
to all descendant drivers, for use in identification of service
instances. A driver library will make storing and retrieving this
information easier.

### Enabling Soft Migration:

When drivers and clients migrate from devfs to services, we must ensure that one
side is capable of handling or providing both connection types, to provide
continuity of connections. Further, we must make sure that the other side does
not provide both connection types, in order to prevent duplicate entries. For
example, If driver A and B both provide Foo connections to clients C and D, if
driver A were to naively switch to services, then the clients would have to
choose between devfs and services, and then only be able to see one offering.
The solution is a multi-step migration. There are several options for achieving
this soft migration, with some additional work on one of the two sides.
Regardless of option, migration stages can progress on a per-class basis to ease
roll-out, which also will allow different classes to customize their migration
strategy.

#### Proposed option: Automatically advertise services through devfs

Modify devfs to automatically advertise a service for every devfs entry. The
service type would be indicated by the class name, using the class name ->
service mapping indicated earlier. In the same change, devfs would also expose
all service capabilities that correspond to the mapped class names to the
bootstrap component. Once services are enabled for a class name, clients can
convert to using services, followed by drivers switching to advertising
services. Driver migration would simply involve removing DevfsAddArgs, and
instead calling AddService. The biggest downside is that driver conversion
would be blocked on all clients of a class type converting.

Migration order would be:
 1) Change devfs to advertise services
 2) Clients convert to services
 3) Drivers advertise services and remove devfs

#### Alternative 1: Create a client tool: DeviceAndServiceWatcher

Create a tool that, when given a class name / service name, connects to both the
aggregated service and the /dev/class/<classname> directory and waits for
entries to appear. When a service or directory entry is added, the client will
be notified and provided with an aggregated service instance, regardless of
source. In order for this tool to be effective, we will require that drivers
never advertise both a service and a devfs entry. This option will be more
difficult to implement, and poses some difficulties in terms of wrapping a fidl
channel to look like a service to make devfs entries behave like services.
However, like the preferred option it does limit the drivers to making just one
change and being sure it is effective. It also requires all clients of a class
to convert before drivers can convert. Clients may fail when drivers switch to
services though, as they may not have the capabilities routed correctly.

Migration order would be:
 1) Add DeviceAndServiceWatcher
 2) Clients migrate to use DeviceAndServiceWatcher
 3) Drivers advertise services and remove devfs
 4) Remove devfs logic from DeviceAndService Watcher when all migrations are
 done


#### Alternative 2: Lazy devfs removal
Have drivers continue to provide a devfs entry after they advertise a service.
After clients convert, drivers can remove devfs code. This has the advantage
of not blocking driver conversions, although later changes will still need to
be made to the driver. Additionally, this has several downsides: The new
AddService code will not get immediately exercised so additional changes to the
driver may be necessary when the clients finally switch to using services, in
addition to the changes the driver will have to make to remove the devfs code.

Migration order would be:
 1) Drivers add services
 2) Clients convert to services
 3) Drivers remove devfs

## Implementation

This migration can be done incrementally for each set of drivers and clients
that interact with a specific class name. A few tools and libraries will need
to be provided, the driver framework will perform several migrations to ensure
that the libraries meet the needs of the migrating drivers and clients. After
ensuring that the migration process is sufficiently ergonomic, migration of
class names will be added as an open task that the broader fuchsia community
can participate in. Specific classes/interfaces will be identified as needing
more attention, and will require a more coordinated approach. This includes
any clients that still use the fuchsia.device/Controller interface, which will
need to be addressed before those clients can migrate. Once all drivers and
clients have migrated to services, the internal implementation of devfs can be
removed.

### Migration mechanics

To further break down how migration will be
achieved, here are the steps for converting a specific class of driver:

### 1) Add class name <-> protocol / service mapping

 1. The mapping of the class to fidl protocol must be deduced from current
    usage.
 2. A service will then need to be created to wrap the protocol, and the
    mapping of the class to service will be recorded in the appropriate header.
    That will allow devfs to start advertising the service.
 3. The CML files will then need to be updated to route the service to the
    clients.

### 2) Client Migration
Client migrations will vary in complexity, as clients could be using either
topological paths or class paths. Client migration can be broken into two
stages, identification of the desired service provider, and connecting to the
service. The first step may already be done by clients using class paths.

#### Identification of the desired service provider
This migration step must be implemented by clients which do any of the
following:

 - Use topological paths
 - Use a hardcoded class path (eg. /dev/class/camera/000)
 - Connect to the `device_controller` for a given devfs entry and call
   `GetTopologicalPath()`.

In each of the above situations, the client is migrating from a situation where
the provider of the service/fidl interface was (or appeared to be) known
synchronously to an aggregated service, where instances will be added and
removed asynchronously, and the instance name cannot be used for
identification.

There are several options available for such clients:

 - Connect to the first service instance available. This is often sufficient
   for simpler tools and tests where only one provider of a service is
       expected, such a power controller. This also allows the client to
       maintain the assumption that connections are synchronous. This is not
       the generally preferred solution, as it ignores the asynchronous nature
       of the system.
 - Query information from each service instance to determine if it is the
   correct instance. This can be achieved in two ways:
   - Connect to the fidl interface for each service instance. Many fidl
     protocols specify information about the device already (for example, the
     `fuchsia.camera/Control` protocol provides a `GetInfo` call. If the
     protocol being served does not currently contain identifying information,
     it could be modified to add such device-specific information.
   - Add an additional `GetInfo` protocol to the service, that can contain the
     device identifying information. Separating the identification interface
     from the main communication interface has several advantages:
     - The identification interface can be mostly handled by a driver library,
       as it provides a static set of information.
     - The identification interface would preferably handle multiple
       simultaneous connections to aid in querying, whereas most drivers cannot
       handle multiple simultaneous connections to their other interfaces.
     - Separating the identification logic makes it easier for future work to
       integrate identification steps into the framework.
   - The main disadvantage of a `GetInfo` interface is that it would take more
     information from the driver to populate, and thus be more difficult for
     devfs to provide automatically. In this case, drivers would need to amend
     their DevfsAddArgs to add the relevant data, and devfs would handle
     serving that interface. This would have to be done by the drivers before
     clients could convert to services.
 - An option that may exist in the future would be to add tags to the component
   to indicate information about the specific device. The client could then
   query the tags associated with every service instance.

#### Device Specific information and Bus information:

There is a lot of information that could help clients identify the correct
driver when connecting to an aggregated service. This information could
include the Device ID, (DID), Product ID (PID), Vendor ID (VID), as well as any
amount of metadata that is passed to the driver. However, when migrating from
topological paths, there should only be three pieces of information that a
client could be losing by not using the topological path:  The driver name,
(which is known by the driver), the bus type and the bus id/address. To
compensate for the loss of the bus information,  The driver framework will add
the ability to propagate bus type and address from the bus driver to all
descendants. This is currently provided as metadata to several types of bus
drivers (such as i2c), so implementation will just involve tagging the bus type
and address as a specific metadata entry, and ensuring that it always
propagates to the children. The driver can then make bus information available
to clients using one of the options above.

#### Connecting to the service provider

Once clients have a strategy for determining which service instance to which
they wish to connect, the connection itself is very similar to connecting to a
class path.

 - The client must ensure that it has access to the capability - it can replace
   the "dev-class" capability with the service capability that it is expecting.
   This may need to be done in multiple files, depending on the topological
   location of the client. Also of note: some shards include propagation of
   the 'dev-class' capability, so it may not always be apparent where the
   capability comes from.
 - The ideal connection method for both devfs and services involves a
   DeviceWatcher. This library notifies the client asynchronously of new
   service provider instances. The biggest change should just be the location
   that is being watched.

### 3) Driver migration
*As noted above, if using a `GetInfo` interface in the service as driver will
be advertising, drivers would need to amend their DevfsAddArgs to add the
relevant data, and devfs would handle serving that interface. This would have
to be done by the drivers before clients could convert to services.*

Driver migration involves calling `AddService` and possibly wrapping served
protocols in a service. If a GetInfo interface is used, the driver will provide
a driver library with the static information to be served instead of passing it
in the DevfsAddArgs. Simultaneous to adding the `AddService` call, the driver
will remove the `devfs_args` from the NodeAddArgs entry. This will disable all
devfs entries associated with the driver.

## Performance

This migration will have negligible effects on performance. Establishing
connections to components is usually done only once in a component's lifcycle,
and the difference in connection types is minimal.

## Ergonomics

Ergonomic concerns have been forefont in the design of this migration. The main
concerns involve removing access via topological paths:

 - It will be more difficult to differentiate some drivers without using
   topological paths, since clients will have to connect to each service to
   determine if it is the desired service. This disadvantage will be overcome
   by providing client libraries that allow the client to provide a conditional
   function which can help select the correct device without added boilerplate.
 - Drivers may need to add interfaces or functions that include identifying
   information. Client libraries can also be provided to assist the driver in
   serving any `GetInfo` protocols.
 - It will no longer be possible to use the shell to navigate the device tree
   using the /dev/sys directory. We hope other ffx tools will serve to meet
   these needs.
Additionally, there will be additional routing that is required to route each
service capability to the clients.

Overall, we feel that the additional complexities are far outwieghed by the
positive gains that this migration will provide.

## Backwards Compatibility

We are providing a soft migration path for drivers and clients that use devfs,
but after a class is converted, we will be ending support for devfs for that
class. Our goal is to remove all instances of legacy interfaces, so we are not
planning on doing any backwards compatibility support after the initial
migration period.

Once all drivers and clients are migrated, devfs will be removed, which means
any other clients will be unable to access drivers via devfs.

## Security considerations

This change will enhance security in a few ways:

 - Clients will no longer be able to use topological paths, which offer
   unrestricted access to any driver in the subtree.
 - Clients will no longer be able to use the fuchsia.device/Controller
   interface whenever they have access to a driver
 - There will be more explicit tracking of what services and protocols each
   client has access to System and component shutdown will become more
   transparent, as dependencies can be tracked explicitly.
 - Bus information will now be shared with drivers, where it was not before.
   This information will be encoded in enums, to limit potential of releasing
   PIIs. Whereas the driver does not need to know its parent's bus
   information, we don't believe that sharing this information with the driver
   poses a security risk.

## Privacy considerations

N/A

## Testing

Testing for this migration will mostly include end-to-end and smoke tests as
well as any relevant host tests, as if anything is done wrong, clients will not
be able to connect to their service providers. Since this change will affect a
number of host tests, we will encourage migrators to manually test with all host
tests that are affected. We are taking the opportunity, as part of removing
reliance on topological paths, to revamp the device enumeration tests. Instead
of checking a specific path, device enumeration tests will instead be checking
if all driver packages are being loaded.

Tests that use DriverTestRealm may be affected by the migration, and should be
treated the same as other clients for the purpose of the migration. Routing
will need to be added to accommodate any tests that use DriverTestRealm.

## Documentation

We are planning to accompany the migration with a detailed explanation of how to
migrate a class of drivers and clients. We will also be providing examples and
best practices going forward on how to connect to services asynchronously. As
use of devfs is eliminated, we will scrub out documentation to remove references
to it, and make sure all driver documentation describes advertising and
connecting to services. In anticipation of the increased dealings with
capability routing on the part of driver authors, we plan to augment the current
documentation on how to ensure a capability is routed to the correct component,
as well as how to debug frequently encountered issues with capability routing.

## Drawbacks, alternatives, and unknowns

### Drawbacks
The main drawbacks in migrating from devfs to services is the service
connections will now be strongly typed, and capability routing must now specify
the service being routed. Both of these changes make the system less forgiving
to set up, but also provide more checks to ensure that it is being used
correctly.

Another drawback of migrating to services is the potential decrease in
stability of component connections. Aggregated services are relatively new,
and there may be unforeseen corner cases that may be exposed through this
migration. Further, this migration involves drivers migrating off of a system
managed by the driver framework team and onto the component framework, which is
supported by its own team. There is always a risk of miscommunication between
teams that could lead to required features losing support.


### Alternatives

Several alternative migration options are discussed in the implementation
section. The main alternative to this migration is to not do it. Although new
drivers and clients would still prefer to use services, devfs could be kept
around for older drivers. Doing this would make adding newer components more
difficult, and would continue to complicate the shutdown process, increasing our
number of flakes. Keeping devfs would also continue to block our cleanup of the
driver lifecycle and generally slow team progress, as we would be forced to work
around the legacy systems.
