This directory contains:

- `repository_rules.bzl` which defines the `fuchsia_idk_repository()`
  repository rule.

- `generate_repository.py`: A script invoked by the repository rule
  that processes an input IDK directory, that uses the official IDK
  layout, and generates an output IDK directory, whose metadata
  files contain Bazel labels for all Ninja artifacts.

- `generate_repository_test.py`: A unit-test for the classes
  defined in `generate_repository.py`.

- `generate_repository_validation.py`: A regression test for
  the `generate_repository.py` script, that parses the
  input from `goldens/{input_idk,src,out}` and compares
  the output to `goldens/expected_idk`, reporting errors
  or differences when they are found.

- `validation_data`: A directory containing the validation input IDK,
   fake source directory, fake Ninja output dir, and expected
   output IDK.
