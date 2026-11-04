# ohmlatl: usage

🇺🇸 English · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 [Español](USAGE.es.md)

This document proposes the `ohmlatl` usage flows before implementation. It is the experience contract: commands, prompts, and expected results. Implementation may adjust details, but it should not change the story without updating this file.

## Concepts

- **Disk image**: a `rootfs.luks` (LUKS2 + integrity + ext4) plus the `manifest.yaml` and SSH keys created with it. It holds the whole computer, including `/home/<user>`, and the VM writes to it. It is built from a `Containerfile` or an OCI image.
- **VM**: a run of an image, with resources and network. There is one VM per image, and it has the image name.
- **Manifest**: `ohmlatl.yaml`. It declares the image name, the user, resources, and network.
- **Forward**: a TCP port on the VM gateway that leads to a host service or to another VM. It is the only way out of the VM besides the internet, when allowed.
- **Supervisor**: a per-VM system unit that owns Firecracker, the serial PTY, the network, and the forwards. It is not a global daemon.
- **Jailer**: Firecracker's sandbox (chroot, namespaces, seccomp, cgroups).

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

Images and VM state live in `~/.local/share/ohmlatl`.

## 1. Check the host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` checks rootless Podman, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs`, and `cryptsetup` with integrity. `--run` checks `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, the SSH client, and whether Docker or ufw block forwarding.

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
  [ok] systemd-run
  [ok] ssh client
  [ok] forward policy
```

If something fails, the command prints the suggested fix and exits non-zero.

## 2. First example: custom disk image

The flow most people will want: run a base container, install x, y, z, and turn the result into a microVM. The example uses Kali Rolling; change the base if you want.

### 2.1 Guest base

Plain `kalilinux/kali-rolling` does not have systemd or sshd. Use the repository base:

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` reads the `Containerfile` declared in `build.containerfile` and uses the embedded Podman. It produces the disk image `kali-base:2026-11-02` and leaves the Podman image with the same name for the next step. `name` and `build.tag` identify the artifact; without a tag, `latest`; `-t` overrides.

### 2.2 Explore and install in the container

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

### 2.3 Generate the bootable disk image

In the project directory, with the example `ohmlatl.yaml`:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

`--from` only uses local images; remote images need a full name such as `docker.io/...`. `-t` sets the name and tag of the result, and the build writes them into the image's `manifest.yaml`. The build exports the rootfs inside `podman unshare`, generates the SSH keys, calculates the size, and asks for the rootfs passphrase:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 3.2 GiB (content + 25% + 512 MiB; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 + integrity (hmac-sha256)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: ~/.local/share/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
  size:   3.2 GiB
```

`--size 8G` sets the size. The build asks for `sudo` once to open the LUKS device-mapper. Temporary files stay in `~/.cache/ohmlatl/build` and are deleted at the end.

### 2.4 Pin it in a Containerfile

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
  tag: 2026-11-02

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

The build fails with a clear message if the image has no systemd, no sshd, or no `user.name` with the declared UID/GID.

## 3. First boot

```bash
ohmlatl run kali-custom:2026-11-02
```

The command asks for `sudo` once, starts the supervisor for the VM `kali-custom`, creates the network, and attaches the serial.

Expected result:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2 + integrity)
[ohmlatl] network: ohmlatl-0 172.31.0.1/30, guest 172.31.0.2, internet on
[ohmlatl] forwards: none
[ohmlatl] starting firecracker (jailer)
[ohmlatl] serial attached; Ctrl-] to detach
...
LUKS passphrase for rootfs:
```

After boot:

```text
kali-custom login:
```

`Ctrl-]` detaches the terminal; the VM keeps running under the supervisor, even after you log out. `--vcpus` and `--memory` override the manifest. No passphrase in argv, environment, manifest, or log.

## 4. Access the VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

The shell uses SSH over the VM network with the key and host key created at build. It ignores your `~/.ssh/config` and never forwards your SSH agent. There is no host directory inside the VM; access is always through here.

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

`logs` shows the serial output. The log lives in `/run/ohmlatl` and disappears when the host reboots.

## 5. Network

By default the VM reaches the internet and nothing else: not the host, not the LAN, not other VMs. The host reaches the VM at its IP (`172.31.0.2`).

To cut the internet and open only specific services, use `vm.network` in the image's `manifest.yaml` (or in `ohmlatl.yaml` before the build). Example: an agent that may only use the Ollama on the host and an MCP server in another VM.

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
[ohmlatl] network: ohmlatl-1 172.31.0.5/30, guest 172.31.0.6, internet off
[ohmlatl] forwards:
  172.31.0.5:11434 -> host 127.0.0.1:11434
  172.31.0.5:8080  -> vm mcp:8080
```

Inside the VM the gateway is the only address that answers:

```bash
curl http://172.31.0.5:11434/api/tags
```

- `to: host:<port>` reaches `127.0.0.1:<port>` on the host, so Ollama can keep listening only on loopback.
- `to: vm/<name>:<port>` reaches another `ohmlatl` VM. If it is stopped, the connection closes.
- Only TCP. `run` always prints the open forwards.
- Changing `vm.network` in the image's `manifest.yaml` takes effect on the next `start`; no rebuild.

## 6. Status and lifecycle

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

`stop` sends Ctrl-Alt-Del to the guest; systemd shuts down cleanly and Firecracker exits. If it does not exit in 30 s, `ohmlatl` kills it. Inside the VM, `poweroff` and `reboot` also end the VM.

```bash
ohmlatl start kali-custom
```

`start` uses the last image and asks for the passphrase again on the serial.

```bash
ohmlatl rm kali-custom
```

Removes the VM state. The image remains; use `ohmlatl image rm` for it.

## 7. Update: inside or rebuild

The rootfs is your computer and keeps everything you do in it. Update it from the inside:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

A rebuild creates a **new, empty computer** from the `Containerfile`:

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

`run` on a stopped VM switches it to the new image. Nothing from the old rootfs passes to the new one: copy out what matters with `ohmlatl cp` before. The old image remains until `ohmlatl image rm kali-custom:2026-11-02`. If the VM is running, `run` fails.

## 8. Take it to another host

The computer is the image directory:

```bash
rsync -a ~/.local/share/ohmlatl/images/kali-custom/ otherhost:.local/share/ohmlatl/images/kali-custom/
```

On the other host, with `ohmlatl` installed:

```bash
ohmlatl run kali-custom:2026-11-02
```

The `rootfs.luks` takes its full size on disk (integrity writes every sector). Stop the VM before copying. The Podman cache stays on the original host: to rebuild on the new host, the whole build chain (`kali-base`, then `kali-custom`) runs again there.

## 9. Multiple VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Each image is its own VM with its own `/30`, allocated automatically. VMs do not see each other; only a `vm/<name>` forward connects them.

## 10. Failures and recovery

**Wrong rootfs passphrase.** `cryptsetup` retries; after three failures it drops to the initramfs rescue shell:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` to attach.

**Tampered rootfs.** The integrity check turns the altered sector into a read error; `e2fsck` or the mount fails and boot drops to rescue. Restore a copy or rebuild the image.

**Firecracker died.** `ohmlatl ps` shows `stopped`; `ohmlatl start kali-custom` boots it again. The initramfs runs `e2fsck -p` before mounting.

**Host rebooted.** VMs do not start on their own; `ohmlatl ps` and `ohmlatl start`.

**No network inside the VM on a host with Docker or ufw.** `ohmlatl doctor --run` shows the rule that lets `ohmlatl-*` through.

**Lost passphrase.** The rootfs cannot be opened. There is no LUKS key on disk; rebuild and start over.

## 11. Non-goals

- No port publishing, ingress, or public proxy. Forwards only go out of the VM; the host reaches the guest IP; external access is your responsibility.
- No bind mount of host directories. Files come and go with `ohmlatl cp`.
- No extra disks or volumes. The computer is the rootfs.
- No GPU or display. The VM is used from the terminal.
- No macOS/Windows. Firecracker requires Linux with KVM.
- No management of tools, processes, plugins, or task flows. That is image content.

## 12. UX contract

- Passphrases are typed on the host TTY at `build` and on the VM TTY at boot. Never in a flag, env, manifest, file, or log.
- Destructive commands ask for confirmation.
- Errors say what failed and what to do; exit codes are stable.
- No global daemon. The supervisor is per VM and exits with it.
- Every network opening is declared in the manifest and printed by `run`.
