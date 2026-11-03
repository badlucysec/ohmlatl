# HANDOFF

Contexto para o próximo responsável que assumir o `ohmlatl`. Leia este arquivo antes de mexer em qualquer coisa. Documento interno, só em pt-BR, assim como o `TECHNICAL.md`; README, USAGE e ROADMAP são traduzidos em três idiomas.

## Regras do operador

Estas regras não são sugestões. Foram pedidas explicitamente e valem para código, docs, commits e mensagens.

- Não mude a arquitetura sem autorização explícita. Se achar que algo está errado, discuta primeiro.
- Não use trailers de coautoria em commit.
- Não atribua autoria de código ou documentação a ferramentas.
- Não use o caractere de travessão longo em textos, docs, mensagens de commit ou código.
- A CLI, os prompts e as mensagens de saída são em inglês.
- A pronúncia do projeto é `ohm-lal`.
- README, USAGE e ROADMAP são traduzidos em português, inglês e espanhol.
- Na raiz fica somente `README.md`. Todo o resto da documentação fica em `docs/`.
- Não faça commit sem pedido explícito do operador.
- O exemplo é Kali Rolling, mas a base deve ser genérica. Não use o exemplo antigo de produto.
- Não crie recursos de publicação de portas, ingress, proxy público ou túnel. O host acessa o guest; acesso externo é responsabilidade do operador.
- Não faça bind mount de diretório do host. O acesso é por `ohmlatl shell`, `exec` e `cp`.
- Não crie uma VM por ferramenta. O ambiente é uma VM por imagem, com o conteúdo que o operador definir.
- O escopo de segurança é drive roubado ou adulterado, com host confiável. Não gaste esforço defendendo contra host comprometido enquanto a VM roda.
- Corte over-engineering: só entra o que o MVP precisa.

## O que é o projeto

`ohmlatl` é uma CLI Rust que gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. O binário é autocontido: embute Firecracker, jailer, o bundle Podman, o kernel e o initramfs.

- Podman é usado somente no build, para transformar `Containerfile` ou imagem OCI em rootfs ext4.
- Firecracker e jailer são usados no runtime.
- O runtime não usa Podman, não tem daemon global e não deixa estado persistente no host.
- O drive externo é o computador. O host é quase um bootloader.

## Arquitetura

```text
HOST
├── ohmlatl (CLI autocontida, raiz de confiança)
│   └── embute Firecracker, jailer, Podman, vmlinux, initramfs
├── $XDG_RUNTIME_DIR/ohmlatl/{bin,jail} (tmpfs, efêmero)
├── supervisor por VM (PTY do serial, serial.log, console.sock)
├── TAP + nftables (via sudo)
└── cache de build do Podman em $XDG_CACHE_HOME

DRIVE
├── bin/ohmlatl
├── images/<nome>/<tag>/{rootfs.luks,manifest.yaml,ssh/}
├── volumes/<volume>.{luks,ext4}
└── vms/<nome>/{state.json,known_hosts,serial.log,console.sock,lock}

GUEST
├── initramfs embutido: prompt LUKS, e2fsck, mounts, switch_root
├── systemd
├── sshd
└── conteúdo da imagem, com /home/<user> dentro do rootfs
```

Detalhes completos em `TECHNICAL.md`, que é a fonte de verdade.

## Decisões tomadas

| Tema | Decisão |
|---|---|
| Linguagem da CLI | Rust |
| VMM | Firecracker com jailer |
| Binários | Firecracker, jailer, Podman, kernel e initramfs embutidos no `ohmlatl`, extraídos para tmpfs |
| Build | Podman embutido, rootless, `podman unshare` obrigatório |
| Runtime | supervisor do usuário; sudo para TAP, nft e jailer |
| Init | só systemd |
| Rootfs | LUKS2 + integrity `hmac-sha256` + ext4, tamanho `--size` ou calculado |
| Home | `/home/<user>` dentro do rootfs; o rebuild recomeça do skeleton |
| Volumes | genéricos, com ou sem LUKS2, ponto de montagem no manifest |
| Volume no host | read-only, só com a VM parada, com lock |
| Kernel e initramfs | gerados no repositório e embutidos; não ficam no drive |
| Passphrase | só no console da VM; a do rootfs é tentada nos volumes antes de perguntar outra |
| Acesso | `shell`/`exec`/`cp` por SSH na rede TAP |
| Stop | `SendCtrlAltDel` + `reboot=k`, kill após timeout |
| Rede | TAP + `/30` alocado no `run` + NAT, sem publish ou ingress |
| Supervisor | um por VM, sem daemon global |
| VM | uma por imagem; nome da VM = nome da imagem |
| Atualização | rebuild com nova tag + `run`; volumes intactos |

## O que foi rejeitado ou removido

- `boot.luks`: removido. Kernel e initramfs vêm do binário confiável, então não há o que proteger no drive.
- Hash SHA-256 do rootfs conferido no host: quebra com o rootfs gravável. Substituído pelo integrity.
- vsock, `ohmlatl-guest` e `ohmlatl-init`: over-engineering.
- Modos `init: exec` e `auto` e a tradução de `ENTRYPOINT`/`CMD`.
- Volumes especiais `home` e `data`.
- Política `luks.passphrase shared|separate` e as chaves `luks.rootfs/home`.
- Prompt de tamanho, shrink com `resize2fs -M`, `build.size`, `--yes` no build.
- `--from-container`: use `ohmlatl podman commit` + `build --from`.
- `meta.json`, `kernels/`, `run/` e `backups/` no drive.
- `kernel build|add|ls`, `image import|export`, `volume resize`, `gc`, `--json`, completions, `doctor --fix/--repair` no MVP.
- Uma microVM por ferramenta ou por instância.
- Bind mount de diretório do host.
- Publicação de portas ou ingress.
- Exemplo baseado em produto de orquestração.
- Coautoria em commits e atribuição de autoria a ferramentas.
- Travessão longo.

## Estado atual do repositório

- O código Rust ainda é o scaffold. Não existe CLI implementada.
- A documentação descreve o desenho acima.
- `vendor/` contém Firecracker v1.17.0 e Podman static v6.1.4 (x86_64) como fonte dos binários embutidos.
- O único commit é o do `vendor/`, hash `32b2c15`, autor `lucy <lucy@localhost>`, sem coautoria.
- `README.md`, `docs/`, `examples/` e `src/` estão sem trackear; `vendor/README.md` está modificado.
- Não há remote configurado.

## Pendências principais

1. P0: scripts de kernel e initramfs; boot de um rootfs de teste pelo Firecracker.
2. P1: build com Podman embutido, `rootfs.luks` com integrity, chave SSH, manifest.
3. P2: supervisor, jailer, TAP/NAT, `run`/`start`/`stop`/`console`/`logs`/`shell`/`exec`/`cp`/`ps`/`rm`, volumes básicos.
4. P3: `volume mount/umount/inspect`, várias VMs.
5. Publicar release com SHA-256 e assinatura.
6. Definir licença e avisos de terceiros.

## Arquivos para ler

- `docs/TECHNICAL.md`: fonte de verdade.
- `docs/README.*.md`, `docs/USAGE.*.md`, `docs/ROADMAP.*.md`: visão, fluxos, fases.
- `examples/kali/Containerfile` e `examples/kali/ohmlatl.yaml`.
- `vendor/README.md`: binários, checksums e uso do Podman.

## Armadilhas

- Não esqueça `podman unshare` no pipeline; não use `podman save`.
- Não crie o ext4 com tamanho chutado: use o tamanho útil do mapper com integrity.
- Não monte um volume no host e no guest ao mesmo tempo; respeite o lock.
- Não confie no `manifest.yaml` do drive sem validar: ele vira cmdline do kernel.
- Não coloque passphrase em argv, env, manifest, cmdline ou log.
- Não use `-march=native`.
- Não use FAT, exFAT ou NTFS no drive.
- Não deixe a CLI em português.
- Não adicione coautoria ou atribuição de ferramenta em commits.
- Não use travessão longo.
- Não crie docs na raiz além do `README.md`.

## Comandos úteis

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
cargo build --release
```

## Glossário

- **rootfs.luks**: disco LUKS2 + integrity + ext4 com o sistema da VM, incluindo a home.
- **volume**: disco extra da VM, ext4 com ou sem LUKS2, montado onde o manifest mandar.
- **manifest**: `ohmlatl.yaml` do projeto, copiado para `manifest.yaml` ao lado do rootfs.
- **supervisor**: processo por VM que controla o Firecracker e o serial.
- **guest**: o sistema dentro da microVM.
- **embutido**: compilado dentro do binário `ohmlatl`, não um arquivo separado no drive.
