# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 Português · 🇪🇸 [Español](ROADMAP.es.md)

Status: pré-implementação. O repositório ainda é o scaffold Rust; este documento é o plano que guia as próximas fases. Os detalhes técnicos estão no `TECHNICAL.md`.

## Visão

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Com Podman no build, transforma um `Containerfile` ou uma imagem OCI num `rootfs.luks` (LUKS2 + ext4, esparso) e, no runtime, inicializa esse rootfs com Firecracker, usando o kernel e o initramfs embutidos no binário.

O computador mora num pendrive ou HD externo: um diretório com o binário `ohmlatl` e as imagens, cada uma um `rootfs.luks` com o sistema, as ferramentas e os dados, mais o manifest dela. Plugue o drive em outra máquina confiável e inicialize lá. O mesmo isolamento serve para rodar ferramentas, projetos e agentes não confiáveis em imagens próprias, com o guest fechado para o host, a LAN e outras VMs, e as aberturas declaradas no manifest. O runtime não usa Podman, não tem daemon global, não deixa nada persistente no host e não faz bind mount de diretórios do host.

O exemplo da documentação é uma imagem Kali Rolling. O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados, serviços e o login por SSH são responsabilidade do operador.

## Escopo

- Um diretório do ohmlatl no drive, criado pelo `init`, com o binário e `images/`.
- Gerar um rootfs a partir de um `Containerfile` ou de uma imagem OCI local, com Podman rootless, gravado direto no drive.
- `rootfs.luks` em LUKS2 (`aes-xts-plain64`), desbloqueado no initramfs com passphrase digitada no console da VM. `/home/<user>` mora no rootfs. Sem discos extras.
- Kernel de uso geral e initramfs gerados no repositório e embutidos no `ohmlatl`.
- Firecracker com jailer, TAP por VM.
- Guest fechado para host, LAN e outras VMs; internet opcional com DNS pelo gateway; forwards TCP para serviços do host e outras VMs.
- Supervisor por VM como unidade de sistema, com serial, `console` e `logs`.
- `shell`, `exec` e `cp` via SSH, com o login que a imagem tiver.
- CLI pequena, sem daemon global, sem estado de VM no drive.
- Release: binário estático (musl), `SHA256SUMS` assinado com a chave SSH do mantenedor (`ssh-keygen -Y`), o código-fonte exato dos componentes GPL embutidos e os avisos de terceiros, também disponíveis em `ohmlatl licenses`.

## Não-objetivos

- Publicação de portas, ingress, proxy reverso ou túnel público. Os forwards só saem da VM; acesso externo é problema do operador.
- Bind mount de diretórios do host.
- Discos extras ou volumes.
- Gerenciamento de chaves SSH.
- GPU, tela, passthrough PCI/USB.
- Módulos de kernel no guest.
- macOS/Windows nativos. O Firecracker exige Linux com KVM.
- Guests sem systemd.
- Gerenciar ferramentas, processos, plugins ou fluxos de tarefa.
- Snapshots de VM. Eles vazam RAM em claro e ficam presos a CPU/versão.
- Proteção contra host comprometido.

## Decisões de arquitetura

| Tema | Decisão |
|---|---|
| VMM | Firecracker com jailer |
| Binários | Firecracker, jailer, Podman, kernel e initramfs embutidos no `ohmlatl` |
| Drive | ext4/btrfs/xfs; diretório do ohmlatl com o binário e `images/`; achado pela localização do binário, por `--home` ou por `OHMLATL_HOME` |
| Host | Linux x86_64, kernel >= 5.10, com `/dev/kvm` e `sudo`, confiável mesmo quando emprestado; nada persistente fica nele |
| Build | Podman rootless embutido, `podman unshare` obrigatório, `mkfs` direto no mapper do LUKS |
| Runtime | um `sudo` por `run`; supervisor como a unidade de sistema `ohmlatl-<uid>-<nome>`; o jailer cai para o UID do operador |
| Init | systemd |
| Rootfs | LUKS2 `aes-xts-plain64` + ext4, esparso, 32 GiB por padrão, `image resize` para mudar; sem integrity ("let it crash") |
| Dados | dentro do rootfs; atualizado por dentro; rebuild cria um computador novo com tag nova |
| Passphrase | TTY do host no build, `ttyS0` da VM no boot; PBKDF fixo |
| Chaves | nenhuma chave LUKS nem chave SSH no drive |
| Acesso | `shell`/`exec`/`cp` via SSH com a senha ou a chave da imagem, uma conexão por boot; console com autologin |
| Rede | TAP + `/30` a cada run; fechada por padrão; NAT opcional; relay de DNS; forwards TCP via `ohmlatl-gateway` |
| Supervisor | um por VM; sobrevive ao logout; sem daemon global |
| Estado de VM | nenhum; `run <nome>:<tag>` é sempre explícito |

## Arquitetura

```text
DRIVE
└── ohmlatl/
    ├── ohmlatl (CLI confiável; embute Firecracker, jailer, Podman, vmlinux, initramfs)
    └── images/<nome>/<tag>/{rootfs.luks,manifest.yaml}

HOST (nada persistente)
├── /run/ohmlatl/ (tmpfs próprio: bin, jail, registros de rede, sockets e log em <uid>/<nome>/)
├── supervisor por VM (unidade de sistema ohmlatl-<uid>-<nome>)
│   ├── Firecracker (via jailer, no cgroup da unidade)
│   ├── PTY do serial (raw)
│   └── forwards TCP + relay de DNS
├── TAP + nftables (tabela inet ohmlatl)
└── ~/.cache/ohmlatl (cache do Podman, só onde se faz build)

GUEST
├── initramfs: prompt da passphrase, fsck, mount, resolv.conf, ohmlatl-gateway, switch_root
├── systemd
├── sshd
└── conteúdo da imagem
```

## Fluxo de boot

```text
ohmlatl run <nome>:<tag>
  → acha o diretório do ohmlatl, confere o dono, valida o manifest, imprime internet e forwards
  → sudo ohmlatl launch
      → monta /run/ohmlatl (tmpfs), extrai os binários
      → systemd-run: supervisor como unidade de sistema
          → /30, TAP, regras nft, forwards, relay de DNS
          → PTY raw + jailer → Firecracker (API: kernel, initramfs, drive, rede, rng)
  → a CLI imprime a rede e anexa ao console
  → kernel + initramfs
      → passphrase do rootfs no ttyS0
      → e2fsck -p
      → monta o rootfs, grava resolv.conf e ohmlatl-gateway
      → switch_root
  → systemd → sshd, autologin no serial
```

A cmdline do kernel só leva `console`, `reboot=k`, `panic` e `ip=` (endereço, gateway). A passphrase nunca entra no manifest, na cmdline, no ambiente ou nos logs.

## Pipeline da imagem de disco

```text
ohmlatl.yaml + Containerfile, ou uma ref OCI local
  → valida nome e tag; recusa uma tag existente
  → podman build / podman create
  → podman export + tar dentro de podman unshare, em ~/.cache/ohmlatl/build no host
  → valida systemd, sshd, usuário e home; avisa se o usuário não tem senha nem chave
  → machine-id, hostname, resolv.conf, host keys do SSH se faltarem, NetworkManager fora do eth0,
    networkd/resolved mascarados, poweroff/halt → reboot, autologin no serial
  → tamanho: --size ou max(32 GiB, 2 x conteúdo + 1 GiB), alinhado a 1 MiB
  → passphrase uma vez → luksFormat + open (sudo), no drive
  → mkfs.ext4 -d no mapper dentro de podman unshare (mke2fs.conf fixo) → e2fsck → close
  → images/<nome>/<tag>/{rootfs.luks,manifest.yaml}; apaga os temporários
```

Regras:

- O pipeline inteiro de export/mkfs roda dentro de `podman unshare`. Fora dele, o rootfs fica com o UID do operador e quebra `sudo`, `sshd`, setuid e capabilities.
- `podman export`, não `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- A árvore exportada só existe no diretório de build no host, que é apagado no fim e na falha; o ext4 é gravado direto dentro do LUKS no drive.
- PBKDF fixo (`argon2id`, 256 MiB, 4 iterações), para o unlock não depender da máquina do build.
- Feature set do ext4 fixo, para o `e2fsck` embutido sempre entender.
- Imagens remotas precisam de nome qualificado e de um `ohmlatl podman pull` explícito.
- Sem segredos em `ARG`/`ENV`/`RUN`, incluindo a senha do usuário; use `podman build --secret`, ou `passwd` dentro do `ohmlatl podman run`.
- Base de CPU genérica; sem `-march=native`.

## Rede

- Um TAP `ohm<N>` por VM, um `/30` de `172.31.0.0/16` alocado a cada run (host `.1`, guest `.2`), recusando subir se o host já usa essa faixa; `OHMLATL_POOL` muda. O guest alcança o lado do host pelo nome `ohmlatl-gateway`, gravado a cada boot.
- nftables numa tabela compartilhada (`inet ohmlatl`), com regras por VM e lock entre supervisores. O `ip_forward` é ligado e nunca desligado; se estava desligado, o roteamento entre as outras interfaces do host continua bloqueado até o reboot (o `doctor --run` mostra como liberar).
- Padrão: o guest alcança a internet por NAT e nada no host, na LAN ou em outras VMs. Nada da LAN chega no guest nem no gateway. Endereços de origem são conferidos. Só o root e o dono da VM conectam do host para o guest.
- `internet: false` corta a internet e o DNS.
- DNS: com `internet: true`, o supervisor faz relay da porta 53 do gateway para o resolver atual do host.
- `forward`: a porta do gateway é redirecionada para um listener do supervisor, que conecta em `127.0.0.1:<porta>` do host ou em outra VM. Nunca ocupa a porta no host. Só TCP.
- Sem `--publish`, ingress ou proxy.
- IPv6 desligado no TAP.

## Ciclo de vida e supervisor

- `run <nome>:<tag>` pede `sudo` uma vez e sobe o supervisor como a unidade de sistema `ohmlatl-<uid>-<nome>`. Recusa se já houver VM com esse nome rodando. O mesmo comando inicializa o computador de novo depois; não existe `start`.
- O supervisor mantém o PTY, o Firecracker, a rede e os forwards; sobrevive ao detach da CLI e ao logout. Status e stop passam pelo socket de controle dele; o `ps` lista os que estão rodando.
- `Ctrl-]` desanexa; `console` reanexa com autologin e reapresenta a saída mais recente; `shell` usa SSH.
- `stop` (ou o desligamento do host) manda Ctrl-Alt-Del pela API do Firecracker; com `reboot=k`, o reboot limpo do guest encerra o Firecracker. Kill após 120 s. Dentro do guest, `poweroff` e `reboot` também encerram a VM. Depois do `stop`, o drive pode ser desmontado.
- Cleanup idempotente quando o supervisor sai, e de novo depois que ele morre: TAP, regras nft, jail, sockets, a conexão SSH mestre.
- SSH: login com a senha ou a chave da imagem; uma conexão mestre por boot, reaproveitada por `exec` e `cp`; `~/.ssh/config` ignorado, sem repasse do agente, sem `known_hosts`.
- `image rm` recusa imagem cuja VM está rodando.

## CLI

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm
ohmlatl image resize <name:tag> <size>

ohmlatl run <name:tag> [--vcpus n] [--memory n]
ohmlatl stop <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>

ohmlatl licenses
```

Todos os comandos aceitam `--home <dir>`. Sem `expose`, `ingress` ou `publish`. `launch`, `supervise` e `cleanup` são internos.

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

O `ohmlatl build` exige o manifest no diretório atual e o copia para `images/<nome>/<tag>/manifest.yaml`, com o nome e a tag finais. O `run` lê essa cópia; editar `vm` nela vale no próximo `run`. `name` + `build.tag` formam a referência da imagem; sem tag, `latest`; `-t` sobrescreve. `build.containerfile` é opcional com `--from`. Memória mínima: 512 MiB. `port: 53` é inválido com `internet: true`. Campos desconhecidos são erro.

## Layout

```text
DRIVE  <diretório do ohmlatl>/{.ohmlatl,ohmlatl,images/<nome>/<tag>/{rootfs.luks,manifest.yaml}}
HOST   /run/ohmlatl/{bin/<sha256>,jail,net,<uid>/<nome>/{ctl.sock,console.sock,ssh.sock,serial.log,ip,error}}
HOST   ~/.cache/ohmlatl/{bin,podman,build}
```

O diretório do ohmlatl e o binário são 0755, para o binário rodar mesmo sob outro UID; `images/` é 0700 e os arquivos dele 0600, do operador. Tudo o que fica no host em `/run` some no reboot. O cache do Podman é cache, não estado, e guarda as imagens de container em claro: `ohmlatl podman system reset` limpa. Fazer rebuild em outro host roda a cadeia de build de novo lá.

## Segurança

| Ameaça | Proteção |
|---|---|
| Drive roubado ou perdido | `rootfs.luks` em LUKS2; nenhuma chave LUKS nem chave SSH no drive |
| `rootfs.luks` adulterado | Sem a chave, a alteração vira lixo no guest e quebra o boot ou um programa |
| Kernel/initramfs adulterado | Embutidos no binário, não soltos no drive |
| Binário `ohmlatl` adulterado no drive | Conferir o SHA-256 contra o release num host novo, ou usar um `ohmlatl` do host com `--home` |
| Outro usuário no host | Diretórios e sockets só do operador; namespacing por UID; só root e o dono conectam no guest |
| Workload escapando para o host | KVM + kernel próprio + seccomp + jailer |
| Workload alcançando host, LAN ou outras VMs | Rejeitado pelo nft por padrão; anti-spoofing; só passam os forwards declarados |
| LAN alcançando o guest ou os forwards | Descartado pelo nft |
| Workload pegando o agente SSH do operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase em logs ou em disco | Lida do TTY, passada por stdin, PTY raw; echo desligado no boot |

Limitações: sem integrity, então a adulteração não é detectada com certeza, e setores ou o arquivo inteiro podem voltar para uma cópia antiga; o arquivo esparso mostra quais blocos foram escritos; apagar arquivos no guest não encolhe o arquivo; tirar o drive com a VM rodando corrompe o disco dela; a host key SSH do guest não é conferida (ligação local ponto a ponto com um host confiável); o `manifest.yaml` fica em claro no drive, então um drive adulterado pode mudar recursos e abrir forwards, que o `run` imprime; um rebuild cria um computador novo e vazio; com `internet: true` o guest alcança a internet sem filtro; sem proteção contra host comprometido, emprestado ou não.

## Portabilidade

- Linux x86_64 com KVM no MVP.
- Kernel e binários com base de CPU genérica; sem `-march=native`.
- O drive muda de host como está; um UID diferente é detectado e a correção é impressa.
- Sempre cold boot; sem snapshots.

## Fases

### P0: Sanidade do host e boot

- `ohmlatl init` e `ohmlatl doctor`.
- Build dos assets em containers (`cargo xtask assets`): kernel 6.18 LTS e initramfs com Buildroot, mais o bundle Podman pela receita do podman-static, os dois com as fontes exatas; `mke2fs.conf` escrito à mão. Os binários do Podman saem do `vendor/`.
- Embutir e extrair os binários.
- Inicializar um rootfs LUKS de teste a partir do drive, com Firecracker sob o jailer, a partir de um supervisor rodado à mão.

**Pronto quando:** o `init` prepara um drive ext4 e recusa FAT/exFAT; a VM de teste inicializa com o kernel e o initramfs embutidos; o prompt da passphrase funciona no PTY raw e nunca aparece no log; a rede sobe por `ip=`; o Ctrl-Alt-Del encerra o Firecracker; os bind mounts de arquivo sobrevivem ao `pivot_root` do jailer; `/dev/ptp0` existe e o `chrony` sincroniza com ele; o `e2fsck` embutido aceita um ext4 feito com o `mke2fs.conf` embutido.

### P1: Pipeline da imagem de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` com o Podman embutido e configuração própria.
- `podman unshare` com export/tar/xattrs; `mkfs` no mapper, no drive.
- Host keys do SSH se faltarem; aviso de senha/chave; cópia do manifest; limpeza do build.
- `image ls/inspect/rm`, `image resize` (crescer e encolher).
- `examples/kali` como primeiro exemplo.

**Pronto quando:** no console serial (autologin), `sudo -n true` e `getcap /bin/ping` funcionam e os arquivos mantêm os donos; o rootfs desbloqueia com a passphrase no console; o `e2fsck` sai limpo; a imagem Kali personalizada com `podman commit` inicializa; uma imagem sem systemd ou sshd falha com mensagem clara; uma tag existente é recusada; nada sobra em `~/.cache/ohmlatl/build` nem como `.partial` no drive depois de sucesso ou falha; o `image resize` cresce e encolhe uma imagem parada, mantém o arquivo esparso e recusa com a VM rodando ou abaixo do mínimo do filesystem.

### P2: Runtime

- `run`, `stop`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- `launch`, supervisor como unidade de sistema, socket de controle, PTY raw, log do serial, detach/attach, cleanup.
- Jailer, TAP `/30`, rede fechada, `internet: true|false`, relay de DNS.

**Pronto quando:** o `shell` entra com a senha da imagem, pedida uma vez por boot, e `exec`/`cp` reaproveitam a conexão; a VM sobrevive ao logout; o guest não alcança o host, a LAN nem outra VM, e a LAN não alcança o guest; o DNS funciona num host com `systemd-resolved` e depois de uma troca de rede; `stop`, `poweroff` dentro do guest e o desligamento do host encerram a VM de forma limpa e deixam o drive pronto para desmontar; `kill -9` no supervisor não deixa TAP, regra nem mount para trás; o drive inicializa num segundo host, e um UID diferente lá é reportado com a correção.

### P3: Forwards e várias VMs

- `forward` para `host:<porta>` e `vm/<nome>:<porta>`.
- Várias VMs ao mesmo tempo, de um ou mais usuários; tabela nft compartilhada.
- `doctor --run` com Docker, ufw e firewalld.

**Pronto quando:** uma VM de agente com `internet: false` alcança o Ollama do host (escutando só em loopback) e um servidor MCP em outra VM, e nada mais; um serviço do host em `0.0.0.0` com a mesma porta continua funcionando; duas VMs rodam ao mesmo tempo sem colisão; parar uma VM não quebra a rede da outra.

### Futuro

Sem plano detalhado: comandos de backup/restore, `image import/export`, forwards UDP, `gc`, `--json`, completions, aarch64.

## Testes

**Unitários**

- Parser e validação do manifest (nomes, tags, UID/GID, portas, forwards, porta 53).
- Descoberta do diretório do ohmlatl (`--home`, `OHMLATL_HOME`, localização do binário).
- Geração da cmdline.
- Cálculo e alinhamento do tamanho.
- Alocação de subnet contra rotas e registros existentes.
- Regras nft por VM.

**Integração**

- Build com Podman → ext4 com donos, xattrs e capabilities, no drive.
- `--from` com uma imagem Kali personalizada; recusa de nomes remotos e curtos.
- Falha clara para imagem sem systemd ou sshd; recusa de tag existente; aviso sem senha nem chave.
- Passphrase sem quebra de linha abre tanto no build quanto no boot.
- `image resize` para cima e para baixo; recusa com a VM rodando e abaixo do mínimo; o arquivo continua esparso.
- Initramfs com `cryptsetup` no `ttyS0`; retry; passphrase errada cai no rescue.
- Jailer + TAP + PTY raw sob `systemd-run`, com `KillMode=mixed`.
- Ctrl-Alt-Del, `poweroff` e `systemctl stop` encerram o Firecracker de forma limpa.
- `kill -9` no Firecracker + `run` + `e2fsck`; `kill -9` no supervisor + cleanup.
- `shell`/`exec`/`cp` via SSH com `ForwardAgent=yes` no `~/.ssh/config` (não pode repassar); uma pergunta de senha por boot.
- Rede: guest bloqueado para host, LAN e outras VMs; origem falsificada descartada; LAN bloqueada para o gateway; outro usuário local bloqueado para o guest; forwards e DNS funcionam; `internet: false` corta a saída e o DNS falha na hora.
- Drive: dono errado e montagem `noexec` são recusados com a correção.

**E2E**

- Primeiro exemplo: `init`, base Kali, `podman run`, instalar x/y/z, `passwd`, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → doctor → build → run → shell → stop → run`.
- Migração para uma tag nova com `cp`.
- Tirar o drive depois do `stop`, plugar em outro host e inicializar lá.
- VM de agente + VM de MCP + Ollama no host.

**Matriz**

- Intel e AMD; 8/16/32 GiB de RAM.
- Pendrive e SSD externo, ext4 e btrfs.
- Hosts com e sem Docker/ufw/firewalld; com e sem `systemd-resolved`.
- e2fsprogs 1.45 e o mais novo disponível no host.
- CPU mais fraca da frota (base de CPU).

## Riscos

| Risco | Mitigação |
|---|---|
| KVM indisponível/aninhado | `doctor` falha cedo com instruções |
| A config de kernel do Firecracker é feita para kernels do Amazon Linux | `check-kconfig.sh` e o teste de boot do P0 |
| Sem `sudo` num host emprestado | Requisito documentado; `doctor --run` verifica |
| Drive tirado com uma VM rodando | `stop` antes de desmontar, documentado; `e2fsck` no boot |
| UID diferente em outro host | Detectado; `sudo chown -R` impresso |
| Drive montado com `noexec` | `doctor` avisa; `ohmlatl` do host com `--home` |
| Prompt do `cryptsetup` no initramfs | Teste no P0 com PTY raw; testes de echo e retry |
| Bind mounts do jailer perdidos depois do `pivot_root` | Teste no P0; alternativa: copiar kernel e initramfs para o jail |
| Donos errados no build | Pipeline inteiro em `podman unshare`; testes de aceite |
| Features do ext4 desconhecidas pelo `e2fsck` embutido | `mke2fs.conf` fixo; matriz com e2fsprogs antigo e novo |
| Relógio do guest errado sem internet | `ptp_kvm` + `chrony` com `refclock PHC`; teste no P0 |
| Rootfs cheio | 32 GiB esparsos por padrão; `--size`; `image resize` |
| Drive cheio | Documentado; `doctor` verifica espaço livre |
| Build morto no meio | `.partial` com lock; o próximo build ou o `doctor --build` limpa |
| Docker/ufw/firewalld descartam o tráfego | `doctor --run` detecta e imprime a regra |
| Gerenciadores de rede no guest sobrescrevendo o `ip=` | NetworkManager unmanaged no `eth0`; networkd e resolved mascarados no build |
| Imagem sem senha nem chave | Build avisa; console com autologin para rodar `passwd` |
| Kali puro sem systemd/sshd | `build` valida e aponta para `examples/kali` |
| Commit de container opaco | Documentado como exploração; o Containerfile é a fonte durável |
| Binário grande (~100 MB com Podman) | Aceito; extraído uma vez por boot do host |

## Itens para confirmar na implementação

- Fragmento final da config do kernel, depois dos testes de boot do P0.

## Critério de MVP

O MVP é P0 a P2: um drive preparado, um host Linux com KVM, uma imagem gerada com Podman, um boot com a passphrase no console da VM, shell via SSH, jailer ativo, rede fechada por padrão, um desligamento limpo e o mesmo drive inicializando num segundo host. O P3 adiciona os forwards e várias VMs trabalhando juntas.
