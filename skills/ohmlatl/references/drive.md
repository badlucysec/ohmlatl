# The drive

The computer lives on a USB stick or external drive, next to the binary. The host only needs Linux x86_64 (kernel 5.10 or newer), `/dev/kvm`, nftables, systemd, an SSH client, and `sudo` for the user. Nothing persistent is left on the host.

## Layout

```text
<dir>/                         # created by `ohmlatl init`, 0755, owned by the user
├── .ohmlatl                   # marker, "format: 1"
├── ohmlatl                    # the binary
└── images/                    # 0700
    └── <name>/<tag>/
        ├── rootfs.luks        # LUKS2 + ext4, sparse, 0600
        └── manifest.yaml      # 0600, read on every run
```

Nothing else belongs there: no SSH keys, no VM state, no logs. Do not add files to `<dir>` or `images/`.

On the host, only temporary state: `/run/ohmlatl/` (tmpfs, gone on host reboot: sockets, `serial.log`, network records) and, on the host where builds run, `~/.cache/ohmlatl/` (embedded Podman bundle and its storage, which holds the container images **in plaintext**; `ohmlatl podman system reset` clears it).

## Finding the directory

In order: `--home <dir>`, then `OHMLATL_HOME`, then the directory of the running binary if it has the `.ohmlatl` marker. With the drive's binary on `PATH`, it just works:

```bash
export PATH="/media/$USER/drive/ohmlatl:$PATH"
```

Only `init`, `build`, `image` and `run` need the directory. `stop`, `ps`, `console`, `logs`, `shell`, `exec` and `cp` work from `/run/ohmlatl/` alone, so they work even if the drive was removed or remounted elsewhere.

## `ohmlatl init <dir>` (human)

```bash
./ohmlatl init /media/$USER/drive/ohmlatl
```

- Filesystem must be ext4, btrfs or xfs. FAT and exFAT are refused (no sparse files, no permissions).
- A freshly formatted ext4 root belongs to root: `init` then asks for sudo once to create `<dir>` owned by the user.
- It copies its own binary into `<dir>`. On a directory that already has the marker, it only updates the binary.
- Mounted `noexec`: only a warning; the drive's binary cannot run, but an `ohmlatl` installed on the host with `--home <dir>` works.

## Ownership

`<dir>`, the binary and `images/` must belong to the current UID. On another host where the user has a different UID, every command stops and prints `sudo chown -R $(id -u):$(id -g) <dir>` (human). Commands also refuse to use sudo if `<dir>`, the binary or a parent directory is writable by another non-root user.

## On another machine

1. On the old host: `ohmlatl stop <vm>` for every VM in `ohmlatl ps`, then unmount the drive.
2. On the new host:

   ```bash
   export PATH="/media/$USER/drive/ohmlatl:$PATH"
   ohmlatl doctor --run
   ohmlatl run <name:tag>        # human
   ```

3. The binary on the drive is only as trustworthy as the drive. On a new host the human can verify it against the release, or use an `ohmlatl` installed on the host with `--home`:

   ```bash
   # SHA256SUMS and SHA256SUMS.sig from the release page of github.com/badlucysec/ohmlatl,
   # allowed_signers from docs/ in that repository (it is not on the drive)
   ssh-keygen -Y verify -f allowed_signers -I badlucysec -n file -s SHA256SUMS.sig < SHA256SUMS
   grep ohmlatl-<version>-x86_64 SHA256SUMS     # <version> from `ohmlatl --version`
   sha256sum /media/$USER/drive/ohmlatl/ohmlatl
   ```

- Network settings travel in each image's `manifest.yaml` and are applied as they are; `run` prints the forwards. A tampered drive could open forwards to the host's loopback, so check that list on a new host.
- Builds need the Podman cache, which stays on the host where it was built: building on another host runs the whole chain (base, then derived) again there.
- A borrowed host must still be trusted: it sees the passphrase and the VM's memory.

## Backup

With the VM stopped, keeping the file sparse, to another ext4, btrfs or xfs filesystem:

```bash
rsync -aS /media/$USER/drive/ohmlatl/images/<name>/ /backup/ohmlatl/images/<name>/
```

`cp -a --sparse=always` also works. A plain `cp` or a FAT/exFAT target fills the holes and the copy takes the full disk size.

## Unplugging

Always `ohmlatl stop <vm>` first. When the host shuts down normally, running VMs get the same clean stop, because each VM's unit is bound to the drive's mount. A drive pulled out with a VM running gives the guest I/O errors and can damage its filesystem; see [troubleshooting.md](troubleshooting.md).
