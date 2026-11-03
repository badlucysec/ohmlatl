# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 Português · 🇪🇸 [Español](USAGE.es.md)

Este documento propõe os fluxos de uso do `ohmlatl` antes da implementação. É o contrato de experiência: comandos, prompts e resultados esperados. A implementação pode ajustar detalhes, mas não deve mudar a história sem atualizar este arquivo.

## Conceitos

- **Imagem de disco**: um `rootfs.luks` (LUKS2 + integridade + ext4) mais o `manifest.yaml` e a chave SSH usados no build. Contém o sistema inteiro, incluindo `/home/<user>`. É construída a partir de um `Containerfile` ou de uma imagem OCI.
- **Volume**: um disco ext4 extra, com ou sem LUKS2. O manifest diz onde a VM o monta. O que precisa sobreviver a um rebuild mora num volume.
- **VM**: uma execução de uma imagem, com volumes, recursos e rede. Há uma VM por imagem, com o nome da imagem.
- **Manifest**: `ohmlatl.yaml`. Declara o nome da imagem, o usuário, os recursos e os volumes.
- **Supervisor**: processo por VM que controla o Firecracker, o PTY do serial e o log. Não é um daemon global.
- **Jailer**: o sandbox do Firecracker (chroot, namespaces, seccomp, cgroups). Exige root para iniciar.

## Instalação (fase inicial)

Enquanto não houver release:

```bash
cargo build --release
install -m 0755 target/release/ohmlatl ~/.local/bin/ohmlatl
ohmlatl --version
```

Resultado esperado:

```text
ohmlatl 0.1.0
```

## 1. Prepare o drive

```bash
ohmlatl init /mnt/vmdisk
```

O comando cria o layout e valida o filesystem. O drive precisa ser ext4, btrfs ou xfs, montado sem `noexec`; FAT/exFAT/NTFS não suportam discos esparsos.

Resultado esperado:

```text
[ohmlatl] store: /mnt/vmdisk
[ohmlatl] created bin/ images/ volumes/ vms/
[ohmlatl] filesystem: ext4 (ok)
```

## 2. Verifique o host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` verifica Podman rootless, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs` e `cryptsetup` com integridade. `--run` verifica `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `sudo` e o cliente SSH.

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

Se algo falhar, o comando mostra a correção sugerida e sai com código diferente de zero.

## 3. Primeiro exemplo: imagem de disco personalizada

O fluxo que a maioria vai querer: rodar um container base, instalar x, y, z e transformar o resultado numa microVM. O exemplo usa Kali Rolling; troque a base se quiser.

### 3.1 Base do guest

O `kalilinux/kali-rolling` puro não tem systemd nem sshd. Use a base do repositório:

```bash
cd examples/kali
ohmlatl build
```

O `ohmlatl build` lê o `Containerfile` declarado em `build.containerfile` e usa o Podman embutido. Ele produz a imagem de disco `kali-base:2026-11-02` e deixa a imagem Podman com o mesmo nome para o próximo passo. `name` e `build.tag` identificam o artefato; sem tag, `latest`; `-t` sobrescreve.

### 3.2 Explore e instale no container

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # se precisar

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Troque `x`, `y` e `z` pelos pacotes que você quer.

### 3.3 Gere a imagem de disco bootável

No diretório do projeto, com o `ohmlatl.yaml` do exemplo:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

O build exporta o rootfs dentro de `podman unshare`, gera a chave SSH, calcula o tamanho e pede a passphrase do rootfs:

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

`--size 8G` define o tamanho. O build pode pedir `sudo` para abrir o device-mapper do LUKS; o `cryptsetup` lê a passphrase do TTY.

### 3.4 Fixe num Containerfile

O commit é um snapshot do filesystem. Quando as ferramentas estabilizarem, passe os comandos para um `Containerfile` para ter rebuild reproduzível:

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

O build falha com mensagem clara se a imagem não tiver systemd, sshd ou o `user.name` com o UID/GID declarado.

## 4. Crie volumes

Volume criptografado:

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

`--luks` pode pedir `sudo` para o device-mapper. A passphrase é lida do TTY, nunca de flag ou variável de ambiente. Se você usar a passphrase do rootfs, o boot pergunta uma vez só.

Volume sem LUKS:

```bash
ohmlatl volume create data --size 8G
```

```text
[ohmlatl] volume data
  file: volumes/data.ext4
  fs:   ext4
  size: 8 GiB
```

O volume não precisa saber o UID: na primeira montagem, a VM entrega a raiz do volume para `user.uid:gid`.

## 5. Primeiro boot

```bash
ohmlatl run kali-custom:2026-11-02
```

O comando cria a VM `kali-custom`, a rede e o supervisor, e anexa o serial. Os volumes e pontos de montagem vêm do manifest.

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

O initramfs tenta a mesma passphrase em cada volume LUKS. Se falhar:

```text
LUKS passphrase for work:
```

Depois do boot:

```text
kali-custom login:
```

`Ctrl-]` desanexa o terminal; a VM continua rodando sob o supervisor. `--vcpus` e `--memory` sobrescrevem o manifest. Nenhuma passphrase em argv, ambiente, manifest ou log.

## 6. Acesse a VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

O shell usa SSH pela rede da VM com a chave gerada no build. Não há diretório do host dentro da VM; o acesso é sempre por aqui.

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

Console serial (rescue ou reanexar):

```bash
ohmlatl console kali-custom
```

Se outro console estiver anexado:

```text
[ohmlatl] console busy: another client is attached
```

Logs:

```bash
ohmlatl logs kali-custom -f
```

`logs` mostra a saída do serial.

## 7. Status e ciclo de vida

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

O `stop` envia Ctrl-Alt-Del ao guest; o systemd desliga de forma limpa e o Firecracker sai. Se não sair em 30 s, o `ohmlatl` mata o processo.

```bash
ohmlatl start kali-custom
```

O `start` usa a última imagem e pede as passphrases de novo no serial.

```bash
ohmlatl rm kali-custom
```

Remove o estado da VM. Imagens e volumes ficam; use `ohmlatl volume rm` para volumes.

## 8. Leia um volume no host

Com a VM parada:

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

A montagem no host é sempre read-only e recusada com a VM rodando. Enquanto montado, `run` e `start` recusam aquele volume.

Se um serviço local do host precisar ser acessado pela VM, ele tem que escutar no gateway da VM, por exemplo `172.31.0.1:8080`. O `ohmlatl` não publica portas.

## 9. Atualize o rootfs

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

O `run` numa VM parada troca para a nova imagem. Os volumes não são tocados; `/home/dev` recomeça do skeleton da imagem, então guarde o que importa em volumes. A nova imagem tem host keys SSH novas, e o `ohmlatl` zera o `known_hosts` da VM. Se a VM estiver rodando, o `run` falha.

## 10. Várias VMs

```bash
ohmlatl run kali-lab:2026-11-02
ohmlatl ps
```

Cada imagem é uma VM própria com seu `/30`, alocado automaticamente (`--subnet` sobrescreve). Um volume só pode estar anexado a uma VM por vez; a segunda tentativa falha com lock.

## 11. Falhas e recuperação

**Passphrase do rootfs errada.** O `cryptsetup` tenta de novo; depois de três falhas, cai no rescue shell do initramfs:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` para anexar.

**Passphrase de volume errada.** Três tentativas, depois rescue shell.

**Rootfs adulterado.** A verificação de integridade transforma o setor alterado em erro de leitura; o `e2fsck` ou a montagem falha e o boot cai no rescue. Reconstrua a imagem.

**Firecracker morreu.** `ohmlatl ps` mostra `stopped`; `ohmlatl start kali-custom` inicia de novo. O initramfs roda `e2fsck -p` antes de montar.

**Host reiniciou.** As VMs não sobem sozinhas; `ohmlatl ps` e `ohmlatl start`.

**Drive removido com a VM rodando.** Não faça isso. Se o drive sumir, o próximo boot roda `e2fsck`.

**Passphrase perdida.** O rootfs pode ser reconstruído. Um volume LUKS é perdido. Não há chave LUKS no drive.

## 12. Fora do escopo

- Sem publicação de portas, ingress ou proxy público. O host acessa o guest; acesso externo é responsabilidade sua.
- Sem bind mount de diretórios do host. A VM só vê discos gerenciados pelo `ohmlatl`.
- Sem GPU. A VM roda só em CPU.
- Sem macOS/Windows. O Firecracker exige Linux com KVM.
- Sem gestão de ferramentas, processos, plugins ou fluxos de tarefa. Isso é conteúdo da imagem.

## 13. Contrato de UX

- As passphrases são digitadas no TTY da VM no boot e no TTY do host na criação (`build`/`volume create`/`volume mount`). Nunca em flag, env, manifest, arquivo ou log.
- Comandos destrutivos pedem confirmação.
- Erros dizem o que falhou e o que fazer; os códigos de saída são estáveis.
- Sem daemon global. O supervisor é por VM e sai junto com ela.
- Todo estado persistente mora no drive.
