# Paper Shader Compiler

This tool compiles the GLSL shaders into SPIR-V binaries (such as those used by RectangleCompositor).

The name is a historical anomaly, from the legendary PaperRenderer which was
deleted in Change-Id: I7c277890896fc61369d7d209182d0c58d0937794.

The resulting compiled shader SPIR-V binaries are then saved to disk in
//src/ui/lib/escher/shaders/spirv. The name for the file is auto_generated
based on the input name of the original shader plus a hash value calculated
from the list of shader variant arguments.

To use:

1) Migrate to your fuchsia root directory.

2) fx set workbench_eng.x64 --with //src/ui/tools:scenic

3) fx build --host //src/ui/tools/paper_shader_compiler

4) fx host-tool paper_shader_compiler
