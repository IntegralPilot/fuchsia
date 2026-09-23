# Manually test suspend/resume on Workbench

This guide provides instructions on how to manually test suspend and resume on a
device running a suitable [Workbench][workbench-guide] image.

## Perform manual suspend and resume {:#perform-manual-suspend-and-resume}

*   {With serial}

    1.  Connect to the device via USB.

    1.  Take an Application Activity lease via `ffx power`'s helper component:

        ```posix-terminal
        ffx power suspend prevent
        ```

    1.  Drop the session wake lease:

        ```posix-terminal
        ffx session drop-power-lease
        ```

    1.  Disconnect USB to eliminate the persistent wake lease caused by the USB
        or charger connection.

    1.  Using serial, drop the Application Activity lease and reacquire it after
        a delay:

        ```posix-terminal
        powerutil suspend prevent --restart --wait-time 5s
        ```

        This will allow the system to suspend as long as no other wake leases
        are active.

    1.  Wait until serial logs show evidence of system suspension. Logs are not
        necessarily flushed prior to suspension, so you may see them abruptly
        stop.

    1.  Reconnect USB or take another suitable action (touch, button press,
        etc.) to resume the device. If `--wait-time` has elapsed, the helper
        component will reacquire an Application Activity lease and prevent
        further suspension on its own.

*   {Without serial}

    1.  Connect to the device via USB.

    1.  Take an Application Activity lease via `ffx power`'s helper component:

        ```posix-terminal
        ffx power suspend prevent
        ```

    1.  Drop the session wake lease:

        ```posix-terminal
        ffx session drop-power-lease
        ```

    1.  Drop the Application Activity lease and reacquire it after a delay:

        ```posix-terminal
        ffx power suspend prevent --restart --wait-time 5s
        ```

    1.  Disconnect USB to eliminate the persistent wake lease caused by the USB
        or charger connection.

    1.  Wait for several seconds. Without a serial connection, you will not
        be able to confirm whether suspension has occurred until you replug
        the device.

    1.  Reconnect USB.

    1.  Use `ffx log` to look for evidence of suspension. There isn't a
        canonical message to indicate this, but at time of writing, the kernel
        message `INFO: Suspending non-boot CPUs...` is a good indicator.

## Troubleshooting {:#troubleshooting}

### Check wake leases {:#check-wake-leases}

Any active wake leases prevent the system from suspending. To check active wake
leases:

*   {Serial}

    ```posix-terminal
    inspect show bootstrap/system-activity-governor:root/wake_leases
    ```

*   {USB}

    ```posix-terminal
    ffx inspect show bootstrap/system-activity-governor:root/wake_leases
    ```

### Increase delay before wake lease reacquisition {:#increase-delay-before-wake-lease-reacquisition}

To give the system longer to suspend, increase the delay using the `--wait-time`
option:

```posix-terminal
powerutil suspend prevent --restart --wait-time 10s
```

<!-- Reference links -->

[workbench-guide]: /docs/development/build/build_workbench.md