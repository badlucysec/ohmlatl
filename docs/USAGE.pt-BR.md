# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 Português · 🇪🇸 [Español](USAGE.es.md)

Este documento propõe os fluxos de uso do `ohmlatl` antes da implementação. É o contrato de experiência: comandos, prompts e resultados esperados. A implementação pode ajustar detalhes, mas não deve mudar a história sem atualizar este arquivo.

## Conceitos

- **Diretório do ohmlatl**: um diretório no seu pendrive ou HD externo, criado por `ohmlatl init`. Guarda o binário `ohmlatl` e `images/`. Nada mais é persistente, nem no drive nem no host.
- **Imagem de disco**: `images/<nome>/<tag>/`, com um `rootfs.luks` (LUKS2 + ext4) e o `manifest.yaml`. Guarda o computador inteiro, incluindo `/home/<user>`, e a VM escreve nele. É gerada a partir de um `Containerfile` ou de uma imagem OCI.
- **VM**: uma execução de uma imagem, com recursos e rede. Roda no máximo uma VM por nome de imagem, com esse nome.
- **Arquivo esparso**: o `rootfs.luks` tem um tamanho de disco, o que a VM enxerga, e um espaço ocupado, o que de fato está guardado no drive. Os trechos nunca escritos são buracos que não ocupam espaço. Um disco de 32 GiB com 3 GiB escritos ocupa cerca de 3 GiB. O `image ls` mostra os dois; o `image resize` muda o primeiro.
- **Manifest**: `ohmlatl.yaml`. Declara o nome da imagem, o usuário, os recursos e a rede.
- **Forward**: uma porta TCP no gateway da VM (`ohmlatl-gateway`) que leva a um serviço do host ou a outra VM. Além da internet, quando liberada, é a única saída da VM.
- **Supervisor**: uma unidade de sistema por VM que controla o Firecracker, o serial, a rede e os forwards. Não é um daemon global.
- **Jailer**: o sandbox do Firecracker (chroot, namespaces, seccomp, cgroups).

## Instalação (fase inicial)

Enquanto não houver release:

```bash
cargo build --release
./target/release/ohmlatl --version
```

Resultado esperado:

```text
ohmlatl 0.1.0
```

## 1. Preparar o drive

Formate o drive em ext4 (btrfs e xfs também funcionam; FAT e exFAT não), monte e rode:

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

Se a raiz do drive não for gravável por você (um ext4 recém-formatado é do root), o `init` pede `sudo` uma vez para criar o diretório para você. O drive fica assim:

```text
/media/dev/drive/ohmlatl/
├── .ohmlatl      # 0644
├── ohmlatl       # 0755
└── images/       # 0700, os seus computadores
```

O `ohmlatl` do drive acha o `images/` sozinho. Para usar um `ohmlatl` instalado no host, passe `--home /media/$USER/drive/ohmlatl` (ou defina `OHMLATL_HOME`).

## 2. Verificar o host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` verifica Podman rootless (pelo menos 65536 subuids e subgids, `newuidmap`, `/dev/fuse`), `e2fsprogs`, `cryptsetup`, `ssh-keygen`, `udevadm`, `sudo` e espaço livre. `--run` verifica `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, o cliente SSH e se Docker, ufw ou firewalld bloqueiam o tráfego das VMs. Os dois conferem o diretório do ohmlatl: filesystem, montagem, dono.

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

Se algo falhar, o comando imprime a correção sugerida e sai com código diferente de zero.

## 3. Primeiro exemplo: imagem de disco personalizada

O fluxo que a maioria vai querer: rodar um container base, instalar x, y, z e transformar o resultado numa microVM no drive. O exemplo usa Kali Rolling; troque a base se quiser.

### 3.1 Base do guest

O `kalilinux/kali-rolling` puro não tem systemd nem sshd. Gere a base do repositório como imagem Podman:

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

Este passo só cria a imagem Podman `kali-base:2026-11-02`, no cache do Podman do host. Sem disco e sem passphrase.

### 3.2 Explorar e instalar no container

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
# systemctl enable my-service.service   # se precisar
passwd dev                              # senha para o ohmlatl shell

apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*
exit
```

Troque `x`, `y` e `z` pelos pacotes que você quer. Para entrar com chave, coloque a sua chave pública em `/home/dev/.ssh/authorized_keys` (dono `dev`, modo 0600). Nunca defina a senha com `chpasswd` num `Containerfile`: ela fica em claro nas camadas da imagem.

### 3.3 Gerar a imagem de disco bootável

Ainda em `examples/kali`, onde está o `ohmlatl.yaml`:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

O `build` precisa de um `ohmlatl.yaml` no diretório atual. `--from` só usa imagens locais; para partir de uma remota, rode antes `ohmlatl podman pull docker.io/...`. `-t` define o nome e a tag do resultado, e o build grava os dois no `manifest.yaml` da imagem. Uma tag que já existe é recusada: um build nunca sobrescreve um computador.

O build pede a passphrase do rootfs e `sudo` uma vez:

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

Se o usuário não tiver nem senha nem `authorized_keys`, o build avisa que o `ohmlatl shell` não vai conseguir entrar; o console continua funcionando. Ele também avisa quando a imagem não tem `sftp-server`, que o `ohmlatl cp` precisa.

O tamanho padrão é 32 GiB ou o dobro do conteúdo mais 1 GiB, o que for maior; `--size 64G` muda isso. O arquivo é esparso: o drive só paga pelo que for escrito. O rootfs exportado fica em `~/.cache/ohmlatl/build` no host durante o build e é apagado no fim, mesmo em caso de falha.

### 3.4 Fixar num Containerfile

O commit é um snapshot do sistema de arquivos. Quando as ferramentas estabilizarem, passe os comandos para um `Containerfile` e tenha um rebuild repetível:

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

Isso cria `kali-custom:2026-11-10`, um computador novo ao lado de `kali-custom:2026-11-02`. Defina a senha depois, pelo console: `ohmlatl console`, depois `sudo passwd dev`. O build falha com mensagem clara se a imagem não tiver systemd, sshd ou o `user.name` com o UID/GID declarados e a home dele.

## 4. Primeiro boot

```bash
ohmlatl run kali-custom:2026-11-02
```

O comando pede `sudo` uma vez, sobe o supervisor da VM `kali-custom`, cria a rede e anexa o serial.

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

Depois do boot, o serial entra sozinho:

```text
dev@kali-custom:~$
```

`Ctrl-]` desanexa o terminal; a VM continua rodando sob o supervisor, mesmo depois do logout. `--vcpus` e `--memory` sobrescrevem o manifest só nesta execução. Nenhuma passphrase em argv, ambiente, manifest ou log. O mesmo `run` inicializa de novo da próxima vez.

## 5. Acessar a VM

```bash
ohmlatl shell kali-custom
```

```text
dev@172.31.0.2's password:
dev@kali-custom:~$
```

O shell usa SSH pela rede da VM, com o login que a imagem tiver: a senha do `dev`, ou a sua chave, se ela estiver no `authorized_keys`. A senha é pedida uma vez por boot: `exec` e `cp` reaproveitam a mesma conexão. O `ohmlatl` ignora o seu `~/.ssh/config`, nunca repassa o seu agente SSH e não guarda `known_hosts` (a ligação com o guest é um TAP ponto a ponto local). Não existe diretório do host dentro da VM; o acesso é sempre por aqui.

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

Console serial (rescue ou reanexar; entra sozinho e mostra primeiro a saída mais recente):

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

`logs` mostra o último 1 MiB da saída do serial. O log fica em `/run/ohmlatl` no host, dura até o próximo `run` e some quando o host reinicia.

## 6. Rede

Por padrão a VM alcança a internet e nada mais: nem o host, nem a LAN, nem outras VMs. Nada da LAN chega na VM. O host alcança a VM pelo IP dela; `ohmlatl shell`, `exec` e `cp` fazem isso por você.

Para cortar a internet e abrir só serviços específicos, edite `vm.network` no `manifest.yaml` da imagem no drive (ou no `ohmlatl.yaml` antes do build). Exemplo: um agente que só pode usar o Ollama do host e um servidor MCP em outra VM.

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

Dentro da VM o gateway é o único endereço que responde:

```bash
curl http://ohmlatl-gateway:11434/api/tags
```

- `ohmlatl-gateway` é gravado no `/etc/hosts` do guest a cada boot. Use o nome, não o IP: a subnet pode mudar entre um `run` e outro.
- `to: host:<porta>` chega em `127.0.0.1:<porta>` do host, então o Ollama pode continuar escutando só em loopback. O forward não ocupa a porta no host.
- `to: vm/<nome>:<porta>` chega em outra VM sua. Se ela estiver parada, a conexão fecha.
- Com `internet: true` a VM resolve nomes via `ohmlatl-gateway`, que repassa para o resolver do host. Com `internet: false` os lookups falham na hora.
- Só TCP. O `run` sempre imprime os forwards abertos. As mudanças valem no próximo `run`; sem rebuild.

## 7. Status e ciclo de vida

```bash
ohmlatl ps
```

```text
NAME          IMAGE                   PID    GUEST         UPTIME
kali-custom   kali-custom:2026-11-02  1234   172.31.0.2    2m
```

`ps` mostra só as VMs rodando. O que existe no drive está em `ohmlatl image ls`:

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

`stop` manda Ctrl-Alt-Del para o guest; o systemd desliga de forma limpa e o Firecracker sai. Se não sair em 120 s, o `ohmlatl` mata o processo. Dentro da VM, `poweroff` e `reboot` também encerram a VM. Quando o host desliga, as VMs rodando recebem o mesmo stop limpo.

Para inicializar de novo, o mesmo comando:

```bash
ohmlatl run kali-custom:2026-11-02
```

Para mudar o tamanho do disco que a VM enxerga, com a VM parada:

```bash
ohmlatl image resize kali-custom:2026-11-02 64G
```

```text
Rootfs passphrase:
[ohmlatl] kali-custom:2026-11-02: 32 GiB -> 64 GiB (2.9 GiB on disk)
[ohmlatl] e2fsck + resize2fs
[ohmlatl] done
```

Crescer é rápido e mantém o arquivo esparso: o espaço novo só ocupa o drive quando a VM grava nele. Encolher também funciona, até o que o filesystem guarda; pede confirmação, e vale fazer backup antes, porque uma queda de energia no meio pode danificar o filesystem. O `resize` pede `sudo` e a passphrase, e recusa com a VM rodando.

Remover uma imagem pede confirmação e é recusado enquanto a VM dela estiver rodando:

```bash
ohmlatl image rm kali-custom:2026-11-02
```

Para trocar a passphrase, com a VM parada:

```bash
cryptsetup luksChangeKey --pbkdf argon2id --pbkdf-memory 262144 \
  --pbkdf-parallel 1 --pbkdf-force-iterations 4 \
  /media/$USER/drive/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
```

## 8. Atualizar: por dentro ou rebuild

O rootfs é o seu computador e guarda tudo o que você faz nele. Atualize por dentro:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

Um rebuild cria um **computador novo e vazio** com uma tag nova. Para levar os seus dados:

```bash
ohmlatl cp kali-custom:/home/dev/work ./work      # VM antiga rodando
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-11-10
ohmlatl cp ./work kali-custom:/home/dev/work
```

Os dados passam pelo disco do host, em que você confia. A imagem antiga continua até `ohmlatl image rm kali-custom:2026-11-02`.

## 9. Em outra máquina

Pare as VMs, desmonte o drive, plugue na outra máquina e rode:

```bash
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --run
ohmlatl run kali-custom:2026-11-02
```

- O host tem que ser Linux x86_64 com KVM, você precisa de `sudo` nele e tem que confiar nele, mesmo emprestado: ele vê a passphrase que você digita e a memória da VM.
- O binário no drive é tão confiável quanto o drive. Num host novo, confira o SHA-256 dele contra o do release antes de rodar, ou use um `ohmlatl` instalado no host com `--home`.
- Se o seu UID nesse host for diferente do dono de `images/`, o `ohmlatl` para e mostra o `sudo chown -R` a rodar.
- A configuração de rede viaja no `manifest.yaml` de cada imagem e é aplicada como está; o `run` imprime os forwards para você conferir.
- Nada persistente fica no host. O cache do Podman fica no host onde você fez o build: para fazer rebuild em outro lugar, a cadeia de build inteira (`kali-base`, depois `kali-custom`) roda de novo lá.

Backup de um computador, com a VM parada, mantendo o arquivo esparso:

```bash
rsync -aS /media/$USER/drive/ohmlatl/images/kali-custom/ /backup/ohmlatl/images/kali-custom/
```

O destino do backup também tem que ser ext4, btrfs ou xfs.

## 10. Várias VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Cada imagem é uma VM com seu próprio `/30`, alocado automaticamente. As VMs não se enxergam; só um forward `vm/<nome>` liga uma à outra. Mantenha trabalho não confiável numa imagem própria, nunca na imagem do seu computador pessoal.

## 11. Falhas e recuperação

**Passphrase do rootfs errada.** O `cryptsetup` tenta de novo; depois de três falhas cai no rescue shell do initramfs:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` para anexar.

**Drive tirado com uma VM rodando.** A VM recebe erros de I/O e o disco dela pode ficar danificado. Plugue o drive de volta (o `ohmlatl stop` funciona até antes disso, e mesmo que o drive volte em outro caminho), depois `ohmlatl run`: o initramfs roda `e2fsck -p` antes de montar. Sempre `stop` antes de tirar o drive.

**Rootfs adulterado.** Sem a chave, uma alteração no `rootfs.luks` vira lixo dentro da VM: o boot ou o `e2fsck` falha, ou um programa quebra. Restaure um backup ou refaça a imagem.

**O Firecracker morreu.** `ohmlatl ps` não mostra mais a VM; `ohmlatl run kali-custom:2026-11-02` inicializa de novo.

**O host reiniciou.** As VMs não sobem sozinhas; `ohmlatl image ls` e `ohmlatl run`.

**Drive cheio.** O arquivo esparso não consegue ocupar mais espaço e a VM recebe erros de I/O, mesmo que o disco que ela enxerga ainda tenha folga. Libere espaço no drive e rode a VM de novo.

**Disco cheio dentro da VM.** `ohmlatl stop`, depois `ohmlatl image resize <nome>:<tag> <tamanho maior>`, depois `ohmlatl run`.

**Dono errado ou montagem `noexec`.** Com dono errado, o `ohmlatl` recusa e mostra o `sudo chown -R` a rodar. Com `noexec`, o binário do drive não executa: remonte sem `noexec`, ou use um `ohmlatl` instalado no host com `--home`.

**Sem rede dentro da VM num host com Docker, ufw ou firewalld.** `ohmlatl doctor --run` mostra a regra que libera `ohm*`.

**O host já usa `172.31.0.0/16`.** O `run` se recusa a subir quando um endereço ou rota do host cai no pool das VMs; defina outro, por exemplo `OHMLATL_POOL=10.213.0.0/16`. Faça o mesmo quando uma rede remota nessa faixa é alcançada pela rota padrão (uma VPC da AWS, por exemplo).

**`run` falha antes do boot.** O `run` imprime o erro do supervisor; se não houver, mostra o comando `sudo journalctl -u ohmlatl-<uid>-<nome>`.

**`ohmlatl shell` pede uma senha que não existe.** A imagem não tem senha nem chave para o usuário. Use `ohmlatl console` (login automático), rode `sudo passwd <usuário>` e tente de novo. Uma imagem sem `sudo` para o usuário precisa da senha definida no `ohmlatl podman run`, antes do build.

**Passphrase perdida.** O rootfs não abre. Não há chave LUKS no drive; refaça a imagem e comece do zero.

## 12. Não-objetivos

- Sem publicação de portas, ingress ou proxy público. Os forwards só saem da VM; acesso externo é responsabilidade sua.
- Sem bind mount de diretórios do host. Arquivos entram e saem com `ohmlatl cp`.
- Sem discos extras nem volumes. O computador é o rootfs.
- Sem gerenciamento de chaves SSH. O login é conteúdo da imagem.
- Sem GPU nem tela. A VM é usada pelo terminal.
- Sem módulos de kernel no guest. O kernel já traz o que um sistema de uso geral precisa (nftables, tun, overlayfs, namespaces, wireguard, fuse).
- Sem macOS/Windows. O Firecracker exige Linux com KVM.
- Sem gerenciamento de ferramentas, processos, plugins ou fluxos de tarefa. Isso é conteúdo da imagem.

## 13. Contrato de UX

- Passphrases são digitadas no TTY do host no `build` e no TTY da VM no boot. Nunca em flag, env, manifest, arquivo ou log.
- Comandos destrutivos pedem confirmação; o `build` nunca sobrescreve uma tag.
- Erros dizem o que falhou e o que fazer; códigos de saída são estáveis.
- Sem daemon global. O supervisor é por VM e sai junto com ela.
- Nada persistente fora do drive; nada no drive além do binário e das imagens.
- Toda abertura de rede é declarada no manifest e impressa pelo `run`.
