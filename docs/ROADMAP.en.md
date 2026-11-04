# ROADMAP

🇺🇸 English · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 [Español](ROADMAP.es.md)

Status: pre-implementation. The repository is still the Rust scaffold; this document is the plan that guides the next phases. Technical details are in `TECHNICAL.md` (pt-BR).

## Vision

`ohmlatl` manages bootable disk images and Firecracker microVM runs. With Podman at build time, it turns a `Containerfile` or an OCI image into a `rootfs.luks` (LUKS2 + integrity + ext4), and at runtime it boots that rootfs with Firecracker, using the kernel and initramfs embedded in the binary.

The computer is the `rootfs.luks`: one file that holds the system, the tools, and the data, and that can be copied to another host and booted there. The same isolation serves to run untrusted tools, projects, and agents, with the network closed by default and openings declared in the manifest. The runtime does not use Podman, has no global daemon, and does not bind mount host directories.

The documentation example is a Kali Rolling image. `ohmlatl` does not manage image contents: tools, IDEs, data, and services are the operator's responsibility.

## Scope

- Build a rootfs from a `Containerfile` or an OCI image, with rootless Podman.
- `rootfs.luks` in LUKS2 with `hmac-sha256` integrity, unlocked in the initramfs with a passphrase typed on the VM console. `/home/<user>` lives in the rootfs. No extra disks.
- Kernel and initramfs built in the repository and embedded in `ohmlatl`.
- Firecracker with jailer, TAP per VM.
- Network closed to host, LAN, and other VMs by default; optional internet; TCP forwards to host services and other VMs.
- Per-VM supervisor with a PTY serial, `console`, and `logs`.
- `shell`, `exec`, and `cp` over SSH.
- Small CLI, no global daemon.

## Non-goals

- Publishing ports, ingress, reverse proxy, or public tunnel. Forwards only go out of the VM; the host reaches the guest IP; external access is the operator's problem.
- Bind mount of host directories.
- Extra disks or volumes.
- GPU, display, PCI/USB passthrough.
- Native macOS/Windows. Firecracker requires Linux with KVM.
- Guests without systemd.
- Managing tools, processes, plugins, or task flows.
- VM snapshots. They leak plaintext RAM and are tied to CPU/version.

## Architecture decisions

| Topic | Decision |
|---|---|
| VMM | Firecracker with jailer |
| Binaries | Firecracker, jailer, Podman, kernel, and initramfs embedded in `ohmlatl` |
| Host | Linux x86_64 with `/dev/kvm` |
| Build | Embedded rootless Podman, `podman unshare` required |
| Runtime | supervisor as a root system unit via one `sudo systemd-run`; jailer drops to the operator's UID |
| Init | systemd |
| Rootfs | LUKS2 + integrity (with journal) + ext4, `--size` or calculated |
| Data | inside the rootfs; updated from the inside; rebuild creates a new computer |
| Passphrase | host TTY at build, VM `ttyS0` at boot; fixed PBKDF |
| Keys | no LUKS key on disk |
| Access | `shell`/`exec`/`cp` over SSH, host key from the build |
| Network | TAP + `/30`; closed by default; optional NAT to the internet; TCP forwards |
| Supervisor | one per VM; survives logout; no global daemon |
| Portability | copy the image directory to another host |

## Architecture

```text
HOST
├── ohmlatl (trusted CLI; embeds Firecracker, jailer, Podman, vmlinux, initramfs)
├── ~/.local/share/ohmlatl/
│   ├── images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
│   └── vms/<name>/state.json
├── /run/ohmlatl/ (tmpfs: bin, jail, console.sock, serial.log)
├── supervisor per VM (system unit ohmlatl-<name>)
│   ├── Firecracker (via jailer)
│   ├── serial PTY
│   └── TCP forwards
└── TAP + nftables

GUEST
├── initramfs: passphrase prompt, fsck, mount, switch_root
├── systemd
├── sshd
└── image content
```

## Boot flow

```text
ohmlatl run <name>:<tag>
  → validate manifest, allocate /30
  → sudo systemd-run: supervisor as a system unit
      → extract embedded binaries to /run/ohmlatl
      → TAP + nft chains + forwards
      → PTY + jailer → Firecracker
  → kernel + initramfs
      → rootfs passphrase on ttyS0
      → e2fsck -p
      → mount rootfs, write resolv.conf
      → switch_root
  → systemd → sshd
  → supervisor keeps serial, process, and forwards
```

The kernel cmdline only carries `console`, `reboot=k`, `panic`, `root`, and `ip=` (address, gateway, DNS). The passphrase never enters the manifest, cmdline, environment, or logs.

## Disk image pipeline

```text
Containerfile or local OCI ref
  → podman build / podman create
  → generate SSH client key and guest host key
  → podman export + tar inside podman unshare, in ~/.cache/ohmlatl/build
  → machine-id, hostname, host key, authorized_keys, NetworkManager off on eth0,
    poweroff/halt → reboot
  → validate systemd, sshd, user
  → size: --size or ceil(du -sb * 1.25) + 512 MiB
  → passphrase once → luksFormat --integrity hmac-sha256 --integrity-no-wipe (sudo)
  → mkfs.ext4 -d with the mapper's usable size → dd into the mapper, close
  → images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/}; delete temporaries
```

Rules:

- The whole export/mkfs pipeline runs inside `podman unshare`. Outside it, the rootfs gets the operator's UID and breaks `sudo`, `sshd`, setuid, and capabilities.
- `podman export`, not `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- No plaintext rootfs outside the build temporary directory, which is deleted at the end and on failure.
- Fixed PBKDF (`argon2id`, 256 MiB, 2 s), so unlocking does not depend on the build machine.
- Remote images need a fully qualified name; `--from` only uses local images.
- No secrets in `ARG`/`ENV`/`RUN`; use `podman build --secret`.
- CPU baseline; no `-march=native`.

## Network

- One TAP per VM, `/30` from `172.31.0.0/16`, host `.1`, guest `.2`, allocated by `run`, skipping networks already in the routing table.
- nftables in a shared table (`inet ohmlatl`) with per-VM chains. The table and `ip_forward` are restored when the last VM stops.
- Default: the guest reaches the internet through NAT, and nothing on the host, the LAN, or other VMs.
- `internet: false` cuts the internet.
- `forward`: the supervisor listens on `<gateway>:<port>` and connects to `127.0.0.1:<port>` on the host or to another VM. TCP only.
- The host reaches `172.31.0.2:<port>`. No `--publish`, ingress, or proxy.
- IPv6 disabled.

## Lifecycle and supervisor

- `run` asks for `sudo` once and starts the supervisor as the system unit `ohmlatl-<name>`. On a stopped VM it switches the image.
- The supervisor keeps the PTY, Firecracker, network, and forwards; it survives CLI detach and logout.
- `Ctrl-]` detaches; `console` reattaches; `shell` uses SSH.
- `stop` sends Ctrl-Alt-Del through the Firecracker API; with `reboot=k`, the guest's clean reboot ends Firecracker. Kill after 30 s. Inside the guest, `poweroff` and `reboot` also end the VM.
- The SSH client key and guest host key are generated at build, next to the image; SSH checks the host key strictly and ignores `~/.ssh/config`.
- `start` reuses the last image and asks for the passphrase again.
- `rm` removes the VM state; the image remains.

## CLI

```text
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm

ohmlatl run <name:tag> [--vcpus n] [--memory n]
ohmlatl start|stop|rm <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>
```

No `expose`, `ingress`, or `publish`.

## Manifest

```yaml
format: 1
name: kali-base

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
    forward:
      - port: 11434
        to: host:11434
      - port: 8080
        to: vm/mcp:8080
```

`ohmlatl build` copies the manifest to `images/<name>/<tag>/manifest.yaml`, with the final name and tag. `run` reads that copy; editing `vm` there takes effect on the next `start`. `name` + `build.tag` form the image reference; without a tag, `latest`; `-t` overrides. `build.containerfile` is optional with `--from`. Minimum memory: 512 MiB. Unknown fields are an error.

## Host layout

```text
~/.local/share/ohmlatl/
├── images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/{id_ed25519,known_hosts}}
└── vms/<name>/state.json

/run/ohmlatl/{bin,jail,<name>/{console.sock,api.sock,serial.log}}
~/.cache/ohmlatl/{bin,podman,build}
```

Files 0600, directories 0700. The Podman cache is a cache, not state: rebuilding on another host runs the build chain again there.

## Security

| Threat | Protection |
|---|---|
| Stolen `rootfs.luks` | LUKS2; no LUKS key on disk |
| Tampered `rootfs.luks` | Per-sector `hmac-sha256` integrity; an altered sector fails to read |
| Tampered kernel/initramfs | Not on disk; embedded in `ohmlatl` |
| Another host user | Files 0600; directories 0700; console socket owned by the operator |
| Workload escaping to the host | KVM + own kernel + seccomp + jailer |
| Workload reaching host, LAN, or other VMs | Blocked by nft by default; only declared forwards pass |
| Workload grabbing the operator's SSH agent | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase in logs or on disk | Read from the TTY, passed through stdin; echo disabled at boot |

Limitations: no protection against rolling back to an old copy of `rootfs.luks`; integrity costs write speed and the file takes its full size; the SSH private key is plaintext on the host (0600); a rebuild creates a new, empty computer; with `internet: true` the guest reaches the internet without a filter; no protection against a compromised host.

## Portability

- Linux x86_64 with KVM in the MVP.
- Kernel and binaries with a CPU baseline; no `-march=native`.
- The image directory is copied as is to another host.
- Always cold boot; no snapshots.

## Phases

### P0: Host sanity and boot

- `ohmlatl doctor`.
- Repository scripts that build the kernel and the initramfs.
- Embedding and extraction of binaries.
- Boot a plain test rootfs with Firecracker, without jailer.

**Done when:** the test VM boots with the embedded kernel and initramfs, gives a console, has network through `ip=`, and Ctrl-Alt-Del ends Firecracker.

### P1: Disk image pipeline

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` with the embedded Podman.
- `podman unshare` with export/tar/xattrs.
- `rootfs.luks` with integrity; SSH keys; manifest copy.
- `image ls/inspect/rm`.
- `examples/kali` as the first example.

**Done when:** `sudo -n true`, `getcap /bin/ping`, and SSH work in the guest; the rootfs unlocks with a passphrase on the console; `e2fsck` is clean; the customized Kali image from `podman commit` boots; an image without systemd or sshd fails with a clear message; a one-byte change in `rootfs.luks` produces a read error; nothing is left in `~/.cache/ohmlatl/build`.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Supervisor as a system unit, PTY, serial log, detach/attach.
- Jailer, TAP `/30`, closed network by default, `internet: true|false`.

**Done when:** the passphrase prompt appears on the VM serial; `shell` enters over SSH with a strict host key; jailer is active; the VM survives logout; the guest cannot reach the host, the LAN, or another VM; `stop` and `poweroff` inside the guest shut down cleanly; `kill -9` on Firecracker followed by `start` passes `e2fsck`.

### P3: Forwards and multiple VMs

- `forward` to `host:<port>` and `vm/<name>:<port>`.
- Automatic subnets; several VMs at the same time; shared nft table.
- `doctor --run` with Docker and ufw.

**Done when:** an agent VM with `internet: false` reaches the host's Ollama and an MCP server in another VM, and nothing else; two VMs run at the same time without collision; stopping one VM does not break the network of the other.

### Future

Not planned in detail: backup/restore, `image import/export`, UDP forwards, `gc`, `--json`, completions, aarch64, rollback protection.

## Tests

**Unit**

- Manifest parser and validation (names, UID/GID, ports, forwards).
- Cmdline generation.
- Size calculation.
- Subnet allocation against existing routes.
- nft rules per VM.

**Integration**

- Podman build → ext4 with ownership, xattrs, and capabilities.
- `--from` with a customized Kali image; refusal of short remote names.
- Clear failure for an image without systemd or sshd.
- `rootfs.luks` with integrity; a one-byte change fails the read.
- Initramfs with `cryptsetup` on `ttyS0`; retry; wrong passphrase drops to rescue.
- Jailer + TAP + serial PTY under `systemd-run`.
- Ctrl-Alt-Del and `poweroff` end Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` over SSH with `ForwardAgent=yes` in `~/.ssh/config` (must not forward).
- Network: guest blocked from host, LAN, and other VMs; forwards work; `internet: false` cuts egress.

**E2E**

- First example: Kali base, `podman run`, install x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `doctor → build → run → shell → stop → start`.
- Copy the image directory to another host and boot it there.
- Agent VM + MCP VM + Ollama on the host.

**Matrix**

- Intel and AMD; 8/16/32 GiB RAM.
- Hosts with and without Docker/ufw.
- Weakest CPU in the fleet (CPU baseline).

## Risks

| Risk | Mitigation |
|---|---|
| KVM unavailable/nested | `doctor` fails early with instructions |
| `cryptsetup` prompt in initramfs | `cttyhack`/dup2 of `ttyS0`; echo and retry tests |
| Wrong ownership in build | Entire pipeline in `podman unshare`; acceptance tests |
| Old `mkfs.ext4 -d` | `doctor` requires e2fsprogs >= 1.43 |
| Host without integrity support | `doctor --build` checks cryptsetup and dm-integrity |
| Integrity write cost | Accepted; documented |
| Guest clock starts wrong (no RTC) | `broken_system_clock` in the initramfs `e2fsck.conf`; chrony in the guest |
| Rootfs full | Default size with 25% + 512 MiB; `--size` |
| Docker/ufw drop forwarding | `doctor --run` detects and prints the rule |
| NetworkManager in the guest overriding `ip=` | `eth0` unmanaged, written at build |
| Plain Kali without systemd/sshd | `build` validates and points to `examples/kali` |
| Opaque container commit | Documented as exploration; Containerfile is the durable source |
| Large binary (~100 MB with Podman) | Accepted; extracted once per host boot |

## Items to confirm during implementation

- Guest kernel version and config.
- Project license and third-party notices.

## MVP criterion

The MVP is P0 to P2: a Linux host with KVM, an image built with Podman, a boot with the passphrase on the VM console, shell over SSH, jailer active, network closed by default, and a clean shutdown. P3 adds forwards and several VMs working together.
