<h1 align="center">ohmlatl</h1>

<p align="center">MicroVMs Firecracker portáteis, declarativas e criptografadas.</p>

<p align="center">Pronúncia: <strong>ohm-lal</strong>.</p>

<p align="center">
  🇺🇸 <a href="README.en.md">English</a> ·
  🇧🇷 Português ·
  🇪🇸 <a href="README.es.md">Español</a>
</p>

<br>

Você formata o computador, troca de máquina ou senta num host emprestado. O ambiente inteiro (sistema, ferramentas, dados e configurações) deveria caber no bolso, e não ficar espalhado em `~/.config`, `/usr/local`, `/var/lib` e uma dúzia de outros lugares que você vai esquecer de copiar.

O `ohmlatl` existe para isso: o computador inteiro é um único arquivo criptografado, `rootfs.luks`, que mora no seu pendrive ou HD externo ao lado do binário `ohmlatl` e inicializa como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). Plugue o drive em outra máquina e inicialize lá. O Firecracker inicializa em cerca de 125 ms e adiciona menos de 5 MiB de overhead de VMM por microVM. A RAM do guest é a que você configurar. É isso que permite carregar um computador completo sem transformar o host num hypervisor pesado e sem depender da distribuição do host.

O mesmo isolamento serve para o que você não quer rodar no host: uma ferramenta de origem duvidosa, um projeto clonado da internet, um PoC, um `npm install` ou `make` de terceiros, um agente. Gere uma imagem separada para isso e rode como uma VM própria: ela alcança a internet, mas não o host, a LAN nem outras VMs, a não ser que você abra uma porta específica.

## O que é o ohmlatl?

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Ele usa o Podman embutido **somente no build** para transformar um `Containerfile` ou uma imagem OCI num rootfs ext4. No runtime não há Podman nem container: há um kernel, um rootfs, um disco virtio e um processo Firecracker.

A imagem de disco é um `rootfs.luks`: LUKS2 contendo o ext4 com o sistema inteiro, incluindo `/home/<user>`, ao lado de um `manifest.yaml`. É um arquivo esparso: só ocupa no drive o que já foi escrito. A VM escreve direto nele: é o seu computador, não um modelo. Você atualiza por dentro (`apt upgrade`), e os dados saem por `ohmlatl cp`.

O binário `ohmlatl` embute Firecracker, jailer, Podman, o kernel do guest e o initramfs. O drive guarda só o binário e as imagens. O initramfs pede a passphrase no console da VM. Nenhuma chave LUKS nem chave SSH fica no drive.

Aqui, `imagem de disco` é o artefato que o Firecracker inicializa. `Imagem OCI` é a fonte usada pelo Podman no build.

O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados, serviços e a forma de login por SSH são responsabilidade sua. Ele conhece imagens, VMs e redes.

## Por que usar?

**Porque o host deixa de importar.** O computador viaja no drive. O host só precisa de Linux, `/dev/kvm`, nftables, systemd, cliente SSH e `sudo` para você. Nada persistente fica nele. Trocar de distribuição ou de máquina não muda o ambiente. Um host emprestado ainda tem que ser de confiança: ele vê a passphrase que você digita e a memória da VM.

**Porque o isolamento é real.** Containers compartilham o kernel do host. Uma microVM tem kernel próprio, seccomp e, com o jailer, chroot, namespaces e cgroups.

**Porque dá para rodar o que não é confiável.** A ferramenta, o projeto ou o agente roda numa VM própria, sob o kernel dela, com acesso por `ohmlatl shell` e `exec`. Não existe diretório compartilhado por onde o código alcance o host; só sai o que você copiar com `ohmlatl cp`. O guest não alcança o host, a LAN nem outras VMs. Você libera exatamente o que ele precisa, por exemplo o Ollama do seu host e um servidor MCP em outra VM, e nada mais. Mantenha isso longe da imagem do seu computador pessoal; para começar limpo, faça rebuild.

**Porque um drive perdido não serve para nada sem a passphrase.** O rootfs é sempre LUKS2, e nenhuma chave fica no drive. A passphrase é digitada no terminal do host quando a imagem é gerada e no `ttyS0` da VM no boot, e existe só na memória. O kernel e o initramfs vêm do binário `ohmlatl`, não da imagem de disco. Alterar o arquivo criptografado sem a chave não injeta código: vira lixo, e o boot ou o programa quebra.

**Porque a imagem é declarativa.** `Containerfile` mais manifest descrevem o sistema. O build usa Podman rootless dentro de `podman unshare`, então donos, xattrs e capabilities ficam corretos dentro do ext4.

**Porque o ciclo de vida é pequeno.** Imagem, VM. `build`, `run`, `shell`, `stop`. Sem orquestrador, sem daemon global, sem estado de VM no drive. O supervisor existe por VM e sai junto com ela.

## Como usar

O fluxo completo, com resultados esperados, está em [USAGE.pt-BR.md](USAGE.pt-BR.md). O plano e as fases estão em [ROADMAP.pt-BR.md](ROADMAP.pt-BR.md). Resumo:

**1. Prepare o drive.** Formatado em ext4 (ou btrfs/xfs):

```bash
./ohmlatl init /media/$USER/drive/ohmlatl
export PATH="/media/$USER/drive/ohmlatl:$PATH"
ohmlatl doctor --build
ohmlatl doctor --run
```

O `init` copia o binário para o drive (pedindo `sudo` uma vez se a raiz do drive for do root). Daí em diante, o `ohmlatl` do drive acha as imagens sozinho.

**2. Gere o container base.**

```bash
cd examples/kali
ohmlatl podman build -t kali-base:2026-11-02 .
```

O exemplo usa `docker.io/kalilinux/kali-rolling`, mas a base pode ser qualquer imagem com systemd e sshd. Este passo só cria uma imagem Podman; sem disco e sem passphrase.

**3. Instale o que quiser num container.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
passwd dev           # senha para o ohmlatl shell
apt-get clean
exit
```

Troque `x`, `y` e `z` pelos pacotes que você quer. Para entrar com chave em vez de senha, coloque a sua chave pública em `/home/dev/.ssh/authorized_keys`.

**4. Transforme o container num computador no drive.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

O build pede a passphrase do rootfs. O disco padrão tem 32 GiB (ou o dobro do conteúdo mais 1 GiB, o que for maior); `--size 64G` muda isso. O arquivo é esparso, então o drive só paga pelo que for escrito. O commit é exploração; quando as ferramentas estabilizarem, passe os comandos para o `Containerfile`.

**5. Inicialize a VM, aqui ou em qualquer outro host.**

```bash
ohmlatl run kali-custom:2026-11-02
```

O `ohmlatl run` pede `sudo` uma vez e anexa o serial; o initramfs pede a passphrase do rootfs. `Ctrl-]` desanexa; a VM continua rodando, mesmo depois do logout.

```bash
ohmlatl shell kali-custom      # shell via SSH (senha uma vez por boot)
ohmlatl console kali-custom    # reanexa ao serial
ohmlatl stop kali-custom       # desligamento limpo; sempre antes de tirar o drive
```

**6. Libere só o que a VM precisa.** No `manifest.yaml` da imagem (`images/<nome>/<tag>/` no drive), ou no `ohmlatl.yaml` antes do build:

```yaml
vm:
  network:
    internet: false
    forward:
      - port: 11434
        to: host:11434      # Ollama do host
      - port: 8080
        to: vm/mcp:8080     # servidor MCP na VM "mcp"
```

Dentro da VM, `ohmlatl-gateway:11434` é o Ollama do host e `ohmlatl-gateway:8080` é o servidor MCP. Nada mais é alcançável. A mudança vale no próximo `run`.

## Princípios

- O drive é o computador; o host é o bootloader, e você confia nele.
- Nenhuma chave LUKS nem chave SSH no drive. A passphrase existe no teclado e na memória.
- O guest não alcança o host, a LAN nem outras VMs sem declaração.
- Sem daemon global, sem bind mount de diretório do host, nada persistente no host.
- A imagem é declarativa; o runtime é pequeno e explícito.

## Desenvolvimento

Os binários locais ficam em `vendor/`. Eles são a fonte dos binários embutidos:

```bash
export PATH="$PWD/vendor/bin:$PATH"
firecracker --version
podman --version
```

Versões, checksums e limites estão em [vendor/README.md](../vendor/README.md).

## Status

Pré-alfa. O repositório ainda é o scaffold Rust; os documentos descrevem o contrato de usabilidade e o plano. Comandos e detalhes podem mudar durante P0 a P2.

## Licença

Ainda não definida.
