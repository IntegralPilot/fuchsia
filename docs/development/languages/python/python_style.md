# Python style guide

The Fuchsia project follows the [Google Python style guide](https://github.com/google/styleguide/blob/gh-pages/pyguide.md){:.external},
with a few [refinements](#refinements).

The Google Python style guide allows more variation (presumably to cover a large
breadth of existing source). This guide has a tighter set of choices. So a
Fuchsia Python file will also comply with the Google style guide, but a Google
Python file might not comply with this guide. See [refinements](#refinements)
below for details.

## Python versions {#python-versions}

Fuchsia vendors its own Python 3 interpreter in the checkout
(`//scripts/fuchsia-vendored-python`, currently Python 3.11 or later as of 2026-08).

All executable Python scripts in the Fuchsia repository must begin with the
following shebang line:

```shell
#!/usr/bin/env fuchsia-vendored-python
```

For more information, see
[RFC-0129](/docs/contribute/governance/rfcs/0129_python_in_fuchsia.md) and
[Build system policies](/docs/development/build/gn_concepts/policies.md#python-scripts-as-build-actions).

## Refinements

The following refinements we make to the Google Python style guide are largely
choices between variations. For example, if the style guide says you may do A,
B, or C we may choose to favor B and avoid the other choices.

### Indentation

Avoid aligning with opening delimiter. Prefer instead to indent using fixed
(4 space) indentation.

(See
[Indentation](https://github.com/google/styleguide/blob/gh-pages/pyguide.md#34-indentation){:.external}
in the Google Python style guide for comparison.)

### Statements

Avoid creating single line statements, even with `if` statements.

```python {.good}
Yes:

    if foo:
        bar(foo)
```

```python {.bad}
No:

    if foo: bar(foo)
```

(See
[Statements](https://github.com/google/styleguide/blob/gh-pages/pyguide.md#314-statements){:.external}
in the Google Python style guide for comparison.)

### Type annotations

Type annotations are strongly encouraged for all new Python code in Fuchsia.
Follow modern Python (3.11+) type annotation conventions in accordance with the
[Google Python Style Guide](https://github.com/google/styleguide/blob/gh-pages/pyguide.md#319-type-annotations){:.external}:

* **PEP 585 Standard Collections:** Use built-in collection types directly
  for generic type hints (`list[str]`, `dict[str, int]`, `set[Path]`,
  `tuple[int, ...]`). Do not import `List`, `Dict`, `Set`, `Tuple` from `typing`.
* **PEP 604 Union Syntax:** Use the `|` operator for union types (`int | float`,
  `str | None`). Do not import `Union` or `Optional` from `typing`.

### Strings

Prefer double quotes for strings (`"`). Use single quotes when the declaration is
more readable with single quotes. For example, `'The cat said "Meow"'` is more readable
than `"The cat said \\"Meow\\""`.

Prefer **f-strings** (`f"..."`) for string formatting and interpolation over
`%` formatting or `.format()`. Maintain double quotes for f-strings.

(See
[Strings](https://github.com/google/styleguide/blob/gh-pages/pyguide.md#310-strings){:.external}
in the Google Python style guide for comparison.)

### Be consistent

Be consistent within a large scope. Avoid displaying small pockets of consistency
within Fuchsia. Being consistent within only a single file or directory is not
consistency.

Within `third_party`, the intent is to follow the existing style for that project
or library. Look for a style guide within that library as appropriate.

(See
[Parting Words](https://github.com/google/styleguide/blob/gh-pages/pyguide.md#4-parting-words){:.external}
in the Google Python style guide.)
