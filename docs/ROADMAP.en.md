# ROADMAP

🇺🇸 English · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 [Español](ROADMAP.es.md)

Status: pre-implementation. The repository is still the Rust scaffold; this document is the plan that guides the next phases. Technical details are in `TECHNICAL.md` (pt-BR).

## Vision

`ohmlatl` manages bootable disk images and Firecracker microVM runs. With Podman at build time, it turns a `Containerfile` or an OCI image into a `rootfs.luks` (LUKS2 + integrity + ext4), and at runtime it boots that rootfs with Firecracker, using the kernel and initramfs embedded in the binary.

The host is thin: binary, `/dev/kvm`, network, and the drive. The real computer (rootfs, volumes, tools, and data) lives on the external drive. The runtime does not use Podman, has no daemon, keeps no persistent state on the host, and does not bind mount host directories.

The documentation example is a Kali Rolling image. `ohmlatl` does not manage image contents: tools, IDEs, data, and services are the operator's responsibility.

## Scope

- Build a rootfs from a `Containerfile` or an OCI image, with rootless Podman.
- `rootfs.luks` in LUKS2 with `hmac-sha256` integrity, unlocked in the initramfs with a passphrase typed on the VM console. `/home/<user>` lives in the rootfs.
- Generic volumes, ext4 with or without LUKS2, mounted where the manifest says.
- Kernel and initramfs built in the repository and embedded in `ohmlatl`.
- Firecracker with jailer, TAP/NAT.
- Per-VM supervisor with a PTY serial, `console`, and `logs`.
- `shell`, `exec`, and `cp` over SSH.
- Small CLI, no global daemon, all persistent state on the drive.

## Non-goals

- Publishing ports, ingress, reverse proxy, or public tunnel. The host reaches the guest IP; external access is the operator's problem.
- Bind mount of host directories.
- GPU, PCI/USB passthrough.
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
| Runtime | per-user supervisor; `sudo` for TAP, nft, and jailer |
| Init | systemd |
| Rootfs | LUKS2 + integrity + ext4, `--size` or calculated |
| Home | inside the rootfs; rebuilt with the image |
| Volumes | ext4, optional LUKS2, mountpoint in the manifest |
| Passphrase | on the VM `ttyS0`; the rootfs passphrase is tried on volumes first |
| Keys | no LUKS key on the drive |
| Access | `shell`/`exec`/`cp` over SSH |
| Network | TAP + `/30` + NAT; no publish/ingress |
| Supervisor | one per VM; no global daemon |
| Update | rebuild with a new tag; volumes untouched |

## Architecture

```text
HOST
├── ohmlatl (trusted CLI; embeds Firecracker, jailer, Podman, vmlinux, initramfs)
├── $XDG_RUNTIME_DIR/ohmlatl/{bin,jail} (tmpfs)
├── VM supervisor (one per VM)
│   ├── Firecracker (via jailer)
│   ├── serial PTY
│   └── console output log
└── TAP + nftables

DRIVE
├── bin/ohmlatl
├── images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volume>.{luks,ext4}
└── vms/<name>/{state.json,known_hosts,serial.log,console.sock,lock}

GUEST
├── initramfs: passphrase prompt, fsck, mount, switch_root
├── systemd
├── sshd
└── image content
```

## Boot flow

```text
ohmlatl run <name>:<tag>
  → validate manifest, take VM and volume locks
  → extract embedded binaries to tmpfs
  → allocate /30, create TAP + nft rules (sudo)
  → start supervisor + PTY
  → start Firecracker via jailer (sudo)
  → kernel + initramfs
      → rootfs passphrase on ttyS0
      → try it on each LUKS volume, ask again if it fails
      → e2fsck -p
      → mount rootfs at /newroot, volumes at /newroot/<mount>
      → switch_root
  → systemd → sshd
  → supervisor keeps serial and process
```

The host passes everything the initramfs needs on the kernel cmdline: `ip=` for the network, `ohmlatl.user=<uid>:<gid>`, and `ohmlatl.disks=<dev>:<luks|ext4>:<mount>,...`. The passphrase never enters the manifest, cmdline, environment, or logs.

## Disk image pipeline

```text
Containerfile or OCI ref
  → podman build / pull + podman create
  → generate SSH client key
  → size: --size or ceil(du -sb * 1.25) + 512 MiB
  → luksFormat --integrity hmac-sha256 (sudo)
  → podman export + tar inside podman unshare
  → clean machine-id, SSH host keys, hostname; install authorized_keys
  → mkfs.ext4 -d with the mapper's usable size
  → dd into the mapper, close
  → images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
```

Rules:

- The whole export/mkfs pipeline runs inside `podman unshare`. Outside it, the rootfs gets the operator's UID and breaks `sudo`, `sshd`, setuid, and capabilities.
- `podman export`, not `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- No secrets in `ARG`/`ENV`/`RUN`; use `podman build --secret`.
- CPU baseline; no `-march=native`.
- The build fails if the image has no systemd, no sshd, or no declared user.

## Volumes

- `volume create <name> --size <n> [--luks]`: `volumes/<name>.luks` or `volumes/<name>.ext4`.
- LUKS volumes have no integrity, because of the write cost.
- On first mount, an empty volume root gets `chown` to the manifest user.
- A volume can only be used by one owner at a time: VM or host mount. Per-file lock.
- `volume mount` on the host is always read-only and only with the VM stopped.

## Network

- One TAP per VM, `/30` from `172.31.0.0/16`, host `.1`, guest `.2`, allocated by `run`.
- NAT via nftables in its own table (`inet ohmlatl`), cleaned up on `stop`.
- Guest IP and DNS through `ip=` on the kernel cmdline; DNS comes from the host resolver.
- No `--publish`, ingress, or proxy. The host reaches `172.31.0.2:<port>`.
- IPv6 disabled. No egress filter, documented as a limitation.

## Lifecycle and supervisor

- `run` creates the VM, starts the supervisor, and attaches the serial. On a stopped VM it switches the image.
- The supervisor keeps the PTY, Firecracker, and output log; it survives CLI detach.
- `Ctrl-]` detaches; `console` reattaches; `shell` uses SSH.
- `stop` sends Ctrl-Alt-Del through the Firecracker API; with `reboot=k`, the guest's clean reboot ends Firecracker. Kill after 30 s. The supervisor cleans TAP/nft and releases locks.
- The SSH key is generated at build, next to the image. The VM `known_hosts` resets when the image changes.
- `start` reuses the last image and asks for passphrases again.
- `rm` removes the VM state; images and volumes remain.

## CLI

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm

ohmlatl volume create <name> --size <n> [--luks]
ohmlatl volume ls|inspect|mount|umount|rm

ohmlatl run <name:tag> [--vcpus n] [--memory n] [--subnet cidr]
ohmlatl start|stop|rm <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>
```

No `expose`, `ingress`, `publish`, or `host-forward`.

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
  disks:
    - volume: work
      mount: /home/dev/work
```

`ohmlatl build` copies the manifest to `images/<name>/<tag>/manifest.yaml`. The manifest is plaintext and is validated before it becomes the kernel cmdline. `name` + `build.tag` form the image reference; without a tag, `latest`; `-t` overrides. `build.containerfile` is optional with `--from`.

## Drive layout

```text
/mnt/vmdisk/
├── bin/ohmlatl
├── images/<name>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volume>.{luks,ext4}
└── vms/<name>/{state.json,known_hosts,serial.log,console.sock,lock}
```

The drive must be mounted without `noexec`. The Podman build cache stays on the host under `$XDG_CACHE_HOME/ohmlatl/podman`; it is a cache, not state.

## Security

| Threat | Protection |
|---|---|
| Drive theft | Rootfs and `--luks` volumes in LUKS2; no LUKS key on the drive |
| Tampered rootfs | Per-sector `hmac-sha256` integrity; an altered sector fails to read |
| Tampered kernel/initramfs | Not on the drive; embedded in `ohmlatl` |
| Tampered `ohmlatl` on the drive | Operator verifies the binary against release SHA-256 or signature, outside the drive |
| Tampered manifest | Strict validation before building the cmdline |
| Another host user | Files 0600; directories 0700 |
| Workload escaping to the host | KVM + own kernel + seccomp + jailer |
| Passphrase in logs | Supervisor records only console output; echo disabled |

Limitations: volumes without `--luks` are plaintext; LUKS volumes have no integrity; no protection against rolling back a whole old `rootfs.luks`; the SSH private key is plaintext on the drive (0600); no protection against a compromised host.

## Portability

- Linux x86_64 with KVM in the MVP.
- Drive in ext4/btrfs/xfs; no FAT/exFAT/NTFS.
- Kernel and binaries with a CPU baseline; no `-march=native`.
- Always cold boot; no snapshots.

## Phases

### P0: Host sanity and boot

- `ohmlatl init` and `ohmlatl doctor`.
- Repository scripts that build the kernel and the initramfs.
- Embedding and extraction of binaries.
- Boot a plain test rootfs with Firecracker, without jailer.

**Done when:** the test VM boots with the embedded kernel and initramfs, gives a console, has network through `ip=`, and Ctrl-Alt-Del ends Firecracker.

### P1: Disk image pipeline

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` with the embedded Podman.
- `podman unshare` with export/tar/xattrs.
- `rootfs.luks` with integrity; SSH key; manifest copy.
- Clean `machine-id`, SSH host keys, and hostname.
- `image ls/inspect/rm`.
- `examples/kali` as the first example.

**Done when:** `sudo -n true`, `getcap /bin/ping`, and SSH work in the guest; the rootfs unlocks with a passphrase on the console; `e2fsck` is clean; the customized Kali image from `podman commit` boots; an image without systemd or sshd fails with a clear message; a one-byte change in `rootfs.luks` produces a read error.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Per-VM supervisor, PTY, output log, detach/attach.
- Jailer, TAP/NAT `/30`.
- `volume create/ls/rm`; volumes in the initramfs with the "same passphrase, else ask" rule.

**Done when:** the passphrase prompt appears on the VM serial; volumes mount where the manifest says, owned by the user; `shell` enters over SSH; jailer is active; `stop` shuts down cleanly; `kill -9` on Firecracker followed by `start` passes `e2fsck`.

### P3: Volumes on the host and multiple VMs

- `volume inspect/mount/umount`, read-only, with the VM stopped.
- Locks between VM and host mount.
- Automatic subnets; two VMs at the same time.

**Done when:** a volume survives a rootfs rebuild; it mounts read-only on the host; a mount with the VM running is refused; two VMs run at the same time without collision.

### Future

Not planned in detail: atomic update with rollback, backup/restore, `image import/export`, `volume resize`, `gc`, `--json`, completions, aarch64, integrity on volumes, rollback protection.

## Tests

**Unit**

- Manifest parser and validation (names, mounts, UID/GID).
- Cmdline generation.
- Size calculation.
- Subnet allocation.
- VM state and volume locks.

**Integration**

- Podman build → ext4 with ownership, xattrs, and capabilities.
- `--from` with a customized Kali image.
- Clear failure for an image without systemd or sshd.
- `rootfs.luks` with integrity; a one-byte change fails the read.
- Initramfs with `cryptsetup` on `ttyS0`; same passphrase on volumes; retry.
- Wrong rootfs passphrase drops to rescue.
- Jailer + TAP + serial PTY.
- Ctrl-Alt-Del ends Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` over SSH.

**E2E**

- First example: Kali base, `podman run`, install x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → build → volume create → run → shell → stop → start`.
- Rootfs rebuild with volumes preserved.
- Read-only volume mount on the host.

**Matrix**

- Intel and AMD; 8/16/32 GiB RAM.
- ext4 and btrfs; USB and NVMe drives.
- Weakest CPU in the fleet (CPU baseline).

## Risks

| Risk | Mitigation |
|---|---|
| KVM unavailable/nested | `doctor` fails early with instructions |
| Jailer + TAP + PTY through `sudo` | First P2 task; `--no-jailer` only for debugging |
| Ctrl-Alt-Del does not end Firecracker | Validate in P0; kill after timeout |
| `cryptsetup` prompt in initramfs | `cttyhack`/dup2 of `ttyS0`; echo and retry tests |
| Wrong ownership in build | Entire pipeline in `podman unshare`; acceptance tests |
| Old `mkfs.ext4 -d` | `doctor` requires e2fsprogs >= 1.43 |
| Host without integrity support | `doctor --build` checks cryptsetup and dm-integrity |
| Integrity write cost on the rootfs | Measure in P1; heavy writes go to volumes |
| Guest clock starts wrong (no RTC) | `broken_system_clock` in the initramfs `e2fsck.conf`; chrony in the guest |
| Rootfs full | Default size with 25% + 512 MiB; `--size` |
| Abrupt drive removal | Mandatory `stop`; `e2fsck` on boot |
| Plain Kali without systemd/sshd | `build` validates and points to `examples/kali` |
| Opaque container commit | Documented as exploration; Containerfile is the durable source |
| No egress filter | Documented limitation |
| Large binary (~100 MB with Podman) | Accepted; extracted once per boot of the host |

## Items to confirm during implementation

- Guest kernel version and config.
- Exact `cryptsetup` options for integrity (sector size, journal).
- Jailer details for TAP and serial.
- Project license and third-party notices.

## MVP criterion

The MVP is P0 to P2: a Linux host with KVM, a prepared drive, an image built with Podman, a boot with the passphrase on the VM console, volumes mounted, shell over SSH, jailer active, and a clean shutdown. P3 completes the drive's day-to-day use.
