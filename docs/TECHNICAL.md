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
- O diretório e o binário são 0755 para o `ohmlatl` rodar mesmo num host em que você tem outro UID; o que é privado fica em `images/` (0700, arquivos 0600). O dono de `images/` tem que ser o UID atual. Se não for, a CLI para e mostra `sudo chown -R $(id -u):$(id -g) <dir>/images`.
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
├── bin/<sha256>/                  # Podman extraído
├── podman/                        # storage do Podman: cache, guarda sistemas em claro
└── build/<id>/                    # rootfs exportado durante o build, apagado no fim e na falha
```

- O cache do Podman guarda as imagens de container em claro no host do build. `ohmlatl podman system reset` limpa.
- Backup de um computador: com a VM parada, `rsync -aS` (ou `cp -a --sparse=always`) do diretório `images/<nome>/<tag>/` para outro filesystem ext4/btrfs/xfs.

## Binários embutidos

O `ohmlatl` embute com `include_bytes!`:

- Firecracker e jailer (`vendor/firecracker`).
- Bundle Podman completo (`vendor/podman`): `podman`, `crun`, `conmon`, `netavark`, `aardvark-dns`, `pasta`, `fuse-overlayfs`, `catatonit`, `etc/containers/policy.json`.
- Kernel `vmlinux`, `initramfs.cpio.gz` e `mke2fs.conf`, gerados no repositório.

O build extrai o Podman para `$XDG_CACHE_HOME/ohmlatl/bin/<sha256>/`. O `launch` (root) extrai Firecracker, jailer, kernel, initramfs e uma cópia do próprio `ohmlatl` para `/run/ohmlatl/bin/<sha256>/`. A extração é atômica: escreve num diretório temporário ao lado e faz `rename()` para `<sha256>/`; se o destino já existe, reaproveita. O hash é do binário `ohmlatl`, então versões diferentes não se misturam e duas execuções simultâneas nunca veem uma extração pela metade. Como a unit roda a cópia em `/run`, nada da VM depende do binário no drive depois que ela sobe.

O `ohmlatl` gera em runtime a configuração do Podman (hoje feita pelo `vendor/bin/podman`):

- `containers.conf` e `storage.conf` (driver overlay com `fuse-overlayfs`, sem `ignore_chown_errors`);
- `registries.conf` com `unqualified-search-registries = []` e `short-name-mode = "enforcing"`: imagem remota precisa de nome qualificado (`docker.io/...`).

Requisitos do host:

- Build: Podman rootless (`newuidmap`/`newgidmap` setuid, pelo menos 65536 ids em `/etc/subuid` e `/etc/subgid`, `/dev/fuse`), `e2fsprogs` >= 1.45 (`mkfs.ext4 -d` com xattrs e o feature set fixo), `cryptsetup` >= 2.1, `ssh-keygen` com `-A -f`, `udevadm`, `sudo`.
- Run: `/dev/kvm`, `/dev/net/tun`, `ip`, `nft` com kernel >= 5.2 (nat na família `inet`), cgroup v2, systemd com `systemd-run`, cliente `ssh`, `sudo`.

`sudo ohmlatl` equivale a root. Quem pode rodar o `ohmlatl` com sudo pode virar root; é o mesmo nível de quem já tem sudo.

O binário no drive é tão confiável quanto o drive. Num host novo, confira o SHA-256 do binário contra o do release antes de rodar, ou use um `ohmlatl` instalado no host com `--home`.

## Kernel e initramfs

Gerados por script no repositório e embutidos. Não existe `ohmlatl kernel` no MVP.

Kernel: config versionada, versão LTS a definir, `CONFIG_MODULES=n` (tudo embutido; o guest não carrega módulos), sem `-march=native`:

- Plataforma: `VIRTIO_MMIO`, `VIRTIO_MMIO_CMDLINE_DEVICES`, `VIRTIO_BLK`, `VIRTIO_NET`, `HW_RANDOM_VIRTIO`, `SERIAL_8250`, `SERIAL_8250_CONSOLE`, `INPUT`, `INPUT_KEYBOARD`, `SERIO_I8042`, `KEYBOARD_ATKBD`, `VT` (o Ctrl-Alt-Del passa pelo teclado do VT), `PTP_1588_CLOCK_KVM`, `KVM_GUEST`.
- Boot e systemd: `BLK_DEV_INITRD`, `DEVTMPFS`, `DEVTMPFS_MOUNT`, `TMPFS`, `TMPFS_POSIX_ACL`, `FHANDLE`, `AUTOFS_FS`, `CGROUPS` (v2, cpu, memory, pids, io, bpf), `SECCOMP`, `UNIX`, `INET`, `IP_PNP`, `EPOLL`, `SIGNALFD`, `TIMERFD`, `INOTIFY_USER`.
- Disco: `BLK_DEV_DM`, `DM_CRYPT`, `CRYPTO_AES_NI_INTEL`, `CRYPTO_XTS`, `EXT4_FS`, `EXT4_FS_POSIX_ACL`, `EXT4_FS_SECURITY`, `VFAT_FS`, `ISO9660_FS`, `SQUASHFS`, `OVERLAY_FS`, `FUSE_FS`.
- Rede e containers dentro do guest: `NETFILTER`, `NF_TABLES` (inet, nat, masq, reject), `NETFILTER_XTABLES` com os matches usados pelo `iptables-nft`, `BRIDGE`, `VETH`, `TUN`, `WIREGUARD`, `NAMESPACES` (net, user, pid, ipc, uts, cgroup), `BPF_SYSCALL`, `IPV6`.

Initramfs: busybox estático, `cryptsetup` estático, `e2fsck` estático da mesma versão do e2fsprogs que define o `mke2fs.conf` embutido, `/init` em shell, `/etc/e2fsck.conf` com `broken_system_clock = 1`.

`mke2fs.conf` embutido: `base_features = sparse_super,large_file,filetype,resize_inode,dir_index,ext_attr` e perfil ext4 com `has_journal,extent,huge_file,flex_bg,metadata_csum,64bit,dir_nlink,extra_isize`, sem `orphan_file` e sem `metadata_csum_seed`. `ext_attr` é obrigatório: sem ele o `mkfs -d` perde xattrs e capabilities. O build usa `MKE2FS_CONFIG` apontando para ele e passa `-O` com a lista completa (base + perfil), então o resultado não depende da distro do host.

## Build

```bash
ohmlatl build [-f Containerfile | --from <ref>] [-t <nome:tag>] [--size <n>]
```

Entradas:

- Exige `ohmlatl.yaml` no diretório atual. Ele dá `user` e `vm`, e o contexto do `podman build` é esse diretório.
- `-f Containerfile` (ou `build.containerfile`) faz `podman build`. `--from <ref>` usa uma imagem **local** (`--pull=never`); imagem remota se traz antes com `ohmlatl podman pull docker.io/...`.
- `-t` vence `name` e `build.tag`; sem tag, `latest`.
- Validação: nome `^[a-z][a-z0-9-]{0,30}$`, tag `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
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
  # validação (ver acima) roda aqui
  : > "$root/etc/machine-id"
  printf "%s\n" "$NAME" > "$root/etc/hostname"
  printf "127.0.0.1 localhost %s\n" "$NAME" > "$root/etc/hosts"
  rm -f "$root/etc/resolv.conf"; : > "$root/etc/resolv.conf"
  ls "$root"/etc/ssh/ssh_host_*_key >/dev/null 2>&1 || ssh-keygen -A -f "$root"
  install -d -m 0755 -o 0 -g 0 "$root/etc/NetworkManager/conf.d"
  printf "[keyfile]\nunmanaged-devices=interface-name:eth0\n" \
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

3. Tamanho: `--size`, ou `max(32 GiB, 2 x conteúdo + 1 GiB)`, com o conteúdo em blocos alocados (`du -sB1` dentro do unshare); arredondado para cima até múltiplo de 1 MiB. O número de inodes do ext4 tem que ser pelo menos o dobro do número de arquivos; se não for, o build passa `-N`. O espaço livre no drive é conferido contra o que o build vai escrever.
4. Passphrase: lida do TTY, com confirmação, uma vez. Recusa vazia e com `\n` ou `\r`. É escrita sem quebra de linha no stdin do `cryptsetup`.
5. LUKS. Antes, `sudo -v`; durante o resto do build, um laço em segundo plano roda `sudo -n -v` a cada 60 s, para o sudo não pedir senha de novo no meio de um `mkfs` demorado:

```bash
mkdir "$PART"; exec 9>"$PART/.lock"; flock -n 9   # lock do build, solto ao sair
truncate -s "$SIZE" "$PART/rootfs.luks"
sudo cryptsetup luksFormat --batch-mode --type luks2 \
  --cipher aes-xts-plain64 --key-size 512 --sector-size 4096 \
  --pbkdf argon2id --pbkdf-memory 262144 --pbkdf-parallel 1 --pbkdf-force-iterations 4 \
  --key-file=- "$PART/rootfs.luks"
sudo cryptsetup open --key-file=- "$PART/rootfs.luks" ohmlatl-build-<id>
```

   O `mkdir` sem `-p` é exclusivo: dois builds da mesma tag não dividem o mesmo `$PART`. Se `$PART` já existe e o `.lock` dele não está preso por ninguém, é sobra de um build morto (SIGKILL, queda de energia): o build fecha qualquer mapper `ohmlatl-build-*` cujo arquivo esteja nele, apaga o `$PART` e segue. O `doctor --build` faz a mesma limpeza. O PBKDF é fixo (sem benchmark): o unlock no guest não depende da máquina que fez o build. 256 MiB de argon2 cabem na memória mínima da VM (512 MiB).

6. ext4 direto no mapper, com donos corretos:

```bash
sudo udevadm settle
sudo chown "$(id -u)" "$(realpath /dev/mapper/ohmlatl-build-<id>)"
podman unshare env MKE2FS_CONFIG="$BIN/mke2fs.conf" mkfs.ext4 -q -F \
  -T ohmlatl -O "$FEATURES" -L rootfs -m 0 \
  -E lazy_itable_init=0,lazy_journal_init=0,nodiscard \
  -d "$BUILD/root" /dev/mapper/ohmlatl-build-<id>
sudo e2fsck -fn /dev/mapper/ohmlatl-build-<id>
sudo cryptsetup close ohmlatl-build-<id>
```

   O `chown` vem logo antes do `mkfs` porque o udev devolve o nó para `root:disk` quando o device é fechado. O que não foi escrito continua buraco no arquivo; o ext4 não lê blocos livres, então o conteúdo deles não importa. As tabelas de inode e o journal são escritos no build (cerca de 800 MiB num disco de 32 GiB).

7. Gravar `$PART/manifest.yaml` (com `name` e `tag` finais) e `rename` de `$PART` para `images/<nome>/<tag>`, no mesmo filesystem, o que preserva o sparse.
8. Trap (sucesso ou falha): `podman unshare rm -rf "$BUILD"`, `cryptsetup close` se aberto, `rm -rf "$PART"` se não renomeado, `podman rm` do container, fim do laço do `sudo`.

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
- `build.containerfile` é opcional com `--from`.
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
4. `sudo <exe> launch --uid <uid> --gid <gid> --home <dir> --image <nome>:<tag> [--vcpus n] [--memory n] [--pool <cidr>]`, com `<exe>` = destino de `/proc/self/exe` (o `secure_path` do sudo não inclui o drive nem `~/.local/bin`).
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
  -p Delegate=yes -p KillMode=mixed -p TimeoutStopSec=150 -p RequiresMountsFor=<home> \
  -p "ExecStopPost=/run/ohmlatl/bin/<sha256>/ohmlatl cleanup <uid> <nome>" \
  /run/ohmlatl/bin/<sha256>/ohmlatl supervise --uid <uid> --gid <gid> --home <dir> --image <nome>:<tag> ...
```

   A unit roda a cópia em `/run/ohmlatl/bin`, então trocar ou apagar o binário do drive com a VM rodando não quebra o cleanup. `RequiresMountsFor` faz o systemd parar a VM antes de desmontar o drive no desligamento do host (e também se o drive for desmontado com a VM rodando). `KillMode=mixed` manda o SIGTERM só para o supervisor, que desliga o guest; sem isso o systemd mataria o Firecracker direto.

Supervisor (root, unit de sistema, sobrevive ao logout):

1. Lê o `manifest.yaml` da imagem no drive. Qualquer falha a partir daqui vai para `error` (do operador) e para o journal.
2. Rede, sob `flock /run/ohmlatl/net.lock` (ver Rede): aloca o `/30`, cria o TAP e as regras, grava `/run/ohmlatl/net/<uid>-<nome>`.
3. Abre os listeners dos forwards e do DNS.
4. Abre o PTY e coloca o slave em modo raw (`cfmakeraw`), para o eco do host nunca devolver a passphrase para a saída.
5. Prepara o chroot do jailer: bind mount read-only de `vmlinux` e `initramfs`, bind mount read-write de `rootfs.luks` (do operador, 0600, no drive), dentro de `/run/ohmlatl/jail/firecracker/<uid>-<nome>/root/`. O supervisor segura `flock` exclusivo no `rootfs.luks` até sair, o que impede `image resize` e `image rm` com a VM rodando. O drive não é confiável para o root: `manifest.yaml` e `rootfs.luks` são abertos com `openat2(RESOLVE_NO_SYMLINKS)`, têm que ser arquivo regular com dono `--uid`, e o bind mount usa `/proc/self/fd/<n>` do arquivo já aberto. Symlink, device ou dono errado: falha.
6. Chama o jailer direto, sem `--daemonize`, com o PTY slave em stdin, stdout e stderr:

```bash
jailer --id <uid>-<nome> --exec-file /run/ohmlatl/bin/<sha256>/firecracker \
  --uid <uid> --gid <gid> --chroot-base-dir /run/ohmlatl/jail \
  --cgroup-version 2 --parent-cgroup system.slice/ohmlatl-<uid>-<nome>.service \
  -- --api-sock /run/api.sock --log-path /run/firecracker.log --level Warning
```

   `--parent-cgroup` é o cgroup da unit, lido de `/proc/self/cgroup` sem a `/` inicial (o jailer recusa caminho absoluto). Sem `--cgroup`, o jailer põe o Firecracker nesse cgroup, então o systemd enxerga e mata o processo junto com a unit. O jailer cria `/dev/kvm` e `/dev/net/tun` no chroot para o UID do operador. O log do Firecracker vai para `/run/firecracker.log` dentro do chroot, e o supervisor repassa para o journal; assim ele não se mistura com o serial no PTY.

7. Configura pela API (`/run/ohmlatl/jail/firecracker/<uid>-<nome>/root/run/api.sock`):
   - `boot-source`: `vmlinux`, `initramfs`, cmdline (abaixo);
   - `drives/rootfs`: `rootfs.luks`, `is_root_device: false`, `is_read_only: false`, `cache_type: "Writeback"` (o padrão `Unsafe` ignora flush do guest e arrisca corromper o ext4 numa queda de energia);
   - `network-interfaces/eth0`: `host_dev_name: ohm<N>`;
   - `machine-config`: vcpus e memória;
   - `entropy`: virtio-rng;
   - `actions`: `InstanceStart`.
8. Grava `ip`, cria `ctl.sock` e `console.sock` com `chown <uid>:<gid>` e modo 0600, responde `ready`. Copia a saída do PTY para o cliente do console e para `serial.log` (buffer circular de 1 MiB, do operador). Ao anexar, o cliente recebe primeiro os últimos 64 KiB do buffer, então o prompt da passphrase aparece mesmo se já tiver sido impresso.

Cmdline:

```text
console=ttyS0 reboot=k panic=1 ip=<guest>::<gw>:255.255.255.252:<nome>:eth0:off
```

`/init` do initramfs:

1. Monta `proc`, `sys`, `devtmpfs`. Instala `trap 'reboot -f' INT`: antes do `switch_root`, o Ctrl-Alt-Del do `stop` chega como SIGINT no PID 1, que é este script. O `cryptsetup` e o rescue rodam como filho em segundo plano com `wait`, para o trap disparar na hora em vez de o `stop` esperar 120 s.
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
-o ControlMaster=auto -o ControlPath=/run/ohmlatl/<uid>/<nome>/ssh.sock -o ControlPersist=yes
```

- A autenticação é a que a imagem tiver: senha do usuário, ou as chaves do operador (`~/.ssh/id_*`, agente) se a pública estiver no `authorized_keys` da imagem. O `ohmlatl` não gera nem guarda chaves.
- A host key não é conferida: a conexão vai do host para o guest por um TAP ponto a ponto local, e só o root do host conseguiria se colocar no meio. O host já é confiável por premissa e vê a RAM da VM. Por isso não existe `known_hosts`, e trocar de imagem não gera alerta.
- `ControlMaster`: antes de `shell`, `exec` ou `cp`, a CLI roda `ssh <opções> -O check ohmlatl`. Se não houver mestre, abre um com `ssh <opções> -o ControlMaster=yes -fN <user>@<ip>`, que pede a senha uma vez. Depois disso, todas as conexões até o próximo `stop` reaproveitam o mestre. Isso vale também para o `scp`, que força `ControlMaster=no` e nunca vira mestre sozinho.
- `shell` e `exec`: `ssh <opções> <user>@<ip do guest>`, com o IP vindo do `ctl.sock`.
- `cp`: `scp -r <opções>`; diretórios são copiados recursivamente.
- `console`: anexa ao `console.sock` e recebe o fim do buffer do serial; só um cliente por vez. O login é automático.
- `logs`: lê `/run/ohmlatl/<uid>/<nome>/serial.log`. Fica em tmpfs, some no reboot do host e é preservado entre o `stop` e o próximo `run`.
- `ps`: lista `/run/ohmlatl/<uid>/*/ctl.sock` e consulta cada um (nome, imagem, IP, pid, uptime). Só mostra VMs rodando; o que existe no drive aparece em `image ls`.

## Stop

1. A CLI manda `stop` pelo `ctl.sock`. O SIGTERM do systemd (`systemctl stop`, desligamento do host) leva ao mesmo caminho.
2. O supervisor manda `PUT /actions {"action_type": "SendCtrlAltDel"}` no `api.sock`.
3. O systemd do guest executa `ctrl-alt-del.target`, um reboot limpo. Com `reboot=k`, o reboot encerra o Firecracker. Dentro do guest, `poweroff`, `halt` e `reboot` também encerram a VM, porque os targets apontam para `reboot.target` (o Firecracker não tem power off).
4. Se o Firecracker não sair em 120 s (acima dos 90 s padrão do systemd), o supervisor mata.
5. Cleanup. Depois dele, nada mais segura arquivos no drive e ele pode ser desmontado.

Cleanup (`ohmlatl cleanup <uid> <nome>`), idempotente. Roda no fim do supervisor e de novo no `ExecStopPost`, que também roda quando o supervisor morre:

1. Mata o Firecracker se ainda estiver vivo.
2. Desmonta os bind mounts e remove o chroot.
3. Sob o lock de rede, lê `/run/ohmlatl/net/<uid>-<nome>`: remove o TAP e as regras da VM; se não sobrar nenhum TAP `ohm*`, remove a tabela `inet ohmlatl`; apaga o registro.
4. Encerra a conexão SSH mestre (`ssh -F /dev/null -o ControlPath=.../ssh.sock -O exit ohmlatl`; o destino é só um placeholder, que o `ssh` exige) e depois remove `ctl.sock`, `console.sock`, `ssh.sock` e `ip`. `serial.log` e `error` ficam até o próximo `run`.

## Rede

Alocação, sob `flock /run/ohmlatl/net.lock`:

- Antes de alocar, o supervisor confere o pool (`172.31.0.0/16`, ou `OHMLATL_POOL`): se algum endereço ou rota do host que não seja de `ohm*` nem a rota padrão cai dentro dele, recusa subir e manda usar `OHMLATL_POOL`.
- O `/30` é o primeiro livre no pool que não pertence a outro registro em `/run/ohmlatl/net/`. É alocado a cada `run`; o guest nunca depende do IP, porque usa `ohmlatl-gateway`. Uma rede remota que usa a mesma faixa mas é alcançada pela rota padrão (por exemplo, uma VPC) não é detectável: nesse caso, configure `OHMLATL_POOL`.
- TAP `ohm<N>` (primeiro N livre): `ip tuntap add dev ohm<N> mode tap user <uid>`, `ip addr add <gw>/30 dev ohm<N>`, `sysctl net.ipv6.conf.ohm<N>.disable_ipv6=1`, `ip link set ohm<N> up`.
- `net.ipv4.ip_forward=1`. Nunca é restaurado: outros serviços (Docker) podem passar a depender dele. Se estava desligado e `/run/ohmlatl/net/forward-was-off` ainda não existe, o supervisor grava esse arquivo e cria a tabela `inet ohmlatl-fwguard` com `iifname != "ohm*" oifname != "ohm*" drop` no `forward`, para o host não virar roteador entre as outras interfaces (LAN, VPN). Essa tabela não é apagada pelo cleanup; fica até o reboot. Efeito colateral: um Docker ou libvirt iniciado depois não roteia enquanto ela existir. O `doctor --run` mostra a trava e o comando para tirá-la (`sudo nft delete table inet ohmlatl-fwguard`).

Tabela `inet ohmlatl` (criada se não existe), com os sets `gws` (IPs de gateway em uso) e `blocked` (`flags interval`: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `100.64.0.0/10` e as redes conectadas do host, inclusive públicas, atualizadas pelo supervisor a cada mudança de rota via netlink). Por VM, regras marcadas com o comentário `<uid>-<nome>`:

- `prerouting` (filter, prioridade raw): `iifname "ohm<N>" ip saddr != <guest> drop`; IPv6 de `ohm*`: drop.
- `prerouting` (nat): `iifname "ohm<N>" ip daddr <gw> tcp dport <port> redirect to :<porta efêmera>` para cada forward; com `internet: true`, o mesmo para 53/udp e 53/tcp.
- `input` (filter):
  - `ip daddr @gws iifname != "lo" iifname != "ohm*" drop`: ninguém de fora (LAN) chega nos gateways nem nos listeners. Só os IPs de gateway, nunca o pool inteiro, para não afetar um host que tenha rede própria na mesma faixa;
  - `iifname "ohm<N>" ct state established,related accept`;
  - `iifname "ohm<N>" ip daddr <gw> ct status dnat ct original proto-dst { <ports> } accept` (e 53 com `internet: true`);
  - `iifname "ohm*" reject`: o guest não alcança nenhum outro serviço do host, e falha na hora.
- `forward` (filter):
  - `oifname "ohm*" ct state established,related accept`; `oifname "ohm*" drop`: LAN e outras VMs não chegam no guest;
  - `iifname "ohm*" ip daddr @blocked reject`: o guest não alcança LAN nem outras VMs;
  - `iifname "ohm<N>" accept` com `internet: true`; `iifname "ohm<N>" reject` com `internet: false`.
- `postrouting` (nat): `iifname "ohm*" oifname != "ohm*" masquerade`.
- `output` (filter): `oifname "ohm<N>" meta skuid != { 0, <uid> } reject`. Só o root e o dono da VM abrem conexão do host para o guest; outros usuários locais não alcançam os serviços dele.

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
Arquivo esparso: o `rootfs.luks` tem dois tamanhos. O **tamanho do disco** é o que a VM enxerga (32 GiB por padrão) e é o que o `truncate` define. O **espaço ocupado** é o que de fato está gravado no drive. Os trechos nunca escritos são buracos: o filesystem do host não guarda nada para eles e devolve zeros na leitura. Por isso um disco de 32 GiB com 3 GiB escritos ocupa cerca de 3 GiB, e o espaço ocupado só cresce quando o guest escreve. O `image ls` mostra os dois. Apagar arquivos dentro do guest não devolve espaço ao drive (sem discard no MVP).

`image resize <nome:tag> <tamanho>`:

- O tamanho é o do disco que a VM enxerga, arredondado para cima até múltiplo de 1 MiB. O arquivo fica com o offset de dados do LUKS (lido do cabeçalho) mais esse tamanho.
- Exige a VM parada: o supervisor segura `flock` exclusivo no `rootfs.luks` enquanto a VM roda, e o `resize` pega o mesmo lock sem esperar; se não conseguir, falha.
- Pede a passphrase no TTY do host (uma vez, sem quebra de linha no stdin) e `sudo`.
- Crescer: `truncate` para o tamanho novo, `cryptsetup open`, `e2fsck -f -p`, `resize2fs` (sem tamanho: ocupa o device inteiro), `cryptsetup close`. A área nova continua esparsa; só os metadados do ext4 da área nova são gravados. Interrompido no meio, o ext4 continua com o tamanho antigo e basta repetir.
- Encolher: `cryptsetup open`, `e2fsck -f -p`, `resize2fs -P` (mínimo; abaixo dele o comando recusa), `resize2fs <tamanho>`, `cryptsetup close` e só então `truncate`. Pede confirmação e recomenda backup antes, porque uma queda no meio do `resize2fs` pode corromper o ext4.
- Erro do `e2fsck` que não seja corrigido sozinho: aborta sem mudar o tamanho.
- O espaço livre do drive não limita o tamanho anunciado, mas o guest recebe erro de I/O se o drive encher; o comando avisa quando a soma dos tamanhos das imagens passa do tamanho do drive.

- Trocar a passphrase, com a VM parada:

```bash
cryptsetup luksChangeKey --pbkdf argon2id --pbkdf-memory 262144 \
  --pbkdf-parallel 1 --pbkdf-force-iterations 4 \
  <home>/images/<nome>/<tag>/rootfs.luks
```

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

`doctor --build`: Podman rootless, pelo menos 65536 ids em subuid/subgid, `newuidmap`/`newgidmap` setuid, `/dev/fuse`, `e2fsprogs` >= 1.45, `cryptsetup` >= 2.1, `ssh-keygen` com `-A -f`, `udevadm`, `sudo`, espaço livre em `$XDG_CACHE_HOME`.

`doctor --run`: `/dev/kvm` acessível, `/dev/net/tun`, `ip`, `nft` com nat em `inet`, cgroup v2, `systemd-run`, `sudo`, cliente `ssh`, firewall de outra tabela com drop ou reject em FORWARD ou INPUT (Docker, ufw, firewalld).

Os dois conferem também o diretório do ohmlatl: filesystem ext4/btrfs/xfs, montagem sem `noexec` (aviso), dono de `images/` igual ao UID atual, sobras de build (`.partial` sem lock, mapper `ohmlatl-build-*`) e espaço livre. O `doctor --run` também mostra se a trava `inet ohmlatl-fwguard` está ativa.

Falha imprime a correção sugerida e sai com código diferente de zero.

## Validação no P0

Itens que dependem do comportamento real do Firecracker v1.17 e do kernel, com teste de aceite:

- O prompt do `cryptsetup` no `ttyS0` via PTY raw aceita a passphrase e não ecoa no `serial.log`.
- `SendCtrlAltDel` com `reboot=k` encerra o Firecracker.
- `ptp_kvm` expõe `/dev/ptp0` no guest e o `chrony` sincroniza com ele.
- O `e2fsck` embutido abre um ext4 criado com o `mke2fs.conf` embutido pelo e2fsprogs do host (1.45 e o mais novo disponível).
- Os bind mounts de `vmlinux`, `initramfs` e `rootfs.luks` feitos no chroot antes do jailer continuam visíveis para o Firecracker depois do `pivot_root` do jailer. Se não continuarem, kernel e initramfs passam a ser copiados para o chroot (tmpfs) e o `rootfs.luks` entra por bind mount do diretório da imagem.

## Pendências

- Versão e config final do kernel.
- Licença do projeto e avisos de terceiros.
- Release com SHA-256, assinatura e oferta do código-fonte dos componentes GPL embutidos (kernel, busybox, cryptsetup, e2fsprogs, crun, pasta, conmon).
