<h1 align="center">ohmlatl</h1>

<p align="center">MicroVMs Firecracker portátiles, declarativas y cifradas.</p>

<p align="center">Pronunciación: <strong>ohm-lal</strong>.</p>

<p align="center">
  🇺🇸 <a href="README.en.md">English</a> ·
  🇧🇷 <a href="README.pt-BR.md">Português</a> ·
  🇪🇸 Español
</p>

<br>

Formateas la computadora, cambias de máquina o te sientas frente a un host prestado. Todo el entorno (sistema, herramientas, datos y configuración) debería caber en el bolsillo, no estar repartido entre `~/.config`, `/usr/local`, `/var/lib` y una docena de lugares que olvidarás copiar.

`ohmlatl` existe para eso: toda la computadora es un único archivo cifrado, `rootfs.luks`, que arranca como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). Copia el archivo a otro host y arráncalo allí. Firecracker arranca en unos 125 ms y agrega menos de 5 MiB de overhead de VMM por microVM. La RAM del guest es la que configures. Eso permite llevar una computadora completa sin convertir el host en un hypervisor pesado y sin depender de la distribución del host.

El mismo aislamiento sirve para lo que no quieres ejecutar en el host: una herramienta de origen dudoso, un proyecto clonado de internet, un PoC, un `npm install` o `make` de terceros, un agente. Ejecútalo dentro de la VM; por defecto no alcanza el host, la LAN ni otras VMs, solo lo que permitas.

## ¿Qué es ohmlatl?

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Usa el Podman embebido **solo en el build** para convertir un `Containerfile` o una imagen OCI en un rootfs ext4. En runtime no hay Podman ni contenedor: hay un kernel, un rootfs, un disco virtio y un proceso Firecracker.

La imagen de disco es un único `rootfs.luks`: LUKS2 con integridad por sector (`hmac-sha256`), que contiene el ext4 con todo el sistema, incluido `/home/<user>`. La VM escribe directamente en él: es tu computadora, no una plantilla. La actualizas desde dentro (`apt upgrade`), y los datos salen con `ohmlatl cp`.

El binario `ohmlatl` embebe Firecracker, jailer, Podman, el kernel del guest y el initramfs. El initramfs pide la passphrase en la consola de la VM. Ninguna clave LUKS queda en disco.

Aquí, `imagen de disco` es el artefacto que Firecracker arranca. `Imagen OCI` es la fuente que usa Podman en el build.

`ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos y servicios son tu responsabilidad. Conoce imágenes, VMs y redes.

## ¿Por qué usarlo?

**Porque el host deja de importar.** La computadora es un archivo. El host de destino solo necesita Linux, `/dev/kvm`, red, utilidades de TAP/nftables, `systemd-run`, cliente SSH y `sudo`. Cambiar de distribución o de máquina no cambia el entorno.

**Porque el aislamiento es real.** Los contenedores comparten el kernel del host. Una microVM tiene kernel propio, seccomp y, con el jailer, chroot, namespaces y cgroups.

**Porque puedes ejecutar lo que no es confiable.** La herramienta, el proyecto o el agente corre dentro de la VM, bajo su propio kernel, con acceso por `ohmlatl shell` y `exec`. No hay directorio compartido por donde el código alcance el host; solo sale lo que copies con `ohmlatl cp`. Por defecto el guest no alcanza el host, la LAN ni otras VMs. Abres exactamente lo que necesita, por ejemplo el Ollama de tu host y un servidor MCP en otra VM, y nada más.

**Porque la manipulación se detecta.** El kernel y el initramfs vienen del binario confiable `ohmlatl`, no de la imagen de disco. El rootfs tiene integridad por sector: un sector alterado falla al leerse en lugar de ejecutar código alterado.

**Porque los datos están cifrados en reposo, sin clave en disco.** El rootfs siempre es LUKS2. La passphrase se escribe en la terminal del host cuando se genera la imagen y en el `ttyS0` de la VM al arrancar, y solo existe en memoria.

**Porque la imagen es declarativa.** `Containerfile` más manifest describen el sistema. El build usa Podman rootless dentro de `podman unshare`, así que dueños, xattrs y capabilities quedan correctos dentro del ext4.

**Porque el ciclo de vida es pequeño.** Imagen, VM. `build`, `run`, `shell`, `stop`. Sin orquestador, sin daemon global. El supervisor existe por VM y termina con ella.

## Cómo usarlo

El flujo completo, con resultados esperados, está en [USAGE.es.md](USAGE.es.md). El plan y las fases están en [ROADMAP.es.md](ROADMAP.es.md). Resumen:

**1. Verifica el host.**

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

**2. Genera la imagen base.**

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` lee el `Containerfile` declarado en `ohmlatl.yaml` y usa el Podman embebido. El ejemplo usa `docker.io/kalilinux/kali-rolling`, pero la base puede ser cualquier imagen con systemd y sshd. El build pide la passphrase del rootfs y deja la imagen Podman `kali-base:2026-11-02` lista para el siguiente paso.

**3. Instala lo que quieras en un contenedor.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
apt-get clean
exit
```

Reemplaza `x`, `y` y `z` por los paquetes que quieras.

**4. Convierte el contenedor en imagen de disco.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

El tamaño por defecto es el contenido más 25% y 512 MiB; `--size 8G` lo cambia. El commit es exploración; cuando las herramientas se estabilicen, pasa los comandos al `Containerfile`.

**5. Arranca la VM.**

```bash
ohmlatl run kali-custom:2026-11-02
```

`ohmlatl run` adjunta el serial; el initramfs pide la passphrase del rootfs. `Ctrl-]` se desconecta; la VM sigue corriendo.

```bash
ohmlatl shell kali-custom      # shell por SSH
ohmlatl console kali-custom    # vuelve al serial
ohmlatl stop kali-custom       # apagado limpio
```

**6. Abre solo lo que la VM necesita.** En `ohmlatl.yaml`:

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434      # Ollama del host
      - port: 8080
        to: vm/mcp:8080     # servidor MCP en la VM "mcp"
```

Dentro de la VM, `172.31.0.1:11434` es el Ollama del host y `172.31.0.1:8080` es el servidor MCP. Nada más es alcanzable.

## Principios

- El rootfs es la computadora; el host es el bootloader.
- Ninguna clave LUKS en disco. La passphrase existe en el teclado y en memoria.
- Host, LAN y otras VMs cerrados por defecto; toda apertura se declara.
- Sin daemon global, sin bind mount de directorios del host.
- La imagen es declarativa; el runtime es pequeño y explícito.

## Desarrollo

Los binarios locales están en `vendor/`. Son la fuente de los binarios embebidos:

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
```

Versiones, checksums y límites están en [vendor/README.md](../vendor/README.md).

## Estado

Pre-alfa. El repositorio todavía es el scaffold Rust; los documentos describen el contrato de usabilidad y el plan. Comandos y detalles pueden cambiar durante P0 a P2.

## Licencia

Aún no definida.
