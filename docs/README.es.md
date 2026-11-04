<h1 align="center">ohmlatl</h1>

<p align="center">MicroVMs Firecracker portátiles, reproducibles y cifradas.</p>

<p align="center">Pronunciación: <strong>ohm-lal</strong>.</p>

<p align="center">
  🇺🇸 <a href="README.en.md">English</a> ·
  🇧🇷 <a href="README.pt-BR.md">Português</a> ·
  🇪🇸 Español
</p>

<br>

Formateas la computadora, cambias de máquina o te sientas frente a un host prestado. Todo el entorno (sistema, herramientas, datos y configuración) debería caber en el bolsillo, no estar repartido entre `~/.config`, `/usr/local`, `/var/lib` y una docena de lugares que olvidarás copiar.

`ohmlatl` existe para eso: convierte un disco externo en una computadora completa que corre como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). Firecracker arranca en unos 125 ms y agrega menos de 5 MiB de overhead de VMM por microVM. La RAM del guest es la que configures. Eso permite llevar una computadora completa en un disco sin convertir el host en un hypervisor pesado y sin depender de la distribución del host.

El mismo aislamiento sirve para lo que no quieres ejecutar en el host: una herramienta de origen dudoso, un proyecto clonado de internet, un PoC, un `npm install` o `make` de terceros. Ejecútalo dentro de la VM; el host queda limpio.

## ¿Qué es ohmlatl?

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Usa el Podman embebido **solo en el build** para convertir un `Containerfile` o una imagen OCI en un rootfs ext4. En runtime no hay Podman, daemon ni contenedor: hay un kernel, un rootfs, discos virtio y un proceso Firecracker.

La imagen de disco es un único `rootfs.luks`: LUKS2 con integridad por sector (`hmac-sha256`), que contiene el ext4 con todo el sistema, incluido `/home/<user>`. Lo que deba sobrevivir a un rebuild va en un volumen: un disco ext4 extra, cifrado o no, montado donde diga el manifest (por ejemplo `/home/dev/work`).

El binario `ohmlatl` embebe Firecracker, jailer, Podman, el kernel del guest y el initramfs. Nada ejecutable queda suelto en el disco salvo el propio `ohmlatl`. El initramfs pide la passphrase en la consola de la VM. Ninguna clave LUKS queda en el disco.

Aquí, `imagen de disco` es el artefacto que Firecracker arranca. `Imagen OCI` es la fuente que usa Podman en el build.

`ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos y servicios son tu responsabilidad. Conoce imágenes, volúmenes, VMs y redes.

## ¿Por qué usarlo?

**Porque el host deja de importar.** El disco lleva las imágenes y los volúmenes. El host de destino solo necesita Linux, `/dev/kvm`, `cryptsetup`, red, utilidades de TAP/nftables, cliente SSH y `sudo`. Cambiar de distribución o de máquina no cambia el entorno.

**Porque el aislamiento es real.** Los contenedores comparten el kernel del host. Una microVM tiene kernel propio, seccomp y, con el jailer, chroot, namespaces y cgroups.

**Porque puedes ejecutar lo que no es confiable.** La herramienta o el proyecto corre dentro de la VM, bajo su propio kernel, con acceso por `ohmlatl shell` y `exec`. No hay directorio compartido por donde el código alcance el host; solo sale lo que copies con `ohmlatl cp`. Para empezar de cero, vuelve a generar la imagen con `ohmlatl build`. El guest tiene red con NAT y alcanza el gateway del host, así que los servicios del host que escuchan en esa IP quedan expuestos a él.

**Porque la manipulación se detecta.** El kernel y el initramfs vienen del binario confiable `ohmlatl`, no del disco. El rootfs tiene integridad por sector: un sector alterado falla al leerse en lugar de ejecutar código alterado. Verifica el propio `ohmlatl` contra el SHA-256 o la firma del release.

**Porque los datos están cifrados en reposo, sin clave en el disco.** El rootfs siempre es LUKS2; los volúmenes son LUKS2 cuando lo pides. Las passphrases se escriben en el `ttyS0` de la VM y solo existen en memoria.

**Porque la imagen es reproducible.** `Containerfile` más manifest describen el sistema. El build usa Podman rootless dentro de `podman unshare`, así que dueños, xattrs y capabilities quedan correctos dentro del ext4.

**Porque el ciclo de vida es pequeño.** Imagen, volumen, VM. `build`, `run`, `shell`, `stop`. Sin orquestador, sin daemon global. El supervisor existe por VM y termina con ella.

**Porque no hay directorio del host dentro de la VM.** Sin bind mounts. El acceso es por `ohmlatl shell`, `exec` y `cp` vía SSH.

## Cómo usarlo

El flujo completo, con resultados esperados, está en [USAGE.es.md](USAGE.es.md). El plan y las fases están en [ROADMAP.es.md](ROADMAP.es.md). Resumen:

**1. Prepara el disco y revisa el host.**

```bash
ohmlatl init /mnt/vmdisk
ohmlatl doctor --build
ohmlatl doctor --run
```

**2. Construye la imagen base.**

```bash
cd examples/kali
ohmlatl build
```

`ohmlatl build` lee el `Containerfile` declarado en `ohmlatl.yaml` y usa el Podman embebido. El ejemplo usa `kalilinux/kali-rolling`, pero la base puede ser cualquier imagen con systemd y sshd. El build pide la passphrase del rootfs y deja la imagen Podman `kali-base:2026-11-02` lista para el siguiente paso.

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

**5. Crea un volumen y arranca la VM.**

```bash
ohmlatl volume create work --size 8G --luks
ohmlatl run kali-custom:2026-11-02
```

`ohmlatl run` adjunta el serial; el initramfs pide la passphrase del rootfs, la prueba en `work` y solo vuelve a preguntar si falla. `Ctrl-]` desacopla; la VM sigue corriendo.

```bash
ohmlatl shell kali-custom      # shell vía SSH
ohmlatl console kali-custom    # volver al serial
ohmlatl stop kali-custom       # apagado limpio
```

**6. Lee un volumen en el host.** Con la VM detenida:

```bash
ohmlatl volume mount work /mnt/work
```

El montaje siempre es de solo lectura. `ohmlatl` no publica puertos ni hace proxy; el host llega a la IP del guest, y el guest llega al host por el gateway (por ejemplo `172.31.0.1:8080`).

## Principios

- El disco es la computadora; el host es el bootloader.
- Ninguna clave LUKS en el disco. La passphrase existe en el teclado y en la memoria.
- Rootfs descartable, volúmenes persistentes.
- Un dueño por disco. Firecracker no bloquea archivos; `ohmlatl` sí.
- Sin daemon global, sin estado persistente en el host, sin bind mount de directorios del host.
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

Pre-alfa. El repositorio todavía es el scaffold de Rust; los documentos describen el contrato de uso y el plan. Comandos y detalles pueden cambiar durante P0 a P2.

## Licencia

Aún no definida.
