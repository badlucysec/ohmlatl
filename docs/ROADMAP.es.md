# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 [Português](ROADMAP.pt-BR.md) · 🇪🇸 Español

Estado: P0 listo. Este documento es el plan que guía las próximas fases. Los detalles técnicos están en `TECHNICAL.md` (pt-BR).

## Visión

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Con Podman en el build, convierte un `Containerfile` o una imagen OCI en un `rootfs.luks` (LUKS2 + ext4, disperso) y, en runtime, arranca ese rootfs con Firecracker, usando el kernel y el initramfs embebidos en el binario.

La computadora vive en un pendrive o disco externo: un directorio con el binario `ohmlatl` y las imágenes, cada una un `rootfs.luks` con el sistema, las herramientas y los datos, más su manifest. Conecta el disco a otra máquina confiable y arráncala allí. El mismo aislamiento sirve para ejecutar herramientas, proyectos y agentes no confiables en sus propias imágenes, con el guest cerrado al host, la LAN y otras VMs, y las aperturas declaradas en el manifest. El runtime no usa Podman, no tiene daemon global, no deja nada persistente en el host y no hace bind mount de directorios del host.

El ejemplo de la documentación es una imagen Kali Rolling. `ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos, servicios y el login por SSH son responsabilidad del operador.

## Alcance

- Un directorio de ohmlatl en el disco, creado por `init`, con el binario e `images/`.
- Generar un rootfs a partir de un `Containerfile` o de una imagen OCI local, con Podman rootless, escrito directamente en el disco.
- `rootfs.luks` en LUKS2 (`aes-xts-plain64`), desbloqueado en el initramfs con una passphrase escrita en la consola de la VM. `/home/<user>` vive en el rootfs. Sin discos extra.
- Kernel de uso general e initramfs generados en el repositorio y embebidos en `ohmlatl`.
- Firecracker con jailer, TAP por VM.
- Guest cerrado al host, la LAN y otras VMs; internet opcional con DNS por el gateway; forwards TCP hacia servicios del host y otras VMs.
- Supervisor por VM como unidad de sistema, con serial, `console` y `logs`.
- `shell`, `exec` y `cp` por SSH, con el login que tenga la imagen.
- CLI pequeña, sin daemon global, sin estado de VM en el disco.
- Release: binario estático (musl), `SHA256SUMS` firmado con la clave SSH del mantenedor (`ssh-keygen -Y`), el código fuente exacto de los componentes GPL embebidos y los avisos de terceros, también disponibles en `ohmlatl licenses`.

## No objetivos

- Publicación de puertos, ingress, proxy inverso o túnel público. Los forwards solo salen de la VM; el acceso externo es problema del operador.
- Bind mount de directorios del host.
- Discos extra o volúmenes.
- Gestión de claves SSH.
- GPU, pantalla, passthrough PCI/USB.
- Módulos de kernel en el guest.
- macOS/Windows nativos. Firecracker requiere Linux con KVM.
- Guests sin systemd.
- Gestionar herramientas, procesos, plugins o flujos de tareas.
- Snapshots de VM. Filtran RAM en claro y quedan atados a CPU/versión.
- Protección contra un host comprometido.

## Decisiones de arquitectura

| Tema | Decisión |
|---|---|
| VMM | Firecracker con jailer |
| Binarios | Firecracker, jailer, Podman, kernel e initramfs embebidos en `ohmlatl` |
| Disco | ext4/btrfs/xfs; directorio de ohmlatl con el binario e `images/`; se encuentra por la ubicación del binario, `--home` u `OHMLATL_HOME` |
| Host | Linux x86_64, kernel >= 5.10, con `/dev/kvm` y `sudo`, confiable aunque sea prestado; no queda nada persistente en él |
| Build | Podman rootless embebido, `podman unshare` obligatorio, `mkfs` directo sobre el mapper de LUKS |
| Runtime | un `sudo` por `run`; supervisor como la unidad de sistema `ohmlatl-<uid>-<nombre>`; el jailer baja al UID del operador |
| Init | systemd |
| Rootfs | LUKS2 `aes-xts-plain64` + ext4, disperso, 32 GiB por defecto, `image resize` para cambiarlo; sin integrity ("let it crash") |
| Datos | dentro del rootfs; se actualiza desde dentro; el rebuild crea una computadora nueva con tag nueva |
| Passphrase | TTY del host en el build, `ttyS0` de la VM al arrancar; PBKDF fijo |
| Claves | ninguna clave LUKS ni clave SSH en el disco |
| Acceso | `shell`/`exec`/`cp` por SSH con la contraseña o la clave de la imagen, una conexión por arranque; consola con autologin |
| Red | TAP + `/30` en cada run; cerrada por defecto; NAT opcional; relay de DNS; forwards TCP vía `ohmlatl-gateway` |
| Supervisor | uno por VM; sobrevive al logout; sin daemon global |
| Estado de VM | ninguno; `run <nombre>:<tag>` siempre es explícito |

## Arquitectura

```text
DISCO
└── ohmlatl/
    ├── ohmlatl (CLI confiable; embebe Firecracker, jailer, Podman, vmlinux, initramfs)
    └── images/<nombre>/<tag>/{rootfs.luks,manifest.yaml}

HOST (nada persistente)
├── /run/ohmlatl/ (tmpfs propio: bin, jail, registros de red, sockets y log en <uid>/<nombre>/)
├── supervisor por VM (unidad de sistema ohmlatl-<uid>-<nombre>)
│   ├── Firecracker (vía jailer, en el cgroup de la unidad)
│   ├── PTY del serial (raw)
│   └── forwards TCP + relay de DNS
├── TAP + nftables (tabla inet ohmlatl)
└── ~/.cache/ohmlatl (caché de Podman, solo donde se hace build)

GUEST
├── initramfs: prompt de passphrase, fsck, mount, resolv.conf, ohmlatl-gateway, switch_root
├── systemd
├── sshd
└── contenido de la imagen
```

## Flujo de arranque

```text
ohmlatl run <nombre>:<tag>
  → encuentra el directorio de ohmlatl, revisa el dueño, valida el manifest, imprime internet y forwards
  → sudo ohmlatl launch
      → monta /run/ohmlatl (tmpfs), extrae los binarios
      → systemd-run: supervisor como unidad de sistema
          → /30, TAP, reglas nft, forwards, relay de DNS
          → PTY raw + jailer → Firecracker (API: kernel, initramfs, drive, red, rng)
  → la CLI imprime la red y se adjunta a la consola
  → kernel + initramfs
      → passphrase del rootfs en ttyS0
      → e2fsck -p
      → monta el rootfs, escribe resolv.conf y ohmlatl-gateway
      → switch_root
  → systemd → sshd, autologin en el serial
```

La cmdline del kernel solo lleva `console`, `reboot=k`, `panic` e `ip=` (dirección, gateway). La passphrase nunca entra en el manifest, la cmdline, el entorno ni los logs.

## Pipeline de la imagen de disco

```text
ohmlatl.yaml + Containerfile, o una ref OCI local
  → valida nombre y tag; rechaza una tag existente
  → podman build / podman create
  → podman export + tar dentro de podman unshare, en ~/.cache/ohmlatl/build en el host
  → valida systemd, sshd, usuario y home; avisa si el usuario no tiene contraseña ni clave
  → machine-id, hostname, resolv.conf, host keys de SSH si faltan, NetworkManager fuera de eth0,
    networkd/resolved enmascarados, poweroff/halt → reboot, autologin en el serial
  → tamaño: --size o max(32 GiB, 2 x contenido + 1 GiB), alineado a 1 MiB
  → passphrase una vez → luksFormat + open (sudo), en el disco
  → mkfs.ext4 -d sobre el mapper dentro de podman unshare (mke2fs.conf fijo) → e2fsck → close
  → images/<nombre>/<tag>/{rootfs.luks,manifest.yaml}; borra los temporales
```

Reglas:

- Todo el pipeline de export/mkfs corre dentro de `podman unshare`. Fuera de él, el rootfs queda con el UID del operador y rompe `sudo`, `sshd`, setuid y capabilities.
- `podman export`, no `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- El árbol exportado solo existe en el directorio de build en el host, que se borra al final y ante un fallo; el ext4 se escribe directamente dentro de LUKS en el disco.
- PBKDF fijo (`argon2id`, 256 MiB, 4 iteraciones), para que el desbloqueo no dependa de la máquina del build.
- Feature set de ext4 fijo, para que el `e2fsck` embebido siempre lo entienda.
- Las imágenes remotas necesitan nombre calificado y un `ohmlatl podman pull` explícito.
- Sin secretos en `ARG`/`ENV`/`RUN`, incluida la contraseña del usuario; usa `podman build --secret`, o `passwd` dentro de `ohmlatl podman run`.
- Base de CPU genérica; sin `-march=native`.

## Red

- Un TAP `ohm<N>` por VM, un `/30` de `172.31.0.0/16` asignado en cada run (host `.1`, guest `.2`), negándose a arrancar si el host ya usa ese rango; `OHMLATL_POOL` lo cambia. El guest alcanza el lado del host por el nombre `ohmlatl-gateway`, escrito en cada arranque.
- nftables en una tabla compartida (`inet ohmlatl`), con reglas por VM y lock entre supervisores. `ip_forward` se activa y nunca se desactiva; si estaba desactivado, el enrutamiento entre las otras interfaces del host sigue bloqueado hasta reiniciar (`doctor --run` muestra cómo liberarlo).
- Por defecto: el guest alcanza internet por NAT y nada en el host, la LAN u otras VMs. Nada de la LAN llega al guest ni al gateway. Las direcciones de origen se verifican. Solo root y el dueño de la VM se conectan desde el host al guest.
- `internet: false` corta internet y el DNS.
- DNS: con `internet: true`, el supervisor reenvía el puerto 53 del gateway al resolver actual del host.
- `forward`: el puerto del gateway se redirige a un listener del supervisor, que se conecta a `127.0.0.1:<puerto>` del host o a otra VM. Nunca ocupa el puerto en el host. Solo TCP.
- Sin `--publish`, ingress ni proxy.
- IPv6 desactivado en el TAP.

## Ciclo de vida y supervisor

- `run <nombre>:<tag>` pide `sudo` una vez e inicia el supervisor como la unidad de sistema `ohmlatl-<uid>-<nombre>`. Se niega si ya hay una VM con ese nombre corriendo. El mismo comando vuelve a arrancar la computadora después; no existe `start`.
- El supervisor mantiene el PTY, Firecracker, la red y los forwards; sobrevive al detach de la CLI y al logout. Estado y stop pasan por su socket de control; `ps` lista las que están corriendo.
- `Ctrl-]` desconecta; `console` vuelve a adjuntar con autologin y repite la salida más reciente; `shell` usa SSH.
- `stop` (o el apagado del host) envía Ctrl-Alt-Del por la API de Firecracker; con `reboot=k`, el reinicio limpio del guest termina Firecracker. Kill después de 120 s. Dentro del guest, `poweroff` y `reboot` también terminan la VM. Después del `stop`, el disco se puede desmontar.
- Limpieza idempotente cuando el supervisor sale, y de nuevo después de que muere: TAP, reglas nft, jail, sockets, la conexión SSH maestra.
- SSH: login con la contraseña o la clave de la imagen; una conexión maestra por arranque, reutilizada por `exec` y `cp`; `~/.ssh/config` ignorado, sin reenvío del agente, sin `known_hosts`.
- `image rm` rechaza una imagen cuya VM está corriendo.

## CLI

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm
ohmlatl image resize <name:tag> <size>
ohmlatl image passwd <name:tag>

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

Todos los comandos aceptan `--home <dir>`. Sin `expose`, `ingress` ni `publish`. `launch`, `supervise` y `cleanup` son internos.

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

`ohmlatl build` exige el manifest en el directorio actual y lo copia a `images/<nombre>/<tag>/manifest.yaml`, con el nombre y la tag finales. `run` lee esa copia; editar `vm` en ella vale en el próximo `run`. `name` + `build.tag` forman la referencia de la imagen; sin tag, `latest`; `-t` sobrescribe. `build.containerfile` es opcional con `--from`. Memoria mínima: 512 MiB. `port: 53` es inválido con `internet: true`. Los campos desconocidos son error.

## Layout

```text
DISCO  <directorio de ohmlatl>/{.ohmlatl,ohmlatl,images/<nombre>/<tag>/{rootfs.luks,manifest.yaml}}
HOST   /run/ohmlatl/{bin/<sha256>,jail,net,<uid>/<nombre>/{ctl.sock,console.sock,ssh.sock,serial.log,ip,error}}
HOST   ~/.cache/ohmlatl/{bin,podman,build}
```

El directorio de ohmlatl y el binario son 0755, para que el binario corra incluso con otro UID; `images/` es 0700 y sus archivos 0600, del operador. Todo lo que queda en el host en `/run` desaparece al reiniciar. La caché de Podman es caché, no estado, y guarda las imágenes de contenedor en claro: `ohmlatl podman system reset` la limpia. Hacer rebuild en otro host vuelve a ejecutar la cadena de build allí.

## Seguridad

| Amenaza | Protección |
|---|---|
| Disco robado o perdido | `rootfs.luks` en LUKS2; ninguna clave LUKS ni clave SSH en el disco |
| `rootfs.luks` manipulado | Sin la clave, el cambio se vuelve basura en el guest y rompe el arranque o un programa |
| Kernel/initramfs manipulado | Embebidos en el binario, no sueltos en el disco |
| Binario `ohmlatl` manipulado en el disco | Comparar el SHA-256 con el release en un host nuevo, o usar un `ohmlatl` del host con `--home` |
| Otro usuario en el host | Directorios y sockets solo del operador; namespacing por UID; solo root y el dueño se conectan al guest |
| Workload escapando al host | KVM + kernel propio + seccomp + jailer |
| Workload alcanzando host, LAN u otras VMs | Rechazado por nft por defecto; anti-spoofing; solo pasan los forwards declarados |
| LAN alcanzando el guest o los forwards | Descartado por nft |
| Workload tomando el agente SSH del operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase en logs o en disco | Leída de la TTY, pasada por stdin, PTY raw; echo desactivado al arrancar |

Limitaciones: sin integrity, así que la manipulación no se detecta con certeza, y sectores o el archivo entero pueden volver a una copia antigua; el archivo disperso muestra qué bloques se escribieron; borrar archivos en el guest no encoge el archivo; quitar el disco con la VM corriendo corrompe su disco; la host key SSH del guest no se verifica (enlace local punto a punto con un host confiable); el `manifest.yaml` queda en claro en el disco, así que un disco manipulado puede cambiar recursos y abrir forwards, que `run` imprime; un rebuild crea una computadora nueva y vacía; con `internet: true` el guest alcanza internet sin filtro; sin protección contra un host comprometido, prestado o no.

## Portabilidad

- Linux x86_64 con KVM en el MVP.
- Kernel y binarios con base de CPU genérica; sin `-march=native`.
- El disco cambia de host tal cual; un UID distinto se detecta y se imprime la corrección.
- Siempre cold boot; sin snapshots.

## Fases

### P0: Sanidad del host y arranque

- `ohmlatl init` y `ohmlatl doctor`.
- Build de los assets en contenedores (`cargo xtask assets`): kernel 6.18 LTS e initramfs con Buildroot, más el bundle de Podman con la receta de podman-static, ambos con sus fuentes exactas; `mke2fs.conf` escrito a mano. Los binarios de Podman salen de `vendor/`.
- Embeber y extraer los binarios.
- Arrancar un rootfs LUKS de prueba desde el disco con Firecracker bajo el jailer, desde un supervisor ejecutado a mano.

**Listo cuando:** `init` prepara un disco ext4 y rechaza FAT/exFAT; la VM de prueba arranca con el kernel y el initramfs embebidos; el prompt de passphrase funciona en el PTY raw y nunca aparece en el log; la red sube por `ip=`; Ctrl-Alt-Del termina Firecracker; los bind mounts de archivos sobreviven al `pivot_root` del jailer; `/dev/ptp0` existe y `chrony` se sincroniza con él; el `e2fsck` embebido acepta un ext4 hecho con el `mke2fs.conf` embebido.

### P1: Pipeline de la imagen de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` con el Podman embebido y configuración propia.
- `podman unshare` con export/tar/xattrs; `mkfs` sobre el mapper, en el disco.
- Host keys de SSH si faltan; aviso de contraseña/clave; copia del manifest; limpieza del build.
- `image ls/inspect/rm`, `image resize` (crecer y encoger), `image passwd`.
- `examples/kali` como primer ejemplo.

**Listo cuando:** en la consola serial (autologin), `sudo -n true` y `getcap /bin/ping` funcionan y los archivos mantienen sus dueños; el rootfs se desbloquea con la passphrase en la consola; `e2fsck` sale limpio; la imagen Kali personalizada con `podman commit` arranca; una imagen sin systemd o sshd falla con un mensaje claro; una tag existente se rechaza; no queda nada en `~/.cache/ohmlatl/build` ni como `.partial` en el disco tras éxito o fallo; `image resize` crece y encoge una imagen detenida, mantiene el archivo disperso y se niega con la VM corriendo o por debajo del mínimo del filesystem.

### P2: Runtime

- `run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- `launch`, supervisor como unidad de sistema, socket de control, PTY raw, log del serial, detach/attach, limpieza.
- Jailer, TAP `/30`, red cerrada, `internet: true|false`, relay de DNS.

**Listo cuando:** `shell` entra con la contraseña de la imagen, pedida una vez por arranque, y `exec`/`cp` reutilizan la conexión; la VM sobrevive al logout; el guest no alcanza el host, la LAN ni otra VM, y la LAN no alcanza el guest; el DNS funciona en un host con `systemd-resolved` y después de un cambio de red; `stop`, `poweroff` dentro del guest y el apagado del host terminan la VM limpiamente y dejan el disco listo para desmontar; `kill -9` sobre el supervisor no deja TAP, regla ni mount atrás; el disco arranca en un segundo host, y un UID distinto allí se reporta con la corrección.

### P3: Forwards y varias VMs

- `forward` hacia `host:<puerto>` y `vm/<nombre>:<puerto>`.
- Varias VMs a la vez, de uno o más usuarios; tabla nft compartida.
- `doctor --run` con Docker, ufw y firewalld.

**Listo cuando:** una VM de agente con `internet: false` alcanza el Ollama del host (escuchando solo en loopback) y un servidor MCP en otra VM, y nada más; un servicio del host en `0.0.0.0` con el mismo puerto sigue funcionando; dos VMs corren a la vez sin colisión; detener una VM no rompe la red de la otra.

### Futuro

Sin plan detallado: comandos de backup/restore, `image import/export`, forwards UDP, `gc`, `--json`, completions, aarch64.

## Pruebas

**Unitarias**

- Parser y validación del manifest (nombres, tags, UID/GID, puertos, forwards, puerto 53).
- Descubrimiento del directorio de ohmlatl (`--home`, `OHMLATL_HOME`, ubicación del binario).
- Generación de la cmdline.
- Cálculo y alineación del tamaño.
- Asignación de subnet contra rutas y registros existentes.
- Reglas nft por VM.

**Integración**

- Build con Podman → ext4 con dueños, xattrs y capabilities, en el disco.
- `--from` con una imagen Kali personalizada; rechazo de nombres remotos y cortos.
- Fallo claro para una imagen sin systemd o sshd; rechazo de una tag existente; aviso sin contraseña ni clave.
- Una passphrase sin salto de línea abre tanto en el build como al arrancar.
- `image resize` hacia arriba y hacia abajo; rechazo con la VM corriendo y por debajo del mínimo; el archivo sigue disperso.
- Initramfs con `cryptsetup` en `ttyS0`; reintento; passphrase incorrecta cae al rescue.
- Jailer + TAP + PTY raw bajo `systemd-run`, con `KillMode=mixed`.
- Ctrl-Alt-Del, `poweroff` y `systemctl stop` terminan Firecracker limpiamente.
- `kill -9` sobre Firecracker + `run` + `e2fsck`; `kill -9` sobre el supervisor + limpieza.
- `shell`/`exec`/`cp` por SSH con `ForwardAgent=yes` en `~/.ssh/config` (no debe reenviar); una pregunta de contraseña por arranque.
- Red: guest bloqueado hacia host, LAN y otras VMs; origen falsificado descartado; LAN bloqueada hacia el gateway; otro usuario local bloqueado hacia el guest; forwards y DNS funcionan; `internet: false` corta la salida y el DNS falla al instante.
- Disco: dueño incorrecto y montaje `noexec` se rechazan con la corrección.

**E2E**

- Primer ejemplo: `init`, base Kali, `podman run`, instalar x/y/z, `passwd`, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → doctor → build → run → shell → stop → run`.
- Migración a una tag nueva con `cp`.
- Quitar el disco después del `stop`, conectarlo a otro host y arrancarlo allí.
- VM de agente + VM de MCP + Ollama en el host.

**Matriz**

- Intel y AMD; 8/16/32 GiB de RAM.
- Pendrive y SSD externo, ext4 y btrfs.
- Hosts con y sin Docker/ufw/firewalld; con y sin `systemd-resolved`.
- e2fsprogs 1.45 y el más nuevo disponible en el host.
- CPU más débil de la flota (base de CPU).

## Riesgos

| Riesgo | Mitigación |
|---|---|
| KVM no disponible/anidado | `doctor` falla temprano con instrucciones |
| La config de kernel de Firecracker está hecha para kernels de Amazon Linux | `check-kconfig.sh` y la prueba de arranque de P0 |
| Sin `sudo` en un host prestado | Requisito documentado; `doctor --run` lo verifica |
| Disco quitado con una VM corriendo | `stop` antes de desmontar, documentado; `e2fsck` al arrancar |
| UID distinto en otro host | Detectado; se imprime `sudo chown -R` |
| Disco montado con `noexec` | `doctor` avisa; `ohmlatl` del host con `--home` |
| Prompt de `cryptsetup` en el initramfs | Prueba en P0 con PTY raw; pruebas de echo y reintento |
| Bind mounts del jailer perdidos después del `pivot_root` | Prueba en P0; alternativa: copiar kernel e initramfs al jail |
| Dueños incorrectos en el build | Pipeline completo en `podman unshare`; pruebas de aceptación |
| Features de ext4 desconocidas por el `e2fsck` embebido | `mke2fs.conf` fijo; matriz con e2fsprogs antiguo y nuevo |
| Reloj del guest incorrecto sin internet | `ptp_kvm` + `chrony` con `refclock PHC`; prueba en P0 |
| Rootfs lleno | 32 GiB dispersos por defecto; `--size`; `image resize` |
| Disco lleno | Documentado; `doctor` verifica espacio libre |
| Build muerto a medias | `.partial` con lock; el próximo build o `doctor --build` lo limpia |
| Docker/ufw/firewalld descartan el tráfico | `doctor --run` lo detecta e imprime la regla |
| Gestores de red en el guest sobrescribiendo `ip=` | NetworkManager unmanaged en `eth0`; networkd y resolved enmascarados en el build |
| Imagen sin contraseña ni clave | El build avisa; consola con autologin para ejecutar `passwd` |
| Kali puro sin systemd/sshd | `build` valida y apunta a `examples/kali` |
| Commit de contenedor opaco | Documentado como exploración; el Containerfile es la fuente durable |
| Binario grande (~100 MB con Podman) | Aceptado; se extrae una vez por arranque del host |

## Elementos a confirmar durante la implementación

- Fragmento final de la config del kernel: confirmado en las pruebas de arranque de P0, sin cambios respecto a la lista de `TECHNICAL.md`.

## Criterio de MVP

El MVP es P0 a P2: un disco preparado, un host Linux con KVM, una imagen generada con Podman, un arranque con la passphrase en la consola de la VM, shell por SSH, jailer activo, red cerrada por defecto, un apagado limpio y el mismo disco arrancando en un segundo host. P3 agrega los forwards y varias VMs trabajando juntas.
