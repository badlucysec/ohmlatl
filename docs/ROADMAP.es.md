# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 Español

Estado: pre-implementación. El repositorio todavía es el scaffold de Rust; este documento es el plan que guía las próximas fases. Los detalles técnicos están en `TECHNICAL.md` (pt-BR).

## Visión

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Con Podman en el build, convierte un `Containerfile` o una imagen OCI en un `rootfs.luks` (LUKS2 + integridad + ext4) y, en runtime, arranca ese rootfs con Firecracker, usando el kernel y el initramfs embebidos en el binario.

El host es delgado: binario, `/dev/kvm`, red y el disco. La computadora real (rootfs, volúmenes, herramientas y datos) vive en el disco externo. El runtime no usa Podman, no tiene daemon, no guarda estado persistente en el host y no hace bind mount de directorios del host.

El ejemplo de la documentación es una imagen Kali Rolling. `ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos y servicios son responsabilidad del operador.

## Alcance

- Build de rootfs a partir de `Containerfile` o imagen OCI, con Podman rootless.
- `rootfs.luks` en LUKS2 con integridad `hmac-sha256`, abierto en el initramfs con una passphrase escrita en la consola de la VM. `/home/<user>` vive en el rootfs.
- Volúmenes genéricos, ext4 con o sin LUKS2, montados donde diga el manifest.
- Kernel e initramfs generados en el repositorio y embebidos en `ohmlatl`.
- Firecracker con jailer, TAP/NAT.
- Supervisor por VM con serial en PTY, `console` y `logs`.
- `shell`, `exec` y `cp` vía SSH.
- CLI pequeña, sin daemon global, todo el estado persistente en el disco.

## Fuera de alcance

- Publicación de puertos, ingress, proxy inverso o túnel público. El host llega a la IP del guest; el acceso externo es problema del operador.
- Bind mount de directorios del host.
- GPU, passthrough PCI/USB.
- macOS/Windows nativo. Firecracker requiere Linux con KVM.
- Guests sin systemd.
- Gestión de herramientas, procesos, plugins o flujos de tareas.
- Snapshots de VM. Filtran RAM en claro y quedan atados a CPU/versión.

## Decisiones de arquitectura

| Tema | Decisión |
|---|---|
| VMM | Firecracker con jailer |
| Binarios | Firecracker, jailer, Podman, kernel e initramfs embebidos en `ohmlatl` |
| Host | Linux x86_64 con `/dev/kvm` |
| Build | Podman rootless embebido, `podman unshare` obligatorio |
| Runtime | supervisor del usuario; `sudo` para TAP, nft y jailer |
| Init | systemd |
| Rootfs | LUKS2 + integridad + ext4, `--size` o calculado |
| Home | dentro del rootfs; se rehace con la imagen |
| Volúmenes | ext4, LUKS2 opcional, punto de montaje en el manifest |
| Passphrase | en el `ttyS0` de la VM; la del rootfs se prueba primero en los volúmenes |
| Claves | ninguna clave LUKS en el disco |
| Acceso | `shell`/`exec`/`cp` vía SSH |
| Red | TAP + `/30` + NAT; sin publish/ingress |
| Supervisor | uno por VM; sin daemon global |
| Actualización | rebuild con nuevo tag; volúmenes intactos |

## Arquitectura

```text
HOST
├── ohmlatl (CLI confiable; embebe Firecracker, jailer, Podman, vmlinux, initramfs)
├── $XDG_RUNTIME_DIR/ohmlatl/{bin,jail} (tmpfs)
├── supervisor de la VM (uno por VM)
│   ├── Firecracker (vía jailer)
│   ├── PTY del serial
│   └── log de salida de la consola
└── TAP + nftables

DISCO
├── bin/ohmlatl
├── images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volumen>.{luks,ext4}
└── vms/<nombre>/{state.json,known_hosts,serial.log,console.sock,lock}

GUEST
├── initramfs: prompt de passphrase, fsck, mount, switch_root
├── systemd
├── sshd
└── contenido de la imagen
```

## Flujo de arranque

```text
ohmlatl run <nombre>:<tag>
  → valida el manifest, toma los locks de la VM y de los volúmenes
  → extrae los binarios embebidos a tmpfs
  → asigna /30, crea TAP + reglas nft (sudo)
  → inicia supervisor + PTY
  → inicia Firecracker vía jailer (sudo)
  → kernel + initramfs
      → passphrase del rootfs en ttyS0
      → la prueba en cada volumen LUKS, vuelve a preguntar si falla
      → e2fsck -p
      → monta rootfs en /newroot, volúmenes en /newroot/<mount>
      → switch_root
  → systemd → sshd
  → el supervisor mantiene serial y proceso
```

El host pasa todo lo que el initramfs necesita en la cmdline del kernel: `ip=` para la red, `ohmlatl.user=<uid>:<gid>` y `ohmlatl.disks=<dev>:<luks|ext4>:<mount>,...`. La passphrase nunca entra en el manifest, la cmdline, el entorno ni los logs.

## Pipeline de la imagen de disco

```text
Containerfile o ref OCI
  → podman build / pull + podman create
  → genera la clave SSH del cliente
  → tamaño: --size o ceil(du -sb * 1.25) + 512 MiB
  → luksFormat --integrity hmac-sha256 (sudo)
  → podman export + tar dentro de podman unshare
  → limpia machine-id, host keys SSH, hostname; instala authorized_keys
  → mkfs.ext4 -d con el tamaño útil del mapper
  → dd en el mapper, cierra
  → images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
```

Reglas:

- Todo el pipeline de export/mkfs corre dentro de `podman unshare`. Fuera de él, el rootfs queda con el UID del operador y rompe `sudo`, `sshd`, setuid y capabilities.
- `podman export`, no `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- Nada de secretos en `ARG`/`ENV`/`RUN`; usa `podman build --secret`.
- Baseline de CPU; sin `-march=native`.
- El build falla si la imagen no tiene systemd, sshd o el usuario declarado.

## Volúmenes

- `volume create <nombre> --size <n> [--luks]`: `volumes/<nombre>.luks` o `volumes/<nombre>.ext4`.
- Los volúmenes LUKS no tienen integridad, por el costo de escritura.
- En el primer montaje, la raíz vacía del volumen recibe `chown` al usuario del manifest.
- Un volumen solo puede tener un dueño a la vez: la VM o el montaje en el host. Lock por archivo.
- `volume mount` en el host siempre es de solo lectura y solo con la VM detenida.

## Red

- Un TAP por VM, `/30` dentro de `172.31.0.0/16`, host `.1`, guest `.2`, asignado por `run`.
- NAT vía nftables en tabla propia (`inet ohmlatl`), limpiada en `stop`.
- IP y DNS del guest vía `ip=` en la cmdline del kernel; el DNS viene del resolver del host.
- Sin `--publish`, ingress ni proxy. El host llega a `172.31.0.2:<puerto>`.
- IPv6 desactivado. Sin filtro de egress, documentado como limitación.

## Ciclo de vida y supervisor

- `run` crea la VM, inicia el supervisor y adjunta el serial. Sobre una VM detenida, cambia la imagen.
- El supervisor mantiene el PTY, Firecracker y el log de salida; sobrevive al detach de la CLI.
- `Ctrl-]` desacopla; `console` vuelve a adjuntar; `shell` usa SSH.
- `stop` envía Ctrl-Alt-Del por la API de Firecracker; con `reboot=k`, el reinicio limpio del guest termina Firecracker. Kill tras 30 s. El supervisor limpia TAP/nft y libera los locks.
- La clave SSH se genera en el build, junto a la imagen. El `known_hosts` de la VM se reinicia cuando cambia la imagen.
- `start` reutiliza la última imagen y vuelve a pedir las passphrases.
- `rm` elimina el estado de la VM; imágenes y volúmenes se quedan.

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

Sin `expose`, `ingress`, `publish` ni `host-forward`.

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

`ohmlatl build` copia el manifest a `images/<nombre>/<tag>/manifest.yaml`. El manifest queda en claro y se valida antes de convertirse en cmdline del kernel. `name` + `build.tag` forman la referencia de la imagen; sin tag, `latest`; `-t` lo sobrescribe. `build.containerfile` es opcional con `--from`.

## Layout del disco

```text
/mnt/vmdisk/
├── bin/ohmlatl
├── images/<nombre>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volumen>.{luks,ext4}
└── vms/<nombre>/{state.json,known_hosts,serial.log,console.sock,lock}
```

El disco debe montarse sin `noexec`. La caché de build de Podman queda en el host en `$XDG_CACHE_HOME/ohmlatl/podman`; es caché, no estado.

## Seguridad

| Amenaza | Protección |
|---|---|
| Robo del disco | Rootfs y volúmenes `--luks` en LUKS2; ninguna clave LUKS en el disco |
| Rootfs manipulado | Integridad `hmac-sha256` por sector; el sector alterado falla al leerse |
| Kernel/initramfs manipulado | No están en el disco; vienen embebidos en `ohmlatl` |
| `ohmlatl` manipulado en el disco | El operador verifica el binario contra el SHA-256 o la firma del release, fuera del disco |
| Manifest manipulado | Validación estricta antes de generar la cmdline |
| Otro usuario en el host | Archivos 0600; directorios 0700 |
| Workload escapando al host | KVM + kernel propio + seccomp + jailer |
| Passphrase en logs | El supervisor registra solo la salida de la consola; echo desactivado |

Limitaciones: los volúmenes sin `--luks` quedan en claro; los volúmenes LUKS no tienen integridad; sin protección contra el rollback de un `rootfs.luks` antiguo completo; la clave SSH privada queda en claro en el disco (0600); sin protección contra un host comprometido.

## Portabilidad

- Linux x86_64 con KVM en el MVP.
- Disco en ext4/btrfs/xfs; sin FAT/exFAT/NTFS.
- Kernel y binarios con baseline de CPU; sin `-march=native`.
- Siempre arranque en frío; sin snapshots.

## Fases

### P0: Sanidad del host y arranque

- `ohmlatl init` y `ohmlatl doctor`.
- Scripts en el repositorio que generan el kernel y el initramfs.
- Embeber y extraer los binarios.
- Arranque de un rootfs de prueba simple con Firecracker, sin jailer.

**Listo cuando:** la VM de prueba arranca con el kernel y el initramfs embebidos, da consola, tiene red vía `ip=` y Ctrl-Alt-Del termina Firecracker.

### P1: Pipeline de la imagen de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` con el Podman embebido.
- `podman unshare` con export/tar/xattrs.
- `rootfs.luks` con integridad; clave SSH; copia del manifest.
- Limpieza de `machine-id`, host keys SSH y hostname.
- `image ls/inspect/rm`.
- `examples/kali` como primer ejemplo.

**Listo cuando:** `sudo -n true`, `getcap /bin/ping` y SSH funcionan en el guest; el rootfs se abre con passphrase en la consola; `e2fsck` sale limpio; la imagen Kali personalizada vía `podman commit` arranca; una imagen sin systemd o sshd falla con un mensaje claro; cambiar un byte en `rootfs.luks` produce un error de lectura.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Supervisor por VM, PTY, log de salida, detach/attach.
- Jailer, TAP/NAT `/30`.
- `volume create/ls/rm`; volúmenes en el initramfs con la regla "misma passphrase, si no, pregunta".

**Listo cuando:** el prompt de passphrase aparece en el serial de la VM; los volúmenes se montan donde dice el manifest, con el usuario como dueño; `shell` entra vía SSH; el jailer está activo; `stop` apaga de forma limpia; `kill -9` sobre Firecracker seguido de `start` pasa `e2fsck`.

### P3: Volúmenes en el host y varias VMs

- `volume inspect/mount/umount`, de solo lectura, con la VM detenida.
- Locks entre la VM y el montaje en el host.
- Subredes automáticas; dos VMs al mismo tiempo.

**Listo cuando:** un volumen sobrevive al rebuild del rootfs; se monta de solo lectura en el host; el montaje con la VM corriendo se rechaza; dos VMs corren al mismo tiempo sin colisión.

### Futuro

Sin plan detallado: actualización atómica con rollback, backup/restore, `image import/export`, `volume resize`, `gc`, `--json`, completions, aarch64, integridad en volúmenes, protección contra rollback.

## Pruebas

**Unitarias**

- Parser y validación del manifest (nombres, mounts, UID/GID).
- Generación de la cmdline.
- Cálculo de tamaño.
- Asignación de subred.
- Estado de la VM y locks de volumen.

**Integración**

- Build Podman → ext4 con dueños, xattrs y capabilities.
- `--from` con imagen Kali personalizada.
- Falla clara para imagen sin systemd o sshd.
- `rootfs.luks` con integridad; cambiar un byte falla la lectura.
- Initramfs con `cryptsetup` en `ttyS0`; misma passphrase en volúmenes; reintento.
- Passphrase del rootfs incorrecta cae al rescue.
- Jailer + TAP + PTY del serial.
- Ctrl-Alt-Del termina Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` vía SSH.

**E2E**

- Primer ejemplo: base Kali, `podman run`, instalar x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → build → volume create → run → shell → stop → start`.
- Rebuild del rootfs con volúmenes preservados.
- Montaje de solo lectura de un volumen en el host.

**Matriz**

- Intel y AMD; 8/16/32 GiB de RAM.
- ext4 y btrfs; discos USB y NVMe.
- CPU más débil de la flota (baseline de CPU).

## Riesgos

| Riesgo | Mitigación |
|---|---|
| KVM no disponible/anidado | `doctor` falla temprano con instrucciones |
| Jailer + TAP + PTY pasando por `sudo` | Primera tarea del P2; `--no-jailer` solo para debug |
| Ctrl-Alt-Del no termina Firecracker | Validar en P0; kill tras timeout |
| Prompt de `cryptsetup` en el initramfs | `cttyhack`/dup2 de `ttyS0`; pruebas de echo y reintento |
| Dueños incorrectos en el build | Pipeline completo en `podman unshare`; pruebas de aceptación |
| `mkfs.ext4 -d` antiguo | `doctor` exige e2fsprogs >= 1.43 |
| Host sin soporte de integridad | `doctor --build` revisa cryptsetup y dm-integrity |
| Costo de escritura de la integridad en el rootfs | Medir en P1; la escritura pesada va a volúmenes |
| El reloj del guest arranca mal (sin RTC) | `broken_system_clock` en el `e2fsck.conf` del initramfs; chrony en el guest |
| Rootfs lleno | Tamaño por defecto con 25% + 512 MiB; `--size` |
| Retiro abrupto del disco | `stop` obligatorio; `e2fsck` en el arranque |
| Kali puro sin systemd/sshd | `build` valida y apunta a `examples/kali` |
| Commit de contenedor opaco | Documentado como exploración; el Containerfile es la fuente durable |
| Sin filtro de egress | Limitación documentada |
| Binario grande (~100 MB con Podman) | Aceptado; se extrae una vez por arranque del host |

## Elementos a confirmar en la implementación

- Versión y config del kernel del guest.
- Opciones exactas de `cryptsetup` para integridad (tamaño de sector, journal).
- Detalles del jailer para TAP y serial.
- Licencia del proyecto y avisos de terceros.

## Criterio de MVP

El MVP es P0 a P2: un host Linux con KVM, un disco preparado, una imagen construida con Podman, un arranque con la passphrase en la consola de la VM, volúmenes montados, shell vía SSH, jailer activo y un apagado limpio. P3 completa el uso diario del disco.
