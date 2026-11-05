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

`ohmlatl` exists for that: the whole computer is one encrypted file, `rootfs.luks`, that lives on your USB stick or external drive next to the `ohmlatl` binary, and boots as a [Firecracker](https://github.com/firecracker-microvm/firecracker) microVM. Plug the drive into another machine and boot it there. Firecracker boots in about 125 ms and adds less than 5 MiB of VMM overhead per microVM. Guest RAM is whatever you configure. That is what makes it possible to carry a complete computer without turning the host into a heavyweight hypervisor and without depending on the host distribution.

The same isolation works for things you do not want to run on the host: a tool of unknown origin, a project cloned from the internet, a PoC, a third-party `npm install` or `make`, an agent. Build a separate image for them and run it as its own VM: it reaches the internet, but not the host, the LAN, or other VMs, unless you open a specific port.

## What is ohmlatl?

`ohmlatl` manages bootable disk images and Firecracker microVM runs. It uses the embedded Podman **only at build time** to turn a `Containerfile` or an OCI image into an ext4 rootfs. At runtime there is no Podman and no container: there is a kernel, a rootfs, a virtio disk, and a Firecracker process.

The disk image is a `rootfs.luks`: LUKS2 holding ext4 with the whole system, including `/home/<user>`, next to a `manifest.yaml`. It is a sparse file: it only takes on the drive what has been written. The VM writes to it directly: it is your computer, not a template. You update it from the inside (`apt upgrade`), and data leaves through `ohmlatl cp`.

The `ohmlatl` binary embeds Firecracker, jailer, Podman, the guest kernel, and the initramfs. The drive holds only the binary and the images. The initramfs asks for the passphrase on the VM console. No LUKS key and no SSH key is stored on the drive.

Here, `disk image` is the artifact Firecracker boots. An `OCI image` is the source used by Podman at build time.

`ohmlatl` does not manage image contents: tools, IDEs, data, services, and how you log in over SSH are your responsibility. It knows about images, VMs, and networks.

## Why use it?

**Because the host stops mattering.** The computer travels on the drive. The host only needs Linux, `/dev/kvm`, nftables, systemd, an SSH client, and `sudo` for you. Nothing persistent is left on it. Switching distributions or machines does not change the environment. A borrowed host must still be one you trust: it sees the passphrase you type and the VM's memory.

**Because isolation is real.** Containers share the host kernel. A microVM has its own kernel, seccomp, and, with the jailer, chroot, namespaces, and cgroups.

**Because you can run what you do not trust.** The tool, project, or agent runs in its own VM, under its own kernel, reached through `ohmlatl shell` and `exec`. There is no shared directory for the code to reach the host; only what you copy out with `ohmlatl cp` leaves. The guest cannot reach the host, the LAN, or other VMs. You open exactly what it needs, for example the Ollama on your host and an MCP server in another VM, and nothing else. Keep it away from the image of your personal computer; to start clean, rebuild.

**Because a lost drive is useless without the passphrase.** The rootfs is always LUKS2, and no key is stored on the drive. The passphrase is typed on the host terminal when the image is built and on the VM's `ttyS0` at boot, and exists only in memory. The kernel and initramfs come from the `ohmlatl` binary, not from the disk image. Changing the encrypted file without the key does not inject code: it turns into garbage, and the boot or the program breaks.

**Because the disk only takes what you use.** The `rootfs.luks` is a sparse file: it has a disk size, what the VM sees (32 GiB by default), and a much smaller footprint on the drive, only what was actually written. Parts never written are holes that the filesystem does not store. A 32 GiB computer with 3 GiB of system takes about 3 GiB on the stick, and grows only as you write. If the VM needs more room (or should see less), `ohmlatl image resize kali-custom:2026-11-02 64G` changes the size it sees, without rebuilding.

**Because the image is declarative.** `Containerfile` plus manifest describe the system. The build uses rootless Podman inside `podman unshare`, so ownership, xattrs, and capabilities are correct inside ext4.

**Because the lifecycle is small.** Image, VM. `build`, `run`, `shell`, `stop`. No orchestrator, no global daemon, no VM state on the drive. The supervisor exists per VM and exits with it.

## How to use

The full flow, with expected results, is in [USAGE.en.md](USAGE.en.md). The plan and phases are in [ROADMAP.en.md](ROADMAP.en.md). Summary:

**1. Prepare the drive.** Formatted as ext4 (or btrfs/xfs):

```bash
./ohmlatl init /media/$USER/drive/ohmlatl
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --build
ohmlatl doctor --run
```

`init` copies the binary to the drive (asking for `sudo` once if the drive's root belongs to root). From then on, the `ohmlatl` on the drive finds its images by itself.

**2. Build the base container.**

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

The example uses `docker.io/kalilinux/kali-rolling`, but the base can be any image with systemd and sshd. This step only creates a Podman image; no disk, no passphrase.

**3. Install whatever you want in a container.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
passwd dev           # password for ohmlatl shell
apt-get clean
exit
```

Replace `x`, `y`, and `z` with the packages you want. For key login instead of a password, put your public key in `/home/dev/.ssh/authorized_keys`.

**4. Turn the container into a computer on the drive.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

The build asks for the rootfs passphrase. The disk defaults to 32 GiB (or twice the content plus 1 GiB, whichever is larger); `--size 64G` changes it. The file is sparse, so the drive only pays for what is written. The commit is exploration; once the tools stabilize, move the commands into the `Containerfile`.

**5. Boot the VM, here or on any other host.**

```bash
ohmlatl run kali-custom:2026-11-02
```

`ohmlatl run` asks for `sudo` once and attaches the serial; the initramfs asks for the rootfs passphrase. `Ctrl-]` detaches; the VM keeps running, even after you log out.

```bash
ohmlatl shell kali-custom      # shell over SSH (password once per boot)
ohmlatl console kali-custom    # reattach to serial
ohmlatl stop kali-custom       # clean shutdown; always before unplugging the drive
```

**6. Open only what the VM needs.** In the image's `manifest.yaml` (`images/<name>/<tag>/` on the drive), or in `ohmlatl.yaml` before the build:

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

Inside the VM, `ohmlatl-gateway:11434` is the host's Ollama and `ohmlatl-gateway:8080` is the MCP server. Nothing else is reachable. The change takes effect on the next `run`.

## Principles

- The drive is the computer; the host is the bootloader, and you trust it.
- No LUKS key and no SSH key on the drive. The passphrase exists on the keyboard and in memory.
- The guest does not reach the host, the LAN, or other VMs unless declared.
- No global daemon, no bind mount of a host directory, nothing persistent on the host.
- The image is declarative; the runtime is small and explicit.

## Development

Firecracker and the jailer live in `vendor/`. For development, `vendor/bin/podman` runs the Podman bundle that `cargo xtask assets` builds into `assets/out/podman`:

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
```

Versions, checksums, and limits are in [vendor/README.md](../vendor/README.md). `cargo xtask assets` builds the guest kernel and initramfs (Buildroot) and the embedded Podman bundle (podman-static recipe) in containers, with the exact sources of every GPL component; it needs `podman` on the host. The release already ships all of it inside the binary.

## Status

Pre-alpha. P0 and P1 are done: `init`, `doctor`, the embedded binaries, image builds and `image`. The runtime (P2: `run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`, the closed network and the DNS relay) is implemented and being validated. Forwards and several VMs working together (P3) do not exist yet; until then `run` refuses a manifest with `forward`. Commands and details may still change.

## License

`ohmlatl` is dual-licensed under [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE), at your option. The binary embeds third-party programs (Firecracker, Podman, the Linux kernel, BusyBox, cryptsetup, and others) that keep their own licenses, some of them GPL. `ohmlatl licenses` lists them with their full texts, and each release publishes the exact source code of the GPL components next to the binary.
