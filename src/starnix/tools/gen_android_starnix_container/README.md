# gen_android_starnix_container

This tool generates a package that contains a starnix container component and
other supporting packages and resources.

If you would like to use this tool in fuchsia.git, refer to the build rule at
/build/starnix/android_starnix_container.gni.

## Adding HAL packages

This tool takes in an optional list of HAL packages.

It will add those packages as subpackages of the container package.

It will generate a filesystem in the [remote bundle][1] format at the `data/odm`
directory in the container package, consisting of init rc files and vendor
manifest fragments, when those files are found in a HAL package. Specifically:

- The HAL package should contain a JSON file `__android_config__/manifest.json`
  with two optional keys, whose values are paths within the package:

  ```json
  {
    "init_rc": "system/init.rc",
    "vintf_manifest": "system/manifest.xml"
  }
  ```

- If the manifest JSON has `init_rc`, that file will be copied to
  `etc/init/${hal_package_name}.rc` in the ODM filesystem.

- If the manifest JSON has `vintf_manifest`, that file will be copied to
  `etc/vintf/manifest/${hal_package_name}.xml` in the ODM filesystem.

If no such keys are found, the remote bundle will contain an empty `etc/init`
and an empty `etc/vintf/manifest` directory.

TODO(https://fxbug.dev/42081021): After documenting the remote bundle format, replace this
link.

[1]: /src/starnix/kernel/fs/fuchsia/remote_bundle.rs
