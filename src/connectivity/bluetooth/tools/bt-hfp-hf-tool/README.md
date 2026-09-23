# bt-hfp-hf-tool

`bt-hfp-hf-tool` is an interactive command-line tool for interacting with the Bluetooth
Hands-Free Profile (HFP) in the Hands-Free (HF) role.

It allows users to send commands and inspect the state of HFP connections via the
`fuchsia.bluetooth.hfp.HandsFree` API.

## Build

Include the tool in your build environment. For example, if using `fx set`, add:

```
--with //src/connectivity/bluetooth/tools/bt-hfp-hf-tool
```

### Adding bt-hfp-hands-free via Assembly Overrides

The `bt-hfp-hands-free` component requires the package & corresponding core shard to be included in the build.
To add it via assembly overrides, define an `assembly_developer_overrides` target in your `//local/BUILD.gn`
and apply the override to your build configuration. For example, if using `fx set`, add:

```gn

assembly_developer_overrides("enable_hfp_hf") {
  flexible_packages = [
    "//src/connectivity/bluetooth/profiles/bt-hfp-hands-free"
  ]
  compiled_packages = [
    {
      name = "core"
      components = [
        {
          component_name = "core"
          shards = [ "//src/connectivity/bluetooth/profiles/bt-hfp-hands-free/meta/bt-hfp-hands-free.core_shard.cml" ]
        },
      ]
    }
  ]
}
```

2. Apply the override to your build configuration. For example, if using `fx set`, add:

```
--assembly-override //local:enable_hfp_hf
```

## Usage

To run the tool, use the following command from the Fuchsia shell:

```
$ bt-hfp-hf-tool
```

This will launch an interactive REPL. You can type `help` to list available commands and their descriptions.

Note: This tool is intended for development and testing. It may interact with the system in ways
that can cause the HFP component to enter an unexpected state.
