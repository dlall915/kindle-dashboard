# boot/

`dropbear-standalone.conf` is an upstart job that starts KOReader's bundled
`dropbear` SSH server at boot, independent of KOReader ever launching.
Investigated on the `explore/standalone-ssh` branch: KOReader's own
`SSH.koplugin` (Settings -> Network -> SSH Server) starts `dropbear` with no
`-F` flag, so it self-daemonizes and reparents to init - confirmed via
`ps -o pid,ppid` showing dropbear's parent as PID 1, not any KOReader
process. A live test (`killall -TERM reader.lua`, then a fresh SSH login)
confirmed dropbear survives KOReader dying outright. The only real gap was
that nothing starts dropbear in the first place unless KOReader's plugin
does - there's no existing boot hook for it. This job closes that gap.

## Why this lives here and not just "on the device"

The device's root filesystem is mounted read-only (`ext3 ro`) after boot,
so `/etc/upstart/` isn't directly writable. `mntroot rw` / `mntroot ro`
(shipped by Amazon, used throughout the Kindle jailbreak community for
exactly this) temporarily remounts it read-write to install a file, then
back to read-only - the same mechanism the jailbreak itself relies on for
persistence. This file is the source of truth; the copy on the device at
`/etc/upstart/dropbear-standalone.conf` is the deployed artifact.

## Install

```sh
scp -P 2222 boot/dropbear-standalone.conf root@<kindle-ip>:/mnt/us/dropbear-standalone.conf
ssh -p 2222 root@<kindle-ip> '
    mntroot rw &&
    cp /mnt/us/dropbear-standalone.conf /etc/upstart/dropbear-standalone.conf &&
    mntroot ro
'
```

Verify with a real reboot (not just a process kill) - `reboot`, wait for
the device to come back on WiFi, then check port 2222 is open *before*
ever opening KOReader's own UI.

## Coexistence with KOReader's own SSH toggle

Both this job and `SSH.koplugin` invoke the identical command, and both
track the same pidfile (`/tmp/dropbear_koreader.pid`). `SSH.koplugin`'s
`isRunning()` check just tests whether that pidfile exists before
starting its own copy, so if this job already started dropbear, toggling
KOReader's SSH server on again is a no-op - no double-bind on port 2222.
