# Magma debug utils

This is used to debug an MSD by calling some FIDL methods on it directly.

It is most easily used via `ffx component explore` for example the following should work on most
platforms:

```
ffx component explore msd --tools fuchsia-pkg://fuchsia.com/magma-debug-utils -c "magma-debug-utils --count=1 --power-state=0,1"
```

Note: Make sure to rerun `fx build` after making changes to this file!
