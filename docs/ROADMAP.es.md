# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 Español

Estado: pre-implementación. El repositorio todavía es el scaffold Rust; este documento es el plan que guía las próximas fases. Los detalles técnicos están en `TECHNICAL.md` (pt-BR).

## Visión

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Con Podman en el build, convierte un `Containerfile` o una imagen OCI en un `rootfs.luks` (LUKS2 + integrity + ext4) y, en runtime, arranca ese rootfs con Firecracker, usando el kernel y el initramfs embebidos en el binario.

La computadora es el `rootfs.luks`: un archivo que guarda el sistema, las herramientas y los datos, y que se puede copiar a otro host y arrancar allí. El mismo aislamiento sirve para ejecutar herramientas, proyectos y agentes no confiables, con la red cerrada por defecto y las aperturas declaradas en el manifest. El runtime no usa Podman, no tiene daemon global y no hace bind mount de directorios del host.

El ejemplo de la documentación es una imagen Kali Rolling. `ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos y servicios son responsabilidad del operador.

## Alcance

- Generar un rootfs a partir de un `Containerfile` o de una imagen OCI, con Podman rootless.
- `rootfs.luks` en LUKS2 con integridad `hmac-sha256`, desbloqueado en el initramfs con una passphrase escrita en la consola de la VM. `/home/<user>` vive en el rootfs. Sin discos extra.
- Kernel e initramfs generados en el repositorio y embebidos en `ohmlatl`.
- Firecracker con jailer, TAP por VM.
- Red cerrada al host, la LAN y otras VMs por defecto; internet opcional; forwards TCP hacia servicios del host y otras VMs.
- Supervisor por VM con serial en PTY, `console` y `logs`.
- `shell`, `exec` y `cp` por SSH.
- CLI pequeña, sin daemon global.

## No objetivos

- Publicación de puertos, ingress, proxy inverso o túnel público. Los forwards solo salen de la VM; el host alcanza la IP del guest; el acceso externo es problema del operador.
- Bind mount de directorios del host.
- Discos extra o volúmenes.
- GPU, pantalla, passthrough PCI/USB.
- macOS/Windows nativos. Firecracker requiere Linux con KVM.
- Guests sin systemd.
- Gestionar herramientas, procesos, plugins o flujos de tareas.
- Snapshots de VM. Filtran RAM en claro y quedan atados a CPU/versión.

## Decisiones de arquitectura

| Tema | Decisión |
|---|---|
| VMM | Firecracker con jailer |
| Binarios | Firecracker, jailer, Podman, kernel e initramfs embebidos en `ohmlatl` |
| Host | Linux x86_64 con `/dev/kvm` |
| Build | Podman rootless embebido, `podman unshare` obligatorio |
| Runtime | supervisor como unidad de sistema root vía un único `sudo systemd-run`; el jailer baja al UID del operador |
| Init | systemd |
| Rootfs | LUKS2 + integrity (con journal) + ext4, `--size` o calculado |
| Datos | dentro del rootfs; se actualiza desde dentro; el rebuild crea una computadora nueva |
| Passphrase | TTY del host en el build, `ttyS0` de la VM al arrancar; PBKDF fijo |
| Claves | ninguna clave LUKS en disco |
| Acceso | `shell`/`exec`/`cp` por SSH, host key del build |
| Red | TAP + `/30`; cerrada por defecto; NAT opcional hacia internet; forwards TCP |
| Supervisor | uno por VM; sobrevive al logout; sin daemon global |
| Portabilidad | copiar el directorio de la imagen a otro host |

## Arquitectura

```text
HOST
├── ohmlatl (CLI confiable; embebe Firecracker, jailer, Podman, vmlinux, initramfs)
├── ~/.local/share/ohmlatl/
│   ├── images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
│   └── vms/<nombre>/state.json
├── /run/ohmlatl/ (tmpfs: bin, jail, console.sock, serial.log)
├── supervisor por VM (unidad de sistema ohmlatl-<nombre>)
│   ├── Firecracker (vía jailer)
│   ├── PTY del serial
│   └── forwards TCP
└── TAP + nftables

GUEST
├── initramfs: prompt de passphrase, fsck, mount, switch_root
├── systemd
├── sshd
└── contenido de la imagen
```

## Flujo de arranque

```text
ohmlatl run <nombre>:<tag>
  → valida el manifest, asigna el /30
  → sudo systemd-run: supervisor como unidad de sistema
      → extrae los binarios embebidos a /run/ohmlatl
      → TAP + chains nft + forwards
      → PTY + jailer → Firecracker
  → kernel + initramfs
      → passphrase del rootfs en ttyS0
      → e2fsck -p
      → monta el rootfs, escribe resolv.conf
      → switch_root
  → systemd → sshd
  → el supervisor mantiene serial, proceso y forwards
```

La cmdline del kernel solo lleva `console`, `reboot=k`, `panic`, `root` e `ip=` (dirección, gateway, DNS). La passphrase nunca entra en el manifest, la cmdline, el entorno ni los logs.

## Pipeline de la imagen de disco

```text
Containerfile o ref OCI local
  → podman build / podman create
  → genera la clave SSH del cliente y la host key del guest
  → podman export + tar dentro de podman unshare, en ~/.cache/ohmlatl/build
  → machine-id, hostname, host key, authorized_keys, NetworkManager fuera de eth0,
    poweroff/halt → reboot
  → valida systemd, sshd, usuario
  → tamaño: --size o ceil(du -sb * 1.25) + 512 MiB
  → passphrase una vez → luksFormat --integrity hmac-sha256 --integrity-no-wipe (sudo)
  → mkfs.ext4 -d con el tamaño útil del mapper → dd al mapper, close
  → images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/}; borra los temporales
```

Reglas:

- Todo el pipeline de export/mkfs corre dentro de `podman unshare`. Fuera de él, el rootfs queda con el UID del operador y rompe `sudo`, `sshd`, setuid y capabilities.
- `podman export`, no `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- Ningún rootfs en claro fuera del directorio temporal del build, que se borra al final y ante un fallo.
- PBKDF fijo (`argon2id`, 256 MiB, 2 s), para que el desbloqueo no dependa de la máquina del build.
- Las imágenes remotas necesitan nombre calificado; `--from` solo usa imágenes locales.
- Sin secretos en `ARG`/`ENV`/`RUN`; usa `podman build --secret`.
- Base de CPU genérica; sin `-march=native`.

## Red

- Un TAP por VM, `/30` de `172.31.0.0/16`, host `.1`, guest `.2`, asignado por `run`, saltando redes que ya están en la tabla de rutas.
- nftables en una tabla compartida (`inet ohmlatl`) con chains por VM. La tabla y `ip_forward` se restauran cuando se detiene la última VM.
- Por defecto: el guest alcanza internet por NAT, y nada en el host, la LAN u otras VMs.
- `internet: false` corta internet.
- `forward`: el supervisor escucha en `<gateway>:<puerto>` y se conecta a `127.0.0.1:<puerto>` del host o a otra VM. Solo TCP.
- El host alcanza `172.31.0.2:<puerto>`. Sin `--publish`, ingress ni proxy.
- IPv6 desactivado.

## Ciclo de vida y supervisor

- `run` pide `sudo` una vez e inicia el supervisor como la unidad de sistema `ohmlatl-<nombre>`. Sobre una VM detenida, cambia la imagen.
- El supervisor mantiene el PTY, Firecracker, la red y los forwards; sobrevive al detach de la CLI y al logout.
- `Ctrl-]` desconecta; `console` vuelve a adjuntar; `shell` usa SSH.
- `stop` envía Ctrl-Alt-Del por la API de Firecracker; con `reboot=k`, el reinicio limpio del guest termina Firecracker. Kill después de 30 s. Dentro del guest, `poweroff` y `reboot` también terminan la VM.
- La clave SSH del cliente y la host key del guest se generan en el build, junto a la imagen; SSH verifica la host key de forma estricta e ignora `~/.ssh/config`.
- `start` reutiliza la última imagen y vuelve a pedir la passphrase.
- `rm` elimina el estado de la VM; la imagen permanece.

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

Sin `expose`, `ingress` ni `publish`.

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

`ohmlatl build` copia el manifest a `images/<nombre>/<tag>/manifest.yaml`, con el nombre y la tag finales. `run` lee esa copia; editar `vm` en ella vale en el próximo `start`. `name` + `build.tag` forman la referencia de la imagen; sin tag, `latest`; `-t` sobrescribe. `build.containerfile` es opcional con `--from`. Memoria mínima: 512 MiB. Los campos desconocidos son error.

## Layout en el host

```text
~/.local/share/ohmlatl/
├── images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/{id_ed25519,known_hosts}}
└── vms/<nombre>/state.json

/run/ohmlatl/{bin,jail,<nombre>/{console.sock,api.sock,serial.log}}
~/.cache/ohmlatl/{bin,podman,build}
```

Archivos 0600, directorios 0700. La caché de Podman es caché, no estado: hacer rebuild en otro host vuelve a ejecutar la cadena de build allí.

## Seguridad

| Amenaza | Protección |
|---|---|
| `rootfs.luks` robado | LUKS2; ninguna clave LUKS en disco |
| `rootfs.luks` manipulado | Integridad `hmac-sha256` por sector; un sector alterado falla al leerse |
| Kernel/initramfs manipulado | No están en disco; embebidos en `ohmlatl` |
| Otro usuario en el host | Archivos 0600; directorios 0700; socket de consola del operador |
| Workload escapando al host | KVM + kernel propio + seccomp + jailer |
| Workload alcanzando host, LAN u otras VMs | Bloqueado por nft por defecto; solo pasan los forwards declarados |
| Workload tomando el agente SSH del operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase en logs o en disco | Leída de la TTY, pasada por stdin; echo desactivado al arrancar |

Limitaciones: sin protección contra rollback a una copia antigua de `rootfs.luks`; el integrity cuesta velocidad de escritura y el archivo ocupa su tamaño completo; la clave SSH privada queda en claro en el host (0600); un rebuild crea una computadora nueva y vacía; con `internet: true` el guest alcanza internet sin filtro; sin protección contra un host comprometido.

## Portabilidad

- Linux x86_64 con KVM en el MVP.
- Kernel y binarios con base de CPU genérica; sin `-march=native`.
- El directorio de la imagen se copia tal cual a otro host.
- Siempre cold boot; sin snapshots.

## Fases

### P0: Sanidad del host y arranque

- `ohmlatl doctor`.
- Scripts en el repositorio que generan el kernel y el initramfs.
- Embeber y extraer los binarios.
- Arrancar un rootfs de prueba simple con Firecracker, sin jailer.

**Listo cuando:** la VM de prueba arranca con el kernel y el initramfs embebidos, da consola, tiene red por `ip=`, y Ctrl-Alt-Del termina Firecracker.

### P1: Pipeline de la imagen de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` con el Podman embebido.
- `podman unshare` con export/tar/xattrs.
- `rootfs.luks` con integrity; claves SSH; copia del manifest.
- `image ls/inspect/rm`.
- `examples/kali` como primer ejemplo.

**Listo cuando:** `sudo -n true`, `getcap /bin/ping` y SSH funcionan en el guest; el rootfs se desbloquea con passphrase en la consola; `e2fsck` sale limpio; la imagen Kali personalizada con `podman commit` arranca; una imagen sin systemd o sshd falla con un mensaje claro; cambiar un byte de `rootfs.luks` produce un error de lectura; no queda nada en `~/.cache/ohmlatl/build`.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Supervisor como unidad de sistema, PTY, log del serial, detach/attach.
- Jailer, TAP `/30`, red cerrada por defecto, `internet: true|false`.

**Listo cuando:** el prompt de passphrase aparece en el serial de la VM; `shell` entra por SSH con host key estricta; el jailer está activo; la VM sobrevive al logout; el guest no alcanza el host, la LAN ni otra VM; `stop` y `poweroff` dentro del guest apagan limpiamente; `kill -9` sobre Firecracker seguido de `start` pasa `e2fsck`.

### P3: Forwards y varias VMs

- `forward` hacia `host:<puerto>` y `vm/<nombre>:<puerto>`.
- Subnets automáticas; varias VMs a la vez; tabla nft compartida.
- `doctor --run` con Docker y ufw.

**Listo cuando:** una VM de agente con `internet: false` alcanza el Ollama del host y un servidor MCP en otra VM, y nada más; dos VMs corren a la vez sin colisión; detener una VM no rompe la red de la otra.

### Futuro

Sin plan detallado: backup/restore, `image import/export`, forwards UDP, `gc`, `--json`, completions, aarch64, protección contra rollback.

## Pruebas

**Unitarias**

- Parser y validación del manifest (nombres, UID/GID, puertos, forwards).
- Generación de la cmdline.
- Cálculo de tamaño.
- Asignación de subnet contra las rutas existentes.
- Reglas nft por VM.

**Integración**

- Build con Podman → ext4 con dueños, xattrs y capabilities.
- `--from` con una imagen Kali personalizada; rechazo de nombres remotos cortos.
- Fallo claro para una imagen sin systemd o sshd.
- `rootfs.luks` con integrity; cambiar un byte falla la lectura.
- Initramfs con `cryptsetup` en `ttyS0`; reintento; passphrase incorrecta cae al rescue.
- Jailer + TAP + PTY del serial bajo `systemd-run`.
- Ctrl-Alt-Del y `poweroff` terminan Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` por SSH con `ForwardAgent=yes` en `~/.ssh/config` (no debe reenviar).
- Red: guest bloqueado hacia host, LAN y otras VMs; los forwards funcionan; `internet: false` corta la salida.

**E2E**

- Primer ejemplo: base Kali, `podman run`, instalar x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `doctor → build → run → shell → stop → start`.
- Copiar el directorio de la imagen a otro host y arrancarlo allí.
- VM de agente + VM de MCP + Ollama en el host.

**Matriz**

- Intel y AMD; 8/16/32 GiB de RAM.
- Hosts con y sin Docker/ufw.
- CPU más débil de la flota (base de CPU).

## Riesgos

| Riesgo | Mitigación |
|---|---|
| KVM no disponible/anidado | `doctor` falla temprano con instrucciones |
| Prompt de `cryptsetup` en el initramfs | `cttyhack`/dup2 de `ttyS0`; pruebas de echo y reintento |
| Dueños incorrectos en el build | Pipeline completo en `podman unshare`; pruebas de aceptación |
| `mkfs.ext4 -d` antiguo | `doctor` exige e2fsprogs >= 1.43 |
| Host sin soporte de integrity | `doctor --build` verifica cryptsetup y dm-integrity |
| Costo de escritura del integrity | Aceptado; documentado |
| El reloj del guest empieza mal (sin RTC) | `broken_system_clock` en el `e2fsck.conf` del initramfs; chrony en el guest |
| Rootfs lleno | Tamaño por defecto con 25% + 512 MiB; `--size` |
| Docker/ufw descartan el forwarding | `doctor --run` lo detecta e imprime la regla |
| NetworkManager en el guest sobrescribiendo `ip=` | `eth0` unmanaged, escrito en el build |
| Kali puro sin systemd/sshd | `build` valida y apunta a `examples/kali` |
| Commit de contenedor opaco | Documentado como exploración; el Containerfile es la fuente durable |
| Binario grande (~100 MB con Podman) | Aceptado; se extrae una vez por arranque del host |

## Elementos a confirmar durante la implementación

- Versión y config del kernel del guest.
- Licencia del proyecto y avisos de terceros.

## Criterio de MVP

El MVP es P0 a P2: un host Linux con KVM, una imagen generada con Podman, un arranque con la passphrase en la consola de la VM, shell por SSH, jailer activo, red cerrada por defecto y un apagado limpio. P3 agrega los forwards y varias VMs trabajando juntas.
