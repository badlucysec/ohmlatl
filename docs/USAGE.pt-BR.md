# ohmlatl: uso

🇺🇸 [English](USAGE.en.md) · 🇧🇷 Português · 🇪🇸 [Español](USAGE.es.md)

Este documento propõe os fluxos de uso do `ohmlatl` antes da implementação. É o contrato de experiência: comandos, prompts e resultados esperados. A implementação pode ajustar detalhes, mas não deve mudar a história sem atualizar este arquivo.

## Conceitos

- **Imagem de disco**: um `rootfs.luks` (LUKS2 + integrity + ext4) mais o `manifest.yaml` e as chaves SSH criadas com ele. Guarda o computador inteiro, incluindo `/home/<user>`, e a VM escreve nele. É gerada a partir de um `Containerfile` ou de uma imagem OCI.
- **VM**: uma execução de uma imagem, com recursos e rede. Há uma VM por imagem, com o nome da imagem.
- **Manifest**: `ohmlatl.yaml`. Declara o nome da imagem, o usuário, os recursos e a rede.
- **Forward**: uma porta TCP no gateway da VM que leva a um serviço do host ou a outra VM. É a única saída da VM além da internet, quando liberada.
- **Supervisor**: uma unidade de sistema por VM que controla o Firecracker, o PTY do serial, a rede e os forwards. Não é um daemon global.
- **Jailer**: o sandbox do Firecracker (chroot, namespaces, seccomp, cgroups).

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

Imagens e estado das VMs ficam em `~/.local/share/ohmlatl`.

## 1. Verificar o host

```bash
ohmlatl doctor --build
ohmlatl doctor --run
```

`--build` verifica Podman rootless, `/etc/subuid`, `/etc/subgid`, `newuidmap`, `/dev/fuse`, `e2fsprogs` e `cryptsetup` com integrity. `--run` verifica `/dev/kvm`, `/dev/net/tun`, `ip`, `nft`, cgroup v2, `systemd-run`, `sudo`, o cliente SSH e se Docker ou ufw bloqueiam o forwarding.

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
  [ok] systemd-run
  [ok] ssh client
  [ok] forward policy
```

Se algo falhar, o comando imprime a correção sugerida e sai com código diferente de zero.

## 2. Primeiro exemplo: imagem de disco personalizada

O fluxo que a maioria vai querer: rodar um container base, instalar x, y, z e transformar o resultado numa microVM. O exemplo usa Kali Rolling; troque a base se quiser.

### 2.1 Base do guest

O `kalilinux/kali-rolling` puro não tem systemd nem sshd. Use a base do repositório:

```bash
cd examples/kali
ohmlatl build
```

O `ohmlatl build` lê o `Containerfile` declarado em `build.containerfile` e usa o Podman embutido. Ele produz a imagem de disco `kali-base:2026-11-02` e deixa a imagem Podman com o mesmo nome para o próximo passo. `name` e `build.tag` identificam o artefato; sem tag, `latest`; `-t` sobrescreve.

### 2.2 Explorar e instalar no container

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

### 2.3 Gerar a imagem de disco bootável

No diretório do projeto, com o `ohmlatl.yaml` do exemplo:

```bash
ohmlatl podman commit kali-dev kali-custom:2026-11-02
ohmlatl build --from kali-custom:2026-11-02 -t kali-custom:2026-11-02
```

`--from` só usa imagens locais; imagens remotas precisam de nome completo, como `docker.io/...`. `-t` define o nome e a tag do resultado, e o build grava os dois no `manifest.yaml` da imagem. O build exporta o rootfs dentro de `podman unshare`, gera as chaves SSH, calcula o tamanho e pede a passphrase do rootfs:

```text
[build] exporting rootfs (podman unshare)
[build] rootfs content: 2.1 GiB
[build] size: 3.2 GiB (content + 25% + 512 MiB; --size to change)
Rootfs passphrase:
Repeat:
[build] luks2 + integrity (hmac-sha256)
[build] mkfs.ext4 + e2fsck
[build] disk image kali-custom:2026-11-02
  rootfs: ~/.local/share/ohmlatl/images/kali-custom/2026-11-02/rootfs.luks
  size:   3.2 GiB
```

`--size 8G` define o tamanho. O build pede `sudo` uma vez para abrir o device-mapper do LUKS. Os arquivos temporários ficam em `~/.cache/ohmlatl/build` e são apagados no fim.

### 2.4 Fixar num Containerfile

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

O build falha com mensagem clara se a imagem não tiver systemd, sshd ou o `user.name` com o UID/GID declarados.

## 3. Primeiro boot

```bash
ohmlatl run kali-custom:2026-11-02
```

O comando pede `sudo` uma vez, sobe o supervisor da VM `kali-custom`, cria a rede e anexa o serial.

Resultado esperado:

```text
[ohmlatl] vm kali-custom
[ohmlatl] rootfs: images/kali-custom/2026-11-02/rootfs.luks (LUKS2 + integrity)
[ohmlatl] network: ohmlatl-0 172.31.0.1/30, guest 172.31.0.2, internet on
[ohmlatl] forwards: none
[ohmlatl] starting firecracker (jailer)
[ohmlatl] serial attached; Ctrl-] to detach
...
LUKS passphrase for rootfs:
```

Depois do boot:

```text
kali-custom login:
```

`Ctrl-]` desanexa o terminal; a VM continua rodando sob o supervisor, mesmo depois do logout. `--vcpus` e `--memory` sobrescrevem o manifest. Nenhuma passphrase em argv, ambiente, manifest ou log.

## 4. Acessar a VM

```bash
ohmlatl shell kali-custom
```

```text
dev@kali-custom:~$
```

O shell usa SSH pela rede da VM com a chave e a host key criadas no build. Ele ignora o seu `~/.ssh/config` e nunca repassa o seu agente SSH. Não existe diretório do host dentro da VM; o acesso é sempre por aqui.

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

`logs` mostra a saída do serial. O log fica em `/run/ohmlatl` e some quando o host reinicia.

## 5. Rede

Por padrão a VM alcança a internet e nada mais: nem o host, nem a LAN, nem outras VMs. O host alcança a VM pelo IP dela (`172.31.0.2`).

Para cortar a internet e abrir só serviços específicos, use `vm.network` no `manifest.yaml` da imagem (ou no `ohmlatl.yaml` antes do build). Exemplo: um agente que só pode usar o Ollama do host e um servidor MCP em outra VM.

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
[ohmlatl] network: ohmlatl-1 172.31.0.5/30, guest 172.31.0.6, internet off
[ohmlatl] forwards:
  172.31.0.5:11434 -> host 127.0.0.1:11434
  172.31.0.5:8080  -> vm mcp:8080
```

Dentro da VM o gateway é o único endereço que responde:

```bash
curl http://172.31.0.5:11434/api/tags
```

- `to: host:<porta>` chega em `127.0.0.1:<porta>` do host, então o Ollama pode continuar escutando só em loopback.
- `to: vm/<nome>:<porta>` chega em outra VM do `ohmlatl`. Se ela estiver parada, a conexão fecha.
- Só TCP. O `run` sempre imprime os forwards abertos.
- Mudar `vm.network` no `manifest.yaml` da imagem vale no próximo `start`; sem rebuild.

## 6. Status e ciclo de vida

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

`stop` manda Ctrl-Alt-Del para o guest; o systemd desliga de forma limpa e o Firecracker sai. Se não sair em 30 s, o `ohmlatl` mata o processo. Dentro da VM, `poweroff` e `reboot` também encerram a VM.

```bash
ohmlatl start kali-custom
```

`start` usa a última imagem e pede a passphrase de novo no serial.

```bash
ohmlatl rm kali-custom
```

Remove o estado da VM. A imagem continua; use `ohmlatl image rm` para ela.

## 7. Atualizar: por dentro ou rebuild

O rootfs é o seu computador e guarda tudo o que você faz nele. Atualize por dentro:

```bash
ohmlatl exec kali-custom -- sudo apt-get update
ohmlatl exec kali-custom -- sudo apt-get -y full-upgrade
```

Um rebuild cria um **computador novo e vazio** a partir do `Containerfile`:

```bash
ohmlatl build -t kali-custom:2026-12-01
ohmlatl stop kali-custom
ohmlatl run kali-custom:2026-12-01
```

`run` numa VM parada troca a imagem. Nada do rootfs antigo passa para o novo: copie antes o que importa com `ohmlatl cp`. A imagem antiga continua até `ohmlatl image rm kali-custom:2026-11-02`. Com a VM rodando, `run` falha.

## 8. Levar para outro host

O computador é o diretório da imagem:

```bash
rsync -a ~/.local/share/ohmlatl/images/kali-custom/ otherhost:.local/share/ohmlatl/images/kali-custom/
```

No outro host, com o `ohmlatl` instalado:

```bash
ohmlatl run kali-custom:2026-11-02
```

O `rootfs.luks` ocupa o tamanho cheio no disco (o integrity grava todos os setores). Pare a VM antes de copiar. O cache do Podman fica no host original: para fazer rebuild no host novo, a cadeia de build inteira (`kali-base`, depois `kali-custom`) roda de novo lá.

## 9. Várias VMs

```bash
ohmlatl run mcp:2026-11-02
ohmlatl run agent:2026-11-02
ohmlatl ps
```

Cada imagem é uma VM com seu próprio `/30`, alocado automaticamente. As VMs não se enxergam; só um forward `vm/<nome>` liga uma à outra.

## 10. Falhas e recuperação

**Passphrase do rootfs errada.** O `cryptsetup` tenta de novo; depois de três falhas cai no rescue shell do initramfs:

```text
(initramfs) #
```

Use `ohmlatl console kali-custom` para anexar.

**Rootfs adulterado.** A verificação de integridade transforma o setor alterado em erro de leitura; o `e2fsck` ou a montagem falha e o boot cai no rescue. Restaure uma cópia ou refaça a imagem.

**O Firecracker morreu.** `ohmlatl ps` mostra `stopped`; `ohmlatl start kali-custom` inicializa de novo. O initramfs roda `e2fsck -p` antes de montar.

**O host reiniciou.** As VMs não sobem sozinhas; `ohmlatl ps` e `ohmlatl start`.

**Sem rede dentro da VM num host com Docker ou ufw.** `ohmlatl doctor --run` mostra a regra que libera `ohmlatl-*`.

**Passphrase perdida.** O rootfs não abre. Não há chave LUKS em disco; refaça a imagem e comece do zero.

## 11. Não-objetivos

- Sem publicação de portas, ingress ou proxy público. Os forwards só saem da VM; o host alcança o IP do guest; acesso externo é responsabilidade sua.
- Sem bind mount de diretórios do host. Arquivos entram e saem com `ohmlatl cp`.
- Sem discos extras nem volumes. O computador é o rootfs.
- Sem GPU nem tela. A VM é usada pelo terminal.
- Sem macOS/Windows. O Firecracker exige Linux com KVM.
- Sem gerenciamento de ferramentas, processos, plugins ou fluxos de tarefa. Isso é conteúdo da imagem.

## 12. Contrato de UX

- Passphrases são digitadas no TTY do host no `build` e no TTY da VM no boot. Nunca em flag, env, manifest, arquivo ou log.
- Comandos destrutivos pedem confirmação.
- Erros dizem o que falhou e o que fazer; códigos de saída são estáveis.
- Sem daemon global. O supervisor é por VM e sai junto com ela.
- Toda abertura de rede é declarada no manifest e impressa pelo `run`.
