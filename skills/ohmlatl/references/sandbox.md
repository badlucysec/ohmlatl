# Running untrusted work in a VM

Use this for anything the user does not want on the host or in their personal computer: a tool of unknown origin, a cloned repository, a PoC or exploit, a third-party `npm install`, `pip install` or `make`, a malware sample for analysis, or an AI agent with shell access.

## Why a VM and not a container

A container shares the host kernel. An ohmlatl VM has its own kernel under KVM, Firecracker's seccomp, and the jailer (chroot, namespaces, cgroups). There is no shared directory: only what is copied out with `ohmlatl cp` leaves. The guest cannot reach the host, the LAN or other VMs, and with `internet: false` not even the internet.

## Recipe

1. **A dedicated image.** Never the image of the user's personal computer. Give it its own name, for example `lab`. Build it from a minimal base plus the tools the work needs (see [images.md](images.md)). For an agent driving it, put the human's public key in `authorized_keys` so `exec` and `cp` need no prompt.

2. **The narrowest network that works.** In its `ohmlatl.yaml` (or later in the image's `manifest.yaml`):

   ```yaml
   format: 1
   name: lab

   build:
     containerfile: Containerfile
     tag: 2026-11-04

   user:
     name: dev
     uid: 1000
     gid: 1000

   vm:
     vcpus: 2
     memory: 4GiB
     network:
       internet: false      # true only while it must download dependencies
       forward:
         - port: 11434
           to: host:11434    # only if it needs the host's Ollama
   ```

   A common pattern: `internet: true` for the first boot to install dependencies, then `internet: false` and `stop` + `run` before running the untrusted code itself. Remember that with `internet: true` the guest reaches the internet without filter.

3. **Build and boot** (human): `ohmlatl build`, `ohmlatl run lab:2026-11-04`, passphrase, `Ctrl-]`. If login is by password, also `ohmlatl shell lab` once.

4. **Bring the work in** (agent):

   ```bash
   ohmlatl cp ./project lab:/home/dev/project
   ```

   For a repository, prefer cloning inside the guest when it has internet (`ohmlatl exec lab -- git clone https://... /home/dev/project`), so nothing of it touches the host first. Install its dependencies in the same internet-on boot, then switch to `internet: false` (step 2) before running it. Review what it will execute first, from inside: `ohmlatl exec lab -- cat /home/dev/project/package.json`, `install.sh`, `Makefile`, and tell the user about anything suspicious.

5. **Run it** (agent):

   ```bash
   ohmlatl exec lab -- sh -c 'cd ~/project && npm install && npm test' > run.log 2>&1
   ```

   For long jobs, start them detached inside the guest and poll:

   ```bash
   ohmlatl exec lab -- sh -c 'cd ~/project && nohup make < /dev/null > ~/build.log 2>&1 &'
   ohmlatl exec lab -- tail -n 50 /home/dev/build.log
   ```

6. **Take only the results out** (agent), and treat them as untrusted data on the host: do not execute what comes back.

   ```bash
   ohmlatl cp lab:/home/dev/project/report.json ./report.json
   ```

7. **Stop** (agent): `ohmlatl stop lab`.

8. **Reset.** The rootfs keeps everything the code did. To start from clean, the human builds again with a new tag (`ohmlatl build -t lab:2026-11-05`) and later removes the old one (`ohmlatl image rm lab:2026-11-04`). There are no snapshots.

## An AI agent inside the VM

To give another agent (or a coding assistant) a box of its own:

- Image `agent` with the agent's runtime and tools installed in the Containerfile.
- `internet: false` and forwards only to what it needs: the model server on the host (`to: host:11434`), an MCP server in another VM (`to: vm/mcp:8080`). Inside, it calls `http://ohmlatl-gateway:<port>`.
- Start the service VMs first (`ohmlatl run mcp:<tag>`), then the agent VM.
- Hand it work with `cp`, read results with `cp`. Never copy the host's credentials, SSH keys or tokens into it; whatever it needs is image content the user chooses.
- Its own `stop`, `poweroff` or `reboot` ends the VM; it cannot start itself again.

## Limits to tell the user

- The host is trusted: it sees the VM's RAM and the passphrase.
- No GPU and no display: terminal workloads only.
- Forwards to `host:<port>` expose that host service to the guest; the guest can do whatever that service allows.
- `ohmlatl` does not filter what the guest does on the internet when `internet: true`.
