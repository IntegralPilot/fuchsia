# TA Manager

This directory contains the TA Manager component and tests. The manager receives connection requests
to TAs and instantiates TA instances to service them. The manager is also responsible for
maintaining the mapping between UUIDs and specific implementations.

## Configuration

The TA Manager is designed to be instantiated in realms that may support one or more Trusted
Applications. The specification of which TAs exist in a realm and how they are configured must be
provided in a separate package from the manager implementation so that they can be validated
separately. The manager requires a directory capability named `config` that contains a file for each
TA provisioned in the realm. The files must be named `<UUID>.json` and contain a single JSON object
with the following keys:

* "url", string: Fuchsia component URL for the implementation selected for this UUID
* "singleInstance", bool: value of the gpd.ta.singleInstance property that controls if multiple
connections are routed to the same instance of the TA or separate instances
* "capabilities", list of scopes: list of capabilities required by this TA. Currently unused.

## Structure

The manager creates a child collection called `ta` and instantiates TA instances as child components
within this collection as required. Each child is expected to expose the protocol
`fuchsia.tee.Application` to its parent. The manager actively proxies all messages on this protocol
through the child so that it can set the return origin on replies.

## Properties

TA manager hosts system properties which TAs may query via the internal core API Property family.
These properties are expected to reflect a mix of platform-specified and hardware-specific values.
The complete properties file served in a deployment should be constructed at assembly time. The
config/system_properties.json5 file contains base platform-specified values.

## Testing

A test configuration of TA Manager is provided in the package ta-manager-realm-test. This differs
from the production configuration in that instead of the manager directly managing a collection and
using the `fuchsia.component.Realm` capability from the framework the test manager does not have a
collection and it uses the `fuchsia.component.Realm` capability from the parent. This way a hermetic
test package can host a collection and route the `fuchsia.component.Realm` from the framework to the
test manager instance so that TAs can be provided in subpackages of the hermetic test package.
