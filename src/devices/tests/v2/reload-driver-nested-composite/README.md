# Reload Driver Nested Composite Test

This test checks that DFv2 reload works when we have a nested composite.

## Scenario

This is the topology that will be tested. The conventions for the graph below are:
 - `X` in the edges indicates `colocation=false` in the child. Otherwise it is `true`.
 - Primary parent of the composite is the parent it is directly under.
 - Node markings are in the form `NodeName(optional bound driver)`

```
           dev(root)
           /   |   \
          /    |    \
         /     |     \
        /      |      \
       /       |       \
     B()    C(top-B)   D()
     |         |        |
     |         |        |
     |        E()       |
     |         |        |
     |         |        |
     X----------        |
     |                  |
     |                  |
F(Composite-A)          |
     |                  |
     |                  |
     G()                |
     |                  |
     |                  |
     |-------------------
     |
     |
 H(Composite-B)
```

## Expectation

When `Composite-B` is reloaded, the following nodes should be reloaded:
 - H
 - G
 - F