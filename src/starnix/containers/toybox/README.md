# Toybox Container

You can use `docker` to create an interactive container with the `toybox` utilities.

## Setup

To get started, use `docker` to download the container and save it as a TAR file:

```posix-terminal
sudo docker pull tianon/toybox
sudo docker save tianon/toybox:latest -o local/toybox.tar
sudo chmod a+r local/toybox.tar
```

Then, use the `starnix_docker_container` GN template to convert the container
into a Fuchsia package:

```gn
starnix_docker_container("toybox") {
  input_path = "//local/toybox.tar"
  package_name = "toybox"
  features = [ "rootfs_rw" ]
}
```

Next, add this target to the build system:

```posix-terminal
fx set workbench_eng.x64 --release --with //src/starnix/containers/toybox
fx build
```

## Run

You can now run the `toybox` container:

```posix-terminal
ffx component run /core/starnix_runner/playground:toybox fuchsia-pkg://fuchsia.com/toybox#meta/toybox_container.cm
```

There is nothing special about running `toybox` in this `playground` collection. You can run
`toybox` anywhere in the component topology that has the `starnix` runner capability.

Once the container is running, you can use the `console` command to get an interactive console:

```posix-terminal
ffx starnix console --moniker /core/starnix_runner/playground:toybox /bin/sh
```
