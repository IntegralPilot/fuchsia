# Escher Shaders

This directory contains the GLSL shader source code used by various Escher
renderers, along with their compiled SPIR-V outputs.

- `flatland/`: Shaders currently used in production Scenic.

- `paper/` and `model_renderer/`: These contain historical shaders originally
used for 3D rendering and the legendary `PaperRenderer`.  They are currently
preserved because they are still used by several tests.

- `spirv/`: Contains the pre-compiled SPIR-V binaries generated from the GLSL
shaders in the directories above, using the `paper_shader_compiler` host tool.
