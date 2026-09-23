# List handles and kernel objects

## `handles`

The `handles` command lists all handles and [VMOs][vmo-docs]. Some of these
Virtual Memory Objects can be mapped but won't have open handles with the
`handles` command.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
handles
      Handle  Type                  Koid
      <none>  ZX_OBJ_TYPE_VMO      30040
  4166674259  ZX_OBJ_TYPE_TIMER    30158
  4167722515  ZX_OBJ_TYPE_PORT     30157
  4169819767  ZX_OBJ_TYPE_CHANNEL  30222
```

You can use `handle` to specify a handle value to see more detailed information.
For example:

Note: This command is `handle` and not `handles`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
handle 4166674259
  Handle  4166674259
    Type  ZX_OBJ_TYPE_TIMER
    Koid  30158
  Rights  ZX_RIGHT_SIGNAL
          ZX_RIGHT_WAIT
          ZX_RIGHT_INSPECT
```

You can also look up an object by koid. Koid lookup only searches the objects
in the debugged process and won't match arbitrary kernel objects owned by other
processes. Koid lookup is the only way to show detailed information for mapped
VMOs that have no open handles.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
handle -k 30108
                   Handle  <none>
                     Type  ZX_OBJ_TYPE_VMO
                     Koid  30108
                   Rights  ZX_RIGHT_NONE
                     Name  data0:blob-60
        VMO size in bytes  4096
              Parent koid  30105
               # children  0
               # mappings  1
              Share count  1
                    Flags  ZX_INFO_VMO_TYPE_PAGED
                           ZX_INFO_VMO_VIA_MAPPING
          Committed bytes  4096
             Cache policy  ZX_CACHE_POLICY_CACHED
           Metadata bytes  176
  Committed change events  0
```

[vmo-docs]: /docs/reference/kernel_objects/vm_object.md
