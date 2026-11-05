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

`ohmlatl` existe para eso: toda la computadora es un único archivo cifrado, `rootfs.luks`, que vive en tu pendrive o disco externo junto al binario `ohmlatl` y arranca como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). Conecta el disco a otra máquina y arráncala allí. Firecracker arranca en unos 125 ms y agrega menos de 5 MiB de overhead de VMM por microVM. La RAM del guest es la que configures. Eso permite llevar una computadora completa sin convertir el host en un hypervisor pesado y sin depender de la distribución del host.

El mismo aislamiento sirve para lo que no quieres ejecutar en el host: una herramienta de origen dudoso, un proyecto clonado de internet, un PoC, un `npm install` o `make` de terceros, un agente. Genera una imagen separada para eso y ejecútala como su propia VM: alcanza internet, pero no el host, la LAN ni otras VMs, salvo que abras un puerto específico.

## ¿Qué es ohmlatl?

`ohmlatl` gestiona imágenes de disco arrancables y ejecuciones de microVMs Firecracker. Usa el Podman embebido **solo en el build** para convertir un `Containerfile` o una imagen OCI en un rootfs ext4. En runtime no hay Podman ni contenedor: hay un kernel, un rootfs, un disco virtio y un proceso Firecracker.

La imagen de disco es un `rootfs.luks`: LUKS2 con el ext4 de todo el sistema, incluido `/home/<user>`, junto a un `manifest.yaml`. Es un archivo disperso: solo ocupa en el disco lo que ya se escribió. La VM escribe directamente en él: es tu computadora, no una plantilla. La actualizas desde dentro (`apt upgrade`), y los datos salen con `ohmlatl cp`.

El binario `ohmlatl` embebe Firecracker, jailer, Podman, el kernel del guest y el initramfs. El disco guarda solo el binario y las imágenes. El initramfs pide la passphrase en la consola de la VM. Ninguna clave LUKS ni clave SSH queda en el disco.

Aquí, `imagen de disco` es el artefacto que Firecracker arranca. `Imagen OCI` es la fuente que usa Podman en el build.

`ohmlatl` no gestiona el contenido de la imagen: herramientas, IDEs, datos, servicios y la forma de entrar por SSH son tu responsabilidad. Conoce imágenes, VMs y redes.

## ¿Por qué usarlo?

**Porque el host deja de importar.** La computadora viaja en el disco. El host solo necesita Linux, `/dev/kvm`, nftables, systemd, cliente SSH y `sudo` para ti. No queda nada persistente en él. Cambiar de distribución o de máquina no cambia el entorno. Un host prestado aún tiene que ser de confianza: ve la passphrase que escribes y la memoria de la VM.

**Porque el aislamiento es real.** Los contenedores comparten el kernel del host. Una microVM tiene kernel propio, seccomp y, con el jailer, chroot, namespaces y cgroups.

**Porque puedes ejecutar lo que no es confiable.** La herramienta, el proyecto o el agente corre en su propia VM, bajo su propio kernel, con acceso por `ohmlatl shell` y `exec`. No hay directorio compartido por donde el código alcance el host; solo sale lo que copies con `ohmlatl cp`. El guest no alcanza el host, la LAN ni otras VMs. Abres exactamente lo que necesita, por ejemplo el Ollama de tu host y un servidor MCP en otra VM, y nada más. Mantenlo lejos de la imagen de tu computadora personal; para empezar limpio, haz rebuild.

**Porque un disco perdido no sirve de nada sin la passphrase.** El rootfs siempre es LUKS2, y ninguna clave queda en el disco. La passphrase se escribe en la terminal del host cuando se genera la imagen y en el `ttyS0` de la VM al arrancar, y solo existe en memoria. El kernel y el initramfs vienen del binario `ohmlatl`, no de la imagen de disco. Alterar el archivo cifrado sin la clave no inyecta código: se convierte en basura, y el arranque o el programa se rompe.

**Porque el disco solo ocupa lo que usas.** El `rootfs.luks` es un archivo disperso: tiene un tamaño de disco, lo que la VM ve (32 GiB por defecto), y un espacio ocupado en el disco externo mucho menor, solo lo que realmente se escribió. Las partes nunca escritas son huecos que el filesystem no guarda. Una computadora de 32 GiB con 3 GiB de sistema ocupa unos 3 GiB en el pendrive, y solo crece a medida que escribes. Si la VM necesita más espacio (o debe ver menos), `ohmlatl image resize kali-custom:2026-11-02 64G` cambia el tamaño que ve, sin rebuild.

**Porque la imagen es declarativa.** `Containerfile` más manifest describen el sistema. El build usa Podman rootless dentro de `podman unshare`, así que dueños, xattrs y capabilities quedan correctos dentro del ext4.

**Porque el ciclo de vida es pequeño.** Imagen, VM. `build`, `run`, `shell`, `stop`. Sin orquestador, sin daemon global, sin estado de VM en el disco. El supervisor existe por VM y termina con ella.

## Cómo usarlo

El flujo completo, con resultados esperados, está en [USAGE.es.md](USAGE.es.md). El plan y las fases están en [ROADMAP.es.md](ROADMAP.es.md). Resumen:

**1. Prepara el disco.** Formateado en ext4 (o btrfs/xfs):

```bash
./ohmlatl init /media/$USER/drive/ohmlatl
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --build
ohmlatl doctor --run
```

`init` copia el binario al disco (pidiendo `sudo` una vez si la raíz del disco es de root). Desde entonces, el `ohmlatl` del disco encuentra sus imágenes solo.

**2. Genera el contenedor base.**

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

El ejemplo usa `docker.io/kalilinux/kali-rolling`, pero la base puede ser cualquier imagen con systemd y sshd. Este paso solo crea una imagen Podman; sin disco y sin passphrase.

**3. Instala lo que quieras en un contenedor.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
passwd dev           # contraseña para ohmlatl shell
apt-get clean
exit
```

Reemplaza `x`, `y` y `z` por los paquetes que quieras. Para entrar con clave en vez de contraseña, pon tu clave pública en `/home/dev/.ssh/authorized_keys`.

**4. Convierte el contenedor en una computadora en el disco.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

El build pide la passphrase del rootfs. El disco por defecto tiene 32 GiB (o el doble del contenido más 1 GiB, lo que sea mayor); `--size 64G` lo cambia. El archivo es disperso, así que el disco solo paga por lo que se escriba. El commit es exploración; cuando las herramientas se estabilicen, pasa los comandos al `Containerfile`.

**5. Arranca la VM, aquí o en cualquier otro host.**

```bash
ohmlatl run kali-custom:2026-11-02
```

`ohmlatl run` pide `sudo` una vez y adjunta el serial; el initramfs pide la passphrase del rootfs. `Ctrl-]` se desconecta; la VM sigue corriendo, incluso después de cerrar sesión.

```bash
ohmlatl shell kali-custom      # shell por SSH (contraseña una vez por arranque)
ohmlatl console kali-custom    # vuelve al serial
ohmlatl stop kali-custom       # apagado limpio; siempre antes de quitar el disco
```

**6. Abre solo lo que la VM necesita.** En el `manifest.yaml` de la imagen (`images/<nombre>/<tag>/` en el disco), o en `ohmlatl.yaml` antes del build:

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

Dentro de la VM, `ohmlatl-gateway:11434` es el Ollama del host y `ohmlatl-gateway:8080` es el servidor MCP. Nada más es alcanzable. El cambio vale en el próximo `run`.

## Principios

- El disco es la computadora; el host es el bootloader, y confías en él.
- Ninguna clave LUKS ni clave SSH en el disco. La passphrase existe en el teclado y en memoria.
- El guest no alcanza el host, la LAN ni otras VMs sin declararlo.
- Sin daemon global, sin bind mount de directorios del host, nada persistente en el host.
- La imagen es declarativa; el runtime es pequeño y explícito.

## Desarrollo

Firecracker y el jailer están en `vendor/`. Para desarrollo, `vendor/bin/podman` ejecuta el bundle de Podman que `cargo xtask assets` genera en `assets/out/podman`:

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
```

Versiones, checksums y límites están en [vendor/README.md](../vendor/README.md). `cargo xtask assets` genera en contenedores el kernel y el initramfs del guest (Buildroot) y el bundle de Podman embebido (receta de podman-static), con las fuentes exactas de cada componente GPL; necesita `podman` en el host. El release ya trae todo eso dentro del binario.

## Estado

Pre-alfa. P0, P1 y P2 están listos: `init`, `doctor`, los binarios embebidos, el build de imágenes, `image` y el runtime (`run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`, la red cerrada y el relay de DNS). P3 también está listo: forwards y varias VMs trabajando juntas. Comandos y detalles todavía pueden cambiar.

## Licencia

`ohmlatl` tiene licencia doble [MIT](../LICENSE-MIT) o [Apache-2.0](../LICENSE-APACHE), a tu elección. El binario embebe programas de terceros (Firecracker, Podman, el kernel Linux, BusyBox, cryptsetup y otros) que mantienen sus propias licencias, algunas GPL. `ohmlatl licenses` los lista con sus textos completos, y cada release publica el código fuente exacto de los componentes GPL junto al binario.
