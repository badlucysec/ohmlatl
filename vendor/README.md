# vendor

Binários de terceiros usados pelo `ohmlatl`. São Linux x86_64. O Firecracker e o jailer daqui são embutidos no binário. O bundle Podman daqui é só para desenvolvimento e sai do git no P0: o bundle embutido passa a ser compilado por `cargo xtask assets` com a receita do podman-static, para o release ter as fontes exatas de cada componente GPL (ver `docs/TECHNICAL.md`, Geração dos assets).

## Firecracker

- Versão: `v1.17.0`
- Arquitetura: `x86_64`
- Origem: `https://github.com/firecracker-microvm/firecracker/releases/tag/v1.17.0`
- Artefato: `firecracker-v1.17.0-x86_64.tgz`
- SHA-256 do tarball: `06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558`
- Diretório: `vendor/firecracker/v1.17.0`
- Atalhos: `vendor/bin/firecracker` e `vendor/bin/jailer`

O release inclui `LICENSE`, `NOTICE`, `THIRD-PARTY`, `SHA256SUMS`, templates de CPU e o filtro seccomp. Os binários `*.debug` foram omitidos; `SHA256SUMS.included` cobre exatamente os arquivos mantidos.

## Podman

- Versão: `v6.1.4`
- Arquitetura: `amd64`
- Origem: `https://github.com/mgoltzsche/podman-static/releases/tag/v6.1.4`
- Artefato: `podman-linux-amd64.tar.gz`
- SHA-256 do tarball: `9dd51073fac5a1742f88cf3caff5554615cb4d67ba554d25e8807be635547af5`
- Assinatura: `vendor/podman/v6.1.4/podman-linux-amd64.tar.gz.asc`
- Chave: `0CCF102C4F95D89E583FF1D4F8B5AF50344BB503`
- Diretório: `vendor/podman/v6.1.4`
- Atalho: `vendor/bin/podman`

O bundle inclui `podman`, `crun`, `runc`, `conmon`, `netavark`, `aardvark-dns`, `pasta`, `pasta.avx2`, `passt`, `fuse-overlayfs`, `fusermount3`, `catatonit`, `rootlessport` e as configurações em `etc/containers`.

A assinatura foi verificada com a chave acima. Para repetir:

```bash
gpg --batch --keyserver hkps://keyserver.ubuntu.com \
    --recv-keys 0CCF102C4F95D89E583FF1D4F8B5AF50344BB503
gpg --batch --verify \
    vendor/podman/v6.1.4/podman-linux-amd64.tar.gz.asc \
    podman-linux-amd64.tar.gz
```

O último comando espera o tarball original, que não fica no repositório.

## Uso

O wrapper `vendor/bin/podman` resolve os caminhos do bundle, gera a configuração em `$XDG_RUNTIME_DIR/ohmlatl-podman` a partir de `vendor/podman/*.conf.in`, usa `vendor/podman/registries.conf` (sem busca de nomes curtos: imagem remota precisa de nome qualificado, como `docker.io/...`) e usa o storage em `$XDG_CACHE_HOME/ohmlatl/podman`, ou em `$HOME/.cache/ohmlatl/podman` quando `XDG_CACHE_HOME` não existe. Nada é instalado em `/usr/local`. O `containers.conf` gerado fixa o `crun`, o `conmon` e o `catatonit` do bundle, para um `crun` do host não ser usado no lugar. Esse wrapper é só para desenvolvimento: o `ohmlatl` embute o bundle compilado por `cargo xtask assets` e gera a mesma configuração em Rust.

```bash
export PATH="$PWD/vendor/bin:$PATH"

firecracker --version
jailer --version
podman --version

podman info
podman run --rm --network=none docker.io/library/alpine:3.20 echo ok
```

## Limites

- Os binários são só para `linux/x86_64`. Para `aarch64`, baixe os artefatos correspondentes.
- O Podman static não tem integração com systemd. Ele serve para o build, não para rodar serviços no host.
- Rootless no host ainda depende de `newuidmap`/`newgidmap`, pelo menos 65536 ids em subuid/subgid e `/dev/fuse`. O `ohmlatl doctor --build` deve validar isso. O storage não usa `ignore_chown_errors`: uma faixa de subuid pequena falha em vez de achatar os donos da imagem.
- Hoje `vendor/` ocupa cerca de 100 MB no Git. Depois do P0 fica só o Firecracker (Apache-2.0, binário oficial) e as configurações em `vendor/podman/*.conf*`. Atualizações trocam binários e checksums no mesmo commit.
