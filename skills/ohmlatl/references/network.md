# Network

Each VM gets its own point-to-point `/30` on a TAP `ohm<N>`, allocated on every `run`. The guest is IPv4 only.

## Default: closed

- The guest reaches the **internet** when `vm.network.internet: true` (the default), and nothing else: not the host, not the LAN (all RFC1918, `169.254/16`, `100.64/10` and the host's connected networks are blocked, re-computed when the host changes networks), not other VMs. Refusals fail immediately (ICMP reject), not by timeout.
- Nothing from the LAN or other VMs reaches the guest.
- The host user who owns the VM (and root) reach the guest at its IP. `ohmlatl shell`, `exec` and `cp` do that; you never need the IP yourself.

## `ohmlatl-gateway`

Inside the guest, `ohmlatl-gateway` is the host end of the `/30`. It is written into the guest's `/etc/hosts` on every boot, and `/etc/resolv.conf` points at it. **Always use the name, never the IP**: the subnet can change between runs.

- With `internet: true`, DNS goes to `ohmlatl-gateway:53`, relayed to the host's resolvers (follows the host's `/etc/resolv.conf`, works with `127.0.0.53` and dnsmasq).
- With `internet: false`, DNS lookups fail immediately. Use forwards by name (`ohmlatl-gateway:<port>`), not hostnames from the internet.
- Only the forward ports (and 53 with internet) answer on the gateway. Everything else is refused.

## Forwards

A forward is a TCP port on `ohmlatl-gateway` that leads to one service outside the guest. It is the only way out of the guest besides the internet. It never opens anything into the guest.

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434      # Ollama on the host
      - port: 8080
        to: vm/mcp:8080     # port 8080 of the VM named "mcp"
```

- `to: host:<port>` connects to `127.0.0.1:<port>` on the host. The service can keep listening on loopback only, which is safer. The forward does not take the port on the host: the host can still use it.
- `to: vm/<name>:<port>` connects to another running VM of the same host user, by image name. If that VM is stopped, connections close right away; start it and they work, no restart of the caller needed. The other VM's service must listen on its `eth0` (or `0.0.0.0`), not only on its loopback.
- The guest port and the target port may differ (`port: 8000`, `to: host:11434`).
- TCP only. No UDP, no ICMP through forwards.
- At most 64 open forward connections per VM; more are closed at once. No idle timeout; TCP keepalive frees dead ones.
- `run` always prints the forwards it opened:

  ```text
  [ohmlatl] internet: off
  [ohmlatl] forwards:
    ohmlatl-gateway:11434 -> host 127.0.0.1:11434
    ohmlatl-gateway:8080  -> vm mcp:8080
  ```

### Changing the network of an image

1. `ohmlatl image inspect <name:tag>`: the image directory is the one of the `rootfs:` line; edit `manifest.yaml` there.
2. Edit `vm.network` (rules in [manifest.md](manifest.md)).
3. `ohmlatl stop <vm>`, then the human runs `ohmlatl run <name:tag>` again (passphrase). No rebuild.
4. Check from inside: `ohmlatl exec <vm> -- getent hosts ohmlatl-gateway`, then `ohmlatl exec <vm> -- curl -s http://ohmlatl-gateway:<port>/` (or any client the image has).

### Connecting several VMs

```bash
ohmlatl run mcp:2026-11-02      # human, first: the service VM
ohmlatl run agent:2026-11-02    # human: the VM with `to: vm/mcp:8080`
ohmlatl ps
```

Each VM gets its own `/30`. Only declared `vm/<name>` forwards connect them, and only in the declared direction: `agent` reaching `mcp` does not let `mcp` reach `agent`.

## What does not exist

No port publishing, ingress, reverse proxy, tunnel, bridge to the LAN, or any way for something outside to open a connection into the guest. If the user needs to reach a guest service from the host, that is `ohmlatl exec`/`shell` or a client running inside the guest; from elsewhere, it is outside ohmlatl's scope and the user's responsibility. Do not build workarounds (SSH `-R`/`-L` through `ohmlatl shell`, socat in the guest) without the user explicitly asking.

## Host-side issues

- **No internet or no DNS in the guest on a host with Docker, ufw or firewalld.** A drop policy in another firewall table wins over ohmlatl's rules. `ohmlatl doctor --run` detects it and prints the `iptables -I` or `nft insert rule` that lets `ohm*` through; the human runs it (root). It lasts until that firewall reloads its rules.
- **ohmlatl's forwarding guard.** If IP forwarding was off on the host, ohmlatl turns it on and adds a table `inet ohmlatl-fwguard` so the host does not route between its other interfaces. It stays until the host reboots; a Docker or libvirt started later will not route while it exists. `doctor --run` shows it and the command to remove it (`sudo nft delete table inet ohmlatl-fwguard`).
- **The host already uses `172.31.0.0/16`.** `run` refuses when a host address or route falls inside the pool. Set another pool for the run, for example `OHMLATL_POOL=10.213.0.0/16 ohmlatl run <name:tag>`. Do the same when a remote network in that range is reached through the default route (an AWS VPC, a corporate VPN): ohmlatl cannot detect that case.
