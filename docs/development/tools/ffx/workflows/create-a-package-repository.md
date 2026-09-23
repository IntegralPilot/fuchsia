# Create a Fuchsia package repository

The [`ffx repository`][ffx-repository] commands can create and manage
Fuchsia package repositories on the host machine.

## Concepts

When a Fuchsia device needs to run new software or update existing software,
the device requests and downloads [Fuchsia packages][fuchsia-package] from
a [Fuchsia package server][fuchsia-package-server], which is a service
that you can run on your host machine. The Fuchsia package server
then serves Fuchsia packages from
a [Fuchsia package repository](#create-a-package-repository) configured
on the host machine.

A Fuchsia package repository is mapped to a directory on the host machine.
When handling requests from Fuchsia devices, the Fuchsia package server looks
for Fuchsia packages in this directory. If they're found, the package server
serves the packages to the devices directly from this directory. Therefore,
this directory on the host machine is where you'll want to store your
in-development Fuchsia packages for target devices.

Once a new Fuchsia package repository is created, you need to register the
package repository to your Fuchsia device (or devices), which permits the device
to download Fuchsia packages from this package repository. Fuchsia devices can
only download Fuchsia packages from their registered Fuchsia package repositories.

Lastly, you can set up the Fuchsia package server to serve from multiple Fuchsia
package repositories on the same host machine, where you may dedicate each package
repository for a specific purpose (for instance, to separate stable packages
from experimental packages).

## Create a package repository {:#create-a-package-repository}

To create and set up a new package repository on your host machine,
do the following:

1. Create a new repository:

   ```posix-terminal
   ffx repository create <REPOPATH>
   ```

   Replace `REPO_PATH` with the directory path for the new repository.

   However, there is no check for existing contents in that directory,
   so make sure that the directory does not exist before running this
   command.

1. (Optional) Set the default name for the repository:

   ```posix-terminal
   ffx repository default set <REPO_NAME>
   ```

   Replace `REPO_NAME` with the name of a package repository,
   for example:

   ```none {:.devsite-disable-click-to-copy}
   ffx repository default set my-repo
   ```

   This is typically not needed since the build directory level
   configuration of `ffx` already contains the correct value.

   After setting the default repository, this command exits silently
   without output.

## Start a Fuchsia package server

To be able to serve packages from Fuchsia package repositories
on your host machine, a Fuchsia package server must be running
on the machine.

To start a new Fuchsia package server, run the following command:

```posix-terminal
ffx repository server start --foreground  --repo-path <REPO_PATH>
```

Replace `REPO_PATH` with the path to a product bundle on your host machine,
for example:

```sh {:.devsite-disable-click-to-copy}
$ ffx repository server start --foreground  --repo-path ~/my-product-bundle
```

Also, notice that this command starts the package server in the foreground
(`--foreground`). This means the server's log messages will be printed to
the terminal window. Alternatively, you can start the package server in the
background by using the `--background` option. In this case, the log messages
will be stored in the `${log.dir}/repo-REPO_NAME.log` file. For more options,
see [Start package servers][start-package-servers].

## Start a Fuchsia package server using a product bundle

To serve packages from an existing product bundle, you need to download the
product bundle and start the package server using the product bundle.

Do the following:

1. View the list of available products:

    ```posix-terminal
    ffx product list
    ```

2. Download your desired product bundle:

    ```posix-terminal
    ffx product download <PRODUCT> <DOWNLOAD_DIR>
    ```

    Replace the following:

    - `PRODUCT_NAME`: The product name of your target Fuchsia device.
    - `DOWNLOAD_DIR`: The directory where you want to download the product
      bundle.

    For example:

    ```sh {:.devsite-disable-click-to-copy}
    S ffx product download core.x64 ~/Downloads/product-bundles/
    ```

3. Start the Fuchsia package server:

    ```posix-terminal
    ffx repository server start --background --product-bundle <DOWNLOAD_DIR>
    ```

    Replace `DOWNLOAD_DIR` with the directory that contains a product bundle,
    for example:

    ```sh {:.devsite-disable-click-to-copy}
    $ ffx repository server start --background  --product-bundle ~/Downloads/product-bundles
    ```

    You can confirm it is a product bundle directory by checking the presence
    of `product.bundle.json` file in the directory. For more information, see
    [Start package servers][start-package-servers].

## Register a package repository to a Fuchsia device {:#register-a-package-repository}

Once the package server is running, you need to register the server address
to the target device.

Do the following:

1. Enable your Fuchsia device to connect to the new repository:

   ```posix-terminal
   ffx target repository register [-r <REPO_NAME>] --alias fuchsia.com --alias chromium.org
   ```

   Replace `REPO_NAME` with the name of the repository that you want the
   Fuchsia device to connect to.

   If the `-r` flag is not specified, the command selects the default repository.
   For example, thiscommand sets your current Fuchsia device to connect to the
   default repository (`my-repo`) at `fuchsia.com`:

   ```none {:.devsite-disable-click-to-copy}
   $ ffx target repository register --alias fuchsia.com --alias chromium.org
   ```

   After registering the repository, this command exits silently without output.

1. Verify that the new repository is registered:

   ```posix-terminal
   ffx target repository list
   ```

   This command prints output similar to the following:

   ```none {:.devsite-disable-click-to-copy}
   $ ffx target repository list
   REPO                  URL                              ALIASES
   fuchsia-pkg://default ["http://10.0.2.2:8083/my-repo"] ["fuchsia.com", "chromium.org"]
   ```

## Deregister a package repository {:#deregister-a-package-repository}

To deregister a Fuchsia package repository from the device,
run the following command:

```posix-terminal
ffx target repository deregister [-r <REPO_NAME>]
```

Replace `REPO_NAME` with the name of a registered repository,
for example:

```none {:.devsite-disable-click-to-copy}
$ ffx target repository deregister -r my-repo
```

If the `-r` option is not specified, the command selects the default
repository.

## Stop a Fuchsia package server {:#stop-a-fuchsia-package-server}

To stop a running Fuchsia package server, run the following command:

```posix-terminal
ffx repository server stop
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx repository server stop
server stopped
```

For more options, see
[Stop running package servers][stop-package-servers].


<!-- Reference links -->

[ffx-repository]: https://fuchsia.dev/reference/tools/sdk/ffx#repository
[fuchsia-package]: /docs/concepts/packages/package.md
[fuchsia-package-server]: /docs/concepts/packages/fuchsia_package_server.md
[start-package-servers]: start-package-servers.md
[stop-package-servers]: stop-package-servers.md

