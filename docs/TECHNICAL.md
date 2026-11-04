# TECHNICAL

Detalhes técnicos do `ohmlatl`. Este documento é a fonte de verdade para artefatos, build, boot, rede e ciclo de vida. README, USAGE e ROADMAP descrevem a experiência; se divergirem deste arquivo, este arquivo vence e os outros devem ser corrigidos. Documento interno, só em pt-BR.

## Escopo do MVP

- Linux x86_64 com KVM.
- O computador vive num pendrive ou HD externo e viaja com ele. No drive ficam só o binário `ohmlatl` e as imagens.
- Firecracker, jailer, bundle Podman, kernel e initramfs embutidos no binário `ohmlatl`.
- Cada computador é um `rootfs.luks` (LUKS2 `aes-xts-plain64` + ext4, arquivo esparso) com o sistema inteiro, incluindo `/home/<user>`, e o `manifest.yaml` ao lado. Sem discos extras.
- Guest com systemd. Sem daemon de guest, sem init próprio, sem vsock.
- Rede por TAP: o guest não alcança host, LAN nem outras VMs; internet opcional; forwards TCP declarados no manifest; relay de DNS no gateway.
- Acesso por SSH na rede TAP, com a autenticação que a imagem tiver (senha ou chave). O `ohmlatl` não gera nem guarda chaves.
- Uma VM rodando por nome de imagem e por usuário.
- Host confiável, mesmo quando emprestado: ele vê a passphrase digitada, a RAM da VM, o console e os dados copiados com `cp`. Você precisa de `sudo` nele.

## Diretório do ohmlatl (no drive)

```text
/media/<user>/<drive>/ohmlatl/       # criado por `ohmlatl init`, 0755, do operador
├── .ohmlatl                         # marcador: "format: 1", 0644
├── ohmlatl                          # o binário, 0755
└── images/                          # 0700, do operador
    └── <nome>/
        ├── <tag>/
        │   ├── rootfs.luks          # LUKS2 + ext4, esparso, 0600
        │   └── manifest.yaml        # manifest usado no build, com name/tag finais, 0600
        ├── .<tag>.lock              # objeto do flock do build; pode sobrar, vazio
        └── .<tag>.partial/          # só durante um build
```

Como a CLI acha o diretório, em ordem:

1. `--home <dir>`;
2. `OHMLATL_HOME`;
3. o diretório do executável (`/proc/self/exe`), se tiver o marcador `.ohmlatl`.

Sem nenhum dos três, a CLI falha e indica `ohmlatl init` ou `--home`. Com o binário do drive no `PATH`, a terceira regra resolve sozinha. Só `init`, `build`, `image` e `run` precisam do diretório. `stop`, `ps`, `console`, `logs`, `shell`, `exec` e `cp` trabalham só com `/run/ohmlatl/<uid>/`, então funcionam com o drive fora ou montado em outro caminho.

Regras do diretório:

- Filesystem ext4, btrfs ou xfs. FAT e exFAT não guardam arquivos esparsos nem permissões, e o FAT limita cada arquivo a 4 GiB; `init` e `doctor` recusam.
- Montado sem `noexec`, para o binário rodar do drive. O automount do desktop monta ext4 com `nosuid,nodev`, o que não atrapalha. Com `noexec`, `init` e `doctor` só avisam: o binário do drive não executa, mas um `ohmlatl` do host com `--home` funciona.
- O diretório e o binário são 0755 para o `ohmlatl` rodar mesmo num host em que você tem outro UID; o que é privado fica em `images/` (0700, arquivos 0600). O dono de `<dir>`, do binário e de `images/` tem que ser o UID atual. Se não for, a CLI para e mostra `sudo chown -R $(id -u):$(id -g) <dir>`. Antes de qualquer `sudo`, a CLI também recusa se `<exe>`, `<dir>` ou algum diretório acima deles for gravável por alguém que não seja root nem o UID atual: senão outro usuário do host poderia trocar o binário que vai rodar como root.
- Tirar o drive com uma VM rodando corrompe o disco dela. `stop` antes de desmontar; o `e2fsck` do próximo boot tenta consertar.

`ohmlatl init <dir>`: cria o diretório (0755), o marcador, `images/` (0700) e copia o próprio binário para `<dir>/ohmlatl`, e confere filesystem, `noexec` (aviso) e dono. Um ext4 recém-formatado tem a raiz `root:root 0755`, então, se o pai de `<dir>` não for gravável pelo operador, o `init` cria `<dir>` com `sudo install -d -m 0755 -o <uid> -g <gid>`. A cópia do binário é atômica (arquivo temporário em `<dir>`, `fsync`, `rename()`); se a origem já é `<dir>/ohmlatl` (mesmo inode), não copia. Num diretório que já tem o marcador, só atualiza o binário.

Fora do drive, no host, ficam só coisas temporárias:

```text
/run/ohmlatl/                      # tmpfs próprio do ohmlatl (ver Run), root, 0755
├── bin/<sha256>/                  # binários extraídos (root 0755) e kernel/initramfs (0644)
├── jail/                          # chroot do jailer, root
├── net/<uid>-<nome>               # registro de rede da VM (TAP, subnet), root 0600
└── <uid>/<nome>/                  # 0700, do operador
    ├── ctl.sock                   # status e stop, 0600
    ├── console.sock               # serial, um cliente por vez, 0600
    ├── ssh.sock                   # conexão SSH mestre (ControlMaster), do operador
    ├── serial.log                 # saída do serial, buffer circular de 1 MiB
    ├── ip                         # IP do guest, para forwards vm/<nome>
    └── error                      # falha do supervisor, lida pela CLI

$XDG_CACHE_HOME/ohmlatl/           # padrão: ~/.cache/ohmlatl; só no host onde se faz build
├── bin/<sha256>/                  # só no build: mke2fs.conf, bin/{podman,crun,fuse-overlayfs,fusermount3,pasta,pasta.avx2},
│                                  #   lib/podman/{conmon,catatonit,netavark,aardvark-dns,rootlessport}, etc/containers/policy.json
├── conf/                          # containers.conf, storage.conf e registries.conf gerados
├── podman/                        # storage do Podman: cache, guarda sistemas em claro
└── build/<id>/                    # rootfs exportado durante o build, apagado no fim e na falha
```

- O cache do Podman guarda as imagens de container em claro no host do build. `ohmlatl podman system reset` limpa.
- Backup de um computador: com a VM parada, `rsync -aS` (ou `cp -a --sparse=always`) do diretório `images/<nome>/<tag>/` para outro filesystem ext4/btrfs/xfs.

## Binários embutidos

O `ohmlatl` embute com `include_bytes!`:

- Firecracker e jailer (`vendor/firecracker`).
- Bundle Podman, compilado por `cargo xtask assets` com a receita do podman-static v6.1.4 (ver Geração dos assets): `podman`, `crun`, `conmon`, `netavark`, `aardvark-dns`, `rootlessport`, `pasta`, `pasta.avx2` (o `pasta` reexecuta essa variante em CPUs com AVX2), `fuse-overlayfs`, `fusermount3`, `catatonit`, `etc/containers/policy.json`. O `runc` e o `passt` avulso não entram.
- Kernel `vmlinux`, `initramfs.cpio.gz` e `mke2fs.conf`, gerados no repositório.

O build extrai o Podman para `$XDG_CACHE_HOME/ohmlatl/bin/<sha256>/`. O `launch` (root) extrai Firecracker, jailer, kernel, initramfs e uma cópia do próprio `ohmlatl` para `/run/ohmlatl/bin/<sha256>/`. A extração é atômica: escreve num diretório temporário ao lado e faz `rename()` para `<sha256>/`; se o destino já existe, reaproveita. O hash é do binário `ohmlatl`, então versões diferentes não se misturam e duas execuções simultâneas nunca veem uma extração pela metade. Como a unit roda a cópia em `/run`, nada da VM depende do binário no drive depois que ela sobe.

O `ohmlatl` gera em runtime a configuração do Podman (a mesma que o `vendor/bin/podman` de desenvolvimento gera para o bundle de `assets/out/podman/`, a partir de `vendor/podman/*.conf.in`):

- `containers.conf` com todos os caminhos apontando para o bundle extraído (`<b>` = `$XDG_CACHE_HOME/ohmlatl/bin/<sha256>`, com um layout próprio, o do podman-static sem o prefixo `usr/local`: `<b>/bin/` e `<b>/lib/podman/`; no podman-static, `pasta` e `pasta.avx2` são symlinks para `passt` e `passt.avx2`, e aqui entram como arquivos reais, copiados desreferenciados), para nada do host ser usado no lugar: `runtime = "crun"` com `[engine.runtimes] crun = ["<b>/bin/crun"]`, `conmon_path = ["<b>/lib/podman/conmon"]`, `helper_binaries_dir = ["<b>/lib/podman", "<b>/bin"]` (netavark, aardvark-dns e rootlessport no primeiro; `pasta` e `pasta.avx2` no segundo) e, em `[containers]`, `init_path = "<b>/lib/podman/catatonit"`. O Podman procura o `crun` primeiro em caminhos fixos do sistema (`/usr/bin/crun` e outros) e só depois no bundle; sem essa configuração, um `crun` do host seria usado;
- `storage.conf` (driver overlay com `mount_program` = `<b>/bin/fuse-overlayfs`, sem `ignore_chown_errors`, `graphroot = $XDG_CACHE_HOME/ohmlatl/podman/storage`, `runroot = $XDG_RUNTIME_DIR/ohmlatl-podman/run`);
- `registries.conf` com `unqualified-search-registries = []` e `short-name-mode = "enforcing"`: imagem remota precisa de nome qualificado (`docker.io/...`).

Os três arquivos ficam em `$XDG_CACHE_HOME/ohmlatl/conf/`. Toda chamada ao Podman (build, `podman unshare`, `ohmlatl podman`) recebe `CONTAINERS_CONF`, `CONTAINERS_STORAGE_CONF`, `CONTAINERS_REGISTRIES_CONF` apontando para eles, `CONTAINERS_POLICY_JSON=<b>/etc/containers/policy.json` (sem ele, um host sem `containers-common` falha com "no policy.json file found") e `CONTAINERS_HELPER_BINARY_DIR=<b>/lib/podman`, como o `vendor/bin/podman` faz hoje.

Requisitos do host:

- Build: Podman rootless (`newuidmap`/`newgidmap` setuid root ou com file capability `cap_setuid`/`cap_setgid`, como no Fedora; pelo menos 65536 ids em `/etc/subuid` e `/etc/subgid`, `/dev/fuse`), `e2fsprogs` >= 1.45 (`mkfs.ext4 -d` com xattrs e o feature set fixo), `cryptsetup` >= 2.1, `ssh-keygen` com `-A -f`, `udevadm`, `sudo`.
- AppArmor (Ubuntu 23.10 em diante e derivados): com `kernel.apparmor_restrict_unprivileged_userns=1`, só programas com perfil AppArmor que libere `userns` conseguem criar user namespace com capabilities, e o perfil que a distro traz cobre só `/usr/bin/podman`. O Podman extraído pelo `ohmlatl` fica sempre em `~/.cache/ohmlatl/bin/<sha256>/bin/podman` (raiz fixa, para um glob cobrir todas as versões; no AppArmor, `*` não atravessa `/`). Quando o sysctl está em 1, o `doctor --build` falha e mostra o perfil e os comandos para instalá-lo uma vez:

  ```text
  # /etc/apparmor.d/ohmlatl-podman
  abi <abi/4.0>,
  include <tunables/global>
  profile ohmlatl-podman /home/*/.cache/ohmlatl/bin/*/bin/podman flags=(unconfined) {
    userns,
    include if exists <local/ohmlatl-podman>
  }
  ```

  `sudo apparmor_parser -r /etc/apparmor.d/ohmlatl-podman`. Com `XDG_CACHE_HOME` fora de `/home/*/.cache`, o `doctor` gera o caminho certo. O runtime (supervisor, jailer, Firecracker) roda como root e não é afetado.
- Run: kernel >= 5.10 (Firecracker v1.17, nat na família `inet`, `openat2`), `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, systemd com `systemd-run`, cliente `ssh`, `sudo`.
- `image resize` e `image passwd`: `cryptsetup` >= 2.1 e `e2fsprogs` >= 1.45 (`e2fsck`, `resize2fs`). O `resize` confere sozinho e o `doctor --run` também.

`sudo ohmlatl` equivale a root. Quem pode rodar o `ohmlatl` com sudo pode virar root; é o mesmo nível de quem já tem sudo.

O binário no drive é tão confiável quanto o drive. Num host novo, confira o SHA-256 do binário contra o do release antes de rodar, ou use um `ohmlatl` instalado no host com `--home`.

## Kernel e initramfs

Gerados no repositório com Buildroot e embutidos no binário. Não existe `ohmlatl kernel` no MVP. O release já traz os assets padrão; quem quiser os próprios gera com `cargo xtask assets` e compila o próprio binário.

### Geração dos assets

```text
assets/
├── Containerfile                  # ambiente de build: debian:bookworm fixado por digest + dependências do Buildroot
├── podman/
│   ├── podman.lock                # tag e commit do podman-static, digests das 3 imagens base, versões dos pacotes apk, versão e SHA-256 de cada fonte
│   ├── pin-bases.patch            # troca as bases do Dockerfile do podman-static pelos digests do lock
│   └── capture-sources.sh         # guarda as fontes exatas e os avisos de cada estágio (ver abaixo)
├── buildroot.lock                 # versão do Buildroot (LTS 2026.02.x) + SHA-256 do tarball
├── mke2fs.conf                    # escrito à mão (ver abaixo)
├── br2-external/
│   ├── external.desc, external.mk, Config.in
│   ├── configs/ohmlatl_defconfig
│   └── board/ohmlatl/
│       ├── linux-firecracker-6.18.config   # cópia de resources/guest_configs/microvm-kernel-ci-x86_64-6.18.config do Firecracker v1.17.0, com SHA-256 anotado
│       ├── linux.fragment                  # as opções da lista abaixo
│       ├── check-kconfig.sh                # confere o .config final contra o fragmento: cada =y ficou =y, cada "is not set" ficou desligada
│       ├── post-build.sh                   # tira do initramfs o que não é usado (mke2fs, tune2fs, veritysetup, integritysetup...)
│       ├── check-brconfig.sh               # falha se busybox, cryptsetup ou e2fsprogs sumiram do .config do Buildroot
│       └── overlay/{init,etc/e2fsck.conf}
└── out/                           # gerado, fora do git: vmlinux, initramfs.cpio.gz, podman/ (bundle), sources/, SHA256SUMS
```

- `cargo xtask assets` usa o `podman` do host (é um requisito só de quem compila o `ohmlatl`, não de quem usa) e tem duas etapas:
  1. **Kernel e initramfs:** baixa o Buildroot do `buildroot.lock` e confere o SHA-256, constrói o container de `assets/Containerfile`, roda `make ohmlatl_defconfig`, `check-brconfig.sh`, `make` e `make legal-info` dentro dele com `BR2_EXTERNAL=/work/assets/br2-external` e copia `vmlinux`, `initramfs.cpio.gz` e o `legal-info` para `assets/out/`.
  2. **Bundle Podman:** o `Dockerfile` do podman-static v6.1.4 tem três bases fixas no texto (`alpine:3.24`, `golang:1.27-alpine3.24`, `rust:1.98-alpine3.24`) e cerca de dez estágios, cada um com `apk add` sem versão. O xtask:
     - aplica `pin-bases.patch`, que troca as três bases pelos digests do `podman.lock` e instala em cada estágio exatamente a lista de pacotes `apk` do lock, inclusive as dependências, cada um como `pkg=versão` (a lista é o `apk info -v` completo daquele estágio). O patch é fixo: as bases viram `ARG ALPINE_IMAGE`, `GOLANG_IMAGE` e `RUST_IMAGE`, e cada `apk add` vira `apk add --no-cache ${APK_<estágio>}`; o xtask preenche esses argumentos a partir do lock, os mesmos em todos os estágios, para o cache de camadas valer entre eles;
     - constrói cada estágio com `--target <estágio>` e, de cada um, extrai os binários e roda `capture-sources.sh`, que guarda em `assets/out/sources/podman/<estágio>/`: o tarball do componente na versão compilada (`git archive` do componente e de cada submódulo, com o SHA-256 conferido contra o `podman.lock`), `apk info -v`, o `APKBUILD` do `aports` de cada pacote instalado no estágio que traz biblioteca estática (`.a`), um conjunto que contém as bibliotecas ligadas, com os arquivos de fonte dele (do mirror de distfiles do Alpine ou da origem, guardados uma vez em `assets/out/sources/podman/aports/`), e os avisos de licença das dependências embutidas (os `LICENSE`/`NOTICE` de cada módulo em `vendor/` do podman e do conmon, e a saída JSON do `cargo about` 0.9.3 sobre netavark e aardvark-dns);
     - monta o bundle final com o último estágio e copia para `assets/out/podman/`.

     Se uma versão de pacote do lock sumiu do mirror do Alpine, o build falha e diz para rodar `cargo xtask assets --update-lock`, que constrói os estágios com os nomes dos pacotes sem versão e regrava no `podman.lock` as listas `apk` e os SHA-256 das fontes (os digests das bases não mudam; trocar uma base é uma edição à mão do lock); essa mudança entra num commit e é revisada como qualquer outra. O release usa o lock do commit, sem `--update-lock`.

  No fim, gera o `SHA256SUMS` de `assets/out/`. Os diretórios de download e de `ccache`, a saída do Buildroot, as fontes do Buildroot e do podman-static, os `aports` e o `cargo-about` ficam em `~/.cache/ohmlatl-assets/`, então só o primeiro build é longo (40 a 60 minutos, por causa da toolchain).
- `ohmlatl_defconfig`:
  - toolchain interna musl, `BR2_SHARED_LIBS=y` (o padrão), `BR2_REPRODUCIBLE=y`, base de CPU x86_64 genérica (sem `-march=native`), `BR2_ROOTFS_POST_BUILD_SCRIPT` = `post-build.sh`;
  - kernel `BR2_LINUX_KERNEL_CUSTOM_VERSION` 6.18.33 (a 6.18 cujo hash o Buildroot 2026.02.3 traz em `linux/from-6.17/linux.hash`, então o tarball é conferido; LTS; o Firecracker v1.17 testa 5.10, 6.1 e 6.18, com suporte ao 6.18 até pelo menos 06/2028), config `linux-firecracker-6.18.config` + `linux.fragment`, saída `vmlinux` (`BR2_LINUX_KERNEL_VMLINUX=y`), `BR2_LINUX_KERNEL_NEEDS_HOST_OPENSSL=y` e `BR2_LINUX_KERNEL_NEEDS_HOST_LIBELF=y` (a base tem `SYSTEM_TRUSTED_KEYRING` e `OBJTOOL`, que precisam de libcrypto e libelf no host do build); depois do build do kernel, `check-kconfig.sh` falha se alguma opção do fragmento não ficou como pedido (o merge de config descarta em silêncio opções com dependência faltando);
  - pacotes: BusyBox, cryptsetup com o backend de criptografia do kernel (`--with-crypto_backend=kernel`, que é o que o Buildroot escolhe sem OpenSSL/gcrypt/nettle; ele usa a interface AF_ALG, por isso o fragmento liga `CRYPTO_USER_API_HASH` e `CRYPTO_USER_API_SKCIPHER`), e2fsprogs. O Buildroot instala o e2fsprogs inteiro; o `post-build.sh` deixa só o `e2fsck`, mantendo as bibliotecas que `busybox`, `cryptsetup` e `e2fsck` usam (a lista começa pelo interpretador de cada executável, o `PT_INTERP` de `readelf -l`, que na musl é `/lib/ld-musl-x86_64.so.1`, e segue pelo fecho recursivo dos `NEEDED` de `readelf -d`, dos executáveis e das bibliotecas; cada nome é mantido junto com o symlink e o arquivo real a que ele aponta). O Buildroot não deixa compilar cryptsetup com `BR2_STATIC_LIBS` (o pacote depende de `!BR2_STATIC_LIBS`, por causa de lvm2 e libargon2), e uma opção com dependência faltando some do `.config` sem aviso; por isso o build também roda `check-brconfig.sh`, que falha se `BR2_PACKAGE_BUSYBOX`, `BR2_PACKAGE_CRYPTSETUP` ou `BR2_PACKAGE_E2FSPROGS` não estiverem `=y` no `.config` final;
  - rootfs `BR2_TARGET_ROOTFS_CPIO=y` com gzip, a partir do `overlay/`. Esse cpio é o initramfs.
- `build.rs` do `ohmlatl`: falha com "rode `cargo xtask assets`" se faltar algum arquivo embutido de `assets/out/` (kernel, initramfs e bundle Podman) ou se ele não bater com o `SHA256SUMS`; `OHMLATL_ASSETS_DIR` aponta para outro diretório. Os arquivos entram por `include_bytes!`, junto com o Firecracker e o jailer do `vendor/firecracker`.
- O release gera os assets do zero, num container limpo, e publica o `legal-info` e o `sources/podman/` como o tarball de fontes.

A config do Firecracker é feita para o kernel de microVM do Amazon Linux e não tem garantia em kernel upstream; o `check-kconfig.sh` e o boot de teste do P0 cobrem esse risco.

Kernel: `# CONFIG_MODULES is not set` (tudo embutido; o guest não carrega módulos). O fragmento garante, além do que já vem na base, estas opções `=y`:

- Plataforma: `VIRTIO_MMIO`, `VIRTIO_BLK`, `VIRTIO_NET`, `HW_RANDOM_VIRTIO`, `SERIAL_8250`, `SERIAL_8250_CONSOLE`, `INPUT`, `INPUT_KEYBOARD`, `SERIO_I8042`, `KEYBOARD_ATKBD`, `VT` (o Ctrl-Alt-Del passa pelo teclado do VT), `PTP_1588_CLOCK_KVM`, `KVM_GUEST`.
- Boot e systemd: `BLK_DEV_INITRD`, `RD_GZIP`, `DEVTMPFS`, `DEVTMPFS_MOUNT`, `TMPFS`, `TMPFS_POSIX_ACL`, `FHANDLE`, `AUTOFS_FS`, `CGROUPS` (v2, cpu, memory, pids, io, bpf), `SECCOMP`, `UNIX`, `INET`, `IP_PNP`, `EPOLL`, `SIGNALFD`, `TIMERFD`, `INOTIFY_USER`.
- Disco: `MD` (o `BLK_DEV_DM` fica dentro dele no Kconfig), `BLK_DEV_DM`, `DM_CRYPT`, `CRYPTO_AES_NI_INTEL`, `CRYPTO_XTS`, `CRYPTO_USER_API_HASH`, `CRYPTO_USER_API_SKCIPHER` (AF_ALG, para o backend do cryptsetup), `EXT4_FS`, `EXT4_FS_POSIX_ACL`, `EXT4_FS_SECURITY`, `VFAT_FS` com `NLS_CODEPAGE_437`, `NLS_ISO8859_1` e `NLS_UTF8` (sem módulos, o vfat não monta sem eles), `ISO9660_FS`, `SQUASHFS`, `OVERLAY_FS`, `FUSE_FS`.
- Rede e containers dentro do guest: `NETFILTER`, `NF_TABLES` (inet, nat, masq, reject), `NETFILTER_XTABLES` com os matches usados pelo `iptables-nft`, `BRIDGE`, `VETH`, `TUN`, `WIREGUARD`, `NAMESPACES` (net, user, pid, ipc, uts, cgroup), `BPF_SYSCALL`, `IPV6`.

Initramfs: BusyBox, `cryptsetup` e `e2fsck` do Buildroot, ligados dinamicamente contra a musl, com o loader e as bibliotecas que eles usam (libcryptsetup, libdevmapper, json-c, popt, libargon2, libblkid, libuuid, libext2fs, libcom_err, libe2p; a lista final é a que o `post-build.sh` calcula) dentro do cpio, `/init` em shell, `/etc/e2fsck.conf` com `broken_system_clock = 1`. O `e2fsck` é o do e2fsprogs que o Buildroot fixa (1.47.x); o feature set do `mke2fs.conf` é anterior a ele, então ele sempre entende o ext4 gerado no host.

`mke2fs.conf` embutido, completo:

```ini
[defaults]
	base_features = sparse_super,large_file,filetype,resize_inode,dir_index,ext_attr
	blocksize = 4096
	inode_size = 256
	inode_ratio = 16384

[fs_types]
	ext4 = {
		features = has_journal,extent,huge_file,flex_bg,metadata_csum,64bit,dir_nlink,extra_isize
	}
```

Sem `orphan_file` e sem `metadata_csum_seed`. `ext_attr` é obrigatório: sem ele o `mkfs -d` perde xattrs e capabilities. `inode_size = 256` dá espaço para `extra_isize` (timestamps além de 2038). O build usa `MKE2FS_CONFIG` apontando para ele e passa também `-O` com a lista completa (base + ext4), então o resultado não depende da distro do host.

## Build

```bash
ohmlatl build [-f Containerfile | --from <ref>] [-t <nome:tag>] [--size <n>]
```

Entradas:

- Exige `ohmlatl.yaml` no diretório atual. Ele dá `user` e `vm`, e o contexto do `podman build` é esse diretório.
- `-f Containerfile` (ou `build.containerfile`) faz `podman build`. `--from <ref>` usa uma imagem **local** (`--pull=never`); imagem remota se traz antes com `ohmlatl podman pull docker.io/...`.
- `-t` vence `name` e `build.tag`; sem tag, `latest` (`-t <nome>` sozinho dá `<nome>:latest`).
- Validação: nome `^[a-z][a-z0-9-]{0,30}$`, tag `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
- Na validação dentro do `unshare`, os caminhos da imagem (`/sbin/init`, `/usr/sbin/sshd`, a home, o `sftp-server`) são resolvidos com `chroot "$root" readlink -e`, para que symlinks absolutos da imagem não apontem para o host.
- Se `images/<nome>/<tag>/` já existe, o build falha. Ele nunca sobrescreve um computador.

Passos (`$BUILD` = `$XDG_CACHE_HOME/ohmlatl/build/<id>`, no host; `$PART` = `<home>/images/<nome>/.<tag>.partial`, no drive):

1. `podman build` (com `-f`) ou nada (com `--from`); depois, sempre, `podman create --pull=never <imagem> /bin/true`, que funciona mesmo sem `CMD`/`ENTRYPOINT`.
2. Export, validação e ajustes dentro de `podman unshare`.
   - Validação, antes de qualquer ajuste: `/sbin/init` resolve para o systemd, existe `/usr/sbin/sshd`, `user.name` existe em `/etc/passwd` com o UID/GID declarados e a home dele existe com esse dono. Se algo falta, o build para com mensagem clara.
   - Aviso, sem parar: se o `sshd_config` da imagem não aponta para um `sftp-server` que existe (o `scp` moderno usa SFTP), o `ohmlatl cp` não vai funcionar.
   - Aviso, sem parar: se o usuário não tem senha utilizável em `/etc/shadow` (vazia, `!` ou `*`) nem `~/.ssh/authorized_keys`, o `ohmlatl shell` não vai conseguir entrar. O console continua funcionando.
   - Host keys do SSH: ficam as que a imagem trouxer. Se não houver nenhuma, o build gera dentro do rootfs (`ssh-keygen -A -f "$root"`), porque o `sshd` não sobe sem elas.
   - O `umask` é 022 dentro do script (o ambiente do Podman usa 077) e os arquivos gerados recebem modo explícito:

```bash
podman unshare sh -euc '
  umask 022
  root="$BUILD/root"; mkdir -m 0755 "$root"
  podman export "$CID" | tar -x --same-owner --numeric-owner \
    --xattrs --xattrs-include="*" --xattrs-exclude="security.selinux" -C "$root"
  for p in etc etc/ssh etc/systemd etc/systemd/system etc/NetworkManager etc/NetworkManager/conf.d; do
    [ -L "$root/$p" ] && { echo "symlink em /$p na imagem" >&2; exit 1; }
  done
  # validação (ver acima) roda aqui, depois da checagem de symlinks: um /etc que aponta
  # para fora da árvore faria a validação ler o /etc do host
  rm -f "$root/etc/machine-id" "$root/etc/hostname" "$root/etc/hosts" \
        "$root/etc/NetworkManager/conf.d/ohmlatl.conf" \
        "$root/etc/systemd/system/serial-getty@ttyS0.service.d/autologin.conf"
  : > "$root/etc/machine-id"
  printf "%s\n" "$NAME" > "$root/etc/hostname"
  printf "127.0.0.1 localhost %s\n" "$NAME" > "$root/etc/hosts"
  rm -f "$root/etc/resolv.conf"; : > "$root/etc/resolv.conf"
  ls "$root"/etc/ssh/ssh_host_*_key >/dev/null 2>&1 || ssh-keygen -A -f "$root" >&2
  install -d -m 0755 -o 0 -g 0 "$root/etc/NetworkManager/conf.d"
  printf "[main]\ndns=none\n\n[keyfile]\nunmanaged-devices=interface-name:eth0\n" \
    > "$root/etc/NetworkManager/conf.d/ohmlatl.conf"
  for u in systemd-networkd.service systemd-networkd-wait-online.service \
           systemd-resolved.service; do
    ln -sf /dev/null "$root/etc/systemd/system/$u"
  done
  ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/poweroff.target"
  ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/halt.target"
  d="$root/etc/systemd/system/serial-getty@ttyS0.service.d"
  install -d -m 0755 -o 0 -g 0 "$d"
  printf "[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin %s --keep-baud 115200,57600,38400,9600 %%I \$TERM\n" \
    "$USER_" > "$d/autologin.conf"
  chmod 0755 "$root"
  chmod 0444 "$root/etc/machine-id"
  chmod 0644 "$root/etc/hostname" "$root/etc/hosts" "$root/etc/resolv.conf" \
    "$root/etc/NetworkManager/conf.d/ohmlatl.conf" "$d/autologin.conf"
  du -sB1 "$root"; find "$root" -xdev | wc -l
'
```

3. Tamanho: `--size`, ou `max(32 GiB, 2 x conteúdo + 1 GiB)`, com o conteúdo em blocos alocados (`du -sB1` dentro do unshare); arredondado para cima até múltiplo de 1 MiB. O número de inodes do ext4 tem que ser pelo menos o dobro do número de arquivos; se não for, o build passa `-N`. O espaço livre no drive é conferido contra o que o build vai escrever (conteúdo, tabelas de inode, journal e cabeçalho LUKS). Um `--size` menor que o conteúdo mais os metadados do ext4 é recusado aqui, antes da passphrase e do `sudo`.
4. Passphrase: lida do TTY, com confirmação, uma vez. Recusa vazia e com `\n` ou `\r`. É escrita sem quebra de linha no stdin do `cryptsetup`.
5. LUKS. Antes, `sudo -v`; durante o resto do build, um laço em segundo plano roda `sudo -n -v` a cada 60 s, para o sudo não pedir senha de novo no meio de um `mkfs` demorado:

```bash
umask 077
mkdir -p -m 0700 "images/<nome>"
exec 9>"images/<nome>/.<tag>.lock"; flock -n 9 || exit 1   # lock do build, antes de tudo
# $PART que já existe aqui, com o lock livre, é sobra de build morto: limpar (ver abaixo)
mkdir "$PART"
truncate -s $((SIZE + 16 * 1024 * 1024)) "$PART/rootfs.luks"; chmod 0600 "$PART/rootfs.luks"
sudo cryptsetup luksFormat --batch-mode --type luks2 \
  --cipher aes-xts-plain64 --key-size 512 --sector-size 4096 --offset 32768 \
  --pbkdf argon2id --pbkdf-memory 262144 --pbkdf-parallel 1 --pbkdf-force-iterations 4 \
  --key-file=- "$PART/rootfs.luks"
sudo cryptsetup open --key-file=- "$PART/rootfs.luks" ohmlatl-build-<id>
```

   O lock `images/<nome>/.<tag>.lock` é pego antes do `mkdir`, então dois builds da mesma tag nunca correm juntos. Com o lock na mão, um `$PART` que já existe é sobra de um build morto (SIGKILL, queda de energia): o build fecha qualquer mapper `ohmlatl-build-*` cujo arquivo esteja nele, apaga o `$PART` e segue. O `doctor --build` faz a mesma limpeza, também sob o lock. O offset de dados do LUKS é fixo em 16 MiB (`--offset 32768`, em setores de 512 bytes), e o arquivo tem `SIZE` + 16 MiB: a VM enxerga exatamente `SIZE`, e build e `image resize` usam a mesma conta. O PBKDF é fixo (sem benchmark): o unlock no guest não depende da máquina que fez o build. 256 MiB de argon2 cabem na memória mínima da VM (512 MiB).

6. ext4 direto no mapper, com donos corretos:

```bash
sudo udevadm settle --exit-if-exists=/dev/mapper/ohmlatl-build-<id>
sudo chown "$(id -u)" "$(realpath /dev/mapper/ohmlatl-build-<id>)"
podman unshare env MKE2FS_CONFIG="$BIN/mke2fs.conf" mkfs.ext4 -q -F \
  -t ext4 -O "$FEATURES" -L rootfs -m 0 \
  -E lazy_itable_init=0,lazy_journal_init=0,nodiscard \
  -d "$BUILD/root" /dev/mapper/ohmlatl-build-<id>
sudo e2fsck -fn /dev/mapper/ohmlatl-build-<id>
sudo cryptsetup close ohmlatl-build-<id>
```

   O `chown` vem logo antes do `mkfs` porque o udev devolve o nó para `root:disk` quando o device é fechado. O `settle` espera só o nó do mapper (`--exit-if-exists`), não a fila inteira: o `cryptsetup open` com suporte a udev (a flag `UDEV` de `cryptsetup --version`, padrão nas distros) só retorna depois que as regras do mapper rodaram, e a fila inteira pode ficar ocupada por muito tempo por eventos de outros devices. Exemplo real: o `nfs-common` instala `99-nfs.rules`, que roda o `nfsrahead` em cada BDI novo; cada mount `fuse-overlayfs` do Podman rootless cria um, e o `nfsrahead` espera cerca de 10 s por cada um, então um `podman build` deixa a fila ocupada por minutos e um `settle` sem `-E` estoura o tempo. O que não foi escrito continua buraco no arquivo; o ext4 não lê blocos livres, então o conteúdo deles não importa. As tabelas de inode e o journal são escritos no build (cerca de 800 MiB num disco de 32 GiB).

7. Gravar `$PART/manifest.yaml` (com `name` e `tag` finais) e `rename` de `$PART` para `images/<nome>/<tag>`, no mesmo filesystem, o que preserva o sparse. O arquivo `.<tag>.lock` fica ao lado; ele é só o objeto do `flock` e pode sobrar.
8. Trap (sucesso ou falha): `podman unshare rm -rf "$BUILD"`, `cryptsetup close` se aberto, `rm -rf "$PART"` se não renomeado, `podman rm` do container, `podman rmi` da imagem feita pelo `podman build` (com `-f`) se ela não tiver tag, fim do laço do `sudo`.

Regras:

- Export, ajustes, `du` e `mkfs` rodam dentro de `podman unshare`. Fora dele, o rootfs fica com o UID do operador e quebra `sudo`, `sshd`, setuid e capabilities.
- `podman export`, não `podman save`.
- Sem segredos em `ARG`/`ENV`/`RUN`; use `podman build --secret`. Senha do usuário também: defina com `passwd` dentro do `ohmlatl podman run`, nunca com `chpasswd` em claro no `Containerfile`.

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

- `format` é obrigatório; o `ohmlatl` recusa versões que não conhece. Campos desconhecidos são erro.
- `name` e `build.tag` seguem as regras do build; `-t` sobrescreve, e o build grava os valores finais no `manifest.yaml` da imagem.
- Obrigatórios: `format`, `name`, `user.name`, `user.uid`, `user.gid`, `vm.vcpus` e `vm.memory`. `build` e `vm.network` são opcionais.
- `build.containerfile` é opcional com `--from`.
- `user.name`: `^[a-z_][a-z0-9_-]{0,31}$` (ele vai para a unit do `serial-getty`).
- Tamanhos (`vm.memory`, `--size`, `image resize`): número com unidade binária opcional (`K`, `M`, `G`, `T`, ou `KiB`, `MiB`, `GiB`, `TiB`); sem unidade, bytes.
- O `ohmlatl` lê um subconjunto de YAML: mapas e listas em bloco, valores simples ou entre aspas, `[]` e comentários. Fluxo (`{...}`), âncoras, tags e blocos de texto são erro.
- `user.uid` e `user.gid` entre 1 e 65534.
- `vm.vcpus` entre 1 e 32; `vm.memory` >= 512 MiB. `run` aceita `--vcpus` e `--memory`, que valem só para aquela execução.
- `vm.network.internet`: padrão `true`.
- `vm.network.forward`: `port` entre 1 e 65535, sem repetição; `port: 53` é inválido com `internet: true`. `to` é `host:<porta>` (loopback do host) ou `vm/<nome>:<porta>` (VM do mesmo usuário).
- O `run` lê o `manifest.yaml` da imagem. Editar `vm` nele vale no próximo `run`.

## Run e boot

```bash
ohmlatl run <nome>:<tag> [--vcpus n] [--memory n]
```

O mesmo comando serve para a primeira vez e para as seguintes. Não existe estado de VM no drive: a tag vai sempre explícita.

CLI (usuário):

1. Acha o diretório do ohmlatl e confere o dono. Valida o manifest. Confere que a imagem existe e que não há VM com esse nome rodando (`ctl.sock` responde). Se houver, falha.
2. Imprime `internet on|off` e os forwards. A rede (TAP, gateway, IP do guest) só existe depois que o supervisor aloca, então é impressa no passo 5.
3. Apaga `/run/ohmlatl/<uid>/<nome>/error`, se existir.
4. `sudo <exe> launch --uid <uid> --gid <gid> --home <realpath do dir> --image <nome>:<tag> [--vcpus n] [--memory n] [--pool <cidr>]`, com `<exe>` = destino de `/proc/self/exe` (o `secure_path` do sudo não inclui o drive nem `~/.local/bin`).
5. Espera `ready` no `ctl.sock`, que devolve TAP, gateway e IP do guest, e imprime a rede. Enquanto espera, consulta `systemctl show -p ActiveState,Result ohmlatl-<uid>-<nome>`: `failed`, ou `inactive` antes do `ready`, é falha. Nesse caso imprime o `error` ou, se não houver, aponta `sudo journalctl -u ohmlatl-<uid>-<nome>` (unit de sistema; o operador comum não lê o journal dela).
6. Anexa ao `console.sock`. `Ctrl-]` desanexa.

`OHMLATL_POOL` (padrão `172.31.0.0/16`) muda a faixa das subnets; a CLI passa o valor como `--pool`, porque o sudo limpa o ambiente.

`launch` (root, curto):

1. Sob `flock /run/ohmlatl.lock`: se `/run/ohmlatl` não é um mount próprio, `mount -t tmpfs -o mode=0755,nosuid ohmlatl /run/ohmlatl`. Sem `nodev` e sem `noexec`, porque o jailer cria nós de device e executa o Firecracker dentro do chroot, e o `/run` do sistema costuma ser `nodev,noexec`.
2. Extrai os binários para `/run/ohmlatl/bin/<sha256>/`.
3. Cria `/run/ohmlatl/<uid>/` (0700) e `/run/ohmlatl/<uid>/<nome>/` (0700), do operador.
4. Inicia a unit:

```bash
systemctl reset-failed ohmlatl-<uid>-<nome> 2>/dev/null || true
systemd-run --unit ohmlatl-<uid>-<nome> \
  -p Delegate=yes -p KillMode=mixed -p TimeoutStopSec=150 \
  -p 'BindsTo="<unit do mount>"' -p 'After="<unit do mount>"' \
  -p "ExecStopPost=/run/ohmlatl/bin/<sha256>/ohmlatl cleanup <uid> <nome>" \
  /run/ohmlatl/bin/<sha256>/ohmlatl supervise --uid <uid> --gid <gid> --home <dir> --image <nome>:<tag> ...
```

   A unit roda a cópia em `/run/ohmlatl/bin`, então trocar ou apagar o binário do drive com a VM rodando não quebra o cleanup. `BindsTo` e `After` na unit do mount do drive (`systemd-escape --path --suffix=mount <ponto de montagem>`, com o ponto achado por `findmnt -T <home>`) fazem o systemd parar a VM antes de desmontar o drive no desligamento do host, e também quando o drive é desmontado por fora do systemd. O nome dessa unit tem sequências como `\x2d` e `\x20`; ele vai entre aspas e com cada `\` dobrado, porque o `systemd-run` remove as barras ao separar o valor e procuraria uma unit que não existe. (Não se usa `RequiresMountsFor`: um caminho com espaço, como o label `USB DISK`, não sobrevive a um `daemon-reload` na unit transitória.) Se o diretório não está num mount próprio (`/`), as duas propriedades ficam de fora. O systemd expande `$VAR` e `${VAR}` nos argumentos de `ExecStart` e `ExecStopPost`, então cada `$` dos argumentos (um label de drive pode ter) é passado como `$$`. `KillMode=mixed` manda o SIGTERM só para o supervisor, que desliga o guest; sem isso o systemd mataria o Firecracker direto.

Supervisor (root, unit de sistema, sobrevive ao logout):

1. Lê o `manifest.yaml` da imagem no drive. Qualquer falha a partir daqui vai para `error` (do operador) e para o journal.
2. Rede, sob `flock /run/ohmlatl/net.lock` (ver Rede), nesta ordem: aloca o `/30` e o nome `ohm<N>` e grava logo `/run/ohmlatl/net/<uid>-<nome>` com os dois, antes de criar qualquer coisa; cria o TAP com o endereço do gateway; abre os listeners dos forwards e do DNS em `<gw>:0` (o bind com `SO_BINDTODEVICE` precisa do TAP pronto); cria as regras nft com as portas efêmeras que os listeners receberam. Assim, uma falha ou um kill em qualquer ponto deixa o registro que o cleanup precisa.
3. Abre o PTY e coloca o slave em modo raw (`cfmakeraw`), para o eco do host nunca devolver a passphrase para a saída.
4. Prepara o chroot do jailer: bind mount read-only de `vmlinux` e `initramfs`, bind mount read-write de `rootfs.luks` (do operador, 0600, no drive), dentro de `/run/ohmlatl/jail/firecracker/<uid>-<nome>/root/`. O supervisor segura `flock` exclusivo no diretório da imagem (`images/<nome>/<tag>/`, aberto com `O_DIRECTORY`) até sair, o que impede `image resize`, `image rm` e `image passwd` com a VM rodando. O lock é no diretório, não no `rootfs.luks`, porque o próprio `cryptsetup` faz `flock` no arquivo da imagem e travaria contra o nosso. O drive não é confiável para o root: o supervisor abre o `--home` (já resolvido com `realpath` pela CLI) e, relativo a ele, abre `manifest.yaml` e `rootfs.luks` com `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`; os dois têm que ser arquivo regular com dono `--uid`, e o bind mount usa `/proc/self/fd/<n>` do arquivo já aberto. Symlink, device ou dono errado: falha.
5. Chama o jailer direto, sem `--daemonize`, com o PTY slave em stdin, stdout e stderr:

```bash
jailer --id <uid>-<nome> --exec-file /run/ohmlatl/bin/<sha256>/firecracker \
  --uid <uid> --gid <gid> --chroot-base-dir /run/ohmlatl/jail \
  --cgroup-version 2 --parent-cgroup system.slice/ohmlatl-<uid>-<nome>.service \
  -- --api-sock /run/api.sock --log-path /run/firecracker.log --level Warning
```

   `--parent-cgroup` é o cgroup da unit, lido de `/proc/self/cgroup` sem a `/` inicial (o jailer recusa caminho absoluto). Sem `--cgroup`, o jailer põe o Firecracker nesse cgroup, então o systemd enxerga e mata o processo junto com a unit. O jailer cria `/dev/kvm` e `/dev/net/tun` no chroot para o UID do operador. O log do Firecracker vai para `/run/firecracker.log` dentro do chroot, e o supervisor repassa para o journal; assim ele não se mistura com o serial no PTY.

6. Configura pela API (`/run/ohmlatl/jail/firecracker/<uid>-<nome>/root/run/api.sock`):
   - `boot-source`: `vmlinux`, `initramfs`, cmdline (abaixo);
   - `drives/rootfs`: `rootfs.luks`, `is_root_device: false`, `is_read_only: false`, `cache_type: "Writeback"` (o padrão `Unsafe` ignora flush do guest e arrisca corromper o ext4 numa queda de energia);
   - `network-interfaces/eth0`: `host_dev_name: ohm<N>`;
   - `machine-config`: vcpus e memória;
   - `entropy`: virtio-rng;
   - `actions`: `InstanceStart`.
7. Grava `ip`, cria `ctl.sock` e `console.sock` com `chown <uid>:<gid>` e modo 0600, responde `ready`. Copia a saída do PTY para o cliente do console e para `serial.log` (buffer circular de 1 MiB, do operador). Ao anexar, o cliente recebe primeiro os últimos 64 KiB do buffer, então o prompt da passphrase aparece mesmo se já tiver sido impresso.

Cmdline:

```text
console=ttyS0 reboot=k panic=1 ip=<guest>::<gw>:255.255.255.252:<nome>:eth0:off
```

`/init` do initramfs:

1. Monta `proc`, `sys`, `devtmpfs` e um tmpfs em `/run` (com `/run/cryptsetup`, o diretório de lock do `cryptsetup`). `cryptsetup` e rescue rodam em primeiro plano, com o console como stdin. Antes do `switch_root`, nada desliga o Ctrl-Alt-Del do kernel (`C_A_D` continua 1; só o systemd faz `reboot(CAD_OFF)`), então o Ctrl-Alt-Del do `stop` reinicia o kernel na hora e, com `reboot=k`, o Firecracker sai: o `stop` no prompt da passphrase ou no rescue é imediato.
2. `cryptsetup open --tries 3 /dev/vda root`, perguntando no `ttyS0` (`Enter passphrase for /dev/vda:`). Três falhas: rescue.
3. `e2fsck -p /dev/mapper/root`. Código >= 4: rescue.
4. Monta `/dev/mapper/root` em `/newroot`.
5. Lê o gateway do `ip=` em `/proc/cmdline`. Remove `/newroot/etc/resolv.conf` se for symlink e escreve `nameserver <gw>` (0644). Reescreve no `/newroot/etc/hosts` a linha marcada `<gw> ohmlatl-gateway # ohmlatl`, mantendo o modo 0644.
6. `switch_root /newroot /sbin/init`.

Rescue: `sh` do busybox no `ttyS0`.

O systemd sobe o `sshd` com as host keys do rootfs e o `serial-getty@ttyS0` com autologin do usuário.

## Acesso

Opções de SSH (nada vem do `~/.ssh/config` do operador):

```text
-F /dev/null -o ForwardAgent=no -o ForwardX11=no
-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o GlobalKnownHostsFile=/dev/null -o LogLevel=ERROR
-o ControlPath=/run/ohmlatl/<uid>/<nome>/ssh.sock
```

- A autenticação é a que a imagem tiver: senha do usuário, ou as chaves do operador (`~/.ssh/id_*`, agente) se a pública estiver no `authorized_keys` da imagem. O `ohmlatl` não gera nem guarda chaves.
- A host key não é conferida: a conexão vai do host para o guest por um TAP ponto a ponto local, e só o root do host conseguiria se colocar no meio. O host já é confiável por premissa e vê a RAM da VM. Por isso não existe `known_hosts`, e trocar de imagem não gera alerta.
- `ControlMaster`: antes de `shell`, `exec` ou `cp`, a CLI roda `ssh <opções> -O check ohmlatl`. Se não houver mestre, abre um com `ssh <opções> -o ControlMaster=yes -o ControlPersist=yes -fN <user>@<ip>`, que pede a senha uma vez; as demais chamadas usam `-o ControlMaster=no`. O `ControlMaster` fica fora das opções comuns porque o `ssh` usa o primeiro valor que recebe de cada opção. Depois disso, todas as conexões até o próximo `stop` reaproveitam o mestre. Isso vale também para o `scp`, que força `ControlMaster=no` e nunca vira mestre sozinho.
- `shell` e `exec`: `ssh <opções> <user>@<ip do guest>`, com o usuário e o IP vindos do `ctl.sock`, sem precisar do drive.
- `cp`: `scp -r <opções>`; diretórios são copiados recursivamente.
- `console`: anexa ao `console.sock` e recebe o fim do buffer do serial; só um cliente por vez. O login é automático.
- `logs`: lê `/run/ohmlatl/<uid>/<nome>/serial.log`. Fica em tmpfs, some no reboot do host e é preservado entre o `stop` e o próximo `run`.
- `ps`: lista `/run/ohmlatl/<uid>/*/ctl.sock` e consulta cada um (nome, imagem, usuário, IP, pid, uptime). Só mostra VMs rodando; o que existe no drive aparece em `image ls`.

## Stop

1. A CLI manda `stop` pelo `ctl.sock`. O SIGTERM do systemd (`systemctl stop`, desligamento do host) leva ao mesmo caminho. A CLI só retorna quando `systemctl show -p ActiveState ohmlatl-<uid>-<nome>` dá `inactive` ou `failed`, ou seja, depois do `ExecStopPost`; assim um `run` logo em seguida não esbarra na unit ainda saindo.
2. O supervisor manda `PUT /actions {"action_type": "SendCtrlAltDel"}` no `api.sock`.
3. O systemd do guest executa `ctrl-alt-del.target`, um reboot limpo. Com `reboot=k`, o reboot encerra o Firecracker. Dentro do guest, `poweroff`, `halt` e `reboot` também encerram a VM, porque os targets apontam para `reboot.target` (o Firecracker não tem power off).
4. Se o Firecracker não sair em 120 s (acima dos 90 s padrão do systemd), o supervisor mata.
5. Cleanup. Depois dele, nada mais segura arquivos no drive e ele pode ser desmontado.

Cleanup (`ohmlatl cleanup <uid> <nome>`), idempotente. Roda no fim do supervisor e de novo no `ExecStopPost`, que também roda quando o supervisor morre:

1. Mata o Firecracker se ainda estiver vivo.
2. Desmonta os bind mounts e remove o chroot.
3. Sob o lock de rede, lê `/run/ohmlatl/net/<uid>-<nome>`: remove o TAP, se existir, os elementos `ohm<N>` dos mapas e o gateway de `gws`, e as cadeias `vm<N>_*` (o que não existir é ignorado); se não sobrar nenhum TAP `ohm*`, remove a tabela `inet ohmlatl`; apaga o registro.
4. Encerra a conexão SSH mestre (`ssh -F /dev/null -o ControlPath=.../ssh.sock -O exit ohmlatl`; o destino é só um placeholder, que o `ssh` exige) e depois remove `ctl.sock`, `console.sock`, `ssh.sock` e `ip`. `serial.log` e `error` ficam até o próximo `run`.

## Rede

Alocação, sob `flock /run/ohmlatl/net.lock`:

- Antes de alocar, o supervisor confere o pool (`172.31.0.0/16`, ou `OHMLATL_POOL`): se algum endereço ou rota do host que não seja de `ohm*` nem a rota padrão cai dentro dele, recusa subir e manda usar `OHMLATL_POOL`.
- O `/30` é o primeiro livre no pool que não pertence a outro registro em `/run/ohmlatl/net/` nem já está atribuído a alguma interface `ohm*` (sobra de um supervisor morto antes do cleanup). É alocado a cada `run`; o guest nunca depende do IP, porque usa `ohmlatl-gateway`. Uma rede remota que usa a mesma faixa mas é alcançada pela rota padrão (por exemplo, uma VPC) não é detectável: nesse caso, configure `OHMLATL_POOL`.
- TAP `ohm<N>` (primeiro N que não está numa interface nem em nenhum registro de `/run/ohmlatl/net/`): `ip tuntap add dev ohm<N> mode tap user <uid>`, `ip addr add <gw>/30 dev ohm<N>`, `sysctl net.ipv6.conf.ohm<N>.disable_ipv6=1` (pulado se `/proc/sys/net/ipv6` não existe, num host com `ipv6.disable=1`), `ip link set ohm<N> up`.
- `net.ipv4.ip_forward=1`. Nunca é restaurado: outros serviços (Docker) podem passar a depender dele. Se estava desligado e `/run/ohmlatl/net/forward-was-off` ainda não existe, o supervisor grava esse arquivo e cria a tabela `inet ohmlatl-fwguard` **antes** de ligar o `ip_forward`, sempre sob o lock de rede, com `iifname != "ohm*" oifname != "ohm*" drop` no `forward`, para o host não virar roteador entre as outras interfaces (LAN, VPN). Essa tabela não é apagada pelo cleanup; fica até o reboot. Efeito colateral: um Docker ou libvirt iniciado depois não roteia enquanto ela existir. O `doctor --run` mostra a trava e o comando para tirá-la (`sudo nft delete table inet ohmlatl-fwguard`).

Tabela `inet ohmlatl` (criada se não existe). As regras gerais existem uma vez por tabela, nas cadeias base. Cada VM tem cadeias regulares próprias (`vm<N>_raw`, `vm<N>_nat`, `vm<N>_in`, `vm<N>_fwd`, `vm<N>_out`), alcançadas pelas cadeias base por mapas de veredito indexados pelo nome do TAP. Assim a ordem nunca depende de qual VM subiu primeiro: o `vmap` decide a VM, e só o que a cadeia dela não aceitar cai nas regras gerais que vêm depois. Testado com `nft -c` e com carga e remoção de uma VM num namespace isolado:

```text
table inet ohmlatl {
  set gws     { type ipv4_addr; }                     # IPs de gateway em uso
  set blocked { type ipv4_addr; flags interval; auto-merge; }     # RFC1918, 169.254/16, 100.64/10 e as redes conectadas do host
  map vm_iif_raw { type ifname : verdict; }           # "ohm<N>" : jump vm<N>_raw
  map vm_iif_nat { type ifname : verdict; }
  map vm_iif_in  { type ifname : verdict; }
  map vm_iif_fwd { type ifname : verdict; }
  map vm_oif_out { type ifname : verdict; }

  chain prerouting_raw  { type filter hook prerouting priority raw;
                          iifname "ohm*" meta nfproto ipv6 drop
                          iifname vmap @vm_iif_raw; }
  chain prerouting_nat  { type nat hook prerouting priority dstnat;
                          iifname vmap @vm_iif_nat; }
  chain input           { type filter hook input priority filter;
                          ip daddr @gws iifname != "lo" iifname != "ohm*" drop
                          iifname vmap @vm_iif_in
                          iifname "ohm*" reject; }
  chain forward         { type filter hook forward priority filter;
                          oifname "ohm*" ct state established,related accept
                          iifname "ohm*" oifname "ohm*" reject
                          oifname "ohm*" drop
                          iifname "ohm*" ip daddr @blocked reject
                          iifname vmap @vm_iif_fwd
                          iifname "ohm*" reject; }
  chain output          { type filter hook output priority filter;
                          oifname vmap @vm_oif_out; }
  chain postrouting_nat { type nat hook postrouting priority srcnat;
                          iifname "ohm*" oifname != "ohm*" masquerade; }
}
```

O conteúdo de `blocked` é recalculado sob o lock de rede a cada VM nova e, por cada supervisor rodando, a cada mudança de rota do host (netlink): as faixas fixas mais as redes conectadas atuais do host, gravadas numa transação só (`flush set` + `add element` no mesmo `nft -f`). A LAN comum (`192.168.1.0/24`, `10.x`) cai dentro de uma faixa fixa; o `auto-merge` junta os intervalos sobrepostos em vez de recusar o conjunto. Assim, trocar de Wi-Fi ou entrar numa LAN com endereço público não abre a LAN para os guests. O `iifname "ohm*" oifname "ohm*" reject` antes do `drop` faz uma VM que tenta alcançar outra falhar na hora, em vez de esperar timeout.

Por VM (exemplo de `ohm0`, guest `172.31.0.2`, gateway `172.31.0.1`):

- `vm0_raw`: `ip saddr != 172.31.0.2 drop` (anti-spoof).
- `vm0_nat`: `ip daddr 172.31.0.1 tcp dport <port> redirect to :<porta efêmera>` para cada forward; com `internet: true`, o mesmo para 53/udp e 53/tcp.
- `vm0_in`: `ct state established,related accept`; `ip daddr 172.31.0.1 ct status dnat ct original proto-dst { <ports> } accept` (e 53 com `internet: true`). O resto volta para a `input` e cai no `reject`.
- `vm0_fwd`: `accept` com `internet: true`; `reject` com `internet: false`.
- `vm0_out`: `meta skuid != { 0, <uid> } reject`. Só o root e o dono da VM abrem conexão do host para o guest.
- Elementos: `"ohm0" : jump vm0_*` em cada mapa e `172.31.0.1` em `gws`.

O que isso garante: ninguém de fora (LAN) chega nos gateways nem nos listeners (só os IPs de gateway, nunca o pool inteiro, para não afetar um host que tenha rede própria na mesma faixa); o guest não alcança nenhum outro serviço do host, nem a LAN, nem outras VMs, e falha na hora; LAN e outras VMs não chegam no guest. Subir uma VM é criar as cinco cadeias e adicionar os elementos; o cleanup remove os elementos (inclusive o de `gws`) e depois as cadeias, ignorando o que já não existir.

Forwards e DNS (supervisor):

- Cada listener escuta em `<gw>:0` (porta efêmera) com `SO_BINDTODEVICE ohm<N>` e só aceita conexão do IP do guest. Como a porta pública vira efêmera pelo `redirect`, nada colide com um listener `0.0.0.0:<porta>` do host, e o host continua livre para usar essas portas.
- `to: host:<porta>`: conecta em `127.0.0.1:<porta>`.
- `to: vm/<nome>:<porta>`: conecta em `<ip>:<porta>`, com o IP lido de `/run/ohmlatl/<uid>/<nome>/ip`. Sem o arquivo (VM parada), fecha a conexão. Só VMs do mesmo UID.
- DNS (`internet: true`): relay de 53/udp e 53/tcp para os nameservers do `/etc/resolv.conf` do host, relido quando muda. Funciona com `127.0.0.53`, dnsmasq e troca de rede. Com `internet: false`, o 53 cai no `reject` e o lookup falha na hora.

O `doctor --run` detecta firewall de outra tabela que descarta o tráfego (policy drop ou reject em FORWARD ou INPUT: Docker, ufw, firewalld) e imprime a regra que libera `ohm*`. Um drop em outra tabela vence o accept da `inet ohmlatl`.

## Imagens

- `image ls` lê `images/`; `image inspect` mostra manifest, tamanho lógico e ocupado.
- `image rm <nome:tag>` recusa imagem de VM rodando e pede confirmação.
- `image resize <nome:tag> <tamanho>` muda o tamanho do disco que a VM enxerga. Ver abaixo.
Arquivo esparso: o `rootfs.luks` tem dois tamanhos. O **tamanho do disco** é o que a VM enxerga (32 GiB por padrão); o arquivo tem esse tamanho mais os 16 MiB fixos do cabeçalho LUKS. O **espaço ocupado** é o que de fato está gravado no drive. Os trechos nunca escritos são buracos: o filesystem do host não guarda nada para eles e devolve zeros na leitura. Por isso um disco de 32 GiB com 3 GiB escritos ocupa cerca de 3 GiB, e o espaço ocupado só cresce quando o guest escreve. O `image ls` mostra os dois. Apagar arquivos dentro do guest não devolve espaço ao drive (sem discard no MVP).

`image resize <nome:tag> <tamanho>`:

- O tamanho é o do disco que a VM enxerga, arredondado para cima até múltiplo de 1 MiB. O arquivo fica com os 16 MiB do cabeçalho LUKS (offset fixo desde o build) mais esse tamanho.
- Exige a VM parada: o supervisor segura `flock` exclusivo no diretório da imagem enquanto a VM roda, e o `resize` pega o mesmo lock sem esperar; se não conseguir, falha. O `cryptsetup` do `resize` faz o próprio lock no `rootfs.luks`, sem conflito.
- Pede a passphrase no TTY do host (uma vez, sem quebra de linha no stdin) e `sudo`. O mapper é `ohmlatl-resize-<pid>` e é fechado também na falha.
- Crescer: `truncate` para o tamanho novo, `cryptsetup open`, `e2fsck -f -p`, `resize2fs` (sem tamanho: ocupa o device inteiro), `cryptsetup close`. A área nova continua esparsa, menos os metadados do ext4: as tabelas de inode da área nova, cerca de 1/64 do que foi acrescentado (perto de 512 MiB a cada 32 GiB), acabam gravadas, pelo `resize2fs` ou pelo `ext4lazyinit` do guest no primeiro boot. Interrompido no meio, o ext4 continua com o tamanho antigo e basta repetir.
- Encolher: `cryptsetup open`, `e2fsck -f -p`, `resize2fs -P` (mínimo; abaixo dele o comando recusa), `resize2fs <tamanho>`, `cryptsetup close` e só então `truncate`. Pede confirmação e recomenda backup antes, porque uma queda no meio do `resize2fs` pode corromper o ext4.
- Erro do `e2fsck` que não seja corrigido sozinho: aborta sem mudar o tamanho.
- O espaço livre do drive não limita o tamanho anunciado, mas o guest recebe erro de I/O se o drive encher; o comando avisa quando a soma dos tamanhos das imagens passa do tamanho do drive.

- `image passwd <nome:tag>` troca a passphrase: pega o lock do diretório da imagem (falha com a VM rodando), lê a passphrase atual e a nova (com confirmação) no TTY do host e roda `cryptsetup luksChangeKey --pbkdf argon2id --pbkdf-memory 262144 --pbkdf-parallel 1 --pbkdf-force-iterations 4`, com as duas entregues por arquivo de chave em memória (`memfd`), nunca em argv. Não precisa de `sudo`: só reescreve o cabeçalho do arquivo, que é do operador.

- Rebuild cria um computador novo e vazio com outra tag. Migração: com a VM antiga rodando, `ohmlatl cp` do que importa para o host; `stop`; `run` da tag nova; `ohmlatl cp` de volta. Os dados passam pelo disco do host, que é confiável.
- Código não confiável roda numa imagem separada, nunca na imagem do computador pessoal.

## Segurança

Escopo: drive roubado, perdido ou adulterado, com host confiável enquanto a VM roda, mais isolamento do workload do guest em relação ao host, à LAN e às outras VMs.

| Ameaça | Proteção |
|---|---|
| Drive roubado ou perdido | `rootfs.luks` em LUKS2 `aes-xts-plain64`; nenhuma chave LUKS nem chave SSH no drive |
| `rootfs.luks` adulterado | Sem a chave, a alteração vira lixo no guest: o boot ou o `e2fsck` falha, ou o programa quebra ("let it crash") |
| Kernel/initramfs adulterado | Embutidos no binário; não ficam soltos no drive |
| Binário `ohmlatl` adulterado no drive | Conferir o SHA-256 do release antes de rodar num host novo, ou usar um `ohmlatl` do host com `--home` |
| Outro usuário no host | `images/` e `/run/ohmlatl/<uid>/` 0700 do operador, arquivos e sockets 0600; namespacing por UID em `/run/ohmlatl` e nas units; `output` só deixa root e o dono conectarem no guest |
| Workload escapando | KVM + kernel próprio + seccomp + jailer |
| Workload alcançando host, LAN ou outras VMs | nft: reject por padrão, anti-spoof, só forwards declarados |
| LAN alcançando o guest ou os forwards | nft: nada de fora chega nos gateways nem atravessa para `ohm*` |
| Workload lendo o agente SSH do operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase em log ou disco | Lida do TTY e passada por stdin; PTY em raw; nunca em argv, env, manifest, cmdline ou log |

Limitações:

- Sem integridade: adulteração não é detectada de forma garantida. É possível voltar setores (ou o arquivo inteiro) para uma versão antiga e embaralhar setores.
- O arquivo esparso revela quais blocos já foram escritos.
- Apagar arquivos no guest não encolhe o arquivo; o drive cheio vira erro de I/O no guest.
- Tirar o drive com a VM rodando corrompe o disco dela.
- A host key do guest não é conferida (ver Acesso).
- O `manifest.yaml` fica em claro no drive. Um drive adulterado pode mudar recursos e abrir forwards para o loopback do host; o `run` imprime a lista antes de subir.
- O cache do Podman e o diretório de build guardam sistemas em claro no host do build.
- Com `internet: true` o guest sai para a internet sem filtro.
- Sem RTC: o relógio vem do `kvm-clock` e o guest deve usar `chrony` com `refclock PHC /dev/ptp0` (`ptp_kvm`), que não depende de internet.
- Sem proteção contra host comprometido, inclusive host emprestado: ele vê a passphrase e a RAM.

## Doctor

`doctor --build`: Podman rootless, pelo menos 65536 ids em subuid/subgid, `newuidmap`/`newgidmap` setuid ou com `cap_setuid`/`cap_setgid`, `/dev/fuse`, perfil AppArmor do Podman quando o host restringe user namespaces (ver abaixo), `e2fsprogs` >= 1.45, `cryptsetup` >= 2.1, `ssh-keygen` com `-A -f`, `udevadm`, `sudo`, espaço livre em `$XDG_CACHE_HOME`.

`doctor --run`: `/dev/kvm` acessível, `/dev/net/tun`, `ip`, `nft` com nat em `inet`, cgroup v2, `systemd-run`, `sudo`, cliente `ssh`, firewall de outra tabela com drop ou reject em FORWARD ou INPUT (Docker, ufw, firewalld); como aviso, `cryptsetup` >= 2.1 e `e2fsprogs` >= 1.45 para `image resize` e `image passwd`.

Ferramentas de `sbin` (`mkfs.ext4`, `e2fsck`, `resize2fs`, `cryptsetup`, `udevadm`) são procuradas também em `/usr/sbin` e `/sbin`, além do `PATH`, porque várias distros não põem `sbin` no `PATH` do usuário comum; `doctor` e os comandos usam o caminho absoluto encontrado.

Os dois conferem também o diretório do ohmlatl: filesystem ext4/btrfs/xfs, montagem sem `noexec` (aviso), dono de `images/` igual ao UID atual, sobras de build e de resize (`.partial` sem lock, mapper `ohmlatl-build-*` ou `ohmlatl-resize-*`) e espaço livre. O `doctor --run` também mostra se a trava `inet ohmlatl-fwguard` está ativa.

Falha imprime a correção sugerida e sai com código diferente de zero.

## Validação no P0

Itens que dependem do comportamento real do Firecracker v1.17 e do kernel, com teste de aceite:

- O prompt do `cryptsetup` no `ttyS0` via PTY raw aceita a passphrase e não ecoa no `serial.log`.
- `SendCtrlAltDel` com `reboot=k` encerra o Firecracker.
- `ptp_kvm` expõe `/dev/ptp0` no guest e o `chrony` sincroniza com ele.
- O `e2fsck` embutido abre um ext4 criado com o `mke2fs.conf` embutido pelo e2fsprogs do host (1.45 e o mais novo disponível).
- Os bind mounts de `vmlinux`, `initramfs` e `rootfs.luks` feitos no chroot antes do jailer continuam visíveis para o Firecracker depois do `pivot_root` do jailer. Se não continuarem, kernel e initramfs passam a ser copiados para o chroot (tmpfs) e o `rootfs.luks` entra por bind mount do diretório da imagem.

No P0 (host Kali com kernel 7.1.5, Firecracker v1.17.0, kernel do guest 6.18.33), todos passaram. Os bind mounts de arquivo aparecem no `mountinfo` do Firecracker depois do `pivot_root`, então a alternativa de cópia não é usada. O `e2fsck` 1.47.3 embutido abriu ext4 feito com o `mke2fs.conf` pelo e2fsprogs 1.45.5 e 1.47.4. O fragmento do kernel passou no `check-kconfig.sh` sem mudanças.

## Licenças e release

Código do `ohmlatl`: `MIT OR Apache-2.0`, à escolha de quem usa (`LICENSE-MIT` e `LICENSE-APACHE` na raiz; campo `license` do `Cargo.toml`). As contribuições entram sob as mesmas duas licenças.

O binário embute programas de terceiros como arquivos separados, extraídos e executados como processos próprios. Nenhum deles é ligado ao código Rust do `ohmlatl`: é agregação, e a licença liberal do `ohmlatl` não muda. As obrigações de cada licença valem para a distribuição desses componentes:

| Componente | Origem | Licença |
|---|---|---|
| Firecracker, jailer | `vendor/firecracker` (release oficial) | Apache-2.0 (com `NOTICE` e `THIRD-PARTY` do release) |
| Podman, conmon, netavark, aardvark-dns, rootlessport | bundle Podman (assets) | Apache-2.0 |
| crun | bundle Podman (assets) | GPL-2.0-or-later |
| fuse-overlayfs | bundle Podman (assets) | GPL-2.0-or-later |
| fusermount3 (libfuse) | bundle Podman (assets) | GPL-2.0-only |
| passt/pasta | bundle Podman (assets) | GPL-2.0-or-later |
| catatonit | bundle Podman (assets) | GPL-2.0-or-later |
| Linux (`vmlinux`) | build de assets | GPL-2.0-only |
| BusyBox | initramfs | GPL-2.0-only |
| cryptsetup | initramfs | GPL-2.0-or-later |
| e2fsprogs (`e2fsck`) | initramfs | GPL-2.0 |

O `runc` do bundle não é embutido (o runtime é o `crun`). Bibliotecas ligadas estaticamente nesses programas (musl, libseccomp, GLib, libcap, libargon2, libdevmapper e outras) entram na mesma lista, com a licença de cada uma. Programas Apache que ligam estaticamente bibliotecas LGPL (o `podman` com a libseccomp, o `conmon` com a GLib) também têm o código-fonte publicado, para permitir religar com outra versão da biblioteca, como pede a LGPL-2.1 §6. O próprio `ohmlatl` liga musl (MIT) e crates Rust; os avisos dessas dependências são gerados com `cargo about` e entram no mesmo `THIRD-PARTY.txt`. O mesmo vale para as dependências embutidas no bundle Podman (módulos Go do podman e crates de netavark e aardvark-dns), cujos avisos o `capture-sources.sh` junta. A cada release, a tabela é conferida contra o arquivo de licença da fonte exata usada; divergência bloqueia o release.

Artefatos de cada release:

- `ohmlatl-<versão>-x86_64`: binário estático (`x86_64-unknown-linux-musl`), para rodar em qualquer distro, inclusive num host emprestado.
- `SHA256SUMS` e `SHA256SUMS.sig`: assinatura SSH (`ssh-keygen -Y sign -n file`), que não exige ferramenta extra: todo host já tem `ssh-keygen`. A chave é a ed25519 do mantenedor (`SHA256:I6R1DMKQ7VaieP7emNay30VqsOIkyG+q8965u2CHS8w`); a pública fica em `docs/allowed_signers`, com o principal `badlucysec` e restrita ao namespace `file`, e a privada nunca sai da máquina do mantenedor. Assinar: `ssh-keygen -Y sign -f ~/.ssh/id_badlucy -n file SHA256SUMS`. Verificar: `ssh-keygen -Y verify -f allowed_signers -I badlucysec -n file -s SHA256SUMS.sig < SHA256SUMS`.
- `ohmlatl-<versão>-sources.tar.zst`: o código-fonte correspondente de todos os componentes GPL e LGPL embutidos e dos programas que ligam LGPL estaticamente, nas versões e com as configs exatas do build: o `legal-info` do Buildroot (kernel com `.config`, BusyBox com config, cryptsetup, e2fsprogs e as bibliotecas do initramfs), mais o `sources/podman/` do build do bundle (podman, conmon, crun, fuse-overlayfs, libfuse, passt, catatonit, os `APKBUILD` e fontes das bibliotecas estáticas e a receita do podman-static) e os scripts que geram tudo. Publicado junto com o binário, no mesmo release, e mantido enquanto o binário estiver disponível.
- `THIRD-PARTY.txt`: componente, versão, licença e origem de cada item, seguidos dos textos completos das licenças e dos `NOTICE`.

O binário embute o `THIRD-PARTY.txt`; `ohmlatl licenses` o imprime. Assim a cópia no drive leva os avisos junto, mesmo sem o release por perto.

O binário do release já traz o kernel e o initramfs padrão embutidos: é um arquivo só. Quem quiser os próprios assets roda `cargo xtask assets` e `cargo build --release` e obtém um binário próprio, com os assets embutidos do mesmo jeito; esse binário não confere com o `SHA256SUMS` do release, por definição.

## Pendências

- Os binários de `vendor/podman/v6.1.4/` saíram do git no P0, quando o `cargo xtask assets` passou a compilar o bundle. Eles continuam nos commits antigos do histórico; só reescrevendo o histórico (force push) eles somem de lá, e isso fica para decisão do mantenedor.
