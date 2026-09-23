# Memory order models for MMU RCU

## Overview

This directory contain a model designed to be run against
[`CDSChecker`](http://plrg.ics.uci.edu/software_page/42-2), an open source model checker which helps
to test that an algorithm obeys its invariants regardless of whatever execution order a hypothetical
architecture could see within the degrees of freedom offered by the formal C++ memory model.

Additional CDSChecker models and background can be found in //zircon/system/ulib/concurrent/models

## Simple Generational RCU Model

The `simple_generational.cc` file is a model for the `rcu::SimpleGenerational` primitive defined in
`simple_generational_rcu.h`. The model simulates two parallel readers and two successive writes to
fully exercise all relevant paths.

### Memory Order Limitations

Unfortunately the already limited number of operations (reads and writes) still causes a state space
explosion when using weak memory orders. Although mere humans have reasoned that weaker orders
should be correct, the model checker is unable to terminate in any reasonable amount of time given
to it.

### Execution

Due to there being effective spinlock style interactions between the threads a degree of scheduler
fairness is needed to ensure progress. The model has been designed to be run with the `-Y` flag to
provide the necessary fairness, while minimizing possible states.
