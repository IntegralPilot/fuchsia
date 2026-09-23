# Evolving Structured Configuration

Configuration values required by a component can change over time. Because some
configuration fields can be modified by other components it is important to take
care when changing a component's configuration schema to avoid breaking other
components that may depend on it.

## When soft transitions are required

This guidance only applies to configuration fields that have a `mutability`
specifier. Modifying fields without `mutability` does not require special care,
as the values are always provided by the component's configuration value file.

## Adding a new mutable field, adding mutability to existing fields

No special considerations are required, as all configuration fields require a
base/default value to be in a component's configuration value file and there
will be no existing providers of the configuration from outside that component.

## Removing a mutable configuration field, removing mutability specifier {#remove-field}

Safely removing a mutable configuration field from a component's schema will
require first working with configuration providers to ensure they are no
longer passing any overrides for the field that will be removed.

If as a component author you want to remove a configuration field from your
component that has a `mutability` specifier:

1. Identify any runtime providers of configuration for the component. For example,
   if the field has `mutability: [ "parent" ]` then you should identify any parent
   components that provide values for that field.
1. Communicate your intent to deprecate and remove the configuration field to
   any stakeholders identified in the previous step.
1. Runtime providers of configuration should stop providing values for that
   configuration field. This may require work from you to continue supporting
   those providers' use cases without providing the field.
1. Once all runtime providers have been identified and are no longer attempting
   to override that configuration field's value, you can safely remove the field
   or its `mutability` specifier.

## Renaming a mutable field {#rename-field}

Renaming a field is equivalent to adding a newly named field and removing the
existing field.

If as a component author you want to rename a configuration field that has a
`mutability` specifier:

1. Add a field with the new name to your configuration, preferring that value
   over the existing field's value in your component's implementation.
1. Follow the steps above to [remove a field], working with stakeholders to
   instead provide values for the newly-named field.

[remove a field]: #remove-field

## Changing a mutable field's type

Changing a mutable field's type requires either using a new name or eliminating
all overriders before changing the type.

1. Add a field with a new name and the correct type to your configuration,
   preferring that value over the existing field's value in your component's
   implementation.
1. Follow the steps above to [remove a field], working with stakeholders to
   instead provide values for the newly-named and newly-typed field.
1. (optional) Once the previous field is removed, follow the steps above to
   [rename a field] to use the name from before these steps.

[rename a field]: #rename-field

## Changing a mutable field's constraints

Increasing a field's `max_len` or `max_size` constraint is always safe.

Reducing a field's `max_len` or `max_size` constraint is only safe if all
externally-provided values are within the new range.
