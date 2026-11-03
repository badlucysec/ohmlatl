# ohmlatl: usage

🇺🇸 English · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 [Español](USAGE.es.md)

This document proposes the `ohmlatl` usage flows before implementation. It is the experience contract: commands, prompts, and expected results. Implementation may adjust details, but it should not change the story without updating this file.

## Concepts

- **Disk image**: a `rootfs.luks` (LUKS2 + integrity + ext4) plus the `manifest.yaml` and SSH key used to build it. It holds the whole system, including `/home/<user>`. It is built from a `Containerfile` or an OCI image.
- **Volume**: an extra ext4 disk, with or without LUKS2. The manifest says where the VM mounts it. Anything that must survive a rebuild lives in a volume.
- **VM**: a run of an image, with volumes, resources, and network. There is one VM per image, and it has the image name.
- **Manifest**: `ohmlatl.yaml`. It declares the image name, the user, resources, and volumes.
- **Supervisor**: a per-VM process that owns Firecracker, the serial PTY, and the log. It is not a global daemon.
- **Jailer**: Firecracker's sandbox (chroot, namespaces, seccomp, cgroups). It requires root at start.

## Installation (early phase)

While there is no release:

```bash
cargo build --release
install -m 0755 target/release/ohmlatl ~/.local/bin/ohmlatl
ohmlatl --version
```

Expected result:

```text
ohmlatl 0.1.0
```

## 1. Prepare the drive

```bash
ohmlatl init /mnt/vmdisk
```

The command creates the layout and validates the filesystem. The drive must be ext4, btrfs, or xfs, mounted without `noexec`; FAT/exFAT/NTFS do not support sparse disks.

Expected result:

```text
[ohmlatl] store: /mnt/vmdisk
[ohmlatl] created bin/ images/ volumes/ vms/
[ohmlatl] filesystem: ext4 (ok)
```

## 2. Check the host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` checks rootless Podman, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs`, and `cryptsetup` with integrity. `--run` checks `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `sudo`, and the SSH client.

Expected result (abridged):

```text
BUILD
  [ok] podman rootless
  [ok] subuid/subgid (65536 ids)
  [ok] newuidmap setuid
  [ok] e2fsprogs 1.47
  [ok] cryptsetup 2.7 (integrity)

RUN
  [ok] /dev/kvm accessible
  [ok] /dev/net/tun
  [ok] iproute2 + nftables
  [ok] cgroup v2
  [ok] ssh client
```

If something fails, the command prints the suggested fix and exits non-zero.

## 3. First example: custom disk image

The flow most people will want: run a base container, install x, y, z, and turn the result into a microVM. The example uses Kali Rolling; change the base if you want.

### 3.1 Guest base

Plain `kalilinux/kali-rolling` does not have systemd or sshd. Use the repository base:

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` reads the `Containerfile` declared in `build.containerfile` and uses the embedded Podman. It produces the disk image `kali-base:2026-11-02` and leaves the Podman image with the same name for the next step. `name` and `build.tag` identify the artifact; without a tag, `latest`; `-t` overrides.

### 3.2 Explore and install in the container

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # if needed

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Replace `x`, `y`, and `z` with the packages you want.

### 3.3 Generate the bootable disk image

In the project directory, with the example `ohmlatl.yaml`:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

The build exports the rootfs inside `podman unshare`, generates the SSH key, calculates the size, and asks for the rootfs passphrase:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 3.2 GiB (content + 25% + 512 MiB; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 + integrity (hmac-sha256)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: images/kali-custom/2026-11-02/rootfs.luks
  size:   3.2 GiB
```

`--size 8G` sets the size. The build may ask for `sudo` to open the LUKS device-mapper; `cryptsetup` reads the passphrase from the TTY.

### 3.4 Pin it in a Containerfile

The commit is a filesystem snapshot. Once the tools stabilize, move the commands into a `Containerfile` for a reproducible rebuild:

```text
kali-custom/
├── Containerfile
└── ohmlatl.yaml
```

`ohmlatl.yaml`:

```yaml
format: 1
name: kali-custom

build:
  containerfile: Containerfile
  tag: 2026-11-02

user:
  name: dev
  uid: 1000
  gid: 1000

vm:
  vcpus: 2
  memory: 4GiB
  disks:
    - volume: work
      mount: /home/dev/work
```

`Containerfile`:

```dockerfile
FROM kali-base:2026-11-02

RUN apt-get update && apt-get install -y --no-install-recommends x y z \
    && rm -rf /var/lib/apt/lists/*
```

Build:

```bash
ohmlatl build
```

The build fails with a clear message if the image has no systemd, no sshd, or no `user.name` with the declared UID/GID.

## 4. Create volumes

Encrypted volume:

```bash
ohmlatl volume create work --size 8G --luks
```

Expected result:

```text
Volume passphrase:
Repeat:
[ohmlatl] volume work
  file: volumes/work.luks
  fs:   ext4
  size: 8 GiB
```

`--luks` may ask for `sudo` for the device-mapper. The passphrase is read from the TTY, never from a flag or the environment. If you use the rootfs passphrase, boot asks only once.

Volume without LUKS:

```bash
ohmlatl volume create data --size 8G
```

```text
[ohmlatl] volume data
  file: volumes/data.ext4
  fs:   ext4
  size: 8 GiB
```

The volume does not need to know the UID: on first mount, the VM gives the volume root to `user.uid:gid`.

## 5. First boot

```bash
ohmlatl run kali-custom:2026-11-02
```

The command creates the VM `kali-custom`, the network, and the supervisor, and attaches the serial. Volumes and mountpoints come from the manifest.

Expected result:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2 + integrity)
[ohmlatl] disk: work (LUKS2) -> /home/dev/work
[ohmlatl] network: ohmlatl-0 172.31.0.1/30, guest 172.31.0.2
[ohmlatl] starting firecracker (jailer)
[ohmlatl] serial attached; Ctrl-] to detach
...
LUKS passphrase for rootfs:
```

The initramfs tries the same passphrase on each LUKS volume. If it fails:

```text
LUKS passphrase for work:
```

After boot:

```text
kali-custom login:
```

`Ctrl-]` detaches the terminal; the VM keeps running under the supervisor. `--vcpus` and `--memory` override the manifest. No passphrase in argv, environment, manifest, or log.

## 6. Access the VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

The shell uses SSH over the VM network with the key generated at build. There is no host directory inside the VM; access is always through here.

```bash
ohmlatl exec kali-custom -- uname -a
```

```text
Linux kali-custom 6.x ...
```

```bash
ohmlatl cp kali-custom:/home/dev/work/report.txt ./report.txt
ohmlatl cp ./input.txt kali-custom:/home/dev/work/input.txt
```

Serial console (rescue or reattach):

```bash
ohmlatl console kali-custom
```

If another console is attached:

```text
[ohmlatl] console busy: another client is attached
```

Logs:

```bash
ohmlatl logs kali-custom -f
```

`logs` shows the serial output.

## 7. Status and lifecycle

```bash
ohmlatl ps
```

```text
NAME          IMAGE                   STATUS    PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  running   1234   172.31.0.2    2m
```

```bash
ohmlatl stop kali-custom
```

```text
[ohmlatl] stopping kali-custom
[ohmlatl] stopped
```

`stop` sends Ctrl-Alt-Del to the guest; systemd shuts down cleanly and Firecracker exits. If it does not exit in 30 s, `ohmlatl` kills it.

```bash
ohmlatl start kali-custom
```

`start` uses the last image and asks for the passphrases again on the serial.

```bash
ohmlatl rm kali-custom
```

Removes the VM state. Images and volumes remain; use `ohmlatl volume rm` for volumes.

## 8. Read a volume on the host

With the VM stopped:

```bash
ohmlatl volume mount work /mnt/work
```

```text
Volume passphrase:
[ohmlatl] mounted volumes/work.luks at /mnt/work (read-only)
```

```bash
ohmlatl volume umount work
```

The host mount is always read-only and refused while the VM is running. While mounted, `run` and `start` refuse that volume.

If a local service on the host must be reached from the VM, it has to listen on the VM gateway, for example `172.31.0.1:8080`. `ohmlatl` does not publish ports.

## 9. Update the rootfs

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

`run` on a stopped VM switches it to the new image. Volumes are not touched; `/home/dev` restarts from the image skeleton, so keep what matters in volumes. The new image has new SSH host keys, and `ohmlatl` resets the VM `known_hosts`. If the VM is running, `run` fails.

## 10. Multiple VMs

```bash
ohmlatl run kali-lab:2026-11-02
ohmlatl ps
```

Each image is its own VM with its own `/30`, allocated automatically (`--subnet` overrides). A volume can only be attached to one VM at a time; the second attempt fails with a lock.

## 11. Failures and recovery

**Wrong rootfs passphrase.** `cryptsetup` retries; after three failures it drops to the initramfs rescue shell:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` to attach.

**Wrong volume passphrase.** Three tries, then rescue shell.

**Tampered rootfs.** The integrity check turns the altered sector into a read error; `e2fsck` or the mount fails and boot drops to rescue. Rebuild the image.

**Firecracker died.** `ohmlatl ps` shows `stopped`; `ohmlatl start kali-custom` boots it again. The initramfs runs `e2fsck -p` before mounting.

**Host rebooted.** VMs do not start on their own; `ohmlatl ps` and `ohmlatl start`.

**Drive removed while the VM was running.** Do not do that. If the drive disappears, the next boot runs `e2fsck`.

**Lost passphrase.** The rootfs can be rebuilt. A LUKS volume is lost. There is no LUKS key on the drive.

## 12. Non-goals

- No port publishing, ingress, or public proxy. The host reaches the guest; external access is your responsibility.
- No bind mount of host directories. The VM only sees disks managed by `ohmlatl`.
- No GPU. The VM runs on CPU only.
- No macOS/Windows. Firecracker requires Linux with KVM.
- No management of tools, processes, plugins, or task flows. That is image content.

## 13. UX contract

- Passphrases are typed on the VM TTY at boot and on the host TTY at creation (`build`/`volume create`/`volume mount`). Never in a flag, env, manifest, file, or log.
- Destructive commands ask for confirmation.
- Errors say what failed and what to do; exit codes are stable.
- No global daemon. The supervisor is per VM and exits with it.
- All persistent state lives on the drive.
