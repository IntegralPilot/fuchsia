# Testing the lite Google Analytics 4 client (manually)
Compile the test
```
fx set core.x64  --with-host '//src/lib/analytics/cpp/core_dev_tools:tests'
fx build
```

Run the test
```
out/default/host_x64/analytics_cpp_core_dev_tools_google_analytics_4_client_manualtest <measurement-id> <measurement-key>
```

Expected result:
- A new event is added to `<measurement-id>`, with name "test_event",
  and parameters p1="v1", p2=2, p3=false (shown as 0), p4=2.5,
  and user properties up1="v1", up2=2, up3=false (shown as 0), up4=2.5
