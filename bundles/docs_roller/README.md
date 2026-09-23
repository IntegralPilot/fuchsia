# Docs Roller Bundle

This directory defines a dedicated bundle (`//bundles/docs_roller`) maintained
for use by the `fuchsia-docs-roller` builder to discover build arguments across
the fuchsia tree.

## Maintenance Rules

Note: Strictly avoid `print()` statements in this bundle or its transitive
dependencies.

The `fuchsia-docs-roller` builder runs `gn args --list --json` to generate
documentation. If any target in this evaluation graph outputs a `print()`
message or warning to `stdout`, it will pollute the output stream and break
JSON parsing (causing build failures).

## Purpose

The intent of this bundle is to reach the maximal subset of GN code plausibly
useful to developers, allowing the docs builder to evaluate arguments for as
many files as possible. If new major components are added to the tree that
have their own argument sets, they should be added to this bundle to ensure
they are documented.

