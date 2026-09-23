<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0215" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

Allow parent components to override values in their children's structured
configuration.

## Motivation

In line with the proposed direction in RFC-0127, parent components should be
able to provide configuration values to their children. For example,
`starnix_runner` would benefit from being able to [pass its own configuration
values][starnix-runner-kernel-config] directly to the `starnix_kernel`s it
launches without creating configuration directories and dynamic offers.

A parent component should be able to pass configuration values when launching a
dynamic child. For example, a parent implemented in Rust should be able to write
code like:

```rs
let overrides = vec![ConfigOverride {
    key: Some("parent_provided".into()),
    value: Some(ConfigValue::Single(ConfigSingleValue::String(
        "foo".to_string(),
    ))),
    ..ConfigOverride::EMPTY
}];

connect_to_protocol::<RealmMarker>()
    .unwrap()
    .create_child(
        &mut CollectionRef { name: "...".into() },
        Child {
            // name, url, startup, ...
            config_overrides: Some(overrides),
            ..Child::EMPTY
        },
        CreateChildArgs::EMPTY,
    )
    .await
    .unwrap()
    .unwrap();
```

In the future it should be possible for the parent component to [configure a
static child in CML](#static-child-config) and [use a generated
library](#generated-library) for dynamic children to reduce verbosity while
ensuring correct types for overrides.

## Stakeholders

**Facilitator:** jamesr@google.com

**Reviewers:**

* geb@google.com (CF)
* ypomortsev@google.com (CF)
* markdittmer@google.com (Security)

**Consulted:** lindkvist@google.com, hjfreyer@google.com

**Socialization:** This proposal was circulated among Component Framework team
members and prospective customers before being submitted as an RFC.

## Requirements

1. Initially, parents may provide configuration values to dynamic children,
   including those instantiated using RealmBuilder. It should eventually be
   possible to configure static children in CML, including using values from the
   parent's configuration.
2. Component authors may evolve internal/test-only/developer-only configuration
   fields without coordinating with parent components.
3. Parent and child components may be updated independently. Children may evolve
   their configuration schema by coordinating soft-transitions with parent
   components.

## Design

In order to make this feature work, we need to define:

* [syntax for child components to opt-in to parent overrides](#mutability)
* [rules for resolving configuration in the presence of schema version
  skew](#version-skew)
* [precedence of parent-provided values compared to other sources of
  configuration](#precedence)
* [API(s) that allow parent components to actually pass values at
  runtime](#create-child)

### Mutable-by-parent configuration fields {#mutability}

Per requirement (2), parents should only be able to override configuration
fields which are marked mutable by the child component.

Component authors will add a `mutable_by` attribute to any configuration fields
which should be allowed to receive overrides. The attribute will accept a list
of strings to accommodate future override mechanisms. Initially the only string
accepted will be `"parent"`. Example CML:

```json5
{
    // ...
    config: {
        fields: {
            enable_new_feature: {
                type: "bool",
                mutable_by: [ "parent" ],
            },
        },
    }
}
```


Mutability will be specified as a list of possible sources to make it easy to
extend this syntax for new overrides sources, including a developer override
database as [originally scoped by RFC-0127][override-service].

A rejected alternative we could pursue in the future would be to [group
configuration fields by their mutability](#grouped-mutability).

#### String keys vs. offsets/ordinals

Component Manager currently resolves packaged configuration values using the
offset of the field in the compiled configuration schema, a small optimization
which requires the component's manifest, configuration values, and compiled
binary to agree on the exact layout of the configuration schema. This will not
be possible for overrides where the provider of values and the configured
component were built with different (but compatible) configuration schemas.

Parent overrides will instead be resolved by checking string equality of the
keys in the child's schema and the provided overrides, allowing the order and
number of fields in the child's schema to change without requiring the child
component to specify explicit ordinals or the parent component to update its
overrides.

A [rejected alternative](#ordinals) to this approach would be to use integer
ordinals for resolution and to require child components to explicitly choose an
ordinal.

#### Packaged defaults

Components with mutable-by-parent configuration fields will still be required to
provide a default/base value in their packaged configuration. This will make the
addition of new parent-visible fields a soft transition.

In the [future](#required-from-parent) we may allow components to require their
parents to provide some configuration values.

#### No over-provisioning

Parent-provided configuration value files must only contain values for fields
which are present and mutable-by-parent in the child component's configuration
schema. Preventing over-provisioning of configuration will provide predictable
behavior for parents without them having to manually verify that the child was
configured as they expect.

### Precedence of values from parents {#precedence}

[Component configuration][component-config] "tailors the behavior of a component
instance to the context it's running in," which implies that the most
authoritative configuration values should be those which encode the most
knowledge of the component instance's context.

Per RFC-0127, Component Manager will resolve a value for each configuration
field, preferring each source in this order:

1. (in the future) values from a developer override service
2. values from a component's parent
3. values from the component's own package

A component developer working on an engineering build can have knowledge about
everything running on the system, which makes those overrides the most
authoritative. A parent can understand the context it provides to its children.
Finally, because the values in the component's own package can only encode an
understanding of how the component was packaged, all other information must be
provided from an outside source.

### Providing values in `Realm.CreateChild` {#create-child}

We will extend `fuchsia.component.decl/Child` to allow specifying configuration
overrides at runtime:

```fidl
library fuchsia.component.decl;

@available(added=HEAD)
type ConfigOverride = table {
    1: key ConfigKey;
    2: value ConfigValue;
};

type Child = table {
    // ...

    @available(added=HEAD)
    6: config_overrides vector<ConfigOverride>:MAX;
};
```

While it would be possible for parents to set configuration for dynamic children
if we added this field to `fuchsia.component.CreateChildArgs`, that would not
offer a path to specifying parent overrides for [statically-defined components
in CML](#static-child-config). The proposed approach ensures there is only a
single source of parent-provided values for Component Manager to consider in the
future.

Parent components providing values at runtime must match the declared type of
the configuration field exactly. No type inference, casting, or integer
promotion will be performed.

## Implementation

The proposed design [has been prototyped][prototype].

The `fuchsia.component.decl` FIDL library will need to have access to a `Value`
type which is currently in `fuchsia.component.config`. However,
`fuchsia.component.config` currently depends on `fuchsia.component.decl`, so
it will take a long time to invert the dependency relationship with soft
transitions. Instead, we will move all of `fuchsia.component.config` into
`fuchsia.component.decl` at the current API level, deprecating and eventually
removing `fuchsia.component.config`. In merging the two libraries we will add
a `Config*` prefix to the appropriate types, e.g. `Value` becomes `ConfigValue`.

Newly defined FIDL API surface for this proposal will only be available at API
level `HEAD` until we've gained experience with the feature and out-of-tree
users are able to make use of structured configuration.

In addition to changes to component declarations, `RealmBuilder` client
libraries will need to be updated to allow passing configuration overrides as
part of the `Child` decl.

## Performance

This RFC proposes that Component Manager will use string equality to match
overrides with their intended fields, which will be a slower operation than the
O(1) index/offset comparison currently used to resolve packaged config values.
This adds a very small amount of computation to component start but is unlikely
to have any observable impact as component start times are typically on the
order of hundreds of milliseconds today. For context, framework overhead in
component start time has not yet been a significant input to any user-facing
product quality issues.

## Ergonomics

In the first iteration of this feature a parent component will need to know the
exact type of the configuration field being overridden which is not as ergonomic
as we can eventually make this feature. For example, a component author will
need to match the exact integer width and signedness of a numeric configuration
field. In the future we can define looser type resolution rules to
[support configuring static children](#static-child-config), and we may also
[generate code from a child's configuration schema](#generated-library) which
would allow language compilers to check the field names and types on behalf of a
developer.

Without the ability to describe a version sequence for a component's config
interface, schema evolution will be a manual & social process. This might be
possible to [address in the future](#fidl-versioning).

Some components may end up with multiple fields that share the same mutability
constraints, with an ergonomic tax imposed by repeating the same attribute for
each field. We do not have any use cases which fit this pattern today, so a
syntax aimed at solving this problem is a [rejected
alternative](#grouped-mutability) that we may choose to revisit in the future.

## Backwards & Forwards Compatibility {#compat}

Authors of components will be responsible for coordinating soft-transitions with
their parent components when they modify the mutable-by-parent portions of their
config schema. This section describes safe evolution procedures for
modifications to configuration schemas.

Configuration fields without a `mutable_by` attribute will not require any
special attention to versioning, as values for those fields can only be provided
from a component's own package.

In the future these steps may be more ergonomically mediated by [FIDL-based
versioning](#fidl-versioning).

### Adding a new mutable-by-parent field, adding mutability to existing fields

No special considerations are required, as all configuration fields will still
require a base/default value to be in a component's own packaged value file.
Once the field is present in a component's configuration schema, parents will be
able to provide a value for the field.

### Removing a mutable-by-parent configuration key, removing mutability modifier

Safely removing a mutable-by-parent configuration field from a component's
schema will require first working with parent components to ensure they are no
longer passing any overrides for the field which will be removed.

For example, to remove a `parent_provided` config field from a component's
configuration interface:

1. component author communicates intent to deprecate and remove
   `parent_provided`
2. parent components stop specifying values for `parent_provided`
3. component author removes `parent_provided` from their configuration schema

### Renaming a mutable-by-parent field

Renaming a field is equivalent to a simultaneous addition & removal and should
generally not be performed in a single step.

### Changing a mutable-by-parent field's type

Changing a field's type is equivalent to a simultaneous addition & removal and
should generally not be performed in a single step.

### Changing a mutable-by-parent field's constraints

Increasing a field's `max_len` or `max_size` constraint is always safe.

Reducing a field's `max_len` or `max_size` constraint is only safe if all
parent-provided values are within the new range.

## Security considerations

The `scrutiny` tool can [make assertions][config-policy] about the final
configuration of components in a built system image. This is an important
safeguard mechanism to protect against accidental misconfigurations during the
build and assembly processes.

We will extend `scrutiny` so that it will refuse configuration fields which both
have policy-enforced values and are mutable-by-parent. This will ensure that
security-critical configuration fields can never be mutated by a parent
component outside the scope of scrutiny's static verification.

## Privacy considerations

Parent overrides allow components to pass runtime data as configuration values.
While some components may choose to use this functionality to pass user data,
there is currently no functionality which automatically records structured
configuration values in logs, metrics, or telemetry. There should be no privacy
impact to implementing this feature.

## Testing

The `config_encoder` library is used to resolve configuration in Component
Manager, `scrutiny`, and related tools. Its unit tests will be expanded to
ensure that incorrectly provisioned parent overrides (unknown key, wrong type,
missing mutability) will be rejected.

Structured configuration integration tests will be expanded to ensure that a
parent component can provide configuration using the `Realm` protocol and using
`RealmBuilder`.

`scrutiny` tests will be expanded to ensure it rejects mutable-by-parent fields
which have statically declared values in a policy file.

## Documentation

CML reference documentation will be updated to match the updated config schema
syntax.

A guide on structured config schema evolution will be written. It will cover
best practices for adding and removing fields. It will include guidance for
managing soft transitions.

New reference documentation for structured configuration value sources and their
precedence will be written.

Existing documentation on verifying security properties of a built image will
be extended with an explanation for why parent-mutable fields are not allowed
for security-relevant configuration.

## Future Work

### Generated bindings for overrides {#generated-library}

The above proposal implies that authors of parent components will need to use
string keys and "dynamically typed" values for providing overrides. This will
result in some boilerplate and makes it possible for a parent to provide the
wrong keys/values or to forget to update a codepath which produces overrides
when there's a new field available. Errors due to incorrectly provisioned
configuration values will only appear at runtime when starting the child
component.

We can eventually improve the developer experience here by allowing authors of
parent-configured components to generate "parent override" libraries. These
could be used by authors of parent components to reduce characters typed and
have their compiler check that they're correctly overriding the child's
configuration:

```rs
let overrides = FooConfigOverrides {
    parent_provided: Some("foo".to_string()),
    ..FooConfigOverrides::EMPTY
};
connect_to_protocol::<RealmMarker>()
    .unwrap()
    .create_child(
        &mut CollectionRef { name: "...".into() },
        Child {
            // name, url, startup, ...
            config_overrides: overrides.to_values(),
            ..Child::EMPTY
        },
        CreateChildArgs::EMPTY,
    )
    .await
    .unwrap()
    .unwrap();
```

### Configuring children in CML {#static-child-config}

A parent component with static children should eventually be able to provide
configuration values when declaring the child in CML. For example:

```json5
{
    children: [
        {
            name: "...",
            url: "...",
            config: {
                parent_provided: "foo",
            },
        },
    ],
}
```

We may want to also provide syntax that lets a parent forward their own
configuration value(s) directly to the static child.

We will need to decide whether `scrutiny` will permit mutable-by-parent fields
when the provided configuration value is statically known.

If we build this feature for CML, passing literal values will require design
work to bridge the gap between JSON5's loose typing for numbers and structured
config's precise types. We will need to choose
`fuchsia.component.decl.ConfigValue` representations for all possible JSON5
inputs, and we will need to define rules which allow Component Manager to make
use of those values for configuration fields which might have narrower
constraints. Much of this design work can be avoided if we wait for a FIDL-based
representation of parent/child relationships, as `fidlc` would be able to
precisely check types against a child's configuration schema. This design work
would not be needed to allow a parent to pass its own config values to a
child.

### Required-from-parent configuration values {#required-from-parent}

Some components may want to require that their parent provides a particular
configuration value without being able to rely on a packaged default.

This would require allowing packaged value files to omit values and teaching the
various libraries which resolve configuration how to handle them.

This is not required to meet current use cases but may be a useful extension to
pursue in the future.

### FIDL-based schemas & versioning {#fidl-versioning}

The Component Framework team has explored the use of FIDL for component
manifests. If that is implemented we may be able to use FIDL availability
annotations to coordinate configuration schema evolution.

### Use string keys to resolve packaged configuration

Taking the approach proposed in this RFC will leave Component Manager with two
identifiers used for resolving configuration values:

1. packaged values will be resolved using integer offsets within the compiled
   config schema
2. parent-provided values will be resolved using string keys from the config
   schema

As discussed in the [alternative below](#ordinals), there are good reasons to
prefer string keys for parent overrides, and the potential motivations for using
integer offsets/ordinals don't benefit us much when resolving packaged values.

We should move packaged values to encoding string keys to reduce fragmentation
in Component Manager's semantics. This change should be largely invisible to
downstream users but will simplify the implementation of config resolution and
make future debugging easier.

### Fuzz config resolution

In the future we will add a fuzzer for Component Manager's manifest parsing and
component resolution. When [that](https://fxbug.dev/42069284) happens, we will
extend the fuzzer to include configuration values from a parent component and
assert that mutability modifiers are respected.

## Drawbacks, alternatives, and unknowns

### Grouping fields with shared mutability constraints {#grouped-mutability}

If a component has many fields which all share the same mutability constraints,
we might consider allowing component authors to group configuration fields
together with shared attributes. For a (non-normative) example, we might define
multiple config sections:

```json5
{
    // ...
    config: {
        // fields which can only be resolved from a component's package
    },
    parent_config: {
        // fields which are mutable by parent
    },
}
```

This direction could be useful for ergonomics but would not be motivated by any
existing use cases we've identified. We can revisit this approach if we discover
verbosity is a significant tax in practice.

Note that this approach could make it harder for component authors to define
configuration fields which have multiple mutability specifiers, e.g. "mutable by
parent and by override service".

### Integer ordinals for override API {#ordinals}

For historical reasons packaged configuration values are resolved to their
fields by using their offset within the list of packaged values. This offers a
more efficient encoding and less runtime overhead than checking for string
equality.

To achieve the same benefits for parent overrides, we would need authors of
child components to explicitly choose an ordinal for their fields, similar to a
FIDL table. Parent component authors would need to specify their overridden
values in terms of these ordinals or use [generated
libraries](#generated-library) for human-legible names.

We would also need to design mechanisms to guide child component authors away
from re-using integer ordinals, whereas well-chosen string keys should be at
less risk of bug-prone reuse.

Ultimately, the binary size and runtime overhead benefits we get from resolving
keys from integers is negligible at the scale of usage we anticipate for
structured configuration. Using string keys lets us defer generated bindings and
insulates child component authors from the complexity of managing their
ordinals, with minimal cost to the system's performance.

[starnix-runner-kernel-config]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/proc/lib/kernel_config/src/lib.rs;l=66;drc=a12b8d30ac0c77c561531d64533bf773c79bee37
[override-service]: /docs/contribute/governance/rfcs/0127_structured_configuration.md#values-from-override-service
[component-config]: /docs/concepts/components/configuration.md
[prototype]: https://fuchsia-review.googlesource.com/c/fuchsia/+/800189
[config-policy]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/security/lib/scrutiny/tests/structured_config/passing_policy.json5;drc=4fa69781ef9165e9066cd588df21d2b24d3031d6
