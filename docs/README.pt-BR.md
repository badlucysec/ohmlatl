<h1 align="center">ohmlatl</h1>

<p align="center">MicroVMs Firecracker portáteis, reproduzíveis e criptografadas.</p>

<p align="center">Pronúncia: <strong>ohm-lal</strong>.</p>

<p align="center">
  🇺🇸 <a href="README.en.md">English</a> ·
  🇧🇷 Português ·
  🇪🇸 <a href="README.es.md">Español</a>
</p>

<br>

Você formata o computador, troca de máquina ou senta num host emprestado. O ambiente inteiro (sistema, ferramentas, dados e configurações) deveria caber no bolso, e não ficar espalhado em `~/.config`, `/usr/local`, `/var/lib` e uma dúzia de outros lugares que você vai esquecer de copiar.

O `ohmlatl` existe para isso: transforma um drive externo num computador completo, rodando como microVM [Firecracker](https://github.com/firecracker-microvm/firecracker). O Firecracker inicializa em cerca de 125 ms e adiciona menos de 5 MiB de overhead de VMM por microVM. A RAM do guest é a que você configurar. É isso que permite carregar um computador completo num drive sem transformar o host num hypervisor pesado e sem depender da distribuição do host.

O mesmo isolamento serve para o que você não quer rodar no host: uma ferramenta de origem duvidosa, um projeto clonado da internet, um PoC, um `npm install` ou `make` de terceiros. Rode dentro da VM; o host continua limpo.

## O que é o ohmlatl?

O `ohmlatl` gerencia imagens de disco bootáveis e execuções de microVMs Firecracker. Ele usa o Podman embutido **somente no build** para transformar um `Containerfile` ou uma imagem OCI num rootfs ext4. No runtime não há Podman, daemon nem container: há um kernel, um rootfs, discos virtio e um processo Firecracker.

A imagem de disco é um único `rootfs.luks`: LUKS2 com integridade por setor (`hmac-sha256`), contendo o ext4 com o sistema inteiro, incluindo `/home/<user>`. O que precisa sobreviver a um rebuild vai num volume: um disco ext4 extra, criptografado ou não, montado onde o manifest disser (por exemplo `/home/dev/work`).

O binário `ohmlatl` embute Firecracker, jailer, Podman, o kernel do guest e o initramfs. Nada executável fica solto no drive além do próprio `ohmlatl`. O initramfs pede a passphrase no console da VM. Nenhuma chave LUKS fica no drive.

Aqui, `imagem de disco` é o artefato que o Firecracker inicializa. `Imagem OCI` é a fonte usada pelo Podman no build.

O `ohmlatl` não gerencia o conteúdo da imagem: ferramentas, IDEs, dados e serviços são responsabilidade sua. Ele conhece imagens, volumes, VMs e redes.

## Por que usar?

**Porque o host deixa de importar.** O drive carrega as imagens e os volumes. O host de destino só precisa de Linux, `/dev/kvm`, `cryptsetup`, rede, utilitários de TAP/nftables, cliente SSH e `sudo`. Trocar de distribuição ou de máquina não muda o ambiente.

**Porque o isolamento é real.** Containers compartilham o kernel do host. Uma microVM tem kernel próprio, seccomp e, com o jailer, chroot, namespaces e cgroups.

**Porque dá para rodar o que não é confiável.** A ferramenta ou o projeto roda dentro da VM, sob o kernel dela, com acesso por `ohmlatl shell` e `exec`. Não existe diretório compartilhado por onde o código alcance o host; só sai o que você copiar com `ohmlatl cp`. Para começar do zero, gere a imagem de novo com `ohmlatl build`. O guest tem rede com NAT e alcança o gateway do host, então serviços do host que escutam nesse IP ficam expostos a ele.

**Porque a adulteração é detectada.** O kernel e o initramfs vêm do binário confiável `ohmlatl`, não do drive. O rootfs tem integridade por setor: um setor alterado falha na leitura em vez de rodar código alterado. Verifique o próprio `ohmlatl` contra o SHA-256 ou a assinatura do release.

**Porque os dados ficam criptografados em repouso, sem chave no drive.** O rootfs é sempre LUKS2; os volumes são LUKS2 quando você pede. As passphrases são digitadas no `ttyS0` da VM e existem só na memória.

**Porque a imagem é reproduzível.** `Containerfile` mais manifest descrevem o sistema. O build usa Podman rootless dentro de `podman unshare`, então donos, xattrs e capabilities ficam corretos dentro do ext4.

**Porque o ciclo de vida é pequeno.** Imagem, volume, VM. `build`, `run`, `shell`, `stop`. Sem orquestrador, sem daemon global. O supervisor existe por VM e sai junto com ela.

**Porque não há diretório do host dentro da VM.** Sem bind mounts. O acesso é por `ohmlatl shell`, `exec` e `cp` via SSH.

## Como usar

O fluxo completo, com resultados esperados, está em [USAGE.pt-BR.md](USAGE.pt-BR.md). O plano e as fases estão em [ROADMAP.pt-BR.md](ROADMAP.pt-BR.md). Resumo:

**1. Prepare o drive e verifique o host.**

```bash
ohmlatl init /mnt/vmdisk
ohmlatl doctor --build
ohmlatl doctor --run
```

**2. Construa a imagem base.**

```bash
cd examples/kali
ohmlatl build
```

O `ohmlatl build` lê o `Containerfile` declarado no `ohmlatl.yaml` e usa o Podman embutido. O exemplo usa `kalilinux/kali-rolling`, mas a base pode ser qualquer imagem com systemd e sshd. O build pede a passphrase do rootfs e deixa a imagem Podman `kali-base:2026-11-02` pronta para o próximo passo.

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

O tamanho padrão é o conteúdo mais 25% e 512 MiB; `--size 8G` muda. O commit é exploração; quando as ferramentas estabilizarem, passe os comandos para o `Containerfile`.

**5. Crie um volume e inicie a VM.**

```bash
ohmlatl volume create work --size 8G --luks
ohmlatl run kali-custom:2026-11-02
```

O `ohmlatl run` anexa o serial; o initramfs pede a passphrase do rootfs, tenta a mesma no `work` e só pergunta de novo se falhar. `Ctrl-]` desanexa; a VM continua rodando.

```bash
ohmlatl shell kali-custom      # shell via SSH
ohmlatl console kali-custom    # reanexar ao serial
ohmlatl stop kali-custom       # desligamento limpo
```

**6. Leia um volume no host.** Com a VM parada:

```bash
ohmlatl volume mount work /mnt/work
```

A montagem é sempre read-only. O `ohmlatl` não publica portas nem faz proxy; o host acessa o IP do guest, e o guest acessa o host pelo gateway (por exemplo `172.31.0.1:8080`).

## Princípios

- O drive é o computador; o host é o bootloader.
- Nenhuma chave LUKS no drive. A passphrase existe no teclado e na memória.
- Rootfs descartável, volumes persistentes.
- Um dono por disco. O Firecracker não trava arquivos; o `ohmlatl` trava.
- Sem daemon global, sem estado persistente no host, sem bind mount de diretório do host.
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

Pré-alfa. O repositório ainda é o scaffold Rust; os documentos descrevem o contrato de uso e o plano. Comandos e detalhes podem mudar durante P0 a P2.

## Licença

Ainda não definida.
