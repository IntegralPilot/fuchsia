---
name: devicetree-visitor
description: >
  Create a new Fuchsia devicetree visitor to parse devicetree node properties into
  FIDL driver metadata and bind properties. Covers `fx create devicetree visitor`,
  fdf_devicetree::DriverVisitor / PropertyParser, compatible-string filtering,
  phandles, and visitor-test-helper. Use when a driver needs custom devicetree
  properties parsed, or when a bind failure traces to a missing visitor for a
  compatible string. Don't use for diagnosing golden mismatches or general bind
  failures (see devicetree-debugging).
---

# Devicetree Visitor Creation

This skill provides a structured process for generating a devicetree visitor.
Devicetree visitors are responsible for converting devicetree data into
driver-specific metadata and bind rules.

## When to use

- When you need to create a new devicetree visitor.
- When facing a driver bind issue that might be due to a missing devicetree
  visitor.

## Useful references

- `docs/development/boards/devicetree-visitors.md`
- `docs/development/boards/devicetree-overview.md`
- `docs/development/boards/devicetree-faq.md`
- `sdk/lib/driver/devicetree/visitors/README.md`

## Workflow

### 1. Setup Phase

1.  Determine which directory the visitor will be added. Typically
    `sdk/lib/driver/devicetree/visitors/drivers/`.
2.  Run `fx create devicetree visitor --lang cpp --path <VISITOR_PATH>` to
    create a scaffolding visitor.
3.  Create a `README.md` in the visitor directory to explain inputs and outputs.
4.  Create a `tasks.md` in the visitor directory to track progress.

### 2. Planning Phase

1.  **Identify Outputs**: Determine what FIDL metadata it should produce and
    what composite nodes it should add.
2.  **Identify Inputs**: Determine what devicetree node properties will be used,
    relevant nodes to parse, and any reference properties (phandles).
3.  **Update Schema**: Write or update the YAML schema file in the module
    directory.
4.  **Update README**: Document the inputs and outputs in the `README.md`.

### 3. Critic Phase

1.  **Initial Review**: Review the plan for flawed assumptions, missing edge
    cases, and potential negative interactions.
2.  **Deep Expert Review**: Challenge every assumption and verify the plan is
    viable by reading surrounding code.

### 4. Implementation Phase

1.  **Implement Visitor**: Update the `Visit` method to parse nodes and create
    FIDL metadata/bind properties.
    - Use `fdf_devicetree::DriverVisitor` as a base.
    - Use `fdf_devicetree::PropertyParser` for complex parsing.
2.  **Add Tests**: Create a test `.dts` file and update test cases using
    `visitor-test-helper`.
3.  **Build and Verify**: Run tests and ensure they pass.

### 5. Verification & Cleanup

1.  Review code, schema, and tests for alignment.
2.  Delete scaffolding files like `tasks.md` once complete.

## Helper Libraries

- **Driver visitors**: Constructor that filters by compatible strings.
- **Property parser**: Configurable parser for node properties and phandles.
- **Multi-visitor**: Combines multiple visitors into one.
- **Load visitors**: Loads shared library visitors from `/lib/visitors`.

## Further Reading

* [Driver Devicetree
  Binding](/src/devices/skills/driver_bind_devicetree/SKILL.md)
  - Guide for writing bind rules and setting dependencies for devicetree
    drivers.
