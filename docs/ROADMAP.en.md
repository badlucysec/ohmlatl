# ROADMAP

🇺🇸 English · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 [Español](ROADMAP.es.md)

Status: pre-implementation. The repository is still the Rust scaffold; this document is the plan that guides the next phases. Technical details are in `TECHNICAL.md` (pt-BR).

## Vision

`ohmlatl` manages bootable disk images and Firecracker microVM runs. With Podman at build time, it turns a `Containerfile` or an OCI image into a `rootfs.luks` (LUKS2 + ext4, sparse), and at runtime it boots that rootfs with Firecracker, using the kernel and initramfs embedded in the binary.

The computer lives on a USB stick or external drive: a directory with the `ohmlatl` binary and the images, each one a `rootfs.luks` with the system, the tools, and the data, plus its manifest. Plug the drive into another trusted machine and boot it there. The same isolation serves to run untrusted tools, projects, and agents in their own images, with the guest closed to the host, the LAN, and other VMs, and openings declared in the manifest. The runtime does not use Podman, has no global daemon, leaves nothing persistent on the host, and does not bind mount host directories.

The documentation example is a Kali Rolling image. `ohmlatl` does not manage image contents: tools, IDEs, data, services, and SSH login are the operator's responsibility.

## Scope

- An ohmlatl directory on the drive, created by `init`, with the binary and `images/`.
- Build a rootfs from a `Containerfile` or a local OCI image, with rootless Podman, written straight to the drive.
- `rootfs.luks` in LUKS2 (`aes-xts-plain64`), unlocked in the initramfs with a passphrase typed on the VM console. `/home/<user>` lives in the rootfs. No extra disks.
- General-purpose kernel and initramfs built in the repository and embedded in `ohmlatl`.
- Firecracker with jailer, TAP per VM.
- Guest closed to host, LAN, and other VMs; optional internet with DNS through the gateway; TCP forwards to host services and other VMs.
- Per-VM supervisor as a system unit, with serial, `console`, and `logs`.
- `shell`, `exec`, and `cp` over SSH, with the login the image has.
- Small CLI, no global daemon, no VM state on the drive.
- Release: a static binary (musl), `SHA256SUMS` signed with the maintainer's SSH key (`ssh-keygen -Y`), the exact source of the embedded GPL components, and the third-party notices, also available through `ohmlatl licenses`.

## Non-goals

- Publishing ports, ingress, reverse proxy, or public tunnel. Forwards only go out of the VM; external access is the operator's problem.
- Bind mount of host directories.
- Extra disks or volumes.
- SSH key management.
- GPU, display, PCI/USB passthrough.
- Kernel modules in the guest.
- Native macOS/Windows. Firecracker requires Linux with KVM.
- Guests without systemd.
- Managing tools, processes, plugins, or task flows.
- VM snapshots. They leak plaintext RAM and are tied to CPU/version.
- Protection against a compromised host.

## Architecture decisions

| Topic | Decision |
|---|---|
| VMM | Firecracker with jailer |
| Binaries | Firecracker, jailer, Podman, kernel, and initramfs embedded in `ohmlatl` |
| Drive | ext4/btrfs/xfs; ohmlatl directory with the binary and `images/`; found from the binary's location, `--home`, or `OHMLATL_HOME` |
| Host | Linux x86_64, kernel >= 5.10, with `/dev/kvm` and `sudo`, trusted even when borrowed; nothing persistent left on it |
| Build | Embedded rootless Podman, `podman unshare` required, `mkfs` straight onto the LUKS mapper |
| Runtime | one `sudo` per `run`; supervisor as the system unit `ohmlatl-<uid>-<name>`; jailer drops to the operator's UID |
| Init | systemd |
| Rootfs | LUKS2 `aes-xts-plain64` + ext4, sparse, 32 GiB default, `image resize` to change; no integrity ("let it crash") |
| Data | inside the rootfs; updated from the inside; rebuild creates a new computer with a new tag |
| Passphrase | host TTY at build, VM `ttyS0` at boot; fixed PBKDF |
| Keys | no LUKS key and no SSH key on the drive |
| Access | `shell`/`exec`/`cp` over SSH with the image's password or key, one connection per boot; console with autologin |
| Network | TAP + `/30` per run; closed by default; optional NAT; DNS relay; TCP forwards via `ohmlatl-gateway` |
| Supervisor | one per VM; survives logout; no global daemon |
| VM state | none; `run <name>:<tag>` is always explicit |

## Architecture

```text
DRIVE
└── ohmlatl/
    ├── ohmlatl (trusted CLI; embeds Firecracker, jailer, Podman, vmlinux, initramfs)
    └── images/<name>/<tag>/{rootfs.luks,manifest.yaml}

HOST (nothing persistent)
├── /run/ohmlatl/ (own tmpfs: bin, jail, net records, <uid>/<name>/ sockets and log)
├── supervisor per VM (system unit ohmlatl-<uid>-<name>)
│   ├── Firecracker (via jailer, in the unit's cgroup)
│   ├── serial PTY (raw)
│   └── TCP forwards + DNS relay
├── TAP + nftables (table inet ohmlatl)
└── ~/.cache/ohmlatl (Podman cache, only where you build)

GUEST
├── initramfs: passphrase prompt, fsck, mount, resolv.conf, ohmlatl-gateway, switch_root
├── systemd
├── sshd
└── image content
```

## Boot flow

```text
ohmlatl run <name>:<tag>
  → find the ohmlatl directory, check owner, validate manifest, print internet and forwards
  → sudo ohmlatl launch
      → mount /run/ohmlatl (tmpfs), extract binaries
      → systemd-run: supervisor as a system unit
          → /30, TAP, nft rules, forwards, DNS relay
          → raw PTY + jailer → Firecracker (API: kernel, initramfs, drive, net, rng)
  → CLI prints the network and attaches to the console
  → kernel + initramfs
      → rootfs passphrase on ttyS0
      → e2fsck -p
      → mount rootfs, write resolv.conf and ohmlatl-gateway
      → switch_root
  → systemd → sshd, serial autologin
```

The kernel cmdline only carries `console`, `reboot=k`, `panic`, and `ip=` (address, gateway). The passphrase never enters the manifest, cmdline, environment, or logs.

## Disk image pipeline

```text
ohmlatl.yaml + Containerfile, or a local OCI ref
  → validate name and tag; refuse an existing tag
  → podman build / podman create
  → podman export + tar inside podman unshare, in ~/.cache/ohmlatl/build on the host
  → validate systemd, sshd, user and home; warn if the user has no password and no key
  → machine-id, hostname, resolv.conf, SSH host keys if missing, NetworkManager off on eth0,
    networkd/resolved masked, poweroff/halt → reboot, serial autologin
  → size: --size or max(32 GiB, 2 x content + 1 GiB), aligned to 1 MiB
  → passphrase once → luksFormat + open (sudo), on the drive
  → mkfs.ext4 -d onto the mapper inside podman unshare (fixed mke2fs.conf) → e2fsck → close
  → images/<name>/<tag>/{rootfs.luks,manifest.yaml}; delete temporaries
```

Rules:

- The whole export/mkfs pipeline runs inside `podman unshare`. Outside it, the rootfs gets the operator's UID and breaks `sudo`, `sshd`, setuid, and capabilities.
- `podman export`, not `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- The exported tree only exists in the build directory on the host, which is deleted at the end and on failure; the ext4 is written straight into LUKS on the drive.
- Fixed PBKDF (`argon2id`, 256 MiB, 4 iterations), so unlocking does not depend on the build machine.
- Fixed ext4 feature set, so the embedded `e2fsck` always understands it.
- Remote images need a fully qualified name and an explicit `ohmlatl podman pull`.
- No secrets in `ARG`/`ENV`/`RUN`, the user password included; use `podman build --secret`, or `passwd` inside `ohmlatl podman run`.
- CPU baseline; no `-march=native`.

## Network

- One TAP `ohm<N>` per VM, a `/30` from `172.31.0.0/16` allocated at each run (host `.1`, guest `.2`), refusing to start if the host already uses that range; `OHMLATL_POOL` changes it. The guest reaches the host side by the name `ohmlatl-gateway`, written on every boot.
- nftables in a shared table (`inet ohmlatl`), with per-VM rules and a lock between supervisors. `ip_forward` is turned on and never turned off; if it was off, forwarding between the host's other interfaces stays blocked until reboot (`doctor --run` shows how to lift it).
- Default: the guest reaches the internet through NAT and nothing on the host, the LAN, or other VMs. Nothing from the LAN reaches the guest or the gateway. Source addresses are checked. Only root and the VM's owner can connect from the host to the guest.
- `internet: false` cuts the internet and DNS.
- DNS: with `internet: true`, the supervisor relays port 53 on the gateway to the host's current resolver.
- `forward`: the gateway port is redirected to a listener of the supervisor, which connects to `127.0.0.1:<port>` on the host or to another VM. It never takes the port on the host. TCP only.
- No `--publish`, ingress, or proxy.
- IPv6 disabled on the TAP.

## Lifecycle and supervisor

- `run <name>:<tag>` asks for `sudo` once and starts the supervisor as the system unit `ohmlatl-<uid>-<name>`. It refuses if a VM with that name is already running. The same command boots the computer again later; there is no `start`.
- The supervisor keeps the PTY, Firecracker, network, and forwards; it survives CLI detach and logout. Status and stop go through its control socket; `ps` lists the running ones.
- `Ctrl-]` detaches; `console` reattaches with autologin and replays the latest output; `shell` uses SSH.
- `stop` (or host shutdown) sends Ctrl-Alt-Del through the Firecracker API; with `reboot=k`, the guest's clean reboot ends Firecracker. Kill after 120 s. Inside the guest, `poweroff` and `reboot` also end the VM. After `stop`, the drive can be unmounted.
- Idempotent cleanup when the supervisor exits, and again after it dies: TAP, nft rules, jail, sockets, the SSH master connection.
- SSH: login with the image's password or key; one master connection per boot, reused by `exec` and `cp`; `~/.ssh/config` ignored, no agent forwarding, no `known_hosts`.
- `image rm` refuses an image whose VM is running.

## CLI

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm
ohmlatl image resize <name:tag> <size>

ohmlatl run <name:tag> [--vcpus n] [--memory n]
ohmlatl stop <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>

ohmlatl licenses
```

All commands accept `--home <dir>`. No `expose`, `ingress`, or `publish`. `launch`, `supervise`, and `cleanup` are internal.

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

`ohmlatl build` requires the manifest in the current directory and copies it to `images/<name>/<tag>/manifest.yaml`, with the final name and tag. `run` reads that copy; editing `vm` there takes effect on the next `run`. `name` + `build.tag` form the image reference; without a tag, `latest`; `-t` overrides. `build.containerfile` is optional with `--from`. Minimum memory: 512 MiB. `port: 53` is invalid with `internet: true`. Unknown fields are an error.

## Layout

```text
DRIVE  <ohmlatl directory>/{.ohmlatl,ohmlatl,images/<name>/<tag>/{rootfs.luks,manifest.yaml}}
HOST   /run/ohmlatl/{bin/<sha256>,jail,net,<uid>/<name>/{ctl.sock,console.sock,ssh.sock,serial.log,ip,error}}
HOST   ~/.cache/ohmlatl/{bin,podman,build}
```

The ohmlatl directory and the binary are 0755, so the binary runs even under another UID; `images/` is 0700 and its files 0600, owned by the operator. Everything on the host under `/run` disappears at reboot. The Podman cache is a cache, not state, and holds container images in plaintext: `ohmlatl podman system reset` clears it. Rebuilding on another host runs the build chain again there.

## Security

| Threat | Protection |
|---|---|
| Stolen or lost drive | `rootfs.luks` in LUKS2; no LUKS key and no SSH key on the drive |
| Tampered `rootfs.luks` | Without the key, a change becomes garbage in the guest and breaks the boot or a program |
| Tampered kernel/initramfs | Embedded in the binary, not loose on the drive |
| Tampered `ohmlatl` binary on the drive | Check its SHA-256 against the release on a new host, or use a host `ohmlatl` with `--home` |
| Another host user | Operator-only directories and sockets; per-UID namespacing; only root and the owner connect to the guest |
| Workload escaping to the host | KVM + own kernel + seccomp + jailer |
| Workload reaching host, LAN, or other VMs | Rejected by nft by default; anti-spoofing; only declared forwards pass |
| LAN reaching the guest or the forwards | Dropped by nft |
| Workload grabbing the operator's SSH agent | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase in logs or on disk | Read from the TTY, passed through stdin, raw PTY; echo disabled at boot |

Limitations: no integrity, so tampering is not detected for sure, and sectors or the whole file can be rolled back to an old copy; the sparse file shows which blocks were written; deleting files in the guest does not shrink the file; unplugging the drive with a VM running corrupts its disk; the guest's SSH host key is not checked (local point-to-point link to a trusted host); the `manifest.yaml` is plaintext on the drive, so a tampered drive can change resources and open forwards, which `run` prints; a rebuild creates a new, empty computer; with `internet: true` the guest reaches the internet without a filter; no protection against a compromised host, borrowed or not.

## Portability

- Linux x86_64 with KVM in the MVP.
- Kernel and binaries with a CPU baseline; no `-march=native`.
- The drive moves between hosts as is; a UID mismatch is detected and the fix is printed.
- Always cold boot; no snapshots.

## Phases

### P0: Host sanity and boot

- `ohmlatl init` and `ohmlatl doctor`.
- Asset build in containers (`cargo xtask assets`): kernel 6.18 LTS and initramfs with Buildroot, plus the Podman bundle from the podman-static recipe, both with their exact sources; hand-written `mke2fs.conf`. The Podman binaries leave `vendor/`.
- Embedding and extraction of binaries.
- Boot a LUKS test rootfs from the drive with Firecracker under the jailer, from a hand-run supervisor.

**Done when:** `init` prepares an ext4 drive and refuses FAT/exFAT; the test VM boots with the embedded kernel and initramfs; the passphrase prompt works on the raw PTY and never shows up in the log; the network comes up through `ip=`; Ctrl-Alt-Del ends Firecracker; file bind mounts survive the jailer's `pivot_root`; `/dev/ptp0` exists and `chrony` syncs with it; the embedded `e2fsck` accepts an ext4 made with the embedded `mke2fs.conf`.

### P1: Disk image pipeline

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` with the embedded Podman and its own configuration.
- `podman unshare` with export/tar/xattrs; `mkfs` onto the mapper on the drive.
- SSH host keys if missing; password/key warning; manifest copy; build cleanup.
- `image ls/inspect/rm`, `image resize` (grow and shrink).
- `examples/kali` as the first example.

**Done when:** on the serial console (autologin), `sudo -n true` and `getcap /bin/ping` work and files keep their owners; the rootfs unlocks with the passphrase on the console; `e2fsck` is clean; the customized Kali image from `podman commit` boots; an image without systemd or sshd fails with a clear message; an existing tag is refused; nothing is left in `~/.cache/ohmlatl/build` or as `.partial` on the drive after success or failure; `image resize` grows and shrinks a stopped image, keeps the file sparse, and refuses while the VM runs or below the filesystem's minimum.

### P2: Runtime

- `run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- `launch`, supervisor as a system unit, control socket, raw PTY, serial log, detach/attach, cleanup.
- Jailer, TAP `/30`, closed network, `internet: true|false`, DNS relay.

**Done when:** `shell` enters with the image password, asked once per boot, and `exec`/`cp` reuse the connection; the VM survives logout; the guest cannot reach the host, the LAN, or another VM, and the LAN cannot reach the guest; DNS works on a host with `systemd-resolved` and after a network change; `stop`, `poweroff` inside the guest, and host shutdown end the VM cleanly and leave the drive ready to unmount; `kill -9` on the supervisor leaves no TAP, rule, or mount behind; the drive boots on a second host, and a UID mismatch there is reported with the fix.

### P3: Forwards and multiple VMs

- `forward` to `host:<port>` and `vm/<name>:<port>`.
- Several VMs at the same time, from one or more users; shared nft table.
- `doctor --run` with Docker, ufw, and firewalld.

**Done when:** an agent VM with `internet: false` reaches the host's Ollama (listening only on loopback) and an MCP server in another VM, and nothing else; a host service on `0.0.0.0` with the same port keeps working; two VMs run at the same time without collision; stopping one VM does not break the network of the other.

### Future

Not planned in detail: backup/restore commands, `image import/export`, UDP forwards, `gc`, `--json`, completions, aarch64.

## Tests

**Unit**

- Manifest parser and validation (names, tags, UID/GID, ports, forwards, port 53).
- ohmlatl directory discovery (`--home`, `OHMLATL_HOME`, binary location).
- Cmdline generation.
- Size calculation and alignment.
- Subnet allocation against existing routes and records.
- nft rules per VM.

**Integration**

- Podman build → ext4 with ownership, xattrs, and capabilities, on the drive.
- `--from` with a customized Kali image; refusal of remote and short names.
- Clear failure for an image without systemd or sshd; refusal of an existing tag; warning without password or key.
- Passphrase without a trailing newline opens both at build and at boot.
- `image resize` up and down; refusal while running and below the minimum; the file stays sparse.
- Initramfs with `cryptsetup` on `ttyS0`; retry; wrong passphrase drops to rescue.
- Jailer + TAP + raw PTY under `systemd-run`, with `KillMode=mixed`.
- Ctrl-Alt-Del, `poweroff`, and `systemctl stop` end Firecracker cleanly.
- `kill -9` on Firecracker + `run` + `e2fsck`; `kill -9` on the supervisor + cleanup.
- `shell`/`exec`/`cp` over SSH with `ForwardAgent=yes` in `~/.ssh/config` (must not forward); one password prompt per boot.
- Network: guest blocked from host, LAN, and other VMs; spoofed source dropped; LAN blocked from the gateway; another local user blocked from the guest; forwards and DNS work; `internet: false` cuts egress and DNS fails fast.
- Drive: wrong owner and `noexec` mount are refused with the fix.

**E2E**

- First example: `init`, Kali base, `podman run`, install x/y/z, `passwd`, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → doctor → build → run → shell → stop → run`.
- Migration to a new tag with `cp`.
- Unplug the drive after `stop`, plug it into another host, and boot it there.
- Agent VM + MCP VM + Ollama on the host.

**Matrix**

- Intel and AMD; 8/16/32 GiB RAM.
- USB stick and external SSD, ext4 and btrfs.
- Hosts with and without Docker/ufw/firewalld; with and without `systemd-resolved`.
- e2fsprogs 1.45 and the newest available on the host.
- Weakest CPU in the fleet (CPU baseline).

## Risks

| Risk | Mitigation |
|---|---|
| KVM unavailable/nested | `doctor` fails early with instructions |
| Firecracker's kernel config is made for Amazon Linux kernels | `check-kconfig.sh` and the P0 boot test |
| No `sudo` on a borrowed host | Documented requirement; `doctor --run` checks it |
| Drive unplugged with a VM running | `stop` before unmounting, documented; `e2fsck` on boot |
| Different UID on another host | Detected; `sudo chown -R` printed |
| Drive mounted `noexec` | `doctor` warns; host `ohmlatl` with `--home` |
| `cryptsetup` prompt in initramfs | P0 test on the raw PTY; echo and retry tests |
| Jailer bind mounts lost after `pivot_root` | P0 test; fallback: copy kernel and initramfs into the jail |
| Wrong ownership in build | Entire pipeline in `podman unshare`; acceptance tests |
| ext4 features unknown to the embedded `e2fsck` | Fixed `mke2fs.conf`; matrix with old and new e2fsprogs |
| Guest clock wrong without internet | `ptp_kvm` + `chrony` with `refclock PHC`; P0 test |
| Rootfs full | 32 GiB sparse default; `--size`; `image resize` |
| Drive full | Documented; `doctor` checks free space |
| Build killed halfway | `.partial` with a lock; the next build or `doctor --build` cleans it up |
| Docker/ufw/firewalld drop the traffic | `doctor --run` detects and prints the rule |
| Network managers in the guest overriding `ip=` | NetworkManager unmanaged on `eth0`; networkd and resolved masked at build |
| Image without password or key | Build warns; console with autologin to run `passwd` |
| Plain Kali without systemd/sshd | `build` validates and points to `examples/kali` |
| Opaque container commit | Documented as exploration; Containerfile is the durable source |
| Large binary (~100 MB with Podman) | Accepted; extracted once per host boot |

## Items to confirm during implementation

- Final kernel config fragment, after the P0 boot tests.

## MVP criterion

The MVP is P0 to P2: a prepared drive, a Linux host with KVM, an image built with Podman, a boot with the passphrase on the VM console, shell over SSH, jailer active, network closed by default, a clean shutdown, and the same drive booting on a second host. P3 adds forwards and several VMs working together.
