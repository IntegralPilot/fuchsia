This directory contains the output of the bison / flex parser generator on the
aidl language definition files. In the Android build bison runs as a build step.
We don't have access to that tool in our build environment, so instead we check
in the output of the generator.

The license situation of this code was examined in https://fxbug.dev/42068556