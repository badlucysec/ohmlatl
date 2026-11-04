<h1 align="center">ohmlatl</h1>

<p align="center">Portable, declarative, encrypted Firecracker microVMs.</p>

<p align="center">Pronunciation: <strong>ohm-lal</strong>.</p>

<p align="center">
  🇺🇸 English ·
  🇧🇷 <a href="README.pt-BR.md">Português</a> ·
  🇪🇸 <a href="README.es.md">Español</a>
</p>

<br>

You reformat the computer, switch machines, or sit down at a borrowed host. The whole environment (system, tools, data, and settings) should fit in your pocket, not be scattered across `~/.config`, `/usr/local`, `/var/lib`, and a dozen other places you will forget to copy.

`ohmlatl` exists for that: the whole computer is a single encrypted file, `rootfs.luks`, booted as a [Firecracker](https://github.com/firecracker-microvm/firecracker) microVM. Copy the file to another host and boot it there. Firecracker boots in about 125 ms and adds less than 5 MiB of VMM overhead per microVM. Guest RAM is whatever you configure. That is what makes it possible to carry a complete computer without turning the host into a heavyweight hypervisor and without depending on the host distribution.

The same isolation works for things you do not want to run on the host: a tool of unknown origin, a project cloned from the internet, a PoC, a third-party `npm install` or `make`, an agent. Run it inside the VM; by default it cannot reach the host, the LAN, or other VMs, only what you allow.

## What is ohmlatl?

`ohmlatl` manages bootable disk images and Firecracker microVM runs. It uses the embedded Podman **only at build time** to turn a `Containerfile` or an OCI image into an ext4 rootfs. At runtime there is no Podman and no container: there is a kernel, a rootfs, a virtio disk, and a Firecracker process.

The disk image is a single `rootfs.luks`: LUKS2 with per-sector integrity (`hmac-sha256`), holding ext4 with the whole system, including `/home/<user>`. The VM writes to it directly: it is your computer, not a template. You update it from the inside (`apt upgrade`), and data leaves through `ohmlatl cp`.

The `ohmlatl` binary embeds Firecracker, jailer, Podman, the guest kernel, and the initramfs. The initramfs asks for the passphrase on the VM console. No LUKS key is stored on disk.

Here, `disk image` is the artifact Firecracker boots. An `OCI image` is the source used by Podman at build time.

`ohmlatl` does not manage image contents: tools, IDEs, data, and services are your responsibility. It knows about images, VMs, and networks.

## Why use it?

**Because the host stops mattering.** The computer is one file. The target host only needs Linux, `/dev/kvm`, a network, TAP/nftables utilities, `systemd-run`, an SSH client, and `sudo`. Switching distributions or machines does not change the environment.

**Because isolation is real.** Containers share the host kernel. A microVM has its own kernel, seccomp, and, with the jailer, chroot, namespaces, and cgroups.

**Because you can run what you do not trust.** The tool, project, or agent runs inside the VM, under its own kernel, reached through `ohmlatl shell` and `exec`. There is no shared directory for the code to reach the host; only what you copy out with `ohmlatl cp` leaves. By default the guest cannot reach the host, the LAN, or other VMs. You open exactly what it needs, for example the Ollama on your host and an MCP server in another VM, and nothing else.

**Because tampering is detected.** The kernel and initramfs come from the trusted `ohmlatl` binary, not from the disk image. The rootfs has per-sector integrity: a modified sector fails to read instead of running altered code.

**Because data is encrypted at rest, with no key on disk.** The rootfs is always LUKS2. The passphrase is typed on the host terminal when the image is built and on the VM's `ttyS0` at boot, and exists only in memory.

**Because the image is declarative.** `Containerfile` plus manifest describe the system. The build uses rootless Podman inside `podman unshare`, so ownership, xattrs, and capabilities are correct inside ext4.

**Because the lifecycle is small.** Image, VM. `build`, `run`, `shell`, `stop`. No orchestrator, no global daemon. The supervisor exists per VM and exits with it.

## How to use

The full flow, with expected results, is in [USAGE.en.md](USAGE.en.md). The plan and phases are in [ROADMAP.en.md](ROADMAP.en.md). Summary:

**1. Check the host.**

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

**2. Build the base image.**

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` reads the `Containerfile` declared in `ohmlatl.yaml` and uses the embedded Podman. The example uses `docker.io/kalilinux/kali-rolling`, but the base can be any image with systemd and sshd. The build asks for the rootfs passphrase and leaves the Podman image `kali-base:2026-11-02` ready for the next step.

**3. Install whatever you want in a container.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
apt-get clean
exit
```

Replace `x`, `y`, and `z` with the packages you want.

**4. Turn the container into a disk image.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

The size defaults to the content plus 25% and 512 MiB; `--size 8G` changes it. The commit is exploration; once the tools stabilize, move the commands into the `Containerfile`.

**5. Boot the VM.**

```bash
ohmlatl run kali-custom:2026-11-02
```

`ohmlatl run` attaches the serial; the initramfs asks for the rootfs passphrase. `Ctrl-]` detaches; the VM keeps running.

```bash
ohmlatl shell kali-custom      # shell over SSH
ohmlatl console kali-custom    # reattach to serial
ohmlatl stop kali-custom       # clean shutdown
```

**6. Open only what the VM needs.** In `ohmlatl.yaml`:

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434      # Ollama on the host
      - port: 8080
        to: vm/mcp:8080     # MCP server in the "mcp" VM
```

Inside the VM, `172.31.0.1:11434` is the host's Ollama and `172.31.0.1:8080` is the MCP server. Nothing else is reachable.

## Principles

- The rootfs is the computer; the host is the bootloader.
- No LUKS key on disk. The passphrase exists on the keyboard and in memory.
- Host, LAN, and other VMs closed by default; every opening is declared.
- No global daemon, no bind mount of a host directory.
- The image is declarative; the runtime is small and explicit.

## Development

Local binaries live in `vendor/`. They are the source of the embedded binaries:

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
```

Versions, checksums, and limits are in [vendor/README.md](../vendor/README.md).

## Status

Pre-alpha. The repository is still the Rust scaffold; the documents describe the usability contract and the plan. Commands and details may change during P0 to P2.

## License

Not defined yet.
