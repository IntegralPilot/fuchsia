# DSO simple example

This directory contains a simple example to use with the [DSO
runner][src/sys/runners/dso/README.md].

## Instructions

To run this example, you'll first need to add the [DSO runner] to the core
environment. You must do this because the DSO runner is not there by default. To
do this, locally edit the file
[`/src/developer/remote-control/meta/laboratory.core_shard.cml`][laboratory-shard]
with the following chunk. (This is assuming that you're using a non-production
build, such as `core.x64`, which includes `ffx-laboratory`.) This step is
necessary because there is no imperative way to configure the laboratory
collection to use the runner.

```json5
    collections: [
        {
            // This collection is used by developers to create and run arbitrary components.
            // The `ffx component run` command adds components to this collection.
            name: "ffx-laboratory",
            environment: "#laboratory-env", // replaces #core-env
            durability: "transient",
        },
    ],
    children: [
        {
            name: "dso_runner",
            url: "fuchsia-pkg://fuchsia.com/dso_runner#meta/dso_runner.cm",
            environment: "#core-env",
        },
    ],
    environments: [
        {
            name: "laboratory-env",
            extends: "#core-env",
            runners: [
                {
                    runner: "elf",
                    from: "parent",
                },
                {
                    runner: "dso",
                    from: "#dso_runner",
                },
            ],
            resolvers: [
                {
                    resolver: "full-resolver",
                    from: "#pkg-resolver",
                    scheme: "fuchsia-pkg",
                },
            ],
        },
    ],
```

Now add `dso_runner` and the example to your package set and rebuild.

```shell
$ fx set core.x64 --with //src/sys/runners/dso:package --with //examples/components/dso
```

You'll need to update your device to include the changes to the laboratory. You
can either `fx ota`, reflash, or if you're using an emulator simply restart it.

```shell
$ ffx emu stop
$ ffx emu start --console
```

Start the package server if it's not already running:

```shell
$ fx serve&
```

Now just run the example like you would any component in the laboratory.

```shell
$ ffx component run core/ffx-laboratory:dso fuchsia-pkg://fuchsia.com/simple_dso#meta/simple_dso.cm
```

You should be able to find evidence the component ran in the logs.

```
$ ffx log --filter dso_runner
ffx log --filter dso_runner
[00079.042964][dso_runner] INFO: Started component url=fuchsia-pkg://fuchsia.com/simple_dso#meta/simple_dso.cm
[00079.043546][dso_runner][simple_dso] INFO: [main.cc(13)] Hello world!
[00079.043591][dso_runner] INFO: Component terminated component_url=fuchsia-pkg://fuchsia.com/simple_dso#meta/simple_dso.cm exit_code=0
[00079.043664][dso_runner] INFO: Component stopped url=fuchsia-pkg://fuchsia.com/simple_dso#meta/simple_dso.cm
```

[laboratory-shard]: /src/developer/remote-control/meta/laboratory.core_shard.cml
