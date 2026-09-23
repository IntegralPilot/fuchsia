# Ninja Test Data

Test data is from a minimal.vim3 build.

```bash
cd fuchsia
git checkout 854ecb7a1f5c9b0c0cb939053086c463319c8498
fx set minimal.vim3
fx clean-build
rm -rf out/default/host_x64/ffx*unversioned
fx build -- --chrome_trace=ninja_trace.json.gz
cp -f out/default/ninja_trace.json.gz tools/build/ninjago/test_data/
```
