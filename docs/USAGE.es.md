# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 [Português](USAGE.pt-BR.md) · 🇪🇸 Español

Este documento propone los flujos de uso de `ohmlatl` antes de la implementación. Es el contrato de experiencia: comandos, prompts y resultados esperados. La implementación puede ajustar detalles, pero no debe cambiar la historia sin actualizar este archivo.

## Conceptos

- **Imagen de disco**: un `rootfs.luks` (LUKS2 + integrity + ext4) más el `manifest.yaml` y las claves SSH creadas con él. Guarda toda la computadora, incluido `/home/<user>`, y la VM escribe en él. Se genera a partir de un `Containerfile` o de una imagen OCI.
- **VM**: una ejecución de una imagen, con recursos y red. Hay una VM por imagen, con el nombre de la imagen.
- **Manifest**: `ohmlatl.yaml`. Declara el nombre de la imagen, el usuario, los recursos y la red.
- **Forward**: un puerto TCP en el gateway de la VM que lleva a un servicio del host o a otra VM. Es la única salida de la VM además de internet, cuando está permitida.
- **Supervisor**: una unidad de sistema por VM que controla Firecracker, el PTY del serial, la red y los forwards. No es un daemon global.
- **Jailer**: el sandbox de Firecracker (chroot, namespaces, seccomp, cgroups).

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

Las imágenes y el estado de las VMs quedan en `~/.local/share/ohmlatl`.

## 1. Verificar el host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` verifica Podman rootless, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs` y `cryptsetup` con integrity. `--run` verifica `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, el cliente SSH y si Docker o ufw bloquean el forwarding.

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
  [ok] systemd-run
  [ok] ssh client
  [ok] forward policy
```

Si algo falla, el comando imprime la corrección sugerida y sale con código distinto de cero.

## 2. Primer ejemplo: imagen de disco personalizada

El flujo que la mayoría querrá: ejecutar un contenedor base, instalar x, y, z y convertir el resultado en una microVM. El ejemplo usa Kali Rolling; cambia la base si quieres.

### 2.1 Base del guest

El `kalilinux/kali-rolling` puro no tiene systemd ni sshd. Usa la base del repositorio:

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` lee el `Containerfile` declarado en `build.containerfile` y usa el Podman embebido. Produce la imagen de disco `kali-base:2026-11-02` y deja la imagen Podman con el mismo nombre para el siguiente paso. `name` y `build.tag` identifican el artefacto; sin tag, `latest`; `-t` sobrescribe.

### 2.2 Explorar e instalar en el contenedor

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

### 2.3 Generar la imagen de disco arrancable

En el directorio del proyecto, con el `ohmlatl.yaml` del ejemplo:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

`--from` solo usa imágenes locales; las imágenes remotas necesitan nombre completo, como `docker.io/...`. `-t` define el nombre y la tag del resultado, y el build los graba en el `manifest.yaml` de la imagen. El build exporta el rootfs dentro de `podman unshare`, genera las claves SSH, calcula el tamaño y pide la passphrase del rootfs:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 3.2 GiB (content + 25% + 512 MiB; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 + integrity (hmac-sha256)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: ~/.local/share/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
  size:   3.2 GiB
```

`--size 8G` define el tamaño. El build pide `sudo` una vez para abrir el device-mapper de LUKS. Los archivos temporales quedan en `~/.cache/ohmlatl/build` y se borran al final.

### 2.4 Fijarlo en un Containerfile

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

El build falla con un mensaje claro si la imagen no tiene systemd, sshd o el `user.name` con el UID/GID declarados.

## 3. Primer arranque

```bash
ohmlatl run kali-custom:2026-11-02
```

El comando pide `sudo` una vez, inicia el supervisor de la VM `kali-custom`, crea la red y adjunta el serial.

Resultado esperado:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2 + integrity)
[ohmlatl] network: ohmlatl-0 172.31.0.1/30, guest 172.31.0.2, internet on
[ohmlatl] forwards: none
[ohmlatl] starting firecracker (jailer)
[ohmlatl] serial attached; Ctrl-] to detach
...
LUKS passphrase for rootfs:
```

Después del arranque:

```text
kali-custom login:
```

`Ctrl-]` desconecta la terminal; la VM sigue corriendo bajo el supervisor, incluso después de cerrar sesión. `--vcpus` y `--memory` sobrescriben el manifest. Ninguna passphrase en argv, entorno, manifest o log.

## 4. Acceder a la VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

El shell usa SSH por la red de la VM con la clave y la host key creadas en el build. Ignora tu `~/.ssh/config` y nunca reenvía tu agente SSH. No hay directorio del host dentro de la VM; el acceso siempre es por aquí.

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

`logs` muestra la salida del serial. El log vive en `/run/ohmlatl` y desaparece cuando el host reinicia.

## 5. Red

Por defecto la VM alcanza internet y nada más: ni el host, ni la LAN, ni otras VMs. El host alcanza la VM por su IP (`172.31.0.2`).

Para cortar internet y abrir solo servicios específicos, usa `vm.network` en el `manifest.yaml` de la imagen (o en `ohmlatl.yaml` antes del build). Ejemplo: un agente que solo puede usar el Ollama del host y un servidor MCP en otra VM.

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
[ohmlatl] network: ohmlatl-1 172.31.0.5/30, guest 172.31.0.6, internet off
[ohmlatl] forwards:
  172.31.0.5:11434 -> host 127.0.0.1:11434
  172.31.0.5:8080  -> vm mcp:8080
```

Dentro de la VM el gateway es la única dirección que responde:

```bash
curl http://172.31.0.5:11434/api/tags
```

- `to: host:<puerto>` llega a `127.0.0.1:<puerto>` del host, así que Ollama puede seguir escuchando solo en loopback.
- `to: vm/<nombre>:<puerto>` llega a otra VM de `ohmlatl`. Si está detenida, la conexión se cierra.
- Solo TCP. `run` siempre imprime los forwards abiertos.
- Cambiar `vm.network` en el `manifest.yaml` de la imagen vale en el próximo `start`; sin rebuild.

## 6. Estado y ciclo de vida

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

`stop` envía Ctrl-Alt-Del al guest; systemd se apaga limpiamente y Firecracker termina. Si no termina en 30 s, `ohmlatl` lo mata. Dentro de la VM, `poweroff` y `reboot` también terminan la VM.

```bash
ohmlatl start kali-custom
```

`start` usa la última imagen y vuelve a pedir la passphrase en el serial.

```bash
ohmlatl rm kali-custom
```

Elimina el estado de la VM. La imagen permanece; usa `ohmlatl image rm` para ella.

## 7. Actualizar: desde dentro o rebuild

El rootfs es tu computadora y guarda todo lo que haces en él. Actualízalo desde dentro:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

Un rebuild crea una **computadora nueva y vacía** a partir del `Containerfile`:

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

`run` sobre una VM detenida cambia la imagen. Nada del rootfs anterior pasa al nuevo: copia antes lo que importa con `ohmlatl cp`. La imagen anterior queda hasta `ohmlatl image rm kali-custom:2026-11-02`. Si la VM está corriendo, `run` falla.

## 8. Llevarla a otro host

La computadora es el directorio de la imagen:

```bash
rsync -a ~/.local/share/ohmlatl/images/kali-custom/ otherhost:.local/share/ohmlatl/images/kali-custom/
```

En el otro host, con `ohmlatl` instalado:

```bash
ohmlatl run kali-custom:2026-11-02
```

El `rootfs.luks` ocupa su tamaño completo en disco (el integrity escribe todos los sectores). Detén la VM antes de copiar. La caché de Podman queda en el host original: para hacer rebuild en el host nuevo, toda la cadena de build (`kali-base`, luego `kali-custom`) vuelve a correr allí.

## 9. Varias VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Cada imagen es su propia VM con su propio `/30`, asignado automáticamente. Las VMs no se ven entre sí; solo un forward `vm/<nombre>` las conecta.

## 10. Fallos y recuperación

**Passphrase del rootfs incorrecta.** `cryptsetup` reintenta; después de tres fallos cae al rescue shell del initramfs:

```text
(initramfs) #
```

Usa `ohmlatl console kali-custom` para adjuntarte.

**Rootfs manipulado.** La verificación de integridad convierte el sector alterado en error de lectura; `e2fsck` o el montaje falla y el arranque cae al rescue. Restaura una copia o vuelve a generar la imagen.

**Firecracker murió.** `ohmlatl ps` muestra `stopped`; `ohmlatl start kali-custom` vuelve a arrancarla. El initramfs ejecuta `e2fsck -p` antes de montar.

**El host reinició.** Las VMs no arrancan solas; `ohmlatl ps` y `ohmlatl start`.

**Sin red dentro de la VM en un host con Docker o ufw.** `ohmlatl doctor --run` muestra la regla que deja pasar `ohmlatl-*`.

**Passphrase perdida.** El rootfs no se puede abrir. No hay clave LUKS en disco; vuelve a generar la imagen y empieza de cero.

## 11. No objetivos

- Sin publicación de puertos, ingress ni proxy público. Los forwards solo salen de la VM; el host alcanza la IP del guest; el acceso externo es tu responsabilidad.
- Sin bind mount de directorios del host. Los archivos entran y salen con `ohmlatl cp`.
- Sin discos extra ni volúmenes. La computadora es el rootfs.
- Sin GPU ni pantalla. La VM se usa desde la terminal.
- Sin macOS/Windows. Firecracker requiere Linux con KVM.
- Sin gestión de herramientas, procesos, plugins ni flujos de tareas. Eso es contenido de la imagen.

## 12. Contrato de UX

- Las passphrases se escriben en la TTY del host en `build` y en la TTY de la VM al arrancar. Nunca en flag, env, manifest, archivo o log.
- Los comandos destructivos piden confirmación.
- Los errores dicen qué falló y qué hacer; los códigos de salida son estables.
- Sin daemon global. El supervisor es por VM y termina con ella.
- Toda apertura de red se declara en el manifest y `run` la imprime.
