# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 Español

Este documento propone los flujos de uso de `ohmlatl` antes de la implementación. Es el contrato de experiencia: comandos, prompts y resultados esperados. La implementación puede ajustar detalles, pero no debe cambiar la historia sin actualizar este archivo.

## Conceptos

- **Directorio de ohmlatl**: un directorio en tu pendrive o disco externo, creado por `ohmlatl init`. Guarda el binario `ohmlatl` e `images/`. Nada más es persistente, ni en el disco ni en el host.
- **Imagen de disco**: `images/<nombre>/<tag>/`, con un `rootfs.luks` (LUKS2 + ext4) y el `manifest.yaml`. Guarda toda la computadora, incluido `/home/<user>`, y la VM escribe en él. Se genera a partir de un `Containerfile` o de una imagen OCI.
- **VM**: una ejecución de una imagen, con recursos y red. Corre como máximo una VM por nombre de imagen, con ese nombre.
- **Archivo disperso**: el `rootfs.luks` tiene un tamaño de disco, lo que la VM ve, y un espacio ocupado, lo que realmente se guarda en el disco externo. Las partes nunca escritas son huecos que no ocupan espacio. Un disco de 32 GiB con 3 GiB escritos ocupa unos 3 GiB. `image ls` muestra los dos; `image resize` cambia el primero.
- **Manifest**: `ohmlatl.yaml`. Declara el nombre de la imagen, el usuario, los recursos y la red.
- **Forward**: un puerto TCP en el gateway de la VM (`ohmlatl-gateway`) que lleva a un servicio del host o a otra VM. Además de internet, cuando está permitida, es la única salida de la VM.
- **Supervisor**: una unidad de sistema por VM que controla Firecracker, el serial, la red y los forwards. No es un daemon global.
- **Jailer**: el sandbox de Firecracker (chroot, namespaces, seccomp, cgroups).

## Instalación (fase inicial)

Mientras no haya release:

```bash
cargo xtask assets          # kernel + initramfs con Buildroot; la primera vez tarda de 40 a 60 minutos
cargo build --release
./target/release/ohmlatl --version
```

Resultado esperado:

```text
ohmlatl 0.1.0
```

## 1. Preparar el disco

Formatea el disco en ext4 (btrfs y xfs también funcionan; FAT y exFAT no), móntalo y ejecuta:

```bash
./target/release/ohmlatl init /media/$USER/drive/ohmlatl
export PATH="/media/$USER/drive/ohmlatl:$PATH"
```

Resultado esperado:

```text
[ohmlatl] directory: /media/dev/drive/ohmlatl
[ohmlatl] filesystem: ext4 (ok)
[ohmlatl] mount: exec allowed (ok)
[ohmlatl] owner: uid 1000 (ok)
[ohmlatl] copied ohmlatl binary
```

Si la raíz del disco no es escribible por ti (un ext4 recién formateado es de root), `init` pide `sudo` una vez para crear el directorio por ti. El disco queda así:

```text
/media/dev/drive/ohmlatl/
├── .ohmlatl      # 0644
├── ohmlatl       # 0755
└── images/       # 0700, tus computadoras
```

El `ohmlatl` del disco encuentra `images/` solo. Para usar un `ohmlatl` instalado en el host, pasa `--home /media/$USER/drive/ohmlatl` (o define `OHMLATL_HOME`).

## 2. Verificar el host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` verifica Podman rootless (al menos 65536 subuids y subgids, `newuidmap`, `/dev/fuse`), `e2fsprogs`, `cryptsetup`, `ssh-keygen`, `udevadm`, `sudo` y espacio libre. `--run` verifica `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, el cliente SSH y si Docker, ufw o firewalld bloquean el tráfico de las VMs. Los dos revisan el directorio de ohmlatl: filesystem, montaje, dueño.

Resultado esperado (resumido):

```text
DRIVE
  [ok] /media/dev/drive/ohmlatl (ext4, exec, uid 1000)

BUILD
  [ok] podman rootless
  [ok] subuid/subgid (65536 ids)
  [ok] newuidmap setuid
  [ok] e2fsprogs 1.47
  [ok] cryptsetup 2.7

RUN
  [ok] /dev/kvm accessible
  [ok] /dev/net/tun
  [ok] iproute2 + nftables
  [ok] systemd-run
  [ok] ssh client
  [ok] no foreign firewall drop
```

Si algo falla, el comando imprime la corrección sugerida y sale con código distinto de cero.

## 3. Primer ejemplo: imagen de disco personalizada

El flujo que la mayoría querrá: ejecutar un contenedor base, instalar x, y, z y convertir el resultado en una microVM en el disco. El ejemplo usa Kali Rolling; cambia la base si quieres.

### 3.1 Base del guest

El `kalilinux/kali-rolling` puro no tiene systemd ni sshd. Genera la base del repositorio como imagen Podman:

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

Este paso solo crea la imagen Podman `kali-base:2026-11-02`, en la caché de Podman del host. Sin disco y sin passphrase.

### 3.2 Explorar e instalar en el contenedor

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # si hace falta
passwd dev                              # contraseña para ohmlatl shell

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Reemplaza `x`, `y` y `z` por los paquetes que quieras. Para entrar con clave, pon tu clave pública en `/home/dev/.ssh/authorized_keys` (dueño `dev`, modo 0600). Nunca definas la contraseña con `chpasswd` en un `Containerfile`: queda en claro en las capas de la imagen.

### 3.3 Generar la imagen de disco arrancable

Todavía en `examples/kali`, donde está `ohmlatl.yaml`:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

`build` necesita un `ohmlatl.yaml` en el directorio actual. `--from` solo usa imágenes locales; para partir de una remota, ejecuta antes `ohmlatl podman pull docker.io/...`. `-t` define el nombre y la tag del resultado, y el build los graba en el `manifest.yaml` de la imagen. Una tag que ya existe se rechaza: un build nunca sobrescribe una computadora.

El build pide la passphrase del rootfs y `sudo` una vez:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 32 GiB, sparse (default; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 (aes-xts-plain64)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: /media/dev/drive/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
  size:   32 GiB (2.9 GiB on disk)
```

Si el usuario no tiene ni contraseña ni `authorized_keys`, el build avisa que `ohmlatl shell` no podrá entrar; la consola sigue funcionando. También avisa cuando la imagen no tiene `sftp-server`, que `ohmlatl cp` necesita.

El tamaño por defecto es 32 GiB o el doble del contenido más 1 GiB, lo que sea mayor; `--size 64G` lo cambia. El archivo es disperso: el disco solo paga por lo que se escriba. El rootfs exportado queda en `~/.cache/ohmlatl/build` en el host durante el build y se borra al final, incluso ante un fallo.

### 3.4 Fijarlo en un Containerfile

El commit es un snapshot del sistema de archivos. Cuando las herramientas se estabilicen, pasa los comandos a un `Containerfile` para tener un rebuild repetible:

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
  tag: 2026-11-10

user:
  name: dev
  uid: 1000
  gid: 1000

vm:
  vcpus: 2
  memory: 4GiB
  network:
    internet: true
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

Esto crea `kali-custom:2026-11-10`, una computadora nueva junto a `kali-custom:2026-11-02`. Define la contraseña después, desde la consola: `ohmlatl console`, luego `sudo passwd dev`. El build falla con un mensaje claro si la imagen no tiene systemd, sshd o el `user.name` con el UID/GID declarados y su home.

## 4. Primer arranque

```bash
ohmlatl run kali-custom:2026-11-02
```

El comando pide `sudo` una vez, inicia el supervisor de la VM `kali-custom`, crea la red y adjunta el serial.

Resultado esperado:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2)
[ohmlatl] internet: on
[ohmlatl] forwards: none
[ohmlatl] starting firecracker (jailer)
[ohmlatl] network: ohm0 172.31.0.1/30, guest 172.31.0.2
[ohmlatl] serial attached; Ctrl-] to detach
...
Enter passphrase for /dev/vda:
```

Después del arranque, el serial inicia sesión solo:

```text
dev@kali-custom:~$
```

`Ctrl-]` desconecta la terminal; la VM sigue corriendo bajo el supervisor, incluso después de cerrar sesión. `--vcpus` y `--memory` sobrescriben el manifest solo en esta ejecución. Ninguna passphrase en argv, entorno, manifest o log. El mismo `run` la vuelve a arrancar la próxima vez.

## 5. Acceder a la VM

```bash
ohmlatl shell kali-custom
```

```text
dev@172.31.0.2's password:
dev@kali-custom:~$
```

El shell usa SSH por la red de la VM, con el login que tenga la imagen: la contraseña de `dev`, o tu clave, si está en `authorized_keys`. La contraseña se pide una vez por arranque: `exec` y `cp` reutilizan la misma conexión. `ohmlatl` ignora tu `~/.ssh/config`, nunca reenvía tu agente SSH y no guarda `known_hosts` (el enlace con el guest es un TAP punto a punto local). No hay directorio del host dentro de la VM; el acceso siempre es por aquí.

```bash
ohmlatl exec kali-custom -- uname -a
```

```text
Linux kali-custom 6.x ...
```

```bash
ohmlatl cp kali-custom:/home/dev/report.txt ./report.txt
ohmlatl cp ./input.txt kali-custom:/home/dev/input.txt
```

Consola serial (rescate o volver a adjuntar; inicia sesión sola y muestra primero la salida más reciente):

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

`logs` muestra el último 1 MiB de la salida del serial. El log vive en `/run/ohmlatl` en el host, dura hasta el próximo `run` y desaparece cuando el host reinicia.

## 6. Red

Por defecto la VM alcanza internet y nada más: ni el host, ni la LAN, ni otras VMs. Nada de la LAN llega a la VM. El host alcanza la VM por su IP; `ohmlatl shell`, `exec` y `cp` lo hacen por ti.

Para cortar internet y abrir solo servicios específicos, edita `vm.network` en el `manifest.yaml` de la imagen en el disco (o en `ohmlatl.yaml` antes del build). Ejemplo: un agente que solo puede usar el Ollama del host y un servidor MCP en otra VM.

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434
      - port: 8080
        to: vm/mcp:8080
```

```bash
ohmlatl run agent:2026-11-02
```

```text
[ohmlatl] internet: off
[ohmlatl] forwards:
  ohmlatl-gateway:11434 -> host 127.0.0.1:11434
  ohmlatl-gateway:8080  -> vm mcp:8080
[ohmlatl] starting firecracker (jailer)
[ohmlatl] network: ohm1 172.31.0.5/30, guest 172.31.0.6
```

Dentro de la VM el gateway es la única dirección que responde:

```bash
curl http://ohmlatl-gateway:11434/api/tags
```

- `ohmlatl-gateway` se escribe en el `/etc/hosts` del guest en cada arranque. Usa el nombre, no la IP: la subnet puede cambiar entre un `run` y otro.
- `to: host:<puerto>` llega a `127.0.0.1:<puerto>` del host, así que Ollama puede seguir escuchando solo en loopback. El forward no ocupa el puerto en el host.
- `to: vm/<nombre>:<puerto>` llega a otra VM tuya. Si está detenida, la conexión se cierra.
- Con `internet: true` la VM resuelve nombres vía `ohmlatl-gateway`, que reenvía al resolver del host. Con `internet: false` las consultas fallan al instante.
- Solo TCP. `run` siempre imprime los forwards abiertos. Los cambios valen en el próximo `run`; sin rebuild.

## 7. Estado y ciclo de vida

```bash
ohmlatl ps
```

```text
NAME          IMAGE                   PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  1234   172.31.0.2    2m
```

`ps` muestra solo las VMs en ejecución. Lo que existe en el disco está en `ohmlatl image ls`:

```text
NAME          TAG          SIZE     ON DISK
kali-custom   2026-11-02   32 GiB   2.9 GiB
kali-custom   2026-11-10   32 GiB   3.1 GiB
```

```bash
ohmlatl stop kali-custom
```

```text
[ohmlatl] stopping kali-custom
[ohmlatl] stopped
```

`stop` envía Ctrl-Alt-Del al guest; systemd se apaga limpiamente y Firecracker termina. Si no termina en 120 s, `ohmlatl` lo mata. Dentro de la VM, `poweroff` y `reboot` también terminan la VM. Cuando el host se apaga, las VMs en ejecución reciben el mismo apagado limpio.

Para arrancarla de nuevo, el mismo comando:

```bash
ohmlatl run kali-custom:2026-11-02
```

Para cambiar el tamaño del disco que ve la VM, con la VM detenida:

```bash
ohmlatl image resize kali-custom:2026-11-02 64G
```

```text
Rootfs passphrase:
[ohmlatl] kali-custom:2026-11-02: 32 GiB -> 64 GiB (2.9 GiB on disk)
[ohmlatl] e2fsck + resize2fs
[ohmlatl] done
```

Crecer es rápido y mantiene el archivo disperso: el espacio nuevo solo ocupa el disco externo cuando la VM escribe en él. Encoger también funciona, hasta lo que guarda el filesystem; pide confirmación, y conviene hacer backup antes, porque un corte de luz en medio puede dañar el filesystem. `resize` pide `sudo` y la passphrase, y se niega con la VM corriendo.

Eliminar una imagen pide confirmación y se rechaza mientras su VM esté corriendo:

```bash
ohmlatl image rm kali-custom:2026-11-02
```

Para cambiar la passphrase, con la VM detenida:

```bash
cryptsetup luksChangeKey --pbkdf argon2id --pbkdf-memory 262144 \
  --pbkdf-parallel 1 --pbkdf-force-iterations 4 \
  /media/$USER/drive/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
```

## 8. Actualizar: desde dentro o rebuild

El rootfs es tu computadora y guarda todo lo que haces en él. Actualízalo desde dentro:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

Un rebuild crea una **computadora nueva y vacía** con una tag nueva. Para llevar tus datos:

```bash
ohmlatl cp kali-custom:/home/dev/work ./work      # VM antigua corriendo
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-11-10
ohmlatl cp ./work kali-custom:/home/dev/work
```

Los datos pasan por el disco del host, en el que confías. La imagen anterior queda hasta `ohmlatl image rm kali-custom:2026-11-02`.

## 9. En otra máquina

Detén las VMs, desmonta el disco, conéctalo a la otra máquina y ejecuta:

```bash
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --run
ohmlatl run kali-custom:2026-11-02
```

- El host tiene que ser Linux x86_64 (kernel 5.10 o más nuevo) con KVM, necesitas `sudo` en él y tienes que confiar en él, aunque sea prestado: ve la passphrase que escribes y la memoria de la VM.
- El binario en el disco es tan confiable como el disco. En un host nuevo, compáralo con el release antes de ejecutarlo, o usa un `ohmlatl` instalado en el host con `--home`:

  ```bash
  # SHA256SUMS y SHA256SUMS.sig vienen de la página del release; allowed_signers, de docs/ en el repositorio
  ssh-keygen -Y verify -f allowed_signers -I badlucysec -n file -s SHA256SUMS.sig < SHA256SUMS
  grep ohmlatl-0.1.0-x86_64 SHA256SUMS
  sha256sum /media/$USER/drive/ohmlatl/ohmlatl
  ```
- Si tu UID en ese host es distinto del dueño del directorio de ohmlatl, `ohmlatl` se detiene y muestra el `sudo chown -R` a ejecutar.
- La configuración de red viaja en el `manifest.yaml` de cada imagen y se aplica tal cual; `run` imprime los forwards para que los revises.
- No queda nada persistente en el host. La caché de Podman queda en el host donde hiciste el build: para hacer rebuild en otro lugar, toda la cadena de build (`kali-base`, luego `kali-custom`) vuelve a correr allí.

Backup de una computadora, con la VM detenida, manteniendo el archivo disperso:

```bash
rsync -aS /media/$USER/drive/ohmlatl/images/kali-custom/ /backup/ohmlatl/images/kali-custom/
```

El destino del backup también tiene que ser ext4, btrfs o xfs.

## 10. Varias VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Cada imagen es su propia VM con su propio `/30`, asignado automáticamente. Las VMs no se ven entre sí; solo un forward `vm/<nombre>` las conecta. Mantén el trabajo no confiable en su propia imagen, nunca en la imagen de tu computadora personal.

## 11. Fallos y recuperación

**Passphrase del rootfs incorrecta.** `cryptsetup` reintenta; después de tres fallos cae al rescue shell del initramfs:

```text
(initramfs) #
```

Usa `ohmlatl console kali-custom` para adjuntarte.

**Disco quitado con una VM corriendo.** La VM recibe errores de I/O y su disco puede quedar dañado. Vuelve a conectar el disco (`ohmlatl stop` funciona incluso antes, y aunque el disco vuelva en otra ruta), luego `ohmlatl run`: el initramfs ejecuta `e2fsck -p` antes de montar. Siempre `stop` antes de quitar el disco.

**Rootfs manipulado.** Sin la clave, un cambio en `rootfs.luks` se convierte en basura dentro de la VM: el arranque o `e2fsck` falla, o un programa se rompe. Restaura un backup o vuelve a generar la imagen.

**Firecracker murió.** `ohmlatl ps` ya no muestra la VM; `ohmlatl run kali-custom:2026-11-02` la vuelve a arrancar.

**El host reinició.** Las VMs no arrancan solas; `ohmlatl image ls` y `ohmlatl run`.

**Disco lleno.** El archivo disperso no puede ocupar más espacio y la VM recibe errores de I/O, aunque el disco que ve todavía tenga lugar. Libera espacio en el disco y vuelve a ejecutar la VM.

**Disco lleno dentro de la VM.** `ohmlatl stop`, luego `ohmlatl image resize <nombre>:<tag> <tamaño mayor>`, luego `ohmlatl run`.

**Dueño incorrecto o montaje `noexec`.** Con el dueño incorrecto, `ohmlatl` se niega y muestra el `sudo chown -R` a ejecutar. Con `noexec`, el binario del disco no arranca: vuelve a montar sin `noexec`, o usa un `ohmlatl` instalado en el host con `--home`.

**Sin red dentro de la VM en un host con Docker, ufw o firewalld.** `ohmlatl doctor --run` muestra la regla que deja pasar `ohm*`.

**El host ya usa `172.31.0.0/16`.** `run` se niega a arrancar cuando una dirección o ruta del host cae en el pool de las VMs; define otro, por ejemplo `OHMLATL_POOL=10.213.0.0/16`. Haz lo mismo cuando una red remota en ese rango se alcanza por la ruta por defecto (una VPC de AWS, por ejemplo).

**`run` falla antes del arranque.** `run` imprime el error del supervisor; si no hay, muestra el comando `sudo journalctl -u ohmlatl-<uid>-<nombre>`.

**`ohmlatl shell` pide una contraseña que no existe.** La imagen no tiene contraseña ni clave para el usuario. Usa `ohmlatl console` (login automático), ejecuta `sudo passwd <usuario>` y vuelve a intentar. Una imagen sin `sudo` para el usuario necesita la contraseña definida con `ohmlatl podman run`, antes del build.

**Passphrase perdida.** El rootfs no se puede abrir. No hay clave LUKS en el disco; vuelve a generar la imagen y empieza de cero.

## 12. No objetivos

- Sin publicación de puertos, ingress ni proxy público. Los forwards solo salen de la VM; el acceso externo es tu responsabilidad.
- Sin bind mount de directorios del host. Los archivos entran y salen con `ohmlatl cp`.
- Sin discos extra ni volúmenes. La computadora es el rootfs.
- Sin gestión de claves SSH. El login es contenido de la imagen.
- Sin GPU ni pantalla. La VM se usa desde la terminal.
- Sin módulos de kernel en el guest. El kernel ya trae lo que un sistema de uso general necesita (nftables, tun, overlayfs, namespaces, wireguard, fuse).
- Sin macOS/Windows. Firecracker requiere Linux con KVM.
- Sin gestión de herramientas, procesos, plugins ni flujos de tareas. Eso es contenido de la imagen.

## 13. Contrato de UX

- Las passphrases se escriben en la TTY del host en `build` y en la TTY de la VM al arrancar. Nunca en flag, env, manifest, archivo o log.
- Los comandos destructivos piden confirmación; `build` nunca sobrescribe una tag.
- Los errores dicen qué falló y qué hacer; los códigos de salida son estables.
- Sin daemon global. El supervisor es por VM y termina con ella.
- Nada persistente fuera del disco; nada en el disco además del binario y las imágenes.
- Toda apertura de red se declara en el manifest y `run` la imprime.
