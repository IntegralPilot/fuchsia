# Stub implementations to satisfy apple_support's MODULE.bazel in BCR.

def _stub_empty_repo_impl(repository_ctx):
    repository_ctx.file("BUILD", content = "# stub empty repo")

_stub_empty_repo = repository_rule(
    implementation = _stub_empty_repo_impl,
)

def _stub_extension_impl(repository_ctx):
    _stub_empty_repo(name = "local_config_apple_cc")
    _stub_empty_repo(name = "local_config_apple_cc_toolchains")

apple_cc_configure_extension = module_extension(implementation = _stub_extension_impl)
