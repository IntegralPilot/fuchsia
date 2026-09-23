# Python Dependency Management
pylibs provides a Python package configuration file and a script to streamline
dependency management and setup using requirements.in. `requirements.in` defines the top-level dependencies and their source paths, while `requirements.txt` is the fully resolved and pinned lockfile generated from it.


## Add new package

1. OSRB Approval: Request Open Source Review Board (OSRB) approval by creating
    an issue using the [OSRB template](https://issuetracker.google.com/issues/new?component=1477857&template=1911599)
    to use the desired Python package and any of its dependencies.
    See [example bug](https://fxbug.dev/42084613) for reference.

2. File a Infra bug to request the import of the package and dependencies source
    code from its upstream repository into the Fuchsia source code.
    See [example bug](https://fxbug.dev/432504093) for reference

3. Add the package to the `requirements.in` file, including the specific
    version and the path within the Fuchsia repository. Example:

    `<package>===0.0.1 # third_party/github.com/<package>` for
    `https://fuchsia.googlesource.com/third_party/github.com/<package>`

4. If the package has dependencies, add those package names to
    `requirements.in` without specifying versions. The script will determine
    the correct versions.

    Example: `<dep_package> # third_party/github.com/python/<dep_package>`

    If a package is listed in dependencies for installation or building, but
    isn't strictly required for Fuchsia, exclude it using the 'ignore' tag.

    Example: `<install_package> # Ignore`

5. Run the `update-pylibs.sh` script and verify the changes in `pylibs`
    configuration file.

    NOTE: If you're a Googler and `update-pylibs.sh` prompts you for a username,
    try running `gpkg update-pylibs.sh` instead.


## Update existing package

1. Update the package version directly in the requirements.in file and run the
    `update-pylibs.sh` script.

2. Verify the `pylibs` configuration file to ensure update was successful.

3. In the `//third_party/pylibs/<package>/src` directory:

    * Use the git checkout command along with the commit ID found in the pylibs configuration file. For example, if the commit ID is 'abcdef123456', you would run:

            git checkout abcdef123456

    * Rebuild your package to ensure everything works correctly.
