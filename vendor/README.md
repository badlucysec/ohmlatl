# vendor

Binários de terceiros usados pelo `ohmlatl`. São Linux x86_64. O Firecracker e o jailer daqui são embutidos no binário. O bundle Podman é compilado por `cargo xtask assets` com a receita do podman-static, para o release ter as fontes exatas de cada componente GPL (ver `docs/TECHNICAL.md`, Geração dos assets).

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

O bundle Podman não fica mais no repositório. O `cargo xtask assets` compila o bundle com a receita do podman-static v6.1.4, fixada em `assets/podman/podman.lock`, e grava em `assets/out/podman/` (`bin/`, `lib/podman/`, `etc/containers/policy.json`). Ficam aqui só as configurações `vendor/podman/*.conf*` e o atalho `vendor/bin/podman`, para desenvolvimento.

O bundle inclui `podman`, `crun`, `conmon`, `netavark`, `aardvark-dns`, `rootlessport`, `pasta`, `pasta.avx2`, `fuse-overlayfs`, `fusermount3`, `catatonit` e `etc/containers/policy.json`. O `runc` e o `passt` avulso não entram.

O primeiro `cargo xtask assets` precisa de um `podman` no host; depois disso, o atalho daqui serve para os próximos.

## Uso

O wrapper `vendor/bin/podman` usa o bundle de `assets/out/podman/`, gera a configuração em `$XDG_RUNTIME_DIR/ohmlatl-podman` a partir de `vendor/podman/*.conf.in`, usa `vendor/podman/registries.conf` (sem busca de nomes curtos: imagem remota precisa de nome qualificado, como `docker.io/...`) e usa o storage em `$XDG_CACHE_HOME/ohmlatl/podman`, ou em `$HOME/.cache/ohmlatl/podman` quando `XDG_CACHE_HOME` não existe. Nada é instalado em `/usr/local`. O `containers.conf` gerado fixa o `crun`, o `conmon` e o `catatonit` do bundle, para um `crun` do host não ser usado no lugar. Esse wrapper é só para desenvolvimento: o `ohmlatl` embute o mesmo bundle e gera a mesma configuração em Rust.

```bash
export PATH="$PWD/vendor/bin:$PATH"

firecracker --version
jailer --version
podman --version

podman info
podman run --rm --network=none docker.io/library/alpine:3.24 echo ok
```

## Limites

- Os binários são só para `linux/x86_64`. Para `aarch64`, baixe os artefatos correspondentes.
- O Podman static não tem integração com systemd. Ele serve para o build, não para rodar serviços no host.
- Rootless no host ainda depende de `newuidmap`/`newgidmap`, pelo menos 65536 ids em subuid/subgid e `/dev/fuse`. O `ohmlatl doctor --build` valida isso. O storage não usa `ignore_chown_errors`: uma faixa de subuid pequena falha em vez de achatar os donos da imagem.
- `vendor/` guarda só o Firecracker (Apache-2.0, binário oficial) e as configurações em `vendor/podman/*.conf*`. Os binários do Podman de antes do P0 continuam nos commits antigos do histórico. Atualizações do Firecracker trocam binários e checksums no mesmo commit.
