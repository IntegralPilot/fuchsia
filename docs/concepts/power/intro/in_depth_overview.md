# Power framework in depth

This section focuses on how power framework works conceptually. This information
equips you to arbitrarily extend the power topology into your code rather than
simply connecting to the part of the topology exposed by System Activity
Governor.


## Overview

Power framework uses a directed, acyclic graph ("DAG") of power dependencies to
manage the system. Power framework does not know the DAG for a particular
system, but relies on components in the system to build the DAG. All operations
that power framework performs are walks of the DAG with different visit order
(ie. pre-, post-, in-) and consequences. **That's basically it, you now
understand the power framework, the rest is implementation details, albeit
important ones.**
