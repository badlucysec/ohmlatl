# Images

A disk image is `images/<name>/<tag>/{rootfs.luks, manifest.yaml}` on the drive: LUKS2 (aes-xts, no integrity) holding ext4 with the whole system, `/home` included. It is built from an OCI image by `ohmlatl build`, then used and changed in place by its VM. The kernel and initramfs come from the `ohmlatl` binary, never from the image.

## What the guest image must have

`build` fails without these:

- **systemd as `/sbin/init`.** Plain base images (`kalilinux/kali-rolling`, `debian`, `ubuntu`) do not have it: install `systemd systemd-sysv`.
- **`/usr/sbin/sshd`** (`openssh-server`), enabled so it starts at boot (`systemctl enable ssh.service` on Debian/Kali, `sshd.service` on Fedora/Arch).
- **The manifest's user** in `/etc/passwd` with the declared uid and gid, and an existing home owned by that uid.

`build` warns (and still builds) without these:

- **An sftp subsystem** in `sshd_config` (`internal-sftp` or an existing `sftp-server`). Without it `ohmlatl cp` fails.
- **A way to log in over SSH**: a password for the user or `~/.ssh/authorized_keys`. Without it only the console works.

Needed in practice:

- `serial-getty@ttyS0.service` enabled (the console). `build` adds the auto-login for the user itself.
- `sudo` for the user, if the agent will administer the guest through `exec`.
- `iproute2`, `ca-certificates`, `udev`, `dbus`. A clock: `chrony` with the `ptp_kvm` refclock keeps time also without internet.

`build` takes care of: `/etc/hostname` and `/etc/hosts` (the image name), an empty `machine-id`, SSH host keys if missing, `resolv.conf` (rewritten on every boot to the gateway), masking `systemd-networkd` and `systemd-resolved` (the network comes from the kernel command line), keeping NetworkManager off `eth0`, and making `poweroff`/`halt` end the VM.

## A generic base Containerfile

Adapted from the repository's `examples/kali/Containerfile`. For Debian or Ubuntu, change only the `FROM` (`docker.io/library/debian:trixie`, `docker.io/library/ubuntu:24.04`): the package list is the same. For other distributions, use their names for the same packages and `sshd.service`. Add the tools the work needs to the `apt-get install` line (`curl`, `git`, compilers), or in a derived Containerfile `FROM` this base.

```dockerfile
FROM docker.io/kalilinux/kali-rolling:latest
# In a real build, pin the base by digest: FROM docker.io/...@sha256:...

ENV DEBIAN_FRONTEND=noninteractive

# No services start during apt in the build.
RUN printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d \
    && chmod +x /usr/sbin/policy-rc.d

RUN apt-get update && apt-get install -y --no-install-recommends \
      systemd systemd-sysv dbus udev kmod iproute2 \
      openssh-server ca-certificates chrony tzdata \
      util-linux sudo libcap2-bin iputils-ping \
    && rm -rf /var/lib/apt/lists/*

# Guest clock from the host (ptp_kvm), also without internet.
RUN mkdir -p /etc/chrony/conf.d \
    && printf 'refclock PHC /dev/ptp0 poll 3 dpoll -2 offset 0\n' \
       > /etc/chrony/conf.d/ohmlatl-ptp.conf

RUN useradd -m -u 1000 -s /bin/bash dev

RUN printf 'dev ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/90-dev \
    && chmod 0440 /etc/sudoers.d/90-dev

RUN systemctl enable serial-getty@ttyS0.service ssh.service chrony.service \
    && systemctl mask getty@tty1.service \
    && (systemctl mask systemd-timesyncd.service || true) \
    && rm -f /usr/sbin/policy-rc.d

CMD ["/bin/bash"]
```

With the matching `ohmlatl.yaml` (user `dev`, uid and gid 1000), see [manifest.md](manifest.md).

## Login: password or key

ohmlatl never creates or stores credentials; login is image content.

- **Key (best for agents).** Put the human's public key in the image, owned by the user, mode 0600:

  ```dockerfile
  COPY --chown=1000:1000 --chmod=0600 authorized_keys /home/dev/.ssh/authorized_keys
  RUN chmod 0700 /home/dev/.ssh && chown 1000:1000 /home/dev/.ssh
  ```

  Ask the human which public key to use (for example `~/.ssh/id_ed25519.pub`) and copy it next to the `Containerfile`: `cp ~/.ssh/id_ed25519.pub ./authorized_keys`. A public key is not a secret; never copy the private key. Then `exec` and `cp` work with no prompt from the first call.
- **Password.** Never in a `Containerfile` or build argument. The human sets it interactively, either before the build (`ohmlatl podman run -it --name x <ref>`, `passwd dev`, `exit`, `ohmlatl podman commit x <ref>`) or after the first boot from the console (`ohmlatl console <vm>`, `sudo passwd dev`). The human then runs `ohmlatl shell <vm>` once per boot and the agent's `exec` and `cp` reuse that connection.

## Two ways to get the content

**Pinned (repeatable, preferred for agents).** Everything in Containerfiles, layered by tag:

```bash
cd base/  && ohmlatl podman build -t base:2026-11-02 .     # agent
cd ../dev && ohmlatl build                                 # human: FROM base:2026-11-02 + ohmlatl.yaml
```

In `dev/`, the `Containerfile` starts `FROM base:2026-11-02` (a local Podman image) and installs the tools; `ohmlatl.yaml` sets `build.containerfile` and `build.tag`.

**Explore, then commit.** For trying things by hand in a container:

```bash
ohmlatl podman run --name dev-x -it base:2026-11-02     # human: install, passwd, exit
ohmlatl podman commit dev-x dev:2026-11-02              # agent
ohmlatl build --from dev:2026-11-02 -t dev:2026-11-02   # human
```

An agent can do the install step without a TTY: `ohmlatl podman run --name dev-x base:2026-11-02 sh -c 'apt-get update && apt-get install -y x y z && apt-get clean'`, then `commit`. Once the tool list is stable, move it into a Containerfile.

Clean package caches before committing (`apt-get clean`, `rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*`): they become disk content.

## Size

- Default disk size: 32 GiB or twice the content plus 1 GiB, whichever is larger. `build --size 64G` changes it.
- The file is sparse: `SIZE` (what the VM sees) and `ON DISK` (what the drive stores) differ; `ohmlatl image ls` shows both.
- Grow or shrink later with `ohmlatl image resize <name:tag> <size>` (human, VM stopped). Growing is safe and quick. Shrinking needs a backup first.
- The drive itself can fill before the VM's disk does: then the guest gets I/O errors. Watch free space on the drive, not only `df` in the guest.
- Deleting files inside the guest does not give space back to the drive (no discard): `ON DISK` only grows.

## Updating and rebuilding

- **Update in place** (keeps all data): `ohmlatl exec <vm> -- sudo apt-get update` and `ohmlatl exec <vm> -- sudo apt-get -y full-upgrade`.
- **Rebuild** (new tag, new **empty** computer): data from the old one is not carried over. Migrate:

  ```bash
  ohmlatl cp dev:/home/dev/work ./work     # old VM running
  ohmlatl stop dev
  # human: ohmlatl run dev:2026-11-10
  ohmlatl cp ./work dev:/home/dev/work
  ```

  The old image stays until the human removes it with `ohmlatl image rm dev:2026-11-02`.
- **Changing the passphrase**: `ohmlatl image passwd <name:tag>` (human, VM stopped).
- The Podman cache stays on the host where it was built. To rebuild on another host, the whole chain (base, then derived images) is built again there.
