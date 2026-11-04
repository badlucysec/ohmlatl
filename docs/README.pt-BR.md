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

O `ohmlatl` existe para isso: o computador inteiro é um único arquivo criptografado, `rootfs.luks`, inicializado como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). Copie o arquivo para outro host e inicialize lá. O Firecracker inicializa em cerca de 125 ms e adiciona menos de 5 MiB de overhead de VMM por microVM. A RAM do guest é a que você configurar. É isso que permite carregar um computador completo sem transformar o host num hypervisor pesado e sem depender da distribuição do host.

O mesmo isolamento serve para o que você não quer rodar no host: uma ferramenta de origem duvidosa, um projeto clonado da internet, um PoC, um `npm install` ou `make` de terceiros, um agente. Rode dentro da VM; por padrão ela não alcança o host, a LAN nem outras VMs, só o que você liberar.

## O que é o ohmlatl?

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Ele usa o Podman embutido **somente no build** para transformar um `Containerfile` ou uma imagem OCI num rootfs ext4. No runtime não há Podman nem container: há um kernel, um rootfs, um disco virtio e um processo Firecracker.

A imagem de disco é um único `rootfs.luks`: LUKS2 com integridade por setor (`hmac-sha256`), contendo o ext4 com o sistema inteiro, incluindo `/home/<user>`. A VM escreve direto nele: é o seu computador, não um modelo. Você atualiza por dentro (`apt upgrade`), e os dados saem por `ohmlatl cp`.

O binário `ohmlatl` embute Firecracker, jailer, Podman, o kernel do guest e o initramfs. O initramfs pede a passphrase no console da VM. Nenhuma chave LUKS fica em disco.

Aqui, `imagem de disco` é o artefato que o Firecracker inicializa. `Imagem OCI` é a fonte usada pelo Podman no build.

O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados e serviços são responsabilidade sua. Ele conhece imagens, VMs e redes.

## Por que usar?

**Porque o host deixa de importar.** O computador é um arquivo. O host de destino só precisa de Linux, `/dev/kvm`, rede, utilitários de TAP/nftables, `systemd-run`, cliente SSH e `sudo`. Trocar de distribuição ou de máquina não muda o ambiente.

**Porque o isolamento é real.** Containers compartilham o kernel do host. Uma microVM tem kernel próprio, seccomp e, com o jailer, chroot, namespaces e cgroups.

**Porque dá para rodar o que não é confiável.** A ferramenta, o projeto ou o agente roda dentro da VM, sob o kernel dela, com acesso por `ohmlatl shell` e `exec`. Não existe diretório compartilhado por onde o código alcance o host; só sai o que você copiar com `ohmlatl cp`. Por padrão o guest não alcança o host, a LAN nem outras VMs. Você libera exatamente o que ele precisa, por exemplo o Ollama do seu host e um servidor MCP em outra VM, e nada mais.

**Porque a adulteração é detectada.** O kernel e o initramfs vêm do binário confiável `ohmlatl`, não da imagem de disco. O rootfs tem integridade por setor: um setor alterado falha na leitura em vez de rodar código alterado.

**Porque os dados ficam criptografados em repouso, sem chave em disco.** O rootfs é sempre LUKS2. A passphrase é digitada no terminal do host quando a imagem é gerada e no `ttyS0` da VM no boot, e existe só na memória.

**Porque a imagem é declarativa.** `Containerfile` mais manifest descrevem o sistema. O build usa Podman rootless dentro de `podman unshare`, então donos, xattrs e capabilities ficam corretos dentro do ext4.

**Porque o ciclo de vida é pequeno.** Imagem, VM. `build`, `run`, `shell`, `stop`. Sem orquestrador, sem daemon global. O supervisor existe por VM e sai junto com ela.

## Como usar

O fluxo completo, com resultados esperados, está em [USAGE.pt-BR.md](USAGE.pt-BR.md). O plano e as fases estão em [ROADMAP.pt-BR.md](ROADMAP.pt-BR.md). Resumo:

**1. Verifique o host.**

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

**2. Gere a imagem base.**

```bash
cd examples/kali
ohmlatl build
```

O `ohmlatl build` lê o `Containerfile` declarado no `ohmlatl.yaml` e usa o Podman embutido. O exemplo usa `docker.io/kalilinux/kali-rolling`, mas a base pode ser qualquer imagem com systemd e sshd. O build pede a passphrase do rootfs e deixa a imagem Podman `kali-base:2026-11-02` pronta para o próximo passo.

**3. Instale o que quiser num container.**

```bash
ohmlatl podman run --name kali-dev -it kali-base:2026-11-02

apt-get update
apt-get install -y x y z
apt-get clean
exit
```

Troque `x`, `y` e `z` pelos pacotes que você quer.

**4. Transforme o container em imagem de disco.**

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

O tamanho padrão é o conteúdo mais 25% e 512 MiB; `--size 8G` muda isso. O commit é exploração; quando as ferramentas estabilizarem, passe os comandos para o `Containerfile`.

**5. Inicialize a VM.**

```bash
ohmlatl run kali-custom:2026-11-02
```

O `ohmlatl run` anexa o serial; o initramfs pede a passphrase do rootfs. `Ctrl-]` desanexa; a VM continua rodando.

```bash
ohmlatl shell kali-custom      # shell via SSH
ohmlatl console kali-custom    # reanexa ao serial
ohmlatl stop kali-custom       # desligamento limpo
```

**6. Libere só o que a VM precisa.** No `ohmlatl.yaml`:

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

Dentro da VM, `172.31.0.1:11434` é o Ollama do host e `172.31.0.1:8080` é o servidor MCP. Nada mais é alcançável.

## Princípios

- O rootfs é o computador; o host é o bootloader.
- Nenhuma chave LUKS em disco. A passphrase existe no teclado e na memória.
- Host, LAN e outras VMs fechados por padrão; toda abertura é declarada.
- Sem daemon global, sem bind mount de diretório do host.
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
