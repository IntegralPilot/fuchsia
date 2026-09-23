## multibind-composite-test

This test verifies that multibind composites work as expected.

It adds following nodes to parent the composites:

* Node A
* Node B
* Node C
* Node D

Since we are testing for multibind, the composites will share parents.
The composite breakdown is as follows:

**Composite node spec 1**

* Node A (primary)
* Node C
* Node D

**Composite node spec 2**

* Node B
* Node D (primary)
