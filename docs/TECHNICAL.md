# TECHNICAL

Detalhes técnicos do `ohmlatl`. Este documento é a fonte de verdade para artefatos, build, boot, rede e ciclo de vida. README, USAGE e ROADMAP descrevem a experiência; se divergirem deste arquivo, este arquivo vence e os outros devem ser corrigidos. Documento interno, só em pt-BR.

## Escopo do MVP

- Linux x86_64 com KVM.
- Firecracker, jailer, bundle Podman, kernel e initramfs embutidos no binário `ohmlatl`.
- O computador é um único `rootfs.luks`: LUKS2 com integridade (`hmac-sha256`) e ext4 com o sistema inteiro, incluindo `/home/<user>`. Sem discos extras.
- Guest com systemd. Sem daemon de guest, sem init próprio, sem vsock.
- Rede por TAP, bloqueada por padrão para host, LAN e outras VMs, com forwards TCP declarados no manifest.
- Acesso por SSH na rede TAP.
- Uma VM por imagem: o nome da VM é o `name` da imagem.

## Layout no host

```text
$XDG_DATA_HOME/ohmlatl/            # padrão: ~/.local/share/ohmlatl, 0700
├── images/<nome>/<tag>/
│   ├── rootfs.luks                # LUKS2 + integrity + ext4
│   ├── manifest.yaml              # manifest usado no build, com name/tag finais
│   └── ssh/
│       ├── id_ed25519             # chave do cliente, 0600
│       └── known_hosts            # host key do guest gerada no build
└── vms/<nome>/state.json          # imagem atual, subnet, status

/run/ohmlatl/                      # tmpfs, root, some no reboot
├── bin/                           # binários embutidos extraídos
├── jail/                          # chroot do jailer
└── <nome>/
    ├── console.sock               # chown para o operador, 0600
    ├── api.sock                   # API do Firecracker
    └── serial.log                 # saída do serial

$XDG_CACHE_HOME/ohmlatl/
├── bin/                           # Podman extraído
├── podman/                        # storage do Podman (cache)
└── build/                         # temporários do build, apagados no fim e na falha
```

- Arquivos 0600, diretórios 0700.
- Device-mapper, TAP e regras nft existem só enquanto são usados.
- Levar o computador para outro host é copiar `images/<nome>/<tag>/` para o mesmo caminho no outro host (`rsync -a` ou `cp -a`). O `rootfs.luks` ocupa o tamanho cheio no disco, porque o integrity grava todos os setores. Não há `image import/export` no MVP.

## Binários embutidos

O `ohmlatl` embute com `include_bytes!`:

- Firecracker e jailer (`vendor/firecracker`).
- Bundle Podman completo (`vendor/podman`): `podman`, `crun`, `conmon`, `netavark`, `aardvark-dns`, `pasta`, `fuse-overlayfs`, `catatonit` e `etc/containers`.
- Kernel `vmlinux` e `initramfs.cpio.gz`, gerados no repositório (ver abaixo).

O build extrai o Podman para `$XDG_CACHE_HOME/ohmlatl/bin`, e o supervisor extrai Firecracker, jailer, kernel e initramfs para `/run/ohmlatl/bin`. Os dois reaproveitam o que já existe se o SHA-256 bater com o embutido. A lógica do `vendor/bin/podman` (gerar `containers.conf` e `storage.conf`, exportar `CONTAINERS_*`) vira código Rust. O `registries.conf` embutido tem `unqualified-search-registries = []`: toda imagem remota precisa de nome qualificado (`docker.io/...`). O `vendor/` é só fonte de build.

Requisitos do host:

- Build: `/dev/fuse`, `newuidmap`/`newgidmap` setuid, `/etc/subuid` e `/etc/subgid`, `e2fsprogs` >= 1.43, `cryptsetup` com suporte a integrity, módulo `dm-integrity`, `ssh-keygen`, `sudo`.
- Run: `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, cliente `ssh`, `sudo`.

## Kernel e initramfs

Os dois são gerados por script no repositório e embutidos. Não existe `ohmlatl kernel` no MVP.

Kernel (config versionada, versão LTS a definir), com tudo embutido, sem módulos:

- virtio-blk, virtio-net, serial 8250, i8042 + atkbd (para Ctrl-Alt-Del)
- ext4, devtmpfs, cgroups e o que o systemd exige
- dm-crypt, dm-integrity, aes, xts, hmac, sha256, authenc
- `CONFIG_IP_PNP` (rede por `ip=`), `CONFIG_BLK_DEV_INITRD`
- IPv6 desligado
- sem `-march=native`

Initramfs: busybox estático, `cryptsetup` estático e `e2fsck` estático, com um `/init` em shell e `/etc/e2fsck.conf` contendo `broken_system_clock = 1`.

## Build

```bash
ohmlatl build [-f Containerfile | --from <ref>] [-t <nome:tag>] [--size <n>]
```

Entradas: `-f Containerfile` (ou `build.containerfile` do manifest) ou `--from <ref>`. O `ohmlatl.yaml` do diretório atual dá `user` e `vm`. `-t` vence `name` e `build.tag`; sem tag, `latest`.

1. `podman build`, ou `--from` com imagem local apenas (`--pull=never`); depois `podman create`.
2. Gerar a chave do cliente e a host key do guest em `$BUILD` (`$XDG_CACHE_HOME/ohmlatl/build/<id>/`):

```bash
ssh-keygen -q -t ed25519 -N '' -f "$BUILD/id_ed25519"
ssh-keygen -q -t ed25519 -N '' -f "$BUILD/ssh_host_ed25519_key"
```

3. Export e preparação do rootfs dentro de `podman unshare`:

```bash
podman unshare sh -euc '
  root="$BUILD/root"; mkdir "$root"
  podman export "$CID" | tar -x --same-owner --numeric-owner \
    --xattrs --xattrs-include="*" --xattrs-exclude="security.selinux" -C "$root"
  : > "$root/etc/machine-id"
  printf "%s\n" "$NAME" > "$root/etc/hostname"
  printf "127.0.0.1 localhost %s\n" "$NAME" > "$root/etc/hosts"
  rm -f "$root"/etc/ssh/ssh_host_*
  install -m 0600 -o 0 -g 0 "$BUILD/ssh_host_ed25519_key"     "$root/etc/ssh/"
  install -m 0644 -o 0 -g 0 "$BUILD/ssh_host_ed25519_key.pub" "$root/etc/ssh/"
  install -d -m 0700 -o "$UID_" -g "$GID_" "$root/home/$USER_/.ssh"
  install -m 0600 -o "$UID_" -g "$GID_" "$BUILD/id_ed25519.pub" \
    "$root/home/$USER_/.ssh/authorized_keys"
  install -d -m 0755 -o 0 -g 0 "$root/etc/NetworkManager/conf.d"
  printf "[keyfile]\nunmanaged-devices=interface-name:eth0\n" \
    > "$root/etc/NetworkManager/conf.d/ohmlatl.conf"
  ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/poweroff.target"
  ln -sf /lib/systemd/system/reboot.target "$root/etc/systemd/system/halt.target"
  du -sb "$root"
'
```

4. Validar o rootfs: systemd em `/sbin/init`, `sshd`, e `user.name` com o UID/GID declarados. Falha com mensagem clara.
5. Calcular o tamanho: `--size`, ou `ceil(du -sb * 1.25) + 512 MiB`.
6. Ler a passphrase no TTY do host, com confirmação, uma única vez. Depois `sudo -v` e o LUKS, com a passphrase entregue por stdin:

```bash
truncate -s <tamanho> rootfs.luks
cryptsetup luksFormat --type luks2 --cipher aes-xts-plain64 \
  --integrity hmac-sha256 --integrity-no-wipe --sector-size 4096 \
  --pbkdf argon2id --pbkdf-memory 262144 --pbkdf-parallel 1 --iter-time 2000 \
  --key-file=- rootfs.luks
cryptsetup open --key-file=- rootfs.luks ohmlatl-build-<id>
blockdev --getsize64 /dev/mapper/ohmlatl-build-<id>   # tamanho útil
```

7. ext4 no tamanho útil exato do mapper, dentro de `podman unshare`:

```bash
truncate -s "$USABLE" "$BUILD/rootfs.ext4"
mkfs.ext4 -q -F -L rootfs -m 0 -d "$root" "$BUILD/rootfs.ext4"
e2fsck -fn "$BUILD/rootfs.ext4"
```

8. `dd if=rootfs.ext4 of=/dev/mapper/ohmlatl-build-<id> bs=4M conv=fsync`, depois `cryptsetup close`. O `dd` escreve o mapper inteiro, inclusive os zeros, então toda tag de integrity fica válida; por isso o `--integrity-no-wipe` é seguro.
9. Mover para `images/<nome>/<tag>/`: `rootfs.luks`, `manifest.yaml` (com `name` e `tag` finais), `ssh/id_ed25519` e `ssh/known_hosts` com a linha `ohmlatl-<nome> ssh-ed25519 <pub>`. Apagar `$BUILD`.

Regras:

- O pipeline de export, chown e mkfs roda inteiro dentro de `podman unshare`. Fora dele, o rootfs fica com o UID do operador e quebra `sudo`, `sshd`, setuid e capabilities.
- `podman export`, não `podman save`.
- Nada do rootfs em claro fica fora de `$BUILD`, e `$BUILD` é apagado no fim e na falha.
- O PBKDF é fixo, para o unlock não depender do hardware de quem fez o build. A VM precisa de no mínimo 512 MiB.
- Sem segredos em `ARG`/`ENV`/`RUN`; use `podman build --secret`.
- Base de CPU genérica, sem `-march=native`.

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

- `format` é obrigatório; o `ohmlatl` recusa versões que não conhece.
- `name` + `build.tag` formam a referência. `-t` sobrescreve, e o build grava os valores finais no `manifest.yaml` da imagem.
- `build.containerfile` é opcional com `--from`.
- `vm.vcpus` e `vm.memory` podem ser sobrescritos por `run --vcpus/--memory`. Memória mínima: 512 MiB.
- `vm.network.internet`: padrão `true`.
- `vm.network.forward`: lista de forwards TCP. `port` é a porta no gateway da VM; `to` é `host:<porta>` (loopback do host) ou `vm/<nome>:<porta>`.
- O `run` usa o `manifest.yaml` da imagem. Para mudar recursos ou rede sem rebuild, edite esse arquivo com a VM parada.
- Validação: `name` em `[a-z0-9-]`, `uid`/`gid` numéricos, portas entre 1 e 65535 sem repetição, nome de VM de destino em `[a-z0-9-]`. Campos desconhecidos são erro.

## Run e boot

```bash
ohmlatl run <nome>:<tag> [--vcpus n] [--memory n]
```

1. Valida o manifest e aloca o primeiro `/30` livre em `172.31.0.0/16`, pulando redes que já aparecem em `ip route`. Grava em `state.json`.
2. `sudo systemd-run --unit ohmlatl-<nome> --collect ohmlatl supervise <nome>`. É o único sudo do `run`, feito com o TTY da CLI.
3. O supervisor (root, unidade de sistema, sobrevive ao logout do operador):
   - extrai os binários para `/run/ohmlatl/bin`;
   - cria o TAP `ohmlatl-<n>` com `172.31.x.1/30`;
   - se for a primeira VM, liga `ip_forward` (anotando o valor original) e cria a tabela `inet ohmlatl`; depois cria as chains desta VM (ver Rede);
   - abre os listeners dos forwards;
   - abre o PTY e chama o jailer direto, sem `--daemonize`, com o PTY como stdin/stdout. O jailer usa `--chroot-base-dir /run/ohmlatl/jail` e `--uid/--gid` do operador; kernel, initramfs e `rootfs.luks` entram no chroot por bind mount;
   - cria `console.sock` com chown para o operador e copia a saída do serial para `serial.log`.
4. Cmdline gerada pelo host:

```text
console=ttyS0 reboot=k panic=1 root=/dev/vda
ip=172.31.0.2::172.31.0.1:255.255.255.252:<nome>:eth0:off:<dns0>:<dns1>
```

   O DNS vem do resolver real do host (via `resolvectl` quando o `/etc/resolv.conf` aponta para `127.0.0.53`). Com `internet: false`, a cmdline vai sem DNS.

5. O `/init` do initramfs:
   - pede a passphrase no `ttyS0` (`cryptsetup open --tries 3`); três falhas: rescue shell;
   - `e2fsck -p` no rootfs; erro não corrigível: rescue shell;
   - monta o rootfs em `/newroot`;
   - grava `/newroot/etc/resolv.conf` a partir de `/proc/net/pnp`, se houver DNS;
   - `switch_root /newroot /sbin/init`.
6. O systemd sobe o `sshd` com a host key gravada no build.
7. A CLI anexa ao `console.sock`. `Ctrl-]` desanexa; a VM continua sob o supervisor.

`start <nome>` repete os passos com a imagem gravada em `state.json`. `run` com um nome de VM parada troca a imagem e inicia. Com a VM rodando, `run` falha.

## Acesso

Opções comuns de SSH (nada vem do `~/.ssh/config` do operador):

```text
-F /dev/null -i images/<nome>/<tag>/ssh/id_ed25519
-o IdentitiesOnly=yes -o ForwardAgent=no -o ForwardX11=no
-o HostKeyAlias=ohmlatl-<nome> -o StrictHostKeyChecking=yes
-o UserKnownHostsFile=images/<nome>/<tag>/ssh/known_hosts
```

- `shell` e `exec`: `ssh <opções> <user>@<guest>`.
- `cp`: `scp <opções>`.
- `console`: anexa ao `console.sock`; só um cliente por vez.
- `logs`: lê `/run/ohmlatl/<nome>/serial.log`. O log fica em tmpfs, some no reboot do host e nunca vai para o disco. O echo da passphrase fica desligado pelo `cryptsetup`.

## Stop

1. `PUT /actions {"action_type": "SendCtrlAltDel"}` no `api.sock`.
2. O systemd do guest executa `ctrl-alt-del.target`, um reboot limpo: para serviços, sync, desmonta. Com `reboot=k`, o reboot encerra o processo Firecracker.
3. Se o Firecracker não sair em 30 s, kill.
4. O supervisor fecha os forwards, remove o TAP e as chains da VM e sai. Se não sobrar nenhum TAP `ohmlatl-*` (lido de `ip link`), remove a tabela `inet ohmlatl` e restaura o `ip_forward` original.

Dentro do guest, `poweroff`, `halt` e `reboot` encerram a VM: o build aponta `poweroff.target` e `halt.target` para `reboot.target`, porque o Firecracker não tem power off e só sai no reboot.

Não depende de SSH.

## Rede

- Um TAP e um `/30` por VM: host `.1`, guest `.2`.
- Tabela `inet ohmlatl`, compartilhada, com chains por VM:
  - `input` vindo do TAP: aceita `established,related`; aceita TCP para `172.31.x.1` nas portas de `forward`; descarta o resto. O guest não alcança nenhum serviço do host fora dos forwards.
  - `forward` vindo do TAP: descarta destinos `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16` e `100.64.0.0/10`, o que cobre LAN e outras VMs; com `internet: true`, aceita o resto com masquerade na saída; com `internet: false`, descarta tudo.
  - Tráfego do host para o guest é livre: o host acessa `172.31.x.2:<porta>`.
- Forwards: o supervisor escuta TCP em `172.31.x.1:<port>` e, para cada conexão, abre outra para `127.0.0.1:<porta>` (`to: host:...`) ou para o IP atual da VM de destino, lido do `state.json` dela (`to: vm/...`). Se a VM de destino estiver parada, a conexão é fechada. O guest sempre fala com o gateway. O `run` imprime os forwards ao subir.
- Só TCP. IPv6 desligado.
- Docker e ufw costumam deixar a chain FORWARD com policy drop, e um drop em outra tabela vence o accept da `inet ohmlatl`. O `doctor --run` detecta e imprime a regra que libera `ohmlatl-*`.
- Sem publicação de portas, ingress ou proxy público.

## Segurança

Escopo: `rootfs.luks` roubado ou adulterado (notebook perdido, arquivo copiado), com host confiável enquanto a VM roda, mais isolamento do workload do guest.

| Ameaça | Proteção |
|---|---|
| `rootfs.luks` roubado | LUKS2; nenhuma chave LUKS em disco |
| `rootfs.luks` adulterado | Integrity `hmac-sha256` por setor: o setor alterado dá erro de I/O no guest |
| Kernel/initramfs adulterado | Não ficam em disco; vêm do binário `ohmlatl` |
| Outro usuário no host | Arquivos 0600, diretórios 0700; `console.sock` só do operador |
| Workload escapando | KVM + kernel próprio + seccomp + jailer |
| Workload alcançando host, LAN ou outras VMs | nft bloqueia por padrão; só os forwards declarados passam |
| Workload lendo o agente SSH do operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase em log ou disco | Lida do TTY e passada por stdin; nunca em argv, env, manifest, cmdline ou log |

Limitações:

- Sem proteção contra rollback: uma cópia antiga inteira do `rootfs.luks` (ou setores antigos) passa no integrity.
- Integrity com journal: escrita de 2x a 3x mais lenta, e o arquivo ocupa o tamanho cheio.
- A chave SSH privada fica em claro no host (0600). Ela só dá acesso à VM rodando, pela rede local do TAP.
- Rebuild gera um computador novo: os dados do rootfs anterior não passam para o novo.
- Com `internet: true` o guest sai para a internet sem filtro.
- Sem RTC: o relógio do guest começa errado até o `chrony` sincronizar.
- Sem proteção contra host comprometido.

## Doctor

`doctor --build` checa: rootless Podman, subuid/subgid, `newuidmap`/`newgidmap`, `/dev/fuse`, `e2fsprogs` >= 1.43, `cryptsetup` com integrity, `dm-integrity`, `ssh-keygen`, espaço livre em `$XDG_DATA_HOME` e `$XDG_CACHE_HOME`.

`doctor --run` checa: `/dev/kvm` com acesso, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, cliente `ssh`, chain FORWARD com policy drop (Docker, ufw).

Falha imprime a correção sugerida e sai com código diferente de zero.

## Pendências

- Versão e config do kernel.
- Licença e avisos de terceiros (Firecracker, Podman, busybox, cryptsetup).
- Publicar SHA-256 e assinatura do `ohmlatl`.
