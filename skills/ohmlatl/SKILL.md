---
name: ohmlatl
description: Operates ohmlatl, the CLI that turns a Containerfile or OCI image into an encrypted (LUKS2) rootfs on a USB stick or external drive and boots it as a Firecracker microVM with a closed network. Use it whenever the user mentions ohmlatl, an `ohmlatl.yaml` or image `manifest.yaml`, a `rootfs.luks`, `ohmlatl-gateway`, or Firecracker microVMs they carry on a drive; when they want to build, boot, stop, resize or move such a computer; reach a VM with shell, exec or cp; open a host service or another VM to a guest with a forward; or run untrusted code, a PoC, a cloned project or an AI agent isolated in its own VM.
---

# ohmlatl

`ohmlatl` (pronounced ohm-lal) carries a whole computer as one encrypted file on a drive and boots it as a Firecracker microVM on any Linux x86_64 host with KVM. Podman is used only at build time; at runtime there is a kernel, an initramfs, a virtio disk and a Firecracker process, owned by a per-VM supervisor.

## Mental model

- **ohmlatl directory**: on the drive, created by `ohmlatl init`: `.ohmlatl`, the `ohmlatl` binary, and `images/`. Nothing else persists, on the drive or on the host.
- **Image** `name:tag`: `images/<name>/<tag>/{rootfs.luks, manifest.yaml}`. The `rootfs.luks` is LUKS2 + ext4 holding the whole system, `/home` included. It is the computer itself, not a template: the VM writes to it.
- **VM**: a run of an image. It is named after the image name, and only one VM per name runs at a time. Commands that act on a running VM take the **name only** (`stop kali`), `run` takes **name:tag** (`run kali:2026-11-02`).
- **Network**: closed. The guest reaches the internet (if allowed) and nothing else: not the host, not the LAN, not other VMs, except through declared TCP forwards on `ohmlatl-gateway`.
- **Files**: no shared directory, ever. Data goes in and out with `ohmlatl cp`; commands run with `ohmlatl exec`.
- **Secrets**: the LUKS passphrase is typed by a human on the host terminal (`build`, `image resize`, `image passwd`) and on the VM serial console (every boot). No key is stored anywhere.

## Who does what

Every passphrase, `[y/N]` confirmation and password prompt is read from `/dev/tty`. An agent has no terminal and must never fake one, so these steps belong to the human. Give them the exact command to paste and wait for them to say it is done.

| You (the agent) run it | The human runs it in their terminal |
|---|---|
| `ohmlatl --version`, `ohmlatl doctor [--build\|--run]` | `ohmlatl init <dir>` (may ask for sudo) |
| `ohmlatl podman build`, `pull`, `images`, `commit`, `rm`, `rmi` | `ohmlatl build ...` (passphrase twice, sudo) |
| `ohmlatl podman run` without `-it`, with a command | `ohmlatl run <name:tag>` (sudo, then the passphrase on the console) |
| `ohmlatl image ls`, `ohmlatl image inspect <name:tag>` | `ohmlatl console <vm>` (typing the passphrase, rescue) |
| `ohmlatl ps`, `ohmlatl logs <vm>` | `ohmlatl shell <vm>` the first time per boot, if login is by password |
| `ohmlatl exec <vm> -- cmd`, `ohmlatl cp ...` (after login, see below) | `ohmlatl image resize`, `image passwd`, `image rm` (passphrase or confirmation) |
| `ohmlatl stop <vm>` | Unplugging the drive, moving it to another host |
| Writing `ohmlatl.yaml`, `Containerfile`, editing an image's `manifest.yaml` | Setting the user's password (`passwd` inside the VM or container) |

Destructive or irreversible steps (`image rm`, shrinking with `image resize`) are the human's decision even where the agent could reach them.

## Rules that always hold

1. **Never handle the passphrase.** Never ask for it in chat, never pass it in argv, environment, file, pipe, here-doc, `expect` or `script`. The CLI refuses anything but a real TTY on purpose. If the user pastes it in chat anyway, do not use it: say it is now exposed, and suggest changing it with `ohmlatl image passwd <name:tag>` (VM stopped).
2. **Never put a password in an image.** No `chpasswd` or `echo user:pass` in a `Containerfile`: it stays in plaintext in the layers. Passwords are set interactively; for agents, use a public key in `authorized_keys`.
3. **Never open the guest to the outside.** No port publishing, ingress, reverse proxy, tunnel or bind mount of a host directory. The only openings are `vm.network.forward` entries, which go out of the guest.
4. **Untrusted code gets its own image**, never the image of the user's personal computer. To start clean, rebuild.
5. **`ohmlatl stop <vm>` before the drive is unplugged** or the host shuts down by hand. A removed drive can damage the filesystem.
6. **A build never overwrites a tag.** Pick a new tag (a date works: `2026-11-10`). A rebuild is a new, empty computer: data must be moved with `cp`.
7. **Only use the commands listed in [references/cli.md](references/cli.md).** `launch`, `supervise` and `cleanup` are internal; there is no `start`, `rm` (for VMs), `publish`, `expose`, `volume` or `mount`.

## Making the agent autonomous

- **SSH login once per boot.** `shell`, `exec` and `cp` share one SSH master connection per VM (`ControlPersist`), kept until `stop`. If the image logs in by password, ask the human to run `ohmlatl shell <vm>` once, type the password and exit; from then on every `exec` and `cp` you run reuses the master without a password. Without that, `exec` fails with `ssh login to <user>@<ip> failed`.
- **Full autonomy: a key.** If the image's user has the human's public key in `~/.ssh/authorized_keys`, the host's `~/.ssh/id_*` keys or ssh-agent log in with no prompt. ohmlatl ignores `~/.ssh/config`, never forwards the agent, and never checks host keys (the link is a local point-to-point TAP).
- **Booting.** `ohmlatl run` needs sudo and a human for the passphrase. Hand the whole `run` to the human; start it yourself only when the user asked you to and `sudo -n true` succeeds (sudo would otherwise prompt on a terminal you do not have). When stdin is not a terminal, `run` starts the VM, prints the network and then `stdin is not a terminal: serial not attached; use ohmlatl console <name>`. Either way, the human types the passphrase in `ohmlatl console <vm>` (or in the attached `run`) and detaches with `Ctrl-]`. You follow the boot without a terminal: `ohmlatl ps` lists it once the supervisor is ready, and `ohmlatl logs <vm>` shows `Enter passphrase for /dev/vda:` until it is typed, then the systemd boot and `<user>@<name>:~$` at the end.
- **Results.** Errors are one line `ohmlatl: error: <what failed and what to do>` with exit code 1; success is exit 0. `doctor` prints `[ok]`, `[warn]` or `[fail] ... fix: <command>` per check. Read the message: it usually names the next command.
- **Inside the guest** you work like on any Linux box through `exec`: `ohmlatl exec <vm> -- sudo apt-get install -y x`. Commands after `--` are passed to the guest untouched; quote shell syntax: `ohmlatl exec <vm> -- sh -c 'cd /src && make'`. `exec` has no TTY, so `sudo` in the guest only works with `NOPASSWD` for the user (the example image has it); otherwise use `sudo -n` and hand the step to the human through `ohmlatl shell <vm>`.

## Common flows

1. **New computer from a base.** Write `ohmlatl.yaml` and `Containerfile` (guest needs systemd, sshd, the declared user), `ohmlatl podman build -t base:<tag> .`, then the human runs `ohmlatl build`. See [references/images.md](references/images.md) and [references/manifest.md](references/manifest.md).
2. **Boot and work.** Human: `ohmlatl run <name:tag>`, passphrase, `Ctrl-]`, and `ohmlatl shell <name>` once if login is by password. You: `ohmlatl exec`, `ohmlatl cp`, `ohmlatl logs`, and `ohmlatl stop <name>` at the end.
3. **Run something untrusted.** Dedicated image, `internet: false` or minimal forwards, files in with `cp`, run with `exec`, results out with `cp`, rebuild to reset. See [references/sandbox.md](references/sandbox.md).
4. **Give a VM one service.** Add a `forward` to the image's `manifest.yaml` on the drive, `stop` and `run` again; inside, use `ohmlatl-gateway:<port>`. See [references/network.md](references/network.md).
5. **Update or migrate.** Update from inside (`exec -- sudo apt-get -y full-upgrade`). A rebuild with a new tag is empty: `cp` the data out, `stop`, `run` the new tag, `cp` it back. See [references/images.md](references/images.md).

## References

Read only the one the task needs:

- [references/cli.md](references/cli.md): every command, its arguments, prompts, output format and who runs it. Read before running any command you have not used yet.
- [references/manifest.md](references/manifest.md): `ohmlatl.yaml` and `manifest.yaml`, every field, limits, the YAML subset, when an edit takes effect.
- [references/images.md](references/images.md): what a guest image needs, writing the Containerfile, the explore-then-commit flow, login setup, size, resize, rebuild and data migration.
- [references/network.md](references/network.md): internet on or off, forwards to the host or to another VM, `ohmlatl-gateway`, DNS, the address pool, host firewalls.
- [references/sandbox.md](references/sandbox.md): running untrusted code, PoCs, third-party builds or AI agents isolated in a VM.
- [references/drive.md](references/drive.md): `init`, the directory layout, filesystems, ownership, another host, verifying the binary, backup.
- [references/troubleshooting.md](references/troubleshooting.md): symptom, cause, and what you do or ask the human to do.
