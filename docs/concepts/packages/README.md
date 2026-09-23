# Packages

Fuchsia packages play an essential role in distributing, updating,
and securing software within a Fuchsia ecosystem.

## Fuchsia packages

* [**Fuchsia packages**][fuchsia-packages]: Provides an overview of
  Fuchsia packages, including their types and structure.
* [**Fuchsia package URLs**][package-url]: Explains the format
  and structure of Fuchsia package uniform resource locators (URLs).
* [**Fuchsia package server**][fuchsia-package-server]: Explains the
  Fuchsia package server and its role in delivering trusted software
  updates.

## Software updates

* [**OTA updates**][ota]: Explains the details of how the OTA
  (Over The Air) update process works in Fuchsia.
* [**The update package**][update-package]: Explains the contents of
  the update package, containing files and rules for system updates.
* [**Software update system**][software-update-system]: Explains
  Fuchsia's software update system and its use of The Update Framework
  (TUF).
* [**Garbage collection**][garbage-collection]: Explains the garbage
  collection process performed to free up disk space on a device for
  OTA.

## Additional resources

For more information on creating, building, and updating Fuchsia
packages, see
[**Developing with Fuchsia packages**][develop-with-packages].

<!-- Reference links -->

[fuchsia-packages]: package.md
[package-url]: package_url.md
[fuchsia-package-server]: fuchsia_package_server.md
[ota]: ota.md
[update-package]: update_pkg.md
[software-update-system]: software_update_system.md
[garbage-collection]: garbage_collection.md
[develop-with-packages]: /docs/development/build/package_update.md
