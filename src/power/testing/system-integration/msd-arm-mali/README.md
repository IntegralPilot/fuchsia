The easiest way to run this test is to use the following product bundle:

```
workbench_eng.vim3
```

And it needs the `enable_non_hermetic_testing` set.

The easiest way to set this is to create a file in `local/BUILD.gn` with the following:

```
assembly_developer_overrides("enable_power_testing") {
  platform = {
     power = {
      enable_non_hermetic_testing = true
    }
  }
}
```

And have the following line in your `args.gn`

```
product_assembly_overrides_label = "//local:enable_power_testing"
```
