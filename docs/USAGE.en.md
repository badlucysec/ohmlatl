# ohmlatl: usage

🇺🇸 English · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 [Español](USAGE.es.md)

This document proposes the `ohmlatl` usage flows before implementation. It is the experience contract: commands, prompts, and expected results. Implementation may adjust details, but it should not change the story without updating this file.

## Concepts

- **ohmlatl directory**: a directory on your USB stick or external drive, created by `ohmlatl init`. It holds the `ohmlatl` binary and `images/`. Nothing else is persistent, on the drive or on the host.
- **Disk image**: `images/<name>/<tag>/`, with a `rootfs.luks` (LUKS2 + ext4) and the `manifest.yaml`. It holds the whole computer, including `/home/<user>`, and the VM writes to it. It is built from a `Containerfile` or an OCI image.
- **VM**: a run of an image, with resources and network. At most one VM per image name runs at a time, and it has that name.
- **Sparse file**: the `rootfs.luks` has a disk size, what the VM sees, and an on-disk size, what is really stored on the drive. Parts never written are holes that take no space. A 32 GiB disk with 3 GiB written takes about 3 GiB. `image ls` shows both; `image resize` changes the first.
- **Manifest**: `ohmlatl.yaml`. It declares the image name, the user, resources, and network.
- **Forward**: a TCP port on the VM gateway (`ohmlatl-gateway`) that leads to a host service or to another VM. Besides the internet, when allowed, it is the only way out of the VM.
- **Supervisor**: a per-VM system unit that owns Firecracker, the serial, the network, and the forwards. It is not a global daemon.
- **Jailer**: Firecracker's sandbox (chroot, namespaces, seccomp, cgroups).

## Installation (early phase)

While there is no release:

```bash
cargo xtask assets          # kernel + initramfs (Buildroot) + Podman bundle; the first run takes 40 to 60 minutes
cargo build --release
./target/release/ohmlatl --version
```

Expected result:

```text
ohmlatl 0.1.0
```

## 1. Prepare the drive

Format the drive as ext4 (btrfs and xfs also work; FAT and exFAT do not), mount it, and run:

```bash
./target/release/ohmlatl init /media/$USER/drive/ohmlatl
export PATH="/media/$USER/drive/ohmlatl:$PATH"
```

Expected result:

```text
[ohmlatl] directory: /media/dev/drive/ohmlatl
[ohmlatl] filesystem: ext4 (ok)
[ohmlatl] mount: exec allowed (ok)
[ohmlatl] owner: uid 1000 (ok)
[ohmlatl] copied ohmlatl binary
```

If the drive's root is not writable by you (a freshly formatted ext4 is owned by root), `init` asks for `sudo` once to create the directory for you. The drive then looks like this:

```text
/media/dev/drive/ohmlatl/
├── .ohmlatl      # 0644
├── ohmlatl       # 0755
└── images/       # 0700, your computers
```

The `ohmlatl` on the drive finds `images/` by itself. To use an `ohmlatl` installed on the host instead, pass `--home /media/$USER/drive/ohmlatl` (or set `OHMLATL_HOME`).

## 2. Check the host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` checks rootless Podman (at least 65536 subuids and subgids, `newuidmap`, `/dev/fuse`), `e2fsprogs`, `cryptsetup`, `ssh-keygen`, `udevadm`, `sudo`, and free space. `--run` checks `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, the SSH client, and whether Docker, ufw, or firewalld block the VM traffic. Both check the ohmlatl directory: filesystem, mount, owner.

Expected result (abridged):

```text
DRIVE
  [ok] /media/dev/drive/ohmlatl (ext4, exec, uid 1000)

BUILD
  [ok] podman rootless
  [ok] subuid/subgid (65536 ids)
  [ok] newuidmap (setuid or file caps)
  [ok] e2fsprogs 1.47
  [ok] cryptsetup 2.7

RUN
  [ok] /dev/kvm accessible
  [ok] /dev/net/tun
  [ok] iproute2 + nftables
  [ok] systemd-run
  [ok] ssh client
  [ok] no foreign firewall drop
```

If something fails, the command prints the suggested fix and exits non-zero. On Ubuntu 23.10 and later, which restrict user namespaces through AppArmor, `doctor --build` shows a small AppArmor profile for the embedded Podman and the two commands to install it once.

## 3. First example: custom disk image

The flow most people will want: run a base container, install x, y, z, and turn the result into a microVM on the drive. The example uses Kali Rolling; change the base if you want.

### 3.1 Guest base

Plain `kalilinux/kali-rolling` does not have systemd or sshd. Build the repository base as a Podman image:

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

This step only creates the Podman image `kali-base:2026-11-02`, in the host's Podman cache. No disk, no passphrase.

### 3.2 Explore and install in the container

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # if needed
passwd dev                              # password for ohmlatl shell

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Replace `x`, `y`, and `z` with the packages you want. To log in with a key instead, put your public key in `/home/dev/.ssh/authorized_keys` (owned by `dev`, mode 0600). Never set the password with `chpasswd` in a `Containerfile`: it stays in plaintext in the image layers.

### 3.3 Generate the bootable disk image

Still in `examples/kali`, where `ohmlatl.yaml` is:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

`build` needs an `ohmlatl.yaml` in the current directory. `--from` only uses local images; to start from a remote one, run `ohmlatl podman pull docker.io/...` first. `-t` sets the name and tag of the result, and the build writes them into the image's `manifest.yaml`. A tag that already exists is refused: a build never overwrites a computer.

The build asks for the rootfs passphrase and for `sudo` once:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 32 GiB, sparse (default; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 (aes-xts-plain64)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: /media/dev/drive/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
  size:   32 GiB (2.9 GiB on disk)
```

If the user has neither a password nor an `authorized_keys`, the build warns that `ohmlatl shell` will not be able to log in; the console still works. It also warns when the image has no `sftp-server`, which `ohmlatl cp` needs.

The default size is 32 GiB or twice the content plus 1 GiB, whichever is larger; `--size 64G` changes it. The file is sparse: the drive only pays for what is written. The exported rootfs stays in `~/.cache/ohmlatl/build` on the host during the build and is deleted at the end, even on failure.

### 3.4 Pin it in a Containerfile

The commit is a filesystem snapshot. Once the tools stabilize, move the commands into a `Containerfile` for a repeatable rebuild:

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
  tag: 2026-11-10

user:
  name: dev
  uid: 1000
  gid: 1000

vm:
  vcpus: 2
  memory: 4GiB
  network:
    internet: true
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

This creates `kali-custom:2026-11-10`, a new computer next to `kali-custom:2026-11-02`. Set the password afterwards from the console: `ohmlatl console`, then `sudo passwd dev`. The build fails with a clear message if the image has no systemd, no sshd, or no `user.name` with the declared UID/GID and its home.

## 4. First boot

```bash
ohmlatl run kali-custom:2026-11-02
```

The command asks for `sudo` once, starts the supervisor for the VM `kali-custom`, creates the network, and attaches the serial.

Expected result:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2)
[ohmlatl] internet: on
[ohmlatl] forwards: none
[ohmlatl] starting firecracker (jailer)
[ohmlatl] network: ohm0 172.31.0.1/30, guest 172.31.0.2
[ohmlatl] serial attached; Ctrl-] to detach
...
Enter passphrase for /dev/vda:
```

After boot, the serial logs in automatically:

```text
dev@kali-custom:~$
```

`Ctrl-]` detaches the terminal; the VM keeps running under the supervisor, even after you log out. `--vcpus` and `--memory` override the manifest for this run only. No passphrase in argv, environment, manifest, or log. The same `run` command boots it again next time.

## 5. Access the VM

```bash
ohmlatl shell kali-custom
```

```text
dev@172.31.0.2's password:
dev@kali-custom:~$
```

The shell uses SSH over the VM network, with the login the image has: the password of `dev`, or your key if it is in `authorized_keys`. The password is asked once per boot: `exec` and `cp` reuse the same connection. `ohmlatl` ignores your `~/.ssh/config`, never forwards your SSH agent, and does not keep a `known_hosts` (the link to the guest is a local point-to-point TAP). There is no host directory inside the VM; access is always through here.

```bash
ohmlatl exec kali-custom -- uname -a
```

```text
Linux kali-custom 6.x ...
```

```bash
ohmlatl cp kali-custom:/home/dev/report.txt ./report.txt
ohmlatl cp ./input.txt kali-custom:/home/dev/input.txt
```

Serial console (rescue or reattach; it logs in automatically and shows the latest output first):

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

`logs` shows the last 1 MiB of serial output. The log lives in `/run/ohmlatl` on the host, stays until the next `run`, and disappears when the host reboots.

## 6. Network

By default the VM reaches the internet and nothing else: not the host, not the LAN, not other VMs. Nothing from the LAN reaches the VM. The host reaches the VM at its IP; `ohmlatl shell`, `exec`, and `cp` do that for you.

To cut the internet and open only specific services, edit `vm.network` in the image's `manifest.yaml` on the drive (or in `ohmlatl.yaml` before the build). Example: an agent that may only use the Ollama on the host and an MCP server in another VM.

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434
      - port: 8080
        to: vm/mcp:8080
```

```bash
ohmlatl run agent:2026-11-02
```

```text
[ohmlatl] internet: off
[ohmlatl] forwards:
  ohmlatl-gateway:11434 -> host 127.0.0.1:11434
  ohmlatl-gateway:8080  -> vm mcp:8080
[ohmlatl] starting firecracker (jailer)
[ohmlatl] network: ohm1 172.31.0.5/30, guest 172.31.0.6
```

Inside the VM the gateway is the only address that answers:

```bash
curl http://ohmlatl-gateway:11434/api/tags
```

- `ohmlatl-gateway` is written into the guest's `/etc/hosts` on every boot. Use the name, not the IP: the subnet can change between runs.
- `to: host:<port>` reaches `127.0.0.1:<port>` on the host, so Ollama can keep listening only on loopback. The forward does not take the port on the host.
- `to: vm/<name>:<port>` reaches another VM of yours. If it is stopped, the connection closes.
- With `internet: true` the VM resolves names through `ohmlatl-gateway`, which relays to the host's resolver. With `internet: false` lookups fail immediately.
- Only TCP. `run` always prints the open forwards. Changes take effect on the next `run`; no rebuild.

## 7. Status and lifecycle

```bash
ohmlatl ps
```

```text
NAME          IMAGE                   PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  1234   172.31.0.2    2m
```

`ps` shows only running VMs. What exists on the drive is in `ohmlatl image ls`:

```text
NAME          TAG          SIZE     ON DISK
kali-custom   2026-11-02   32 GiB   2.9 GiB
kali-custom   2026-11-10   32 GiB   3.1 GiB
```

```bash
ohmlatl stop kali-custom
```

```text
[ohmlatl] stopping kali-custom
[ohmlatl] stopped
```

`stop` sends Ctrl-Alt-Del to the guest; systemd shuts down cleanly and Firecracker exits. If it does not exit in 120 s, `ohmlatl` kills it. Inside the VM, `poweroff` and `reboot` also end the VM. When the host shuts down, running VMs get the same clean stop.

Boot it again with the same command:

```bash
ohmlatl run kali-custom:2026-11-02
```

To change the disk size the VM sees, with the VM stopped:

```bash
ohmlatl image resize kali-custom:2026-11-02 64G
```

```text
Rootfs passphrase:
[ohmlatl] kali-custom:2026-11-02: 32 GiB -> 64 GiB (2.9 GiB on disk)
[ohmlatl] e2fsck + resize2fs
[ohmlatl] done
```

Growing is quick and keeps the file sparse: the new space only takes room on the drive when the VM writes to it. Shrinking also works, down to what the filesystem holds; it asks for confirmation, and a backup first is wise, because a power cut in the middle can damage the filesystem. `resize` asks for `sudo` and the passphrase, and refuses while the VM is running.

Removing an image asks for confirmation and is refused while its VM is running:

```bash
ohmlatl image rm kali-custom:2026-11-02
```

To change the passphrase, with the VM stopped:

```bash
ohmlatl image passwd kali-custom:2026-11-02
```

It asks for the current passphrase and the new one twice, and refuses while the VM is running.

## 8. Update: inside or rebuild

The rootfs is your computer and keeps everything you do in it. Update it from the inside:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

A rebuild creates a **new, empty computer** with a new tag. To move your data to it:

```bash
ohmlatl cp kali-custom:/home/dev/work ./work      # old VM running
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-11-10
ohmlatl cp ./work kali-custom:/home/dev/work
```

The data passes through the host disk, which you trust. The old image remains until `ohmlatl image rm kali-custom:2026-11-02`.

## 9. On another machine

Stop the VMs, unmount the drive, plug it into the other machine, and run:

```bash
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --run
ohmlatl run kali-custom:2026-11-02
```

- The host must be Linux x86_64 (kernel 5.10 or newer) with KVM, you need `sudo` there, and you must trust it, even if it is borrowed: it sees the passphrase you type and the VM's memory.
- The binary on the drive is only as trustworthy as the drive. On a new host, check it against the release before running it, or use an `ohmlatl` installed on the host with `--home`:

  ```bash
  # SHA256SUMS and SHA256SUMS.sig from the release page, allowed_signers from docs/ in the repository
  ssh-keygen -Y verify -f allowed_signers -I badlucysec -n file -s SHA256SUMS.sig < SHA256SUMS
  grep ohmlatl-0.1.0-x86_64 SHA256SUMS
  sha256sum /media/$USER/drive/ohmlatl/ohmlatl
  ```
- If your UID on that host differs from the owner of the ohmlatl directory, `ohmlatl` stops and shows the `sudo chown -R` to run.
- The network settings travel in each image's `manifest.yaml` and are applied as they are; `run` prints the forwards so you can check them.
- Nothing persistent stays on the host. The Podman cache stays on the host where you built: to rebuild elsewhere, the whole build chain (`kali-base`, then `kali-custom`) runs again there.

Backup of a computer, with the VM stopped, keeping the file sparse:

```bash
rsync -aS /media/$USER/drive/ohmlatl/images/kali-custom/ /backup/ohmlatl/images/kali-custom/
```

The backup target must also be ext4, btrfs, or xfs.

## 10. Multiple VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Each image is its own VM with its own `/30`, allocated automatically. VMs do not see each other; only a `vm/<name>` forward connects them. Keep untrusted work in its own image, never in the image of your personal computer.

## 11. Failures and recovery

**Wrong rootfs passphrase.** `cryptsetup` retries; after three failures it drops to the initramfs rescue shell:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` to attach.

**Drive removed while a VM was running.** The VM gets I/O errors and its disk may be damaged. Plug the drive back in (`ohmlatl stop` works even before that, and even if the drive comes back at another path), then `ohmlatl run`: the initramfs runs `e2fsck -p` before mounting. Always `stop` before unplugging.

**Tampered rootfs.** Without the key, a change to `rootfs.luks` turns into garbage inside the VM: the boot or `e2fsck` fails, or a program crashes. Restore a backup or rebuild the image.

**Firecracker died.** `ohmlatl ps` no longer shows the VM; `ohmlatl run kali-custom:2026-11-02` boots it again.

**Host rebooted.** VMs do not start on their own; `ohmlatl image ls` and `ohmlatl run`.

**Drive full.** The sparse file cannot take more space and the VM gets I/O errors, even if the disk the VM sees still has room. Free space on the drive and run the VM again.

**Disk full inside the VM.** `ohmlatl stop`, then `ohmlatl image resize <name>:<tag> <bigger size>`, then `ohmlatl run`.

**Wrong owner or `noexec` mount.** With the wrong owner, `ohmlatl` refuses and shows the `sudo chown -R` to run. With `noexec`, the binary on the drive does not start: remount without `noexec`, or use an `ohmlatl` installed on the host with `--home`.

**No network inside the VM on a host with Docker, ufw, or firewalld.** `ohmlatl doctor --run` shows the rule that lets `ohm*` through.

**The host already uses `172.31.0.0/16`.** `run` refuses to start when a host address or route falls in the VM pool; set another one, for example `OHMLATL_POOL=10.213.0.0/16`. Do the same when a remote network in that range is reached through the default route (an AWS VPC, for example).

**`run` fails before the boot.** `run` prints the supervisor error; if there is none, it shows the `sudo journalctl -u ohmlatl-<uid>-<name>` command.

**`ohmlatl shell` asks for a password that does not exist.** The image has no password and no key for the user. Use `ohmlatl console` (automatic login), run `sudo passwd <user>`, and try again. An image without `sudo` for the user needs the password set with `ohmlatl podman run` before the build.

**Lost passphrase.** The rootfs cannot be opened. There is no LUKS key on the drive; rebuild and start over.

## 12. Non-goals

- No port publishing, ingress, or public proxy. Forwards only go out of the VM; external access is your responsibility.
- No bind mount of host directories. Files come and go with `ohmlatl cp`.
- No extra disks or volumes. The computer is the rootfs.
- No SSH key management. Login is image content.
- No GPU or display. The VM is used from the terminal.
- No kernel modules in the guest. The kernel already includes what a general-purpose system needs (nftables, tun, overlayfs, namespaces, wireguard, fuse).
- No macOS/Windows. Firecracker requires Linux with KVM.
- No management of tools, processes, plugins, or task flows. That is image content.

## 13. UX contract

- Passphrases are typed on the host TTY at `build` and on the VM TTY at boot. Never in a flag, env, manifest, file, or log.
- Destructive commands ask for confirmation; `build` never overwrites a tag.
- Errors say what failed and what to do; exit codes are stable.
- No global daemon. The supervisor is per VM and exits with it.
- Nothing persistent outside the drive; nothing on the drive besides the binary and the images.
- Every network opening is declared in the manifest and printed by `run`.
