# Manifest reference

Two files share one format:

- **`ohmlatl.yaml`**: next to the `Containerfile`, in the directory where `ohmlatl build` runs. Read only by `build`.
- **`images/<name>/<tag>/manifest.yaml`**: written by `build` with the final name and tag. Read by `run` on every boot. `ohmlatl image inspect <name:tag>` prints its path (the `rootfs:` line's directory) and its contents.

Editing `vm` in the image's `manifest.yaml` takes effect on the next `run` (stop the VM first, then run it again); no rebuild. Editing `ohmlatl.yaml` only affects future builds. The image's `name`, `build` and `user` are history: changing them in `manifest.yaml` does not rename or retag anything.

## Full example

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

## Fields

| Field | Required | Rule |
|---|---|---|
| `format` | yes | `1`. Any other value is refused. |
| `name` | yes | `^[a-z][a-z0-9-]{0,30}$`. Also the VM name. `-t` overrides it. |
| `build.containerfile` | no | Path relative to the build directory. Not needed with `--from`; `-f` overrides it. |
| `build.tag` | no | `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`. Default `latest`. `-t` overrides it. |
| `user.name` | yes | `^[a-z_][a-z0-9_-]{0,31}$`. Must exist in the image's `/etc/passwd` with this uid and gid and an existing home owned by the uid. Used for the serial auto-login and for SSH. |
| `user.uid`, `user.gid` | yes | Integers from 1 to 65534. |
| `vm.vcpus` | yes | 1 to 32. `run --vcpus` overrides it for one run. |
| `vm.memory` | yes | A size of at least 512 MiB. `run --memory` overrides it for one run. |
| `vm.network.internet` | no | `true` (default) or `false`. |
| `vm.network.forward` | no | List of `{port, to}`. See below. |

`build` and `vm.network` may be left out entirely.

### Forwards

Each item:

- `port`: 1 to 65535, the port on `ohmlatl-gateway` inside the guest. No port may repeat. `port: 53` is refused with `internet: true` (the gateway's port 53 is the DNS relay then).
- `to`: `host:<port>` (the host's `127.0.0.1:<port>`) or `vm/<name>:<port>` (another VM of the same host user, by image name). Ports have no leading zero.

Semantics are in [network.md](network.md).

## Sizes

An integer with an optional binary unit: `K`, `M`, `G`, `T`, or `KiB`, `MiB`, `GiB`, `TiB` (`B` or nothing means bytes). Decimals are not accepted: write `1536MiB`, not `1.5GiB`. The same rule applies to `--size`, `--memory` and `image resize`.

## The YAML subset

The parser is strict on purpose:

- Allowed: block mappings, block lists (`- item`), plain or quoted scalars, `[]` (empty list), `#` comments (at line start or after a space).
- Errors: tabs in indentation, flow collections (`{a: 1}`, `[1, 2]`), anchors and aliases, tags (`!!str`), block scalars (`|`, `>`), duplicate keys, and **any unknown field**.
- Indent with spaces, consistently.

Errors name the line or field: `line 7: bad indentation`, `unknown field vm.disk`, `missing field user.gid`, `vm.memory: 256MiB (want a size of at least 512MiB)`, `vm.network.forward[0].to: localhost:80 (want host:<port> or vm/<name>:<port>)`.

## Common edits

Cut the internet and open one host service (agent edits the image's manifest, then `ohmlatl stop <vm>`; the human runs `ohmlatl run <name:tag>` again):

```yaml
vm:
  vcpus: 2
  memory: 4GiB
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434
```

There is no field for extra disks, volumes, mounts, published ports, GPU, display or kernel modules: they are non-goals, not missing features.
