# Reload Driver Cross-Colocated Test

This test checks that DFv2 driver reload/restart works properly when drivers are colocated across independent branches of the node DAG using a custom string-based driver host tag (`driver_host = "shared-host"`), specifically covering composite nodes whose parents span both restarting and non-restarting branches.

## Scenario

This is the node topology that is tested. The conventions for the graph below are:
 - `X` in the edges indicates `colocate=false` in the child node addition. Otherwise it is `true`.
 - Node markings are in the form `NodeName(Host ID)`.

```
               dev(root)
              /         \
             X           X
            /             \
      left_parent     right_parent
       (Host 1)         (Host 2)
          |                 |
          X                 |
          |                 |
       child_a         right_child
     (driver=target)    (Host 2)
       (Host 3)             |
          |                 |
     child_a_sub            |
       (Host 3)             |
          \                 /
           \               /
            \             /
             composite: child_b
               (driver=leaf)
                 (Host 3)
```

## Details

1. `dev` (root driver) creates `left_parent` and `right_parent` as child nodes with `colocate = false`. They are placed into separate driver host instances (`Host 1` and `Host 2`). It also defines a `CompositeNodeSpec` for `child_b` with `driver_host = "shared-host"`, with parents `right_child` (primary) and `child_a_sub`.
2. `left_parent` driver adds child node `child_a` with `driver_host = "shared-host"`.
3. `target` driver (`target.cm`) binds to `child_a` in `Host 3`.
4. `target` driver adds child node `child_a_sub` (colocated in `Host 3`).
5. `right_parent` driver adds child node `right_child` (colocated in `Host 2`).
6. When both parents match, composite node `child_b` is assembled and assigned to `Host 3` (`shared-host`).
7. `leaf` driver (`leaf.cm`) binds to `child_b` in `Host 3`.
8. Both `child_a` (and its child `child_a_sub`) and `child_b` reside in the same driver host process (`Host 3`, moniker `driver-host-shared-host`).

## Expectation

When `target` driver is restarted via `restart_driver_hosts("fuchsia-boot:///dtr#meta/target.cm")`:
 - Driver Manager collects all driver hosts containing `target.cm` (`Host 3`).
 - `child_b` is in `Host 3`, but its primary parent `right_child` is in `Host 2` (not restarting), while its secondary parent `child_a_sub` is under `child_a` in `Host 3`.
 - Driver Manager's BFS must recognize that `child_b` has an ancestor (`child_a`) in the restarting set, and therefore must NOT pick `child_b` as an independent topmost node to restart (which would set its shutdown intent to `kRestart` and skip removal, deadlocking `child_a_sub` and `child_a`).
 - Instead, only `child_a` is picked to restart, cascading removal through `child_a_sub` to `child_b`.
 - `child_b` (running `leaf`) is re-bound into the **new** driver host process once `child_a` restarts.
 - The host process KOIDs for `child_a` and `child_b` after restart should be **identical to each other** and **different from their initial host process KOID**.
 - `left_parent` and `right_parent` must **not** be restarted and must retain their initial host process KOIDs.

