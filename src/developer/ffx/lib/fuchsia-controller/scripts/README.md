# Rapid local development environment

Use these scripts to install a development environment and rapidly
iterate on the code:

```bash
# Install a development environment.
$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller/scripts/install.sh

# Format all code for Fuchsia Controller. Run this before uploading a CL!
$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller/scripts/format.sh

# Run all tests and generate coverage. It is much faster than going through
# the whole build process, and you can output HTML using the --html-dir
# parameter.
$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller/scripts/coverage.sh --html-dir ~/fuchsia-controller-coverage

# Clean up by running uninstall.
$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller/scripts/uninstall.sh
```

## Updating local dependencies

Warning: Do not update `requirements.txt` manually!

For security reasons, we pin all hashes of modules installed with
pip. If any versions of dependencies are changed in either
`base-tooling-requirements.txt` or `pyproject.toml`, rerun the
following:

```bash
$FUCHSIA_DIR/src/developer/ffx/lib/fuchsia-controller/scripts/update_deps.sh
```

This will create a new venv and recompile the `requirements.txt`
file with associated hashes. You can then upload a CL containing
the updated requirements.