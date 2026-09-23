These tests use a [Chaos Engineering](https://en.wikipedia.org/wiki/Chaos_engineering#Chaos_Monkey) approach to find bugs in Scenic.

The tests create a Scenic session, add randomly perturb the state of that session. For example,
the `FlatlandActor` creates a tree of views, randomly adding or deleting views at each step. This
means, for example, that some views will become disconnected from the root view.