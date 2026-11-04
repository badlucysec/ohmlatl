# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 Português · 🇪🇸 [Español](ROADMAP.es.md)

Status: pré-implementação. O repositório ainda é o scaffold Rust; este documento é o plano que guia as próximas fases. Os detalhes técnicos estão no `TECHNICAL.md`.

## Visão

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Com Podman no build, transforma um `Containerfile` ou uma imagem OCI num `rootfs.luks` (LUKS2 + integrity + ext4) e, no runtime, inicializa esse rootfs com Firecracker, usando o kernel e o initramfs embutidos no binário.

O computador é o `rootfs.luks`: um arquivo que guarda o sistema, as ferramentas e os dados, e que pode ser copiado para outro host e inicializado lá. O mesmo isolamento serve para rodar ferramentas, projetos e agentes não confiáveis, com a rede fechada por padrão e as aberturas declaradas no manifest. O runtime não usa Podman, não tem daemon global e não faz bind mount de diretórios do host.

O exemplo da documentação é uma imagem Kali Rolling. O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados e serviços são responsabilidade do operador.

## Escopo

- Gerar um rootfs a partir de um `Containerfile` ou de uma imagem OCI, com Podman rootless.
- `rootfs.luks` em LUKS2 com integridade `hmac-sha256`, desbloqueado no initramfs com passphrase digitada no console da VM. `/home/<user>` mora no rootfs. Sem discos extras.
- Kernel e initramfs gerados no repositório e embutidos no `ohmlatl`.
- Firecracker com jailer, TAP por VM.
- Rede fechada para host, LAN e outras VMs por padrão; internet opcional; forwards TCP para serviços do host e outras VMs.
- Supervisor por VM com serial em PTY, `console` e `logs`.
- `shell`, `exec` e `cp` via SSH.
- CLI pequena, sem daemon global.

## Não-objetivos

- Publicação de portas, ingress, proxy reverso ou túnel público. Os forwards só saem da VM; o host alcança o IP do guest; acesso externo é problema do operador.
- Bind mount de diretórios do host.
- Discos extras ou volumes.
- GPU, tela, passthrough PCI/USB.
- macOS/Windows nativos. O Firecracker exige Linux com KVM.
- Guests sem systemd.
- Gerenciar ferramentas, processos, plugins ou fluxos de tarefa.
- Snapshots de VM. Eles vazam RAM em claro e ficam presos a CPU/versão.

## Decisões de arquitetura

| Tema | Decisão |
|---|---|
| VMM | Firecracker com jailer |
| Binários | Firecracker, jailer, Podman, kernel e initramfs embutidos no `ohmlatl` |
| Host | Linux x86_64 com `/dev/kvm` |
| Build | Podman rootless embutido, `podman unshare` obrigatório |
| Runtime | supervisor como unidade de sistema root via um único `sudo systemd-run`; o jailer cai para o UID do operador |
| Init | systemd |
| Rootfs | LUKS2 + integrity (com journal) + ext4, `--size` ou calculado |
| Dados | dentro do rootfs; atualizado por dentro; rebuild cria um computador novo |
| Passphrase | TTY do host no build, `ttyS0` da VM no boot; PBKDF fixo |
| Chaves | nenhuma chave LUKS em disco |
| Acesso | `shell`/`exec`/`cp` via SSH, host key do build |
| Rede | TAP + `/30`; fechada por padrão; NAT opcional para a internet; forwards TCP |
| Supervisor | um por VM; sobrevive ao logout; sem daemon global |
| Portabilidade | copiar o diretório da imagem para outro host |

## Arquitetura

```text
HOST
├── ohmlatl (CLI confiável; embute Firecracker, jailer, Podman, vmlinux, initramfs)
├── ~/.local/share/ohmlatl/
│   ├── images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
│   └── vms/<nome>/state.json
├── /run/ohmlatl/ (tmpfs: bin, jail, console.sock, serial.log)
├── supervisor por VM (unidade de sistema ohmlatl-<nome>)
│   ├── Firecracker (via jailer)
│   ├── PTY do serial
│   └── forwards TCP
└── TAP + nftables

GUEST
├── initramfs: prompt da passphrase, fsck, mount, switch_root
├── systemd
├── sshd
└── conteúdo da imagem
```

## Fluxo de boot

```text
ohmlatl run <nome>:<tag>
  → valida o manifest, aloca o /30
  → sudo systemd-run: supervisor como unidade de sistema
      → extrai os binários embutidos para /run/ohmlatl
      → TAP + chains nft + forwards
      → PTY + jailer → Firecracker
  → kernel + initramfs
      → passphrase do rootfs no ttyS0
      → e2fsck -p
      → monta o rootfs, grava o resolv.conf
      → switch_root
  → systemd → sshd
  → o supervisor mantém serial, processo e forwards
```

A cmdline do kernel só leva `console`, `reboot=k`, `panic`, `root` e `ip=` (endereço, gateway, DNS). A passphrase nunca entra no manifest, na cmdline, no ambiente ou nos logs.

## Pipeline da imagem de disco

```text
Containerfile ou ref OCI local
  → podman build / podman create
  → gera a chave SSH do cliente e a host key do guest
  → podman export + tar dentro de podman unshare, em ~/.cache/ohmlatl/build
  → machine-id, hostname, host key, authorized_keys, NetworkManager fora do eth0,
    poweroff/halt → reboot
  → valida systemd, sshd, usuário
  → tamanho: --size ou ceil(du -sb * 1.25) + 512 MiB
  → passphrase uma vez → luksFormat --integrity hmac-sha256 --integrity-no-wipe (sudo)
  → mkfs.ext4 -d com o tamanho útil do mapper → dd no mapper, close
  → images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}; apaga os temporários
```

Regras:

- O pipeline inteiro de export/mkfs roda dentro de `podman unshare`. Fora dele, o rootfs fica com o UID do operador e quebra `sudo`, `sshd`, setuid e capabilities.
- `podman export`, não `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- Nenhum rootfs em claro fora do diretório temporário do build, que é apagado no fim e na falha.
- PBKDF fixo (`argon2id`, 256 MiB, 2 s), para o unlock não depender da máquina do build.
- Imagens remotas precisam de nome qualificado; `--from` só usa imagens locais.
- Sem segredos em `ARG`/`ENV`/`RUN`; use `podman build --secret`.
- Base de CPU genérica; sem `-march=native`.

## Rede

- Um TAP por VM, `/30` de `172.31.0.0/16`, host `.1`, guest `.2`, alocado pelo `run`, pulando redes que já estão na tabela de rotas.
- nftables numa tabela compartilhada (`inet ohmlatl`) com chains por VM. A tabela e o `ip_forward` são restaurados quando a última VM para.
- Padrão: o guest alcança a internet por NAT, e nada no host, na LAN ou em outras VMs.
- `internet: false` corta a internet.
- `forward`: o supervisor escuta em `<gateway>:<porta>` e conecta em `127.0.0.1:<porta>` do host ou em outra VM. Só TCP.
- O host alcança `172.31.0.2:<porta>`. Sem `--publish`, ingress ou proxy.
- IPv6 desligado.

## Ciclo de vida e supervisor

- `run` pede `sudo` uma vez e sobe o supervisor como a unidade de sistema `ohmlatl-<nome>`. Numa VM parada, troca a imagem.
- O supervisor mantém o PTY, o Firecracker, a rede e os forwards; sobrevive ao detach da CLI e ao logout.
- `Ctrl-]` desanexa; `console` reanexa; `shell` usa SSH.
- `stop` manda Ctrl-Alt-Del pela API do Firecracker; com `reboot=k`, o reboot limpo do guest encerra o Firecracker. Kill após 30 s. Dentro do guest, `poweroff` e `reboot` também encerram a VM.
- A chave SSH do cliente e a host key do guest são geradas no build, ao lado da imagem; o SSH confere a host key de forma estrita e ignora o `~/.ssh/config`.
- `start` reaproveita a última imagem e pede a passphrase de novo.
- `rm` remove o estado da VM; a imagem continua.

## CLI

```text
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm

ohmlatl run <name:tag> [--vcpus n] [--memory n]
ohmlatl start|stop|rm <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>
```

Sem `expose`, `ingress` ou `publish`.

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

O `ohmlatl build` copia o manifest para `images/<nome>/<tag>/manifest.yaml`, com o nome e a tag finais. O `run` lê essa cópia; editar `vm` nela vale no próximo `start`. `name` + `build.tag` formam a referência da imagem; sem tag, `latest`; `-t` sobrescreve. `build.containerfile` é opcional com `--from`. Memória mínima: 512 MiB. Campos desconhecidos são erro.

## Layout no host

```text
~/.local/share/ohmlatl/
├── images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/{id_ed25519,known_hosts}}
└── vms/<nome>/state.json

/run/ohmlatl/{bin,jail,<nome>/{console.sock,api.sock,serial.log}}
~/.cache/ohmlatl/{bin,podman,build}
```

Arquivos 0600, diretórios 0700. O cache do Podman é cache, não estado: fazer rebuild em outro host roda a cadeia de build de novo lá.

## Segurança

| Ameaça | Proteção |
|---|---|
| `rootfs.luks` roubado | LUKS2; nenhuma chave LUKS em disco |
| `rootfs.luks` adulterado | Integridade `hmac-sha256` por setor; um setor alterado falha na leitura |
| Kernel/initramfs adulterado | Não ficam em disco; embutidos no `ohmlatl` |
| Outro usuário no host | Arquivos 0600; diretórios 0700; socket do console do operador |
| Workload escapando para o host | KVM + kernel próprio + seccomp + jailer |
| Workload alcançando host, LAN ou outras VMs | Bloqueado pelo nft por padrão; só passam os forwards declarados |
| Workload pegando o agente SSH do operador | `-F /dev/null`, `ForwardAgent=no`, `ForwardX11=no` |
| Passphrase em logs ou em disco | Lida do TTY, passada por stdin; echo desligado no boot |

Limitações: sem proteção contra rollback para uma cópia antiga do `rootfs.luks`; o integrity custa velocidade de escrita e o arquivo ocupa o tamanho cheio; a chave SSH privada fica em claro no host (0600); um rebuild cria um computador novo e vazio; com `internet: true` o guest alcança a internet sem filtro; sem proteção contra host comprometido.

## Portabilidade

- Linux x86_64 com KVM no MVP.
- Kernel e binários com base de CPU genérica; sem `-march=native`.
- O diretório da imagem é copiado como está para outro host.
- Sempre cold boot; sem snapshots.

## Fases

### P0: Sanidade do host e boot

- `ohmlatl doctor`.
- Scripts no repositório que geram o kernel e o initramfs.
- Embutir e extrair os binários.
- Inicializar um rootfs de teste simples com Firecracker, sem jailer.

**Pronto quando:** a VM de teste inicializa com o kernel e o initramfs embutidos, dá console, tem rede por `ip=`, e o Ctrl-Alt-Del encerra o Firecracker.

### P1: Pipeline da imagem de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` com o Podman embutido.
- `podman unshare` com export/tar/xattrs.
- `rootfs.luks` com integrity; chaves SSH; cópia do manifest.
- `image ls/inspect/rm`.
- `examples/kali` como primeiro exemplo.

**Pronto quando:** `sudo -n true`, `getcap /bin/ping` e SSH funcionam no guest; o rootfs desbloqueia com passphrase no console; o `e2fsck` sai limpo; a imagem Kali personalizada com `podman commit` inicializa; uma imagem sem systemd ou sshd falha com mensagem clara; alterar um byte do `rootfs.luks` produz erro de leitura; nada sobra em `~/.cache/ohmlatl/build`.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Supervisor como unidade de sistema, PTY, log do serial, detach/attach.
- Jailer, TAP `/30`, rede fechada por padrão, `internet: true|false`.

**Pronto quando:** o prompt da passphrase aparece no serial da VM; o `shell` entra via SSH com host key estrita; o jailer está ativo; a VM sobrevive ao logout; o guest não alcança o host, a LAN nem outra VM; `stop` e `poweroff` dentro do guest desligam de forma limpa; `kill -9` no Firecracker seguido de `start` passa no `e2fsck`.

### P3: Forwards e várias VMs

- `forward` para `host:<porta>` e `vm/<nome>:<porta>`.
- Subnets automáticas; várias VMs ao mesmo tempo; tabela nft compartilhada.
- `doctor --run` com Docker e ufw.

**Pronto quando:** uma VM de agente com `internet: false` alcança o Ollama do host e um servidor MCP em outra VM, e nada mais; duas VMs rodam ao mesmo tempo sem colisão; parar uma VM não quebra a rede da outra.

### Futuro

Sem plano detalhado: backup/restore, `image import/export`, forwards UDP, `gc`, `--json`, completions, aarch64, proteção contra rollback.

## Testes

**Unitários**

- Parser e validação do manifest (nomes, UID/GID, portas, forwards).
- Geração da cmdline.
- Cálculo de tamanho.
- Alocação de subnet contra as rotas existentes.
- Regras nft por VM.

**Integração**

- Build com Podman → ext4 com donos, xattrs e capabilities.
- `--from` com uma imagem Kali personalizada; recusa de nomes remotos curtos.
- Falha clara para imagem sem systemd ou sshd.
- `rootfs.luks` com integrity; alterar um byte falha a leitura.
- Initramfs com `cryptsetup` no `ttyS0`; retry; passphrase errada cai no rescue.
- Jailer + TAP + PTY do serial sob `systemd-run`.
- Ctrl-Alt-Del e `poweroff` encerram o Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` via SSH com `ForwardAgent=yes` no `~/.ssh/config` (não pode repassar).
- Rede: guest bloqueado para host, LAN e outras VMs; forwards funcionam; `internet: false` corta a saída.

**E2E**

- Primeiro exemplo: base Kali, `podman run`, instalar x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `doctor → build → run → shell → stop → start`.
- Copiar o diretório da imagem para outro host e inicializar lá.
- VM de agente + VM de MCP + Ollama no host.

**Matriz**

- Intel e AMD; 8/16/32 GiB de RAM.
- Hosts com e sem Docker/ufw.
- CPU mais fraca da frota (base de CPU).

## Riscos

| Risco | Mitigação |
|---|---|
| KVM indisponível/aninhado | `doctor` falha cedo com instruções |
| Prompt do `cryptsetup` no initramfs | `cttyhack`/dup2 do `ttyS0`; testes de echo e retry |
| Donos errados no build | Pipeline inteiro em `podman unshare`; testes de aceite |
| `mkfs.ext4 -d` antigo | `doctor` exige e2fsprogs >= 1.43 |
| Host sem suporte a integrity | `doctor --build` verifica cryptsetup e dm-integrity |
| Custo de escrita do integrity | Aceito; documentado |
| Relógio do guest começa errado (sem RTC) | `broken_system_clock` no `e2fsck.conf` do initramfs; chrony no guest |
| Rootfs cheio | Tamanho padrão com 25% + 512 MiB; `--size` |
| Docker/ufw descartam o forwarding | `doctor --run` detecta e imprime a regra |
| NetworkManager no guest sobrescrevendo o `ip=` | `eth0` unmanaged, gravado no build |
| Kali puro sem systemd/sshd | `build` valida e aponta para `examples/kali` |
| Commit de container opaco | Documentado como exploração; o Containerfile é a fonte durável |
| Binário grande (~100 MB com Podman) | Aceito; extraído uma vez por boot do host |

## Itens para confirmar na implementação

- Versão e config do kernel do guest.
- Licença do projeto e avisos de terceiros.

## Critério de MVP

O MVP é P0 a P2: um host Linux com KVM, uma imagem gerada com Podman, um boot com a passphrase no console da VM, shell via SSH, jailer ativo, rede fechada por padrão e um desligamento limpo. O P3 adiciona os forwards e várias VMs trabalhando juntas.
