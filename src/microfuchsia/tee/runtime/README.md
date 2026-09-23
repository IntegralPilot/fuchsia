# TEE Runtime

This directory contains the TEE runtime component. This is currently implemented
as a binary packaged with the TA shared library. The binary loads the TA shared
library and looks up the entry points. In future changes this binary will expose
a FIDL protocol to interact with the TA from other components.