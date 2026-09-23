# Organization

Note: Use `/tools/fidl/scripts/canonical_example/scaffold.py` to set up your
FIDL samples. This script automatically sets up most of the recommendations
established in this document.

All samples in this directory should follow these conventions. Occasionally
these conventions can conflict and choosing one over another can lead to very
different outcomes for these samples. To address these possible conflicts,
review the following priorities.

## Priorities

In order of most to least important, samples should be:

1. Minimal and focused
1. Realistic
1. Well-commented
1. Well-tested

Conflicts between these priorities should be resolved in the order stated. For
example, tests are important, but well tested code should not come at the
expense of an overly complex sample--minimal and focused is a higher priority.
Similarly, samples shouldn't be less realistic to make it easier to test.

## Directory structure

By having a consistent directory structure, users can easily move between
samples. This familiarity is leveraged to better emphasize what's interesting in
an sample.

All samples should use the following structure:

```
- name-of-sample
  README.md
  - baseline
    - cpp_natural
    - cpp_wire
    - fidl
    - meta
    - realm
    - rust
    - test
  - riff-name
    - cpp_natural
    - cpp_wire
    - fidl
    - meta
    - realm
    - rust
    - test
  ...
```

Note: You don't necessarily have to create every wire binding for every sample,
but that's a good default to start with. The main reasons to _not_ do a binding
is if the motivating sample would be un-idiomatic with that binding, or if the
features in the sample aren't supported for that binding.

## README.md

It is an intentional goal that samples in these directories be useful and
understandable even without the prose from fuchsia.dev. To achieve this, make
extensive use of README.md files that explain what's going on.

Every sample should, at least, have a `README.md` that clearly outlines:

1. What the sample is
1. What features the sample showcases

## Tests

All samples should be well tested. For example, if an sample contains a client
and server, there should be unit tests for each, and an integration test that
exercises the real client and server together.

Additionally, tests should utilize the `example-tester` framework. For an
example of it's use, look at the [key_value_store baseline test directory].

[key_value_store baseline test directory]: ./key_value_store/baseline/test
