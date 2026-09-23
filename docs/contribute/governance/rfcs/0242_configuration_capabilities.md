<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0242" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Outline the use of configuration [capabilities][capabilities] in the
component framework and the syntax that will be added to CML files to support
them.

It should be noted that this document supercedes parts of the earlier Structured
Configuration RFCs, which are mentioned in the prior arts section.

## Motivation

Components in Fuchsia use [structured configuration][rfc-0127] to get
configuration values. Today configuration values are mostly loaded out of a
component's package. It is possible for a parent component to imperatively set
configuration values when launching a dynamic component, but most components in
Fuchsia are static.

There are multiple motivations for adding this feature:

- Allowing a static parent to configure a child's config.
- Routing a single config value to multiple components.
- Providing configuration values from a CML file.

The first customer for Configuration Capabilities is system assembly, as this
feature will make it possible to remove technical debt. The technical debt
exists because there is currently no way to provide configuration to a static
component besides editing that component's package. System assembly currently
opens fuchsia packages, edits their configuration value files, and then
re-packages them. This is technical debt as it changes the hash value of the
package and the config value file is technically a "private" API of the
component that is not easily evolvable.

For out of tree packages that cannot be rebuilt, configuration capabilities
provides a way for those components to be configured. There is currently no way
to edit those component's configurations within a static topology.

This feature will likely be useful in many future configuration use cases.

## Stakeholders


_Facilitator:_ neelsa@google.com

_Reviewers:_

- System Assembly: aaronwood@google.com
- Component Framework: geb@google.com
- Security: markdittmer@google.com
- Driver Framework: surajmalhotra@google.com
- Component Framework: wittrock@google.com

_Socialization:_ This issue has been discussed with the Component Framework team
before writing the RFC.

## Requirements

Configuration Capabilities allows CML authors to set the configuration values of
static components. They allow for the routing of configuration values through
the topology so that multiple components can use the same value. Routing values
through the topology allows indirect ancestors to specify values, where the
existing feature only allows direct parents to specify values.

Configuration Capabilities should follow existing Component Framework features
when possible to be consistent with the rest of the system.

## Design

The bulk of the design is CML syntax for declaring and routing a configuration
capability. A configuration capability has both its type and value defined in
CML. A configuration capability is routed to a component that wishes to use
it. The component using the configuration capability will be able to see the
value when the component is started by using the existing structured
configuration libraries.

Below is an example for defining a configuration capability:

```json5
// Configuration capabilities can be defined in any component.
// When a configuration capability is defined, a value must be set for it.
capabilities: [
  {
    // Define the name of the capability.
    config: "fuchsia.netstack.UseNetstack3",
    // Define the type of the configuration.
    type: "bool",
    // When defining a configuration capability, there *must* be a value.
    // If the route for the configuration capability ends at this definition, the
    // value of the capability will always be this value.
    value: "true"
  },
  // The below shows a string config.
  {
      config: "fuchsia.config.MyString",
      type: "string",
      max_size: 100,
      value: "test",
  },
  // The below shows a vector config.
  {
      config: "fuchsia.config.MyUint8Vector",
      type: "vector",
      element: { type: "uint8" },
      max_count: 100,
      value: [1, 2, 3 ],
  },
]
```

The `type`, `element`, `max_count`, `max_size` fields on the `config` capability
have identical meanings to their use in the `config` stanza. All values that are
supported in the `config` stanza are supported in the `capabilities` block.

The `type` field supports the following:

- bool
- uint8, uint16, uint32, uint64
- int8, int16, int32, int64
- string
- vector

The using block supports the same `type`, `element`, `max_count`, and `max_size`
fields as the capabilities block.

Below is example syntax for using a configuration capability:

```json5
// Using a configuration capability means that this configuration will show
// up in the component's structured config at runtime with the specified `key`
// name.
// NOTE: Using a configuration capability will override the value in the Config
// Value File (CVF) and the value obtained from the "mutability" setting.
use: [
  {
    config: "fuchsia.netstack.LaunchProcess",
    // This is the name that the component will see in its Structured Config
    // struct. If the `use` is optional this must match a name in the "config"
    // block of the component's manifest.
    key: "can_launch_process",
    // The using field needs the type information so that the component's
    // Config Value File format can be created.
    type: "bool",
    // From is identical to other capabilities.
    from: "parent",
  },
  // The below shows a vector config.
  {
    config: "fuchsia.netstack.MyUint8Vector",
    key: "my_vector",
    type: "vector",
    element: { type: "uint8" },
    max_count: 100,
    from: "parent",
  },
  // The below shows a string config.
  {
    config: "fuchsia.netstack.MyString",
    key: "my_string",
    type: "string",
    max_size: 256,
    from: "parent",
  },
]
```

Below is example syntax for routing a configuration capability:

```json5
// Expose and Offer behave the same as any other capability.
expose: [
  {
    config: "fuchsia.netstack.UseNetstack3",
    as: "fuchsia.mycomponent.UseVersion3",
    from: "self",
  },
]
offer: [
  {
    config: "fuchsia.config.MyString",
    as: "fuchsia.mycomponent.ProcessName",
    from: "self",
    to: "#mychild",
   },
]
```

### Optional Capabilities

If a component is using a configuration capability with the availability set to
`optional`, and that capability is being routed from `void`, then the component
will receive the value from the Configuration Value File (CVF). This feature
exists to support soft migrations for updating configurations.

### Deprecation of the 'config' block

Before configuration capabilities, the config fields were declared in the
`config` stanza in the CML. This information is now being replaced by the
information in the `use` stanza. This means that the Structured Configuration
schema that is generated for a component will be a union of the field
declarations in the `use` block and the `config` stanza.

Once all structured config clients have been migrated to configuration
capabilities, support for the config block in CML will be removed.

### Deprecation of the 'mutability: parent' feature

Before configuration capabilities, a config value could be declared as
`mutability: parent`, which would allow the parent of that component to
change the value.

Users of this feature will be migrated to `use` a configuration capability
from their parent. Once all users have been migrated, this feature will be
removed.

## Implementation

The implementation of these CML features is relatively straight forward and can
be done in tree in a few CLs. Clients can migrate to using configuration
capabilities at any time as this does not affect existing structured
configuration features.

We will need to ensure Scrutiny understands the configuration capabilities and
can verify that a specific product has a given configuration.

## Performance

Starting a component may become slightly slower due to Component Framework
needing to perform routing on the configuration capabilities a component uses.
This could be significant if the capabilities are coming from a component that
is unresolved and needs to downloaded. This should be negligible for resolved
components.

## Ergonomics

The ergonomics for this new CML capability are identical to existing
capabilities. It is a bit verbose to add and route a configuration capability,
but the syntax was chosen to match existing capability syntax as much as
possible. Where possible, the syntax was also chosen to match the existing
`config` block syntax.  When the two syntax's conflicted, the existing
capability syntax was preferred. Matching existing syntax reduces cognitive
effort by users familiar with the component framework.

The CF team may decide to keep the `config` block as syntactic sugar for
declaring a configuration capability, or add additional syntactic sugar if we
need better ergonomics.

## Backwards Compatibility

This is a new feature. It is backwards compatible with existing structured
configuration features.

It is worth noting that routing a configuration capability to a component will
always take precedence over the value in the CVF file. If a component has an
optional configuration capability route and the route is not present, then the
value in the CVF file will be used.

## Security considerations

This feature should not impact security. Scrutiny and other tools to
investigate CML will work on this new capability type.

It could be argued that setting capabilities in this way will be more visible
than the existing strategy of editing values within a component's package.

If a component is using a required configuration capability and one is not
routed to it, the component will be unable to start. This should not be a
security problem because the parent of a component is trusted, and these routes
can be statically verified to be correct.

## Privacy considerations

This proposal should have no impact on privacy.

## Testing

This feature will need unit and integration tests. The general testing areas
are:

- Parsing the new CML syntax.
- Routing the new capability through the topology.
- Attempting to start a component with missing configuration capabilities.
- Attempting to start a component with a configuration capability of the wrong
  type.
- Starting a dynamic component with a configuration capability.

The feature will need the following tests added to Scrutiny:

- Testing that Scrutiny can resolve values for configuration capabilities.
- Testing precedence/override logic for different sources of config values.
- Testing interactions between configuration capabilities, the config block, and
  mutability.

None of these tests require new test infrastructure.

## Documentation

Fuchsia.dev will need to be updated to have documentation on configuration
capabilities. The existing capability docs will be used as a template for this
new documentation.

## Drawbacks, alternatives, and unknowns

### Alternative: Implement a configuration override service

The original
[Structured Configuration RFC][rfc-0127]
proposed that structured config is directly sourced from a component's package.
The RFC specifically calls out that it is not attempting to address
"configuration data that is set by other components (except parent components
and admin components)". One alternative is to continue to not support a
capability based configuration scheme.

One of the drawbacks to Configuration Capabilities instead of the override
service is that Configuration Capabilities is more verbose and more complicated.
Implementing something globally is always going to be simpler initially.
However, Fuchsia has found that capability based systems end up being more
composable and understandable in the long run. It is very helpful to be able to
say that a specific configuration is only used by a subset of the component
topology, or to use two different values within two different components.
Configuration Capabilities make this easier to express and embodies the
capability based system used elsewhere in Fuchsia. Using the existing CML
syntax for configuration allows the feature to be understood more easily by
fuchsia developers, and it composes well with component framework's existing
tools.

### Drawback: Many additional use fields

Keeping the type information in the `use` block adds 6 additional fields
which are only valid for configuration capabilities.

It would be possible to combine these 6 fields into one field with subfields,
but adding the fields was preferable as it kept the syntax close to the
existing `config` block syntax.

### Drawback: Conflating type information and routing

One drawback to putting the configuration schema in the `use` declaratio is
that the `use` block defines the type information and also defines routing.
Some components may wish to define their configuration without specifying
routing, and then route their configuration differently in different scenarios.
This is simpler if the type information is in a different place than the Use
block.

Component authors that want to change how their types are defined must modify
their `use` blocks instead of only modifying their `config` blocks.

If the combination of configuration definition and routing becomes a persistent
pain point for developers, then Component Framework team may revisit this
decision.

### Alternative: Keep configuration type definition in the `config` block

One alternative is to not have a "type" field in the `use` declaration, and to
instead rely on the type information in the existing "config" block. This was
the case in the earlier versions of this RFC. This alternative addresses the
current drawback of conflating type information and routing. It makes it easier
for clients to define configuration in one place and to route it in another
place (possibly including different CML shards to route differently based on
different configurations).

In the current Structured Configuration-based implementation in which Component
Manager "pushes" configuration to the component when launching it, Component
Manager needs to ensure that the routed capability's type matches that of the
target Structured Configuration field. It was decided that the type information
would be placed on the `use` declaration because this is symmetric with the
Capability Declaration. When Component Manager performs routing it needs to
ensure that the type information lines up, which means that logically the
information should be contained at both ends (Capability and Use) of the route.

Another reason for keeping the type information on the `use` declaration is that
it becomes possible to keep all of the information in the `use` declaration and
not require a matching "config" block entry. This keeps the information in a
single location in the CML file, which makes it easier to audit.

### Alternative: `as` field in `use` declaration

With the Structured Configuration field name and type information encapsulated
within the `config` block, `as` could be used to specify the field to which the
capability is being provided. This would be consistent with how most non-`path`
"renaming" is done in CML.

The `key` term was used instead because the field operates differently than
other `as` fields. `as` is normally optional, but `key` is required. `key` must
also match an existing key in the `config` block if the capability is optional.

### Unknown: Balancing existing Structured Config inconsistencies with Capabilities

There are a number of ways that the existing Structured Configuration is
inconsistent with how other Capabilities work.

One inconsistency is that the program is deeply dependent on the format of the
Structured Configuration that is defined in the component manifest. Normally it
is the manifest that depends on the program, and Structured Configuration makes
this relationship circular. This is visible in the fact that the Structured
Configuration build rules require adding an extra layer between the component,
component manifest, and the program. This circular inconsistency will not be
addressed by Configuration Capabilities, but it should hopefully be addressed
in a followup RFC.

Another inconsistency is that Structured Configuration is "pushed" to the
component when it is started, where other capabilities are "pulled" as the
component accesses them. This inconsistency is also not addressed in this
RFC. The purpose of this RFC is to add routing and capability support within
Component Manifests. This RFC does not change the interface between a program
and Structured Configuration, so that the number of migrations needed is
limited.

Fuchsia will be in a better place to address these inconsistencies after
implementing Configuration Capabilities and seeing how they are used. More
client data will help us address pain points and implement followup changes.

## Prior art and references

Prior structured configuration rfcs:

- [rfc-0127]
- [rfc-0146]
- [rfc-0215]

[capabilities]: /docs/concepts/components/v2/capabilities/README.md
[rfc-0127]: /docs/contribute/governance/rfcs/0127_structured_configuration.md
[rfc-0146]: /docs/contribute/governance/rfcs/0146_structured_config_schemas_in_cml.md
[rfc-0215]: /docs/contribute/governance/rfcs/0215_structured_config_parent_overrides.md
