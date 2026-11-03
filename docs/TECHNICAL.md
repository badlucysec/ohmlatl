# TECHNICAL

Detalhes técnicos do `ohmlatl`. Este documento é a fonte de verdade para artefatos, build, boot, rede e ciclo de vida. README, USAGE e ROADMAP descrevem a experiência; se divergirem deste arquivo, este arquivo vence e os outros devem ser corrigidos. Documento interno, só em pt-BR.

## Escopo do MVP

- Linux x86_64 com KVM.
- Firecracker, jailer, bundle Podman, kernel e initramfs embutidos no binário `ohmlatl`.
- Rootfs em LUKS2 com integridade (`hmac-sha256`). `/home/<user>` mora dentro do rootfs.
- Volumes genéricos, com ou sem LUKS2, montados onde o manifest mandar.
- Guest com systemd. Sem daemon de guest, sem init próprio, sem vsock.
- Acesso por SSH na rede TAP.
- Uma VM por imagem: o nome da VM é o `name` da imagem.

## Layout do drive

```text
/mnt/vmdisk/
├── bin/ohmlatl
├── images/<nome>/<tag>/
│   ├── rootfs.luks        # LUKS2 + integrity + ext4
│   ├── manifest.yaml      # cópia do ohmlatl.yaml usado no build
│   └── ssh/id_ed25519     # chave do cliente, 0600
├── volumes/<volume>.luks  # LUKS2 + ext4
├── volumes/<volume>.ext4  # ext4 simples
└── vms/<nome>/
    ├── state.json         # imagem atual, subnet, pid, status
    ├── known_hosts
    ├── serial.log
    ├── console.sock
    └── lock
```

Estado no host:

- `$XDG_RUNTIME_DIR/ohmlatl/bin`: binários embutidos extraídos (tmpfs, 0700). Efêmero.
- `$XDG_RUNTIME_DIR/ohmlatl/jail`: chroot do jailer. Efêmero.
- `$XDG_CACHE_HOME/ohmlatl/podman`: storage do Podman. É cache de build, não viaja com o drive.
- Device-mapper, TAP e tabela nft existem só enquanto são usados.

Nada persistente fica no host fora do cache de build.

## Binários embutidos

O `ohmlatl` embute com `include_bytes!`:

- Firecracker e jailer (`vendor/firecracker`).
- Bundle Podman completo (`vendor/podman`): `podman`, `crun`, `conmon`, `netavark`, `aardvark-dns`, `pasta`, `fuse-overlayfs`, `catatonit` e `etc/containers`.
- Kernel `vmlinux` e `initramfs.cpio.gz`, gerados no repositório (ver abaixo).

Na primeira chamada que precisa deles, o `ohmlatl` extrai para `$XDG_RUNTIME_DIR/ohmlatl/bin` e reaproveita se o SHA-256 bater com o embutido. A lógica do `vendor/bin/podman` (gerar `containers.conf` e `storage.conf`, exportar `CONTAINERS_*`) vira código Rust. O `vendor/` é só fonte de build.

Requisitos do host:

- Build: `/dev/fuse`, `newuidmap`/`newgidmap` setuid, `/etc/subuid` e `/etc/subgid`, `e2fsprogs` >= 1.43, `cryptsetup` com suporte a integrity, módulo `dm-integrity`, `sudo`.
- Run: `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, cliente `ssh`, `sudo`.
- Montar volume LUKS no host: `cryptsetup`.

## Kernel e initramfs

Os dois são gerados por script no repositório e embutidos. Não ficam no drive e não existe `ohmlatl kernel` no MVP.

Kernel (config versionada, versão LTS a definir), com tudo embutido, sem módulos:

- virtio-blk, virtio-net, serial 8250, i8042 + atkbd (para Ctrl-Alt-Del)
- ext4, devtmpfs, cgroups e o que o systemd exige
- dm-crypt, dm-integrity, aes, xts, hmac, sha256, authenc
- `CONFIG_IP_PNP` (rede por `ip=`), `CONFIG_BLK_DEV_INITRD`
- sem `-march=native`

Initramfs: busybox estático, `cryptsetup` estático e `e2fsck` estático, com um `/init` em shell e `/etc/e2fsck.conf` contendo `broken_system_clock = 1`.

## Build

Entradas: `-f Containerfile` (ou `build.containerfile` do manifest) ou `--from <ref>`. O `ohmlatl.yaml` do diretório atual dá `user` e `vm`; `-t` sobrescreve `name:tag`; sem tag, `latest`.

1. `podman build` (ou `podman pull` para `--from`) e `podman create`.
2. Gerar a chave SSH do cliente (`ssh-keygen -t ed25519`) em `images/<nome>/<tag>/ssh/`.
3. Calcular o tamanho: `--size`, ou `ceil(du -sb * 1.25) + 512 MiB` sobre o rootfs exportado.
4. Criar o LUKS (sudo, passphrase lida do TTY pelo `cryptsetup`):

```bash
truncate -s <tamanho> rootfs.luks
cryptsetup luksFormat --type luks2 --cipher aes-xts-plain64 \
  --integrity hmac-sha256 --sector-size 4096 --pbkdf argon2id rootfs.luks
cryptsetup open rootfs.luks ohmlatl-build
blockdev --getsize64 /dev/mapper/ohmlatl-build   # tamanho útil
```

5. Export e ext4 dentro de `podman unshare`, com o tamanho útil exato do mapper:

```bash
podman unshare sh -euc '
  root=$(mktemp -d)
  podman export "$CID" | tar -x --same-owner --numeric-owner \
    --xattrs --xattrs-include="*" --xattrs-exclude="security.selinux" -C "$root"
  : > "$root/etc/machine-id"
  printf "%s\n" "$NAME" > "$root/etc/hostname"
  printf "127.0.0.1 localhost %s\n" "$NAME" > "$root/etc/hosts"
  rm -f "$root"/etc/ssh/ssh_host_*
  install -d -m 0700 -o "$UID_" -g "$GID_" "$root/home/$USER_/.ssh"
  install -m 0600 -o "$UID_" -g "$GID_" key.pub "$root/home/$USER_/.ssh/authorized_keys"
  truncate -s "$USABLE" rootfs.ext4
  mkfs.ext4 -q -F -L rootfs -m 0 -d "$root" rootfs.ext4
  e2fsck -fn rootfs.ext4
'
```

6. `dd if=rootfs.ext4 of=/dev/mapper/ohmlatl-build bs=4M conv=fsync`, `cryptsetup close`, apagar `rootfs.ext4`.
7. Copiar `ohmlatl.yaml` para `manifest.yaml` ao lado do `rootfs.luks`.

Regras:

- `podman export`, não `podman save`.
- Tudo que toca no rootfs extraído roda em `podman unshare`. Fora dele, os arquivos ficam com o UID do operador e quebram `sudo`, `sshd`, setuid e capabilities.
- `security.selinux` é excluído para não levar rótulos do host.
- O `luksFormat --integrity` zera o dispositivo inteiro; o build de alguns GiB leva tempo proporcional.
- Nada de segredo em `ARG`, `ENV` ou `RUN`; use `podman build --secret`.
- O build falha se a imagem não tiver `/sbin/init` apontando para systemd, `sshd` ou o usuário `user.name` com `user.uid`/`user.gid`.
- O `resolv.conf` não é gravado no build; o initramfs gera no boot.

Testes de aceitação do build:

- `stat -c '%u:%g' /etc/sudoers` dá `0:0` no ext4.
- `getcap /bin/ping` mostra capabilities.
- `e2fsck -fn` limpo.
- Alterar um byte no payload do `rootfs.luks` produz erro de I/O ao ler o setor.

## Volumes

```bash
ohmlatl volume create <nome> --size <n> [--luks]
```

- Sem `--luks`: `truncate` + `mkfs.ext4 -m 1` em `volumes/<nome>.ext4`. Sem sudo.
- Com `--luks`: LUKS2 sem integrity (custo de escrita) + ext4 em `volumes/<nome>.luks`. Sudo.
- `volume create` não sabe o UID. Na primeira montagem, se a raiz do volume tiver só `lost+found`, o initramfs faz `chown <uid>:<gid>`.
- Um volume só é usado por um dono por vez: `flock` exclusivo no arquivo do volume, pego pelo supervisor e pelo `volume mount`.
- `volume mount <nome> <dir>` só aceita a VM parada e sempre monta read-only (loop, sudo; LUKS pede a passphrase no TTY).

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
  disks:
    - volume: work
      mount: /home/dev/work
```

- `format` é obrigatório; o `ohmlatl` recusa versões que não conhece.
- `name` + `build.tag` formam a referência. `-t` sobrescreve.
- `build.containerfile` é opcional com `--from`.
- `vm.vcpus` e `vm.memory` podem ser sobrescritos por `run --vcpus/--memory`.
- `vm.disks` é a lista de volumes; a ordem vira `vdb`, `vdc`, etc.
- O `manifest.yaml` fica em claro no drive. Não é segredo, mas é entrada não confiável: o `run` valida antes de gerar a cmdline (`name` e `volume` em `[a-z0-9-]`, `mount` absoluto sem `..`, espaço ou vírgula, `uid`/`gid` numéricos).

## Run e boot

```bash
ohmlatl run <nome>:<tag> [--vcpus n] [--memory n]
```

1. Valida o manifest e pega o lock da VM e dos volumes.
2. Extrai os binários embutidos, se preciso.
3. Aloca o primeiro `/30` livre em `172.31.0.0/16` (`--subnet` sobrescreve) e grava em `state.json`.
4. Via `sudo`: cria o TAP `ohmlatl-<n>`, liga `ip_forward` se estava desligado (e anota para restaurar), cria a tabela `inet ohmlatl` com NAT.
5. Inicia o supervisor (processo do usuário, separado da CLI). O supervisor cria o PTY e chama o jailer via `sudo`, com o PTY como stdio do Firecracker. O jailer usa `--chroot-base-dir $XDG_RUNTIME_DIR/ohmlatl/jail`, `--uid/--gid` do operador e bind mount de kernel, initramfs, rootfs e volumes no chroot.
6. Cmdline gerada pelo host:

```text
console=ttyS0 reboot=k panic=1 root=/dev/vda
ip=172.31.0.2::172.31.0.1:255.255.255.252:<nome>:eth0:off:<dns0>:<dns1>
ohmlatl.user=1000:1000
ohmlatl.disks=vdb:luks:/home/dev/work,vdc:ext4:/data
```

   O DNS vem do resolver real do host (via `resolvectl` quando o `/etc/resolv.conf` aponta para `127.0.0.53`).

7. O `/init` do initramfs:
   - pede a passphrase do rootfs no `ttyS0` (`cryptsetup open --tries 3`); três falhas: rescue shell;
   - tenta a mesma passphrase em cada volume LUKS e pergunta outra só se falhar; três falhas: rescue shell;
   - `e2fsck -p` em cada ext4; erro não corrigível: rescue shell;
   - monta o rootfs em `/newroot` e cada volume em `/newroot/<mount>` (cria o diretório se faltar), com o `chown` da primeira montagem;
   - grava `/newroot/etc/resolv.conf` a partir de `/proc/net/pnp`;
   - `switch_root /newroot /sbin/init`.
8. O systemd sobe o `sshd`, que gera as host keys no primeiro boot.
9. A CLI anexa ao `console.sock`. `Ctrl-]` desanexa; a VM continua sob o supervisor.

`start <nome>` repete os passos com a imagem gravada em `state.json`. `run` com um nome de VM parada troca a imagem e inicia; se a tag mudou, zera o `known_hosts`. Com a VM rodando, `run` falha.

## Acesso

- `shell` e `exec`: `ssh -i images/<nome>/<tag>/ssh/id_ed25519 -o UserKnownHostsFile=vms/<nome>/known_hosts -o StrictHostKeyChecking=accept-new <user>@<guest>`.
- `cp`: `scp` com as mesmas opções.
- `console`: anexa ao `console.sock`; só um cliente por vez.
- `logs`: lê `serial.log`. O supervisor grava só a saída do serial; o echo da passphrase fica desligado pelo `cryptsetup`.

## Stop

1. `PUT /actions {"action_type": "SendCtrlAltDel"}` no socket da API do Firecracker.
2. O systemd do guest executa `ctrl-alt-del.target` (reboot limpo: para serviços, sync, desmonta). Com `reboot=k`, o reboot encerra o processo Firecracker.
3. Se o Firecracker não sair em 30 s, kill.
4. O supervisor remove TAP e regras nft, restaura `ip_forward` se foi ele que ligou, solta os locks e sai.

Não depende de SSH.

## Rede

- Um TAP e um `/30` por VM: host `.1`, guest `.2`.
- NAT em tabela própria `inet ohmlatl`, removida no stop da última VM.
- Sem publicação de portas, ingress ou proxy. O host acessa `<guest>:<porta>`; o guest acessa o host em `<gateway>:<porta>`.
- IPv6 desligado. Sem filtro de egress.

## Segurança

Escopo: drive roubado ou adulterado, com host confiável.

| Ameaça | Proteção |
|---|---|
| Drive roubado | Rootfs e volumes `--luks` em LUKS2; nenhuma chave LUKS no drive |
| Rootfs adulterado | Integrity `hmac-sha256` por setor: o setor alterado dá erro de I/O no guest |
| Kernel/initramfs adulterado | Não ficam no drive; vêm do binário `ohmlatl` |
| `ohmlatl` adulterado no drive | O operador verifica o binário contra SHA-256 ou assinatura do release, fora do drive |
| Manifest adulterado | Validação estrita antes de gerar a cmdline; o pior caso é mudar recursos ou o ponto de montagem |
| Outro usuário no host | Arquivos 0600, diretórios 0700 |
| Workload escapando | KVM + kernel próprio + seccomp + jailer |

Limitações:

- Volumes sem `--luks` ficam em claro.
- Volumes LUKS não têm integridade: adulteração vira lixo, não erro.
- Sem proteção contra rollback de um `rootfs.luks` antigo inteiro.
- A chave SSH privada fica em claro no drive (0600). Ela só dá acesso à VM rodando, pela rede local do TAP.
- O rebuild troca as host keys do guest; o `known_hosts` é zerado.
- Sem RTC: o relógio do guest começa errado até o `chrony` sincronizar.
- Sem proteção contra host comprometido.

## Doctor

`doctor --build` checa: rootless Podman, subuid/subgid, `newuidmap`/`newgidmap`, `/dev/fuse`, `e2fsprogs` >= 1.43, `cryptsetup` com integrity, `dm-integrity`, espaço livre, drive em ext4/btrfs/xfs e sem `noexec`.

`doctor --run` checa: `/dev/kvm` com acesso, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `sudo`, cliente `ssh`.

Falha imprime a correção sugerida e sai com código diferente de zero.

## Riscos para validar cedo

- Jailer + TAP + PTY repassado pelo `sudo` (P2, primeira tarefa).
- `SendCtrlAltDel` + `reboot=k` encerrando o Firecracker v1.17.
- `cryptsetup` estático com integrity e prompt no `ttyS0`.
- Custo de escrita do integrity no rootfs.

## Pendências

- Versão e config do kernel.
- Licença e avisos de terceiros (Firecracker, Podman, busybox, cryptsetup).
- Publicar SHA-256 e assinatura do `ohmlatl`.
