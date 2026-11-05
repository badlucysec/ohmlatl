# Troubleshooting

Start with the error line (`ohmlatl: error: ...`): it says what failed and usually what to run. For host problems, run `ohmlatl doctor` and follow each `fix:`. For a VM, `ohmlatl ps` and `ohmlatl logs <vm>` show where the boot stopped.

## Build

| Symptom | Cause | What to do |
|---|---|---|
| `ohmlatl.yaml in the current directory: ...` | `build` runs where `ohmlatl.yaml` is. | `cd` to that directory. |
| `ohmlatl.yaml: line N: ...`, `unknown field ...` | Manifest outside the strict schema or YAML subset. | Fix it with [manifest.md](manifest.md). |
| `<name>:<tag> already exists ...` | Builds never overwrite. | Use a new tag (`-t name:2026-11-10`). |
| `podman create <ref> failed; --from uses local images only` | The image is not in ohmlatl's Podman storage. | `ohmlatl podman pull docker.io/...` or `ohmlatl podman build`. Images from the host's own podman/docker are not visible. |
| `short-name ... did not resolve` (Podman) | No unqualified registries. | Write `docker.io/library/debian:trixie`, not `debian`. |
| `/sbin/init in the image is not systemd` | Base without systemd. | Install `systemd systemd-sysv` (Debian family). |
| `the image has no /usr/sbin/sshd` | No SSH server. | Install `openssh-server` and enable it. |
| `user dev is not in /etc/passwd`, `has uid:gid ...`, `home ... does not exist` / `not owned` | Image and `user` in the manifest disagree. | `useradd -m -u <uid> <name>` in the Containerfile, or fix the manifest. |
| warning `ohmlatl cp will not work` | No sftp subsystem or `sftp-server`. | Install `openssh-sftp-server` or set `Subsystem sftp internal-sftp`, rebuild. |
| warning `ohmlatl shell will not log in` | No password and no `authorized_keys`. | Add a key (see [images.md](images.md)) or set a password later from the console. |
| `--size ... is too small`, `the build writes about X to the drive, Y free` | Disk too small or drive full. | Larger `--size`, or free space on the drive. |
| `another build of <ref> is running` | Same tag being built. | Wait or use another tag. |
| Podman fails with user namespace errors | Rootless Podman not ready (subuid/subgid, `newuidmap`, `/dev/fuse`, Ubuntu AppArmor). | `ohmlatl doctor --build` and its `fix:` lines (human, root). |
| `/dev/tty: ...` | A prompt without a terminal. | It is a human step: give the command to the human. |

## Boot

| Symptom | Cause | What to do |
|---|---|---|
| `<name> is already running` | One VM per image name. | `ohmlatl stop <name>` first, if that is what the user wants. |
| `no image <name:tag>` | Typo, or the tag does not exist. | `ohmlatl image ls`. |
| `run` stops before the boot with an error or `see: sudo journalctl -u ohmlatl-<uid>-<name>` | Supervisor failed (manifest, network, permissions). | Read the error; the journal needs the human (root). |
| `run` refuses because of the pool | A host address or route is inside `172.31.0.0/16`. | `OHMLATL_POOL=10.213.0.0/16 ohmlatl run <name:tag>`. |
| `ohmlatl logs` ends at `Enter passphrase for /dev/vda:` | Waiting for the human. | Ask the human to run `ohmlatl console <vm>`, type it, `Ctrl-]`. |
| `(initramfs) #` in the logs | Three wrong passphrases, or `e2fsck` could not repair. | Human: `ohmlatl console <vm>` for the rescue shell; or `ohmlatl stop <vm>` and `run` again. |
| `e2fsck` errors or garbage in the boot | Drive removed while running, or a tampered/corrupt `rootfs.luks`. | `run` again (the initramfs runs `e2fsck -p`). If it keeps failing: restore a backup or rebuild. |
| `console busy: another client is attached` | One console client at a time. | The other client detaches with `Ctrl-]`. |
| A stale passphrase prompt in the console | The console replays the last 64 KiB. | Look at `ohmlatl ps` and fresh `logs` before assuming the VM waits for it. |
| VM gone from `ps` | Firecracker died, guest powered off, or host rebooted. | `ohmlatl logs <vm>` for the reason (lost on host reboot); the human runs `ohmlatl run <name:tag>` again. VMs never start by themselves. |

## Access

| Symptom | Cause | What to do |
|---|---|---|
| `no VM <vm> running` | Not booted, or the name includes a tag. | `ohmlatl ps`; commands other than `run` take the name only. |
| `ssh login to <user>@<ip> failed` | No master connection and no usable key; or no TTY for the password. | Human: `ohmlatl shell <vm>` once per boot; or add a key to the image. |
| `ohmlatl shell` asks for a password that does not exist | The user has no password and no key. | Human: `ohmlatl console <vm>`, `sudo passwd <user>`, retry. Without sudo in the image, set it before the build with `ohmlatl podman run -it`. |
| `exec` or `cp` fails right after boot | sshd not up yet, or the passphrase was not typed. | Wait for the login prompt in `ohmlatl logs <vm>`, retry. |
| `cp needs exactly one <vm>:<path>` | Both or neither side is remote. | One side local, one `<vm>:<path>`. |
| `cp` fails with an sftp error | No `sftp-server` in the image. | Install it in the guest (`exec -- sudo apt-get install -y openssh-sftp-server`) or rebuild. |

## Network

| Symptom | Cause | What to do |
|---|---|---|
| No internet or DNS in the guest | `internet: false`, or a host firewall (Docker, ufw, firewalld) drops `ohm*`. | Check the manifest; `ohmlatl doctor --run` prints the rule to add (human, root). |
| `curl ohmlatl-gateway:<port>` refused | No forward for that port, or changed manifest without a new `run`. | Add the forward, `stop`, `run`. |
| Forward to `host:<port>` connects then closes | Nothing listens on the host's `127.0.0.1:<port>`. | Start the service on the host loopback. |
| Forward to `vm/<name>:<port>` closes at once | The target VM is stopped, or its service listens only on its loopback. | `ohmlatl run` it; make the service listen on `0.0.0.0`. |
| Docker or libvirt stopped routing after ohmlatl ran | The `inet ohmlatl-fwguard` table. | `doctor --run` shows it; human: `sudo nft delete table inet ohmlatl-fwguard`. |
| Guest cannot reach the LAN, the host or another VM | By design. | Only a declared forward opens a path. |

## Disk and drive

| Symptom | Cause | What to do |
|---|---|---|
| I/O errors in the guest while `df` shows room | The drive is full (the sparse file cannot grow). | Free space on the drive, `stop`, `run`. |
| Disk full inside the guest | The VM's disk size is reached. | `ohmlatl stop <vm>`, human: `ohmlatl image resize <name:tag> <bigger>`, `run`. |
| `<ref> is in use: stop its VM first` | `resize`, `passwd` and `rm` need the VM stopped. | `ohmlatl stop <vm>`. |
| Every command prints a `sudo chown -R ...` | The directory belongs to another UID (other host). | Human runs the printed command. |
| `Permission denied` running the drive's binary | Drive mounted `noexec`. | Remount without `noexec`, or a host `ohmlatl` with `--home <dir>`. |
| `... unknown .ohmlatl format`, no directory found | Not an ohmlatl directory, or wrong `--home`. | `ohmlatl init <dir>` (human) or the right `--home`. |
| Lost passphrase | No key exists anywhere. | The image cannot be opened. Rebuild and start over. |
