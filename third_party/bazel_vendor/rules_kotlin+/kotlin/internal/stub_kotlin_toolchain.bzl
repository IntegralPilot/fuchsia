def _stub_kotlin_toolchain_info_impl(ctx):
    return [platform_common.ToolchainInfo()]

stub_kotlin_toolchain_info = rule(
    implementation = _stub_kotlin_toolchain_info_impl,
    provides = [platform_common.ToolchainInfo],
    attrs = {},
)
