# ROADMAP

🇺🇸 [English](ROADMAP.en.md) · 🇧🇷 Português · 🇪🇸 [Español](ROADMAP.es.md)

Status: pré-implementação. O repositório ainda é o scaffold Rust; este documento é o plano que guia as próximas fases. Os detalhes técnicos estão em `TECHNICAL.md`.

## Visão

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Com Podman no build, transforma um `Containerfile` ou uma imagem OCI num `rootfs.luks` (LUKS2 + integridade + ext4) e, no runtime, inicializa esse rootfs com Firecracker, usando o kernel e o initramfs embutidos no binário.

O host é fino: binário, `/dev/kvm`, rede e o drive. O computador de verdade (rootfs, volumes, ferramentas e dados) mora no drive externo. O runtime não usa Podman, não tem daemon, não guarda estado persistente no host e não faz bind mount de diretórios do host.

O exemplo da documentação é uma imagem Kali Rolling. O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados e serviços são responsabilidade do operador.

## Escopo

- Build de rootfs a partir de `Containerfile` ou imagem OCI, com Podman rootless.
- `rootfs.luks` em LUKS2 com integridade `hmac-sha256`, aberto no initramfs com passphrase digitada no console da VM. `/home/<user>` mora no rootfs.
- Volumes genéricos, ext4 com ou sem LUKS2, montados onde o manifest disser.
- Kernel e initramfs gerados no repositório e embutidos no `ohmlatl`.
- Firecracker com jailer, TAP/NAT.
- Supervisor por VM com serial em PTY, `console` e `logs`.
- `shell`, `exec` e `cp` via SSH.
- CLI pequena, sem daemon global, todo estado persistente no drive.

## Fora do escopo

- Publicação de portas, ingress, proxy reverso ou túnel público. O host acessa o IP do guest; acesso externo é problema do operador.
- Bind mount de diretórios do host.
- GPU, passthrough PCI/USB.
- macOS/Windows nativo. O Firecracker exige Linux com KVM.
- Guests sem systemd.
- Gestão de ferramentas, processos, plugins ou fluxos de tarefa.
- Snapshots de VM. Vazam RAM em claro e ficam presos a CPU/versão.

## Decisões de arquitetura

| Tema | Decisão |
|---|---|
| VMM | Firecracker com jailer |
| Binários | Firecracker, jailer, Podman, kernel e initramfs embutidos no `ohmlatl` |
| Host | Linux x86_64 com `/dev/kvm` |
| Build | Podman rootless embutido, `podman unshare` obrigatório |
| Runtime | supervisor do usuário; `sudo` para TAP, nft e jailer |
| Init | systemd |
| Rootfs | LUKS2 + integridade + ext4, `--size` ou calculado |
| Home | dentro do rootfs; refeita com a imagem |
| Volumes | ext4, LUKS2 opcional, ponto de montagem no manifest |
| Passphrase | no `ttyS0` da VM; a do rootfs é tentada primeiro nos volumes |
| Chaves | nenhuma chave LUKS no drive |
| Acesso | `shell`/`exec`/`cp` via SSH |
| Rede | TAP + `/30` + NAT; sem publish/ingress |
| Supervisor | um por VM; sem daemon global |
| Atualização | rebuild com nova tag; volumes intactos |

## Arquitetura

```text
HOST
├── ohmlatl (CLI confiável; embute Firecracker, jailer, Podman, vmlinux, initramfs)
├── $XDG_RUNTIME_DIR/ohmlatl/{bin,jail} (tmpfs)
├── supervisor da VM (um por VM)
│   ├── Firecracker (via jailer)
│   ├── PTY do serial
│   └── log de saída do console
└── TAP + nftables

DRIVE
├── bin/ohmlatl
├── images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volume>.{luks,ext4}
└── vms/<nome>/{state.json,known_hosts,serial.log,console.sock,lock}

GUEST
├── initramfs: prompt de passphrase, fsck, mount, switch_root
├── systemd
├── sshd
└── conteúdo da imagem
```

## Fluxo de boot

```text
ohmlatl run <nome>:<tag>
  → valida o manifest, pega os locks da VM e dos volumes
  → extrai os binários embutidos para tmpfs
  → aloca /30, cria TAP + regras nft (sudo)
  → inicia supervisor + PTY
  → inicia Firecracker via jailer (sudo)
  → kernel + initramfs
      → passphrase do rootfs no ttyS0
      → tenta a mesma em cada volume LUKS, pergunta de novo se falhar
      → e2fsck -p
      → monta rootfs em /newroot, volumes em /newroot/<mount>
      → switch_root
  → systemd → sshd
  → supervisor mantém serial e processo
```

O host passa tudo que o initramfs precisa na cmdline do kernel: `ip=` para a rede, `ohmlatl.user=<uid>:<gid>` e `ohmlatl.disks=<dev>:<luks|ext4>:<mount>,...`. A passphrase nunca entra no manifest, na cmdline, no ambiente ou nos logs.

## Pipeline da imagem de disco

```text
Containerfile ou ref OCI
  → podman build / pull + podman create
  → gera a chave SSH do cliente
  → tamanho: --size ou ceil(du -sb * 1.25) + 512 MiB
  → luksFormat --integrity hmac-sha256 (sudo)
  → podman export + tar dentro de podman unshare
  → limpa machine-id, host keys SSH, hostname; instala authorized_keys
  → mkfs.ext4 -d com o tamanho útil do mapper
  → dd no mapper, fecha
  → images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
```

Regras:

- Todo o pipeline de export/mkfs roda dentro de `podman unshare`. Fora dele, o rootfs fica com o UID do operador e quebra `sudo`, `sshd`, setuid e capabilities.
- `podman export`, não `podman save`.
- `tar --same-owner --numeric-owner --xattrs --xattrs-include='*' --xattrs-exclude=security.selinux`.
- Nada de segredo em `ARG`/`ENV`/`RUN`; use `podman build --secret`.
- Baseline de CPU; sem `-march=native`.
- O build falha se a imagem não tiver systemd, sshd ou o usuário declarado.

## Volumes

- `volume create <nome> --size <n> [--luks]`: `volumes/<nome>.luks` ou `volumes/<nome>.ext4`.
- Volumes LUKS não têm integridade, por causa do custo de escrita.
- Na primeira montagem, a raiz vazia do volume recebe `chown` para o usuário do manifest.
- Um volume só pode ter um dono por vez: a VM ou a montagem no host. Lock por arquivo.
- `volume mount` no host é sempre read-only e só com a VM parada.

## Rede

- Um TAP por VM, `/30` dentro de `172.31.0.0/16`, host `.1`, guest `.2`, alocado pelo `run`.
- NAT via nftables em tabela própria (`inet ohmlatl`), limpa no `stop`.
- IP e DNS do guest via `ip=` na cmdline do kernel; o DNS vem do resolver do host.
- Sem `--publish`, ingress ou proxy. O host acessa `172.31.0.2:<porta>`.
- IPv6 desligado. Sem filtro de egress, documentado como limitação.

## Ciclo de vida e supervisor

- `run` cria a VM, inicia o supervisor e anexa o serial. Numa VM parada, troca a imagem.
- O supervisor mantém o PTY, o Firecracker e o log de saída; sobrevive ao detach da CLI.
- `Ctrl-]` desanexa; `console` reanexa; `shell` usa SSH.
- `stop` envia Ctrl-Alt-Del pela API do Firecracker; com `reboot=k`, o reboot limpo do guest encerra o Firecracker. Kill após 30 s. O supervisor limpa TAP/nft e solta os locks.
- A chave SSH é gerada no build, ao lado da imagem. O `known_hosts` da VM é zerado quando a imagem muda.
- `start` reaproveita a última imagem e pede as passphrases de novo.
- `rm` remove o estado da VM; imagens e volumes ficam.

## CLI

```text
ohmlatl init <dir>
ohmlatl doctor [--build|--run]

ohmlatl build [-f Containerfile | --from <ref>] [-t <name:tag>] [--size <n>]
ohmlatl podman <args>
ohmlatl image ls|inspect|rm

ohmlatl volume create <name> --size <n> [--luks]
ohmlatl volume ls|inspect|mount|umount|rm

ohmlatl run <name:tag> [--vcpus n] [--memory n] [--subnet cidr]
ohmlatl start|stop|rm <vm>
ohmlatl ps
ohmlatl shell <vm> [-- cmd]
ohmlatl exec <vm> -- cmd
ohmlatl cp <vm>:<path> <local>
ohmlatl cp <local> <vm>:<path>
ohmlatl logs <vm> [-f]
ohmlatl console <vm>
```

Sem `expose`, `ingress`, `publish` ou `host-forward`.

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

O `ohmlatl build` copia o manifest para `images/<nome>/<tag>/manifest.yaml`. O manifest fica em claro e é validado antes de virar cmdline do kernel. `name` + `build.tag` formam a referência da imagem; sem tag, `latest`; `-t` sobrescreve. `build.containerfile` é opcional com `--from`.

## Layout do drive

```text
/mnt/vmdisk/
├── bin/ohmlatl
├── images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volume>.{luks,ext4}
└── vms/<nome>/{state.json,known_hosts,serial.log,console.sock,lock}
```

O drive precisa estar montado sem `noexec`. O cache de build do Podman fica no host em `$XDG_CACHE_HOME/ohmlatl/podman`; é cache, não estado.

## Segurança

| Ameaça | Proteção |
|---|---|
| Drive roubado | Rootfs e volumes `--luks` em LUKS2; nenhuma chave LUKS no drive |
| Rootfs adulterado | Integridade `hmac-sha256` por setor; o setor alterado falha na leitura |
| Kernel/initramfs adulterado | Não ficam no drive; vêm embutidos no `ohmlatl` |
| `ohmlatl` adulterado no drive | O operador verifica o binário contra SHA-256 ou assinatura do release, fora do drive |
| Manifest adulterado | Validação estrita antes de gerar a cmdline |
| Outro usuário no host | Arquivos 0600; diretórios 0700 |
| Workload escapando para o host | KVM + kernel próprio + seccomp + jailer |
| Passphrase em log | O supervisor grava só a saída do console; echo desligado |

Limitações: volumes sem `--luks` ficam em claro; volumes LUKS não têm integridade; sem proteção contra rollback de um `rootfs.luks` antigo inteiro; a chave SSH privada fica em claro no drive (0600); sem proteção contra host comprometido.

## Portabilidade

- Linux x86_64 com KVM no MVP.
- Drive em ext4/btrfs/xfs; sem FAT/exFAT/NTFS.
- Kernel e binários com baseline de CPU; sem `-march=native`.
- Sempre cold boot; sem snapshots.

## Fases

### P0: Sanidade do host e boot

- `ohmlatl init` e `ohmlatl doctor`.
- Scripts no repositório que geram o kernel e o initramfs.
- Embutir e extrair os binários.
- Boot de um rootfs de teste simples com Firecracker, sem jailer.

**Pronto quando:** a VM de teste inicializa com o kernel e o initramfs embutidos, dá console, tem rede via `ip=` e o Ctrl-Alt-Del encerra o Firecracker.

### P1: Pipeline da imagem de disco

- `build -f`, `build --from`, `-t`, `--size`.
- `ohmlatl podman <args>` com o Podman embutido.
- `podman unshare` com export/tar/xattrs.
- `rootfs.luks` com integridade; chave SSH; cópia do manifest.
- Limpeza de `machine-id`, host keys SSH e hostname.
- `image ls/inspect/rm`.
- `examples/kali` como primeiro exemplo.

**Pronto quando:** `sudo -n true`, `getcap /bin/ping` e SSH funcionam no guest; o rootfs abre com passphrase no console; o `e2fsck` sai limpo; a imagem Kali personalizada via `podman commit` inicializa; uma imagem sem systemd ou sshd falha com mensagem clara; a mudança de um byte no `rootfs.luks` produz erro de leitura.

### P2: Runtime

- `run`, `start`, `stop`, `rm`, `ps`, `console`, `logs`, `shell`, `exec`, `cp`.
- Supervisor por VM, PTY, log de saída, detach/attach.
- Jailer, TAP/NAT `/30`.
- `volume create/ls/rm`; volumes no initramfs com a regra "mesma passphrase, senão pergunta".

**Pronto quando:** o prompt de passphrase aparece no serial da VM; os volumes montam onde o manifest diz, com o usuário como dono; `shell` entra via SSH; o jailer está ativo; `stop` desliga de forma limpa; `kill -9` no Firecracker seguido de `start` passa no `e2fsck`.

### P3: Volumes no host e várias VMs

- `volume inspect/mount/umount`, read-only, com a VM parada.
- Locks entre VM e montagem no host.
- Subnets automáticas; duas VMs ao mesmo tempo.

**Pronto quando:** um volume sobrevive ao rebuild do rootfs; monta read-only no host; a montagem com a VM rodando é recusada; duas VMs rodam ao mesmo tempo sem colisão.

### Futuro

Sem plano detalhado: update atômico com rollback, backup/restore, `image import/export`, `volume resize`, `gc`, `--json`, completions, aarch64, integridade nos volumes, proteção contra rollback.

## Testes

**Unitários**

- Parser e validação do manifest (nomes, mounts, UID/GID).
- Geração da cmdline.
- Cálculo de tamanho.
- Alocação de subnet.
- Estado da VM e locks de volume.

**Integração**

- Build Podman → ext4 com donos, xattrs e capabilities.
- `--from` com imagem Kali personalizada.
- Falha clara para imagem sem systemd ou sshd.
- `rootfs.luks` com integridade; a mudança de um byte falha a leitura.
- Initramfs com `cryptsetup` no `ttyS0`; mesma passphrase nos volumes; nova tentativa.
- Passphrase do rootfs errada cai no rescue.
- Jailer + TAP + PTY do serial.
- Ctrl-Alt-Del encerra o Firecracker.
- `kill -9` + `e2fsck`.
- `shell`/`exec`/`cp` via SSH.

**E2E**

- Primeiro exemplo: base Kali, `podman run`, instalar x/y/z, `podman commit`, `build --from`, `run`, passphrase, `shell`.
- `init → build → volume create → run → shell → stop → start`.
- Rebuild do rootfs com volumes preservados.
- Montagem read-only de volume no host.

**Matriz**

- Intel e AMD; 8/16/32 GiB de RAM.
- ext4 e btrfs; drives USB e NVMe.
- CPU mais fraca da frota (baseline de CPU).

## Riscos

| Risco | Mitigação |
|---|---|
| KVM indisponível/aninhado | `doctor` falha cedo com instruções |
| Jailer + TAP + PTY passando pelo `sudo` | Primeira tarefa do P2; `--no-jailer` só para debug |
| Ctrl-Alt-Del não encerra o Firecracker | Validar no P0; kill após timeout |
| Prompt do `cryptsetup` no initramfs | `cttyhack`/dup2 do `ttyS0`; testes de echo e nova tentativa |
| Donos errados no build | Pipeline inteiro em `podman unshare`; testes de aceitação |
| `mkfs.ext4 -d` antigo | `doctor` exige e2fsprogs >= 1.43 |
| Host sem suporte a integridade | `doctor --build` verifica cryptsetup e dm-integrity |
| Custo de escrita da integridade no rootfs | Medir no P1; escrita pesada vai para volumes |
| Relógio do guest começa errado (sem RTC) | `broken_system_clock` no `e2fsck.conf` do initramfs; chrony no guest |
| Rootfs cheio | Tamanho padrão com 25% + 512 MiB; `--size` |
| Remoção abrupta do drive | `stop` obrigatório; `e2fsck` no boot |
| Kali puro sem systemd/sshd | `build` valida e aponta para `examples/kali` |
| Commit de container opaco | Documentado como exploração; o Containerfile é a fonte durável |
| Sem filtro de egress | Limitação documentada |
| Binário grande (~100 MB com Podman) | Aceito; extraído uma vez por boot do host |

## Itens a confirmar na implementação

- Versão e config do kernel do guest.
- Opções exatas do `cryptsetup` para integridade (tamanho de setor, journal).
- Detalhes do jailer para TAP e serial.
- Licença do projeto e avisos de terceiros.

## Critério de MVP

O MVP é P0 a P2: um host Linux com KVM, um drive preparado, uma imagem construída com Podman, um boot com a passphrase no console da VM, volumes montados, shell via SSH, jailer ativo e um desligamento limpo. O P3 completa o uso diário do drive.
