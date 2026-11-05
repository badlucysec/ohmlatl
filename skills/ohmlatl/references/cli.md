# CLI reference

The full command set, from `ohmlatl` with no arguments:

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]
ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls
ohmlatl image inspect|rm <name:tag>
ohmlatl image resize <name:tag> <size>
ohmlatl image passwd <name:tag>
ohmlatl run <name:tag> [--vcpus n] [--memory n]
ohmlatl stop <vm>
ohmlatl ps
ohmlatl console <vm>
ohmlatl logs <vm> [-f]
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl --version
```

Nothing else exists. `launch`, `supervise` and `cleanup` are internal (run by `run` through sudo and by the systemd unit): never call them. A wrong argument list prints the usage above and exits 1.

## Conventions

- **Exit code**: 0 on success, 1 on any failure. Errors are printed as `ohmlatl: error: <message>`.
- **Prefixes**: progress lines start with `[ohmlatl]` (or `[build]` during a build).
- **`<name:tag>`** names an image on the drive. **`<vm>`** is a running VM, always the image **name** without the tag.
- **Names**: `^[a-z][a-z0-9-]{0,30}$`. **Tags**: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
- **Sizes** (`--size`, `--memory`, `image resize`): a number with an optional binary unit, `K`, `M`, `G`, `T` or `KiB`, `MiB`, `GiB`, `TiB`; no unit means bytes.
- **`--home <dir>`** is accepted by every command and points at the ohmlatl directory. Without it: `OHMLATL_HOME`, else the directory of the running binary (the binary on the drive finds its own images).
- **`OHMLATL_POOL`** (default `172.31.0.0/16`) changes the range VM subnets come from; read by `run`.
- **`--`** in `shell` and `exec`: everything after it goes to the guest unchanged; no ohmlatl flag is taken from it.
- **Prompts**: passphrases and `[y/N]` confirmations are read from `/dev/tty`. Without a terminal they fail. Those commands are for the human.

## Setup

### `ohmlatl init <dir>`
Human. Creates the ohmlatl directory on the drive (`.ohmlatl`, a copy of the binary, `images/`) and checks filesystem (ext4, btrfs or xfs), `exec` mount and ownership. If the drive root is not writable by the user, it asks for sudo once. See [drive.md](drive.md).

### `ohmlatl doctor [--build|--run]`
Agent. Checks the host without changing it. No flag runs both groups.
- `DRIVE`: the ohmlatl directory (filesystem, exec, owner).
- `BUILD`: rootless Podman (65536 subuids/subgids, `newuidmap`, `/dev/fuse`), `e2fsprogs`, `cryptsetup`, `ssh-keygen`, `udevadm`, `sudo`, free space. On Ubuntu 23.10+ it prints an AppArmor profile for the embedded Podman and the commands to install it.
- `RUN`: `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, SSH client, and Docker, ufw or firewalld rules that would drop VM traffic.

Output lines are `[ok] ...`, `[warn] ...` or `[fail] ...` followed by `fix: <what to run>`. Exit 1 if any check fails. Fixes that need root are for the human.

### `ohmlatl --version`
Prints `ohmlatl <version>`.

## Build

### `ohmlatl podman <args>`
Agent (non-interactive uses). Runs the embedded Podman with `<args>` untouched, `--home` included. Rootless, with its own storage on the host (`~/.cache/ohmlatl/podman/storage`), separate from any system Podman or Docker and never on the drive: images built with the host's own `podman` or `docker` are not visible here. There are no unqualified search registries: always write full references (`docker.io/library/debian:trixie`, not `debian`). Typical: `podman build -t base:2026-11-02 .`, `podman pull docker.io/...`, `podman images`, `podman commit <ctr> <ref>`, `podman rm`, `podman rmi`, `podman run --rm <ref> <cmd>`. `podman run -it` is interactive: give it to the human.

### `ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]`
Human (passphrase twice, then sudo once). Must run in the directory holding `ohmlatl.yaml`.
- `-f <file>`: the Containerfile; default is `build.containerfile` from the manifest. The build context is the current directory.
- `--from <ref>`: start from a local Podman image instead of a Containerfile. Remote images must be pulled first. `-f` and `--from` exclude each other.
- `-t <name:tag>`: overrides `name` and `build.tag`; `-t name` alone means tag `latest`. Without `-t` and without `build.tag`, the tag is `latest`.
- `--size <n>`: disk size the VM sees. Default: 32 GiB or twice the content plus 1 GiB, whichever is larger. Too small for the content is refused before the passphrase.

Refuses an existing tag (`already exists ... use another tag`), a concurrent build of the same tag, and a drive without room for what will be written. Validates the image: no systemd as `/sbin/init`, no `/usr/sbin/sshd`, a missing user, wrong uid:gid, a missing home or one not owned by the uid all fail. Warns (still builds) when there is no sftp subsystem or `sftp-server` (`cp` will not work) and when the user has no password and no `authorized_keys` (`shell` will not log in). Writes `images/<name>/<tag>/{rootfs.luks, manifest.yaml}`. The exported rootfs lives in `~/.cache/ohmlatl/build` during the build and is deleted at the end, also on failure.

Before handing it to the human, you can validate cheaply: the manifest parses (see [manifest.md](manifest.md)), the base image exists (`ohmlatl podman images`), and the guest contents are right (`ohmlatl podman run --rm <ref> sh -c 'id dev; ls -ld /home/dev; ls -l /sbin/init /usr/sbin/sshd'`).

## Images on the drive

### `ohmlatl image ls`
Agent. All images on the drive, running or not:
```text
NAME          TAG          SIZE     ON DISK
kali-custom   2026-11-02   32 GiB   2.9 GiB
```
`SIZE` is what the VM sees, `ON DISK` what the sparse file really takes.

### `ohmlatl image inspect <name:tag>`
Agent. Prints `image:`, `rootfs:` (absolute path), `size:`, `on disk:`, then the image's `manifest.yaml`, and fails if that manifest no longer parses. Use it to find the path of the manifest to edit.

### `ohmlatl image resize <name:tag> <size>`
Human (sudo, passphrase; confirmation when shrinking). Changes the disk size the VM sees, running `e2fsck` and `resize2fs`. Growing is quick and keeps the file sparse; shrinking works down to what the filesystem holds and risks corruption on power loss, so back up first. Refused while the VM runs (`<ref> is in use: stop its VM first`).

### `ohmlatl image passwd <name:tag>`
Human. Changes the LUKS passphrase: current one, then the new one twice. Refused while the VM runs.

### `ohmlatl image rm <name:tag>`
Human (confirmation). Deletes the image directory and everything in it. Refused while the VM runs. Irreversible: never suggest it without the user asking.

## Runtime

### `ohmlatl run <name:tag> [--vcpus n] [--memory n]`
Human (sudo once; the passphrase on the console). The same command for the first and every later boot. `--vcpus` (1 to 32) and `--memory` (at least 512 MiB) override the manifest for this run only.

Prints `vm`, `rootfs`, `internet: on|off`, the forwards, `starting firecracker (jailer)`, then `network: ohm<N> <gw>/30, guest <ip>` once the supervisor is ready, then attaches the serial (`Ctrl-]` detaches; the VM keeps running, also after logout). Without a terminal on stdin it does not attach and says to use `ohmlatl console <name>`. Refused if a VM with that name is running (`<name> is already running (ohmlatl stop <name>)`). If the supervisor fails, prints its error or the `sudo journalctl -u ohmlatl-<uid>-<name>` command to read.

The VM then waits for the passphrase on its console (`Enter passphrase for /dev/vda:`, three tries, then the initramfs rescue shell). After boot the serial logs in the user automatically.

### `ohmlatl stop <vm>`
Agent. Sends Ctrl-Alt-Del; the guest's systemd shuts down cleanly and Firecracker exits; after 120 s it is killed. Prints `stopping <vm>` and `stopped`, and returns only after cleanup, so a `run` right after works. Works at the passphrase prompt too. Inside the guest, `poweroff` and `reboot` also end the VM. Error `no VM <vm> running` if it is not up.

### `ohmlatl ps`
Agent. Running VMs only:
```text
NAME          IMAGE                   PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  1234   172.31.0.2    2m
```
A VM appears once its supervisor is ready, before the passphrase is typed.

### `ohmlatl logs <vm> [-f]`
Agent. The last 1 MiB of serial output (`-f` follows; stop it yourself, it never ends). Kept in `/run/ohmlatl` on the host until the next `run` of that VM, so it also explains a VM that just died. Error `no logs for <vm>` if it never ran since the host booted.

### `ohmlatl console <vm>`
Human. Attaches the serial (auto-login shell, rescue, passphrase prompt); replays the last 64 KiB first, so an old prompt may show up again. One client at a time (`console busy: another client is attached`). Needs a terminal (`console needs a terminal`).

### `ohmlatl shell <vm> [-- cmd]`
Human the first time per boot if login is by password; afterwards either. SSH to `<user>@<guest ip>` with a TTY; with `-- cmd`, runs `cmd` with a TTY.

### `ohmlatl exec <vm> -- cmd`
Agent. Runs `cmd` in the guest over SSH without a TTY; stdout, stderr and the exit code are the command's. Example: `ohmlatl exec dev -- sh -c 'cd ~/src && make test'`.

### `ohmlatl cp <vm>:<path> <local>` / `ohmlatl cp <local> <vm>:<path>`
Agent. `scp -r` over the same connection: directories are recursive. Exactly one side is `<vm>:<path>`. Needs `sftp-server` in the image.

### SSH details (shell, exec, cp)
- One master connection per VM and boot (`ControlPersist`), opened by the first of these commands, reused by all others until `stop`. Only the opening asks for a password.
- Authentication is whatever the image has: the user's password, or the host user's `~/.ssh/id_*` keys / ssh-agent when the public key is in the guest's `authorized_keys`.
- Uses `-F /dev/null` (ignores `~/.ssh/config`), no agent or X11 forwarding, no host key checking and no `known_hosts`.
- If the master cannot open: `ssh login to <user>@<ip> failed`. Without a terminal, password login is impossible: ask the human to run `ohmlatl shell <vm>` once, or set up a key ([images.md](images.md)).
