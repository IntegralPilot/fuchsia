---
name: devicetree-schema
description: >
  Write and validate Fuchsia devicetree binding schema YAML files (dt-schema
  meta-schema, $id, properties, required, $ref types, DTS examples). Use when
  creating or updating a devicetree binding, documenting the properties a
  devicetree visitor requires, or validating a schema against a .dtb. Don't use
  for writing visitor C++ parsing code (see devicetree-visitor) or diagnosing
  golden mismatches (see devicetree-debugging).
---

# Devicetree Schema Creation

Devicetree schemas describe the requirements on the content of a devicetree node
pertaining to a specific device or class of device. They are documented using
YAML files.

## When to use

- When creating or updating a devicetree binding.
- When documenting or updating the properties required for a devicetree visitor.

## Structure of a Schema File

Fuchsia devicetree schemas follow the [Devicetree Schema
Tools](https://github.com/devicetree-org/dt-schema) meta-schema.

### Key Fields

- **$id**: A unique URI for the schema (e.g.,
  `http://devicetree.org/schemas/smc.yaml#`).
- **$schema**: The meta-schema URI (usually
  `http://devicetree.org/meta-schemas/core.yaml#`).
- **title**: A brief title for the binding.
- **maintainers**: List of maintainers.
- **description**: Detailed description of the binding and its properties.
- **properties**: Definition of each property allowed in the node.
- **required**: List of required properties.
- **examples**: Example devicetree source (DTS) snippets.

### Common Property Types

Types are referenced from `/schemas/types.yaml`.

- **uint32**: `$ref: /schemas/types.yaml#/definitions/uint32`
- **uint32-array**: `$ref: /schemas/types.yaml#/definitions/uint32-array`
- **phandle-array**: `$ref: /schemas/types.yaml#/definitions/phandle-array`
- **string**: `$ref: /schemas/types.yaml#/definitions/string`

## Example: `smc.yaml`

```yaml
%YAML 1.2
---
$id: http://devicetree.org/schemas/smc.yaml#
$schema: http://devicetree.org/meta-schemas/core.yaml#

title: Fuchsia Secure Monitor Call consumer

properties:
  smcs:
    description: Array of secure monitor call capabilities.
    $ref: /schemas/types.yaml#/definitions/uint32-array
    items:
      maxItems: 3
      minItems: 3

  smc-names:
    description: Optional names corresponding to the smcs entries.

additionalProperties: true

examples:
  - |
    display-device {
      compatible = "sample,display";
      smcs = <4 1 0>;
      smc-names = "display";
    };
```

## Best Practices

1.  **Follow Industry Standards**: Reference established devicetree bindings
    from the [dt-schema
    repository](https://github.com/devicetree-org/dt-schema/tree/main/dtschema/schemas)
    for common patterns and best practices.
2.  **Use Precise Definitions**: Define the type and constraints for all
    properties explicitly. Use fields like `minItems`, `maxItems`, `enum`, and
    `$ref` to create robust, self-describing schemas.
3.  **Reference Local Examples**: Look at existing Fuchsia schemas in
    `sdk/lib/driver/devicetree/visitors/` to see how libraries and drivers are
    documented.
4.  **Verify with Examples**: Always provide a complete and valid DTS snippet in
    the `examples` section of your schema.

## Validation

Fuchsia does not yet have integrated in-tree schema validation tools. For
instructions on how to validate schema files or devicetree blobs (DTB), please
refer to the external [dt-schema](https://github.com/devicetree-org/dt-schema)
project.
