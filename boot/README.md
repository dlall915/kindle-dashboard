# boot/

`dropbear-standalone.conf` is an upstart job. It starts KOReader's
bundled `dropbear` SSH server at boot, without a dependency on
KOReader.

This job came from an investigation on the `explore/standalone-ssh`
branch. KOReader's own `SSH.koplugin` (Settings -> Network -> SSH
Server) starts `dropbear` with no `-F` flag. Because of this, `dropbear`
self-daemonizes and reparents to init. `ps -o pid,ppid` confirms this:
dropbear's parent is PID 1, not a KOReader process.

A live test confirmed that dropbear survives when KOReader stops
outright: `killall -TERM reader.lua`, then a fresh SSH login.

The one real gap: nothing starts dropbear unless KOReader's plugin
does, since no boot hook exists for it. This job closes that gap.

## Why this lives here and not just "on the device"

After boot, the device mounts its root filesystem as read-only
(`ext3 ro`). Because of this, `/etc/upstart/` is not directly writable.

`mntroot rw` and `mntroot ro` are Amazon commands. The Kindle jailbreak
community uses them for exactly this task. `mntroot rw` remounts the
filesystem as read-write, to install a file. `mntroot ro` remounts it
back to read-only. The jailbreak itself relies on this same mechanism
for persistence.

This file is the source of truth. The copy on the device, at
`/etc/upstart/dropbear-standalone.conf`, is the deployed artifact.

## Install

```sh
scp -P 2222 boot/dropbear-standalone.conf root@<kindle-ip>:/mnt/us/dropbear-standalone.conf
ssh -p 2222 root@<kindle-ip> '
    mntroot rw &&
    cp /mnt/us/dropbear-standalone.conf /etc/upstart/dropbear-standalone.conf &&
    mntroot ro
'
```

Check this job with a real reboot, not only a process kill:
1. Run `reboot`.
2. Wait for the device to come back on WiFi.
3. Check that port 2222 is open, before you open KOReader's own UI.

## Coexistence with KOReader's own SSH toggle

This job and `SSH.koplugin` run the identical command. Both track the
same pidfile: `/tmp/dropbear_koreader.pid`.

`SSH.koplugin`'s `isRunning()` check only tests whether that pidfile
exists, before it starts its own copy of dropbear. If this job already
started dropbear, a later toggle of KOReader's SSH server is a no-op.
There is no double-bind on port 2222.
