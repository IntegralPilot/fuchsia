This directory contains hand-crafted textual SELinux policies and their compiled binary policies.

Policies can be compiled using the `checkpolicy` Linux utility:

```
checkpolicy --mls --sort --optimize -c 33 --output src/starnix/lib/selinux/testdata/micro_policies/${POLICY_NAME}_policy.pp -t selinux src/starnix/lib/selinux/testdata/micro_policies/${POLICY_NAME}_policy.conf
```

or to rebuild all of the micro-policies from Bash:
```
for file in src/starnix/lib/selinux/testdata/micro_policies/*.conf; do checkpolicy --mls --sort --optimize -c 33 --output ${file%.conf}.pp $file >/dev/null; done
```
