#!/usr/bin/env bash
# Wire the host's SSH identity into the container so `git push` works from inside it — including
# from agents (Claude Code, Codex) that have no terminal to answer a prompt.
#
# devcontainer.json bind-mounts the host's ~/.ssh **read-only** at $HOST_SSH_DIR: the container
# can read the identity but can never rewrite, rotate or delete the host's keys. Runs from
# postStartCommand, so it re-arms on every container start (an ssh-agent does not survive one).
# Best-effort by design: a missing mount or a passphrase-locked key must not break the container.
set -uo pipefail

HOST_SSH="${HOST_SSH_DIR:-$HOME/.ssh-host}"
SSH_DIR="$HOME/.ssh"
AGENT_ENV="$SSH_DIR/agent.env"

mkdir -p "$SSH_DIR"
chmod 700 "$SSH_DIR"

echo "==> Setting up SSH for git push"

# 1. Trust github.com up front, so the first push never stalls on the interactive
#    "authenticity of host ... can't be established" prompt in a non-tty agent session.
if ! ssh-keygen -F github.com -f "$SSH_DIR/known_hosts" >/dev/null 2>&1; then
  ssh-keyscan -t rsa,ecdsa,ed25519 github.com 2>/dev/null >>"$SSH_DIR/known_hosts" || true
fi

# 2. Copy the mounted identities in. Copy rather than point IdentityFile at the mount: ssh rejects
#    a key whose file is group/other-readable or owned by another uid, and the bind mount carries
#    the host's ownership, which need not map to `vscode`. A private key counts only when its .pub
#    sits next to it — that is what tells a key apart from config, sockets and known_hosts.
keys=()
if [ -d "$HOST_SSH" ]; then
  shopt -s nullglob
  for pub in "$HOST_SSH"/*.pub; do
    key="${pub%.pub}"
    [ -f "$key" ] || continue
    install -m 600 "$key" "$SSH_DIR/" && install -m 644 "$pub" "$SSH_DIR/" || continue
    keys+=("$SSH_DIR/$(basename "$key")")
  done
  # The host's ssh config often carries the Host->key mapping the identity depends on.
  [ -f "$HOST_SSH/config" ] && install -m 600 "$HOST_SSH/config" "$SSH_DIR/"
  shopt -u nullglob
else
  echo "    no host key mount at $HOST_SSH — rebuild the container to pick up the ssh mount"
fi

# 3. Find an agent to hold them. `ssh-add -l` exits 0 with identities, 1 for a reachable but empty
#    agent, 2 when no agent is reachable at all.
ssh-add -l >/dev/null 2>&1
case $? in
  0) echo "    agent already holds an identity ($SSH_AUTH_SOCK)"; ;;
  2)
    # No agent (or a stale socket): reuse the one this script started earlier, else spawn one.
    [ -r "$AGENT_ENV" ] && . "$AGENT_ENV" >/dev/null 2>&1
    ssh-add -l >/dev/null 2>&1
    if [ $? -eq 2 ]; then
      (umask 077 && ssh-agent -s >"$AGENT_ENV")
      . "$AGENT_ENV" >/dev/null 2>&1
    fi
    ;;
esac

# 4. Load the keys. SSH_ASKPASS=false + </dev/null keeps a passphrase-protected key from hanging
#    the container start: it fails, we say so, and the developer runs `ssh-add` themselves.
for key in "${keys[@]:-}"; do
  [ -n "$key" ] || continue
  if DISPLAY= SSH_ASKPASS=/bin/false ssh-add "$key" </dev/null >/dev/null 2>&1; then
    echo "    loaded $(basename "$key")"
  else
    echo "    could not load $(basename "$key") (passphrase?) — run: ssh-add ~/.ssh/$(basename "$key")"
  fi
done

# 5. Export the agent to every future shell, so a terminal or an agent session started later in
#    this container inherits it. Guarded on SSH_AUTH_SOCK so VS Code's own forwarded agent, when it
#    carries an identity, keeps priority over ours.
if ! grep -q 'ssh/agent.env' "$HOME/.bashrc" 2>/dev/null; then
  cat >>"$HOME/.bashrc" <<'SNIP'

# devcontainer: reuse the container ssh-agent (see .devcontainer/ssh-setup.sh)
if [ -r "$HOME/.ssh/agent.env" ] && ! ssh-add -l >/dev/null 2>&1; then
  . "$HOME/.ssh/agent.env" >/dev/null 2>&1
fi
SNIP
fi

ssh-add -l >/dev/null 2>&1 \
  && echo "    ready: $(ssh-add -l | wc -l) identity(ies) available to git push" \
  || echo "    WARNING: no identity loaded; git push over SSH will fail"

exit 0
