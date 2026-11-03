# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 Español

Este documento propone los flujos de uso de `ohmlatl` antes de la implementación. Es el contrato de experiencia: comandos, prompts y resultados esperados. La implementación puede ajustar detalles, pero no debe cambiar la historia sin actualizar este archivo.

## Conceptos

- **Imagen de disco**: un `rootfs.luks` (LUKS2 + integridad + ext4) más el `manifest.yaml` y la clave SSH usados en el build. Contiene todo el sistema, incluido `/home/<user>`. Se construye a partir de un `Containerfile` o de una imagen OCI.
- **Volumen**: un disco ext4 extra, con o sin LUKS2. El manifest dice dónde lo monta la VM. Lo que deba sobrevivir a un rebuild vive en un volumen.
- **VM**: una ejecución de una imagen, con volúmenes, recursos y red. Hay una VM por imagen, con el nombre de la imagen.
- **Manifest**: `ohmlatl.yaml`. Declara el nombre de la imagen, el usuario, los recursos y los volúmenes.
- **Supervisor**: proceso por VM que controla Firecracker, el PTY del serial y el log. No es un daemon global.
- **Jailer**: el sandbox de Firecracker (chroot, namespaces, seccomp, cgroups). Requiere root para iniciar.

## Instalación (fase inicial)

Mientras no haya release:

```bash
cargo build --release
install -m 0755 target/release/ohmlatl ~/.local/bin/ohmlatl
ohmlatl --version
```

Resultado esperado:

```text
ohmlatl 0.1.0
```

## 1. Prepara el disco

```bash
ohmlatl init /mnt/vmdisk
```

El comando crea el layout y valida el filesystem. El disco debe ser ext4, btrfs o xfs, montado sin `noexec`; FAT/exFAT/NTFS no soportan discos dispersos.

Resultado esperado:

```text
[ohmlatl] store: /mnt/vmdisk
[ohmlatl] created bin/ images/ volumes/ vms/
[ohmlatl] filesystem: ext4 (ok)
```

## 2. Revisa el host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` revisa Podman rootless, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs` y `cryptsetup` con integridad. `--run` revisa `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `sudo` y el cliente SSH.

Resultado esperado (resumido):

```text
BUILD
  [ok] podman rootless
  [ok] subuid/subgid (65536 ids)
  [ok] newuidmap setuid
  [ok] e2fsprogs 1.47
  [ok] cryptsetup 2.7 (integrity)

RUN
  [ok] /dev/kvm accessible
  [ok] /dev/net/tun
  [ok] iproute2 + nftables
  [ok] cgroup v2
  [ok] ssh client
```

Si algo falla, el comando muestra la corrección sugerida y sale con código distinto de cero.

## 3. Primer ejemplo: imagen de disco personalizada

El flujo que la mayoría querrá: correr un contenedor base, instalar x, y, z y convertir el resultado en una microVM. El ejemplo usa Kali Rolling; cambia la base si quieres.

### 3.1 Base del guest

El `kalilinux/kali-rolling` puro no tiene systemd ni sshd. Usa la base del repositorio:

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` lee el `Containerfile` declarado en `build.containerfile` y usa el Podman embebido. Produce la imagen de disco `kali-base:2026-11-02` y deja la imagen Podman con el mismo nombre para el siguiente paso. `name` y `build.tag` identifican el artefacto; sin tag, `latest`; `-t` lo sobrescribe.

### 3.2 Explora e instala en el contenedor

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # si hace falta

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Reemplaza `x`, `y` y `z` por los paquetes que quieras.

### 3.3 Genera la imagen de disco arrancable

En el directorio del proyecto, con el `ohmlatl.yaml` del ejemplo:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

El build exporta el rootfs dentro de `podman unshare`, genera la clave SSH, calcula el tamaño y pide la passphrase del rootfs:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 3.2 GiB (content + 25% + 512 MiB; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 + integrity (hmac-sha256)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: images/kali-custom/2026-11-02/rootfs.luks
  size:   3.2 GiB
```

`--size 8G` define el tamaño. El build puede pedir `sudo` para abrir el device-mapper de LUKS; `cryptsetup` lee la passphrase del TTY.

### 3.4 Fíjalo en un Containerfile

El commit es un snapshot del filesystem. Cuando las herramientas se estabilicen, pasa los comandos a un `Containerfile` para tener un rebuild reproducible:

```text
kali-custom/
├── Containerfile
└── ohmlatl.yaml
```

`ohmlatl.yaml`:

```yaml
format: 1
name: kali-custom

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

`Containerfile`:

```dockerfile
FROM kali-base:2026-11-02

RUN apt-get update && apt-get install -y --no-install-recommends x y z \
    && rm -rf /var/lib/apt/lists/*
```

Build:

```bash
ohmlatl build
```

El build falla con un mensaje claro si la imagen no tiene systemd, sshd o el `user.name` con el UID/GID declarado.

## 4. Crea volúmenes

Volumen cifrado:

```bash
ohmlatl volume create work --size 8G --luks
```

Resultado esperado:

```text
Volume passphrase:
Repeat:
[ohmlatl] volume work
  file: volumes/work.luks
  fs:   ext4
  size: 8 GiB
```

`--luks` puede pedir `sudo` para el device-mapper. La passphrase se lee del TTY, nunca de un flag o del entorno. Si usas la passphrase del rootfs, el arranque pregunta una sola vez.

Volumen sin LUKS:

```bash
ohmlatl volume create data --size 8G
```

```text
[ohmlatl] volume data
  file: volumes/data.ext4
  fs:   ext4
  size: 8 GiB
```

El volumen no necesita conocer el UID: en el primer montaje, la VM entrega la raíz del volumen a `user.uid:gid`.

## 5. Primer arranque

```bash
ohmlatl run kali-custom:2026-11-02
```

El comando crea la VM `kali-custom`, la red y el supervisor, y adjunta el serial. Los volúmenes y puntos de montaje vienen del manifest.

Resultado esperado:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2 + integrity)
[ohmlatl] disk: work (LUKS2) -> /home/dev/work
[ohmlatl] network: ohmlatl-0 172.31.0.1/30, guest 172.31.0.2
[ohmlatl] starting firecracker (jailer)
[ohmlatl] serial attached; Ctrl-] to detach
...
LUKS passphrase for rootfs:
```

El initramfs prueba la misma passphrase en cada volumen LUKS. Si falla:

```text
LUKS passphrase for work:
```

Después del arranque:

```text
kali-custom login:
```

`Ctrl-]` desacopla la terminal; la VM sigue corriendo bajo el supervisor. `--vcpus` y `--memory` sobrescriben el manifest. Ninguna passphrase en argv, entorno, manifest o log.

## 6. Accede a la VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

El shell usa SSH por la red de la VM con la clave generada en el build. No hay directorio del host dentro de la VM; el acceso siempre es por aquí.

```bash
ohmlatl exec kali-custom -- uname -a
```

```text
Linux kali-custom 6.x ...
```

```bash
ohmlatl cp kali-custom:/home/dev/work/report.txt ./report.txt
ohmlatl cp ./input.txt kali-custom:/home/dev/work/input.txt
```

Consola serial (rescate o volver a adjuntar):

```bash
ohmlatl console kali-custom
```

Si otra consola está adjunta:

```text
[ohmlatl] console busy: another client is attached
```

Logs:

```bash
ohmlatl logs kali-custom -f
```

`logs` muestra la salida del serial.

## 7. Estado y ciclo de vida

```bash
ohmlatl ps
```

```text
NAME          IMAGE                   STATUS    PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  running   1234   172.31.0.2    2m
```

```bash
ohmlatl stop kali-custom
```

```text
[ohmlatl] stopping kali-custom
[ohmlatl] stopped
```

`stop` envía Ctrl-Alt-Del al guest; systemd apaga de forma limpia y Firecracker termina. Si no termina en 30 s, `ohmlatl` mata el proceso.

```bash
ohmlatl start kali-custom
```

`start` usa la última imagen y vuelve a pedir las passphrases en el serial.

```bash
ohmlatl rm kali-custom
```

Elimina el estado de la VM. Imágenes y volúmenes se quedan; usa `ohmlatl volume rm` para volúmenes.

## 8. Lee un volumen en el host

Con la VM detenida:

```bash
ohmlatl volume mount work /mnt/work
```

```text
Volume passphrase:
[ohmlatl] mounted volumes/work.luks at /mnt/work (read-only)
```

```bash
ohmlatl volume umount work
```

El montaje en el host siempre es de solo lectura y se rechaza con la VM corriendo. Mientras está montado, `run` y `start` rechazan ese volumen.

Si la VM debe llegar a un servicio local del host, este tiene que escuchar en el gateway de la VM, por ejemplo `172.31.0.1:8080`. `ohmlatl` no publica puertos.

## 9. Actualiza el rootfs

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

`run` sobre una VM detenida la cambia a la nueva imagen. Los volúmenes no se tocan; `/home/dev` vuelve al skeleton de la imagen, así que guarda lo importante en volúmenes. La nueva imagen trae host keys SSH nuevas, y `ohmlatl` reinicia el `known_hosts` de la VM. Si la VM está corriendo, `run` falla.

## 10. Varias VMs

```bash
ohmlatl run kali-lab:2026-11-02
ohmlatl ps
```

Cada imagen es una VM propia con su `/30`, asignado automáticamente (`--subnet` lo sobrescribe). Un volumen solo puede estar adjunto a una VM a la vez; el segundo intento falla con lock.

## 11. Fallas y recuperación

**Passphrase del rootfs incorrecta.** `cryptsetup` reintenta; tras tres fallas cae al rescue shell del initramfs:

```text
(initramfs) #
```

Usa `ohmlatl console kali-custom` para adjuntarte.

**Passphrase de volumen incorrecta.** Tres intentos, luego rescue shell.

**Rootfs manipulado.** La verificación de integridad convierte el sector alterado en error de lectura; `e2fsck` o el montaje falla y el arranque cae al rescue. Reconstruye la imagen.

**Firecracker murió.** `ohmlatl ps` muestra `stopped`; `ohmlatl start kali-custom` la arranca de nuevo. El initramfs corre `e2fsck -p` antes de montar.

**El host se reinició.** Las VMs no arrancan solas; `ohmlatl ps` y `ohmlatl start`.

**Disco retirado con la VM corriendo.** No lo hagas. Si el disco desaparece, el siguiente arranque corre `e2fsck`.

**Passphrase perdida.** El rootfs se puede reconstruir. Un volumen LUKS se pierde. No hay clave LUKS en el disco.

## 12. Fuera de alcance

- Sin publicación de puertos, ingress ni proxy público. El host llega al guest; el acceso externo es tu responsabilidad.
- Sin bind mount de directorios del host. La VM solo ve discos gestionados por `ohmlatl`.
- Sin GPU. La VM corre solo en CPU.
- Sin macOS/Windows. Firecracker requiere Linux con KVM.
- Sin gestión de herramientas, procesos, plugins o flujos de tareas. Eso es contenido de la imagen.

## 13. Contrato de UX

- Las passphrases se escriben en el TTY de la VM al arrancar y en el TTY del host al crear (`build`/`volume create`/`volume mount`). Nunca en flag, env, manifest, archivo o log.
- Los comandos destructivos piden confirmación.
- Los errores dicen qué falló y qué hacer; los códigos de salida son estables.
- Sin daemon global. El supervisor es por VM y termina con ella.
- Todo el estado persistente vive en el disco.
