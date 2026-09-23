To update project.json.gz:

cd $FUCHSIA_DIR
fx set core.x64 --with //tools/check-licenses/util/testdata/example
fx gn gen out/default --all --ide=json --tree
cd $FUCHSIA_DIR/out/default && gzip -f -k -9 project.json
cp project.json.gz $FUCHSIA_DIR/tools/check-licenses/util/testdata/example
fx set core.x64 --with //tools/check-licenses/util:tests && fx test

The resulting project.json file is massive. On my machine, the compressed file
was over 20MB. You may want to open up the file in your editor of choice, and
delete sections of the file that do not contain the path
"//tools/check-licenses/". Doing this has no bearing on the test, can save tons
of space and allow the test to run faster.
