# Security model and guardrails

gBuild runs unrestricted: there are **no approval prompts, ever**. Every tool
call the model requests executes immediately — file edits, shell commands,
deletes, network fetches, MCP tools. This page documents that posture and the
guardrails you can layer on top.

> Run gBuild only on systems and in workspaces where this is intentional. For
> untrusted repositories, generated code you do not intend to monitor, or
> unattended automation, run gBuild inside a container, VM, or remote sandbox
> (see [Sandbox](18-sandbox.md)).

---

## The posture

- **No permission modes.** Always-approve is the only mode. `Ctrl+O`,
  `Shift+Tab`, `/always-approve`, and `/auto` do not exist; the CLI flags
  `--always-approve` / `--yolo` / `--permission-mode` and the
  `[ui] permission_mode` config key are accepted for compatibility but have no
  effect.
- **No folder trust.** Repo-local configuration — hooks, MCP servers, plugins,
  permission rules, `.envrc`, LSP servers, workflows — always loads. Review an
  unfamiliar checkout's `.gbuild/`, `.claude/`, and `.envrc` before working in
  it.
- **Headless runs auto-approve.** `gbuild -p …` executes every tool call with
  no flags required.
- **Hooks still run.** `PreToolUse` hooks execute before every tool call and
  can deny it (see [Hooks](10-hooks.md)). Hooks fail open: a crashing or
  missing hook does not block the call.
- **`deny` rules still bind.** A matching `[permission]` deny rule rejects the
  tool call even in unrestricted operation. This is the one rule-based
  guardrail (below).
- **The sandbox is opt-in.** `--sandbox <profile>` applies OS-level isolation
  (Landlock on Linux, Seatbelt on macOS) for runs that need it. Off by
  default.

## What still checks a tool call

When the model requests a tool, exactly two checks happen, in order:

1. **`PreToolUse` hooks.** A hook deny stops the call. A hook allow (or no
   hook) falls through.
2. **`deny` rules.** The merged rule set from every configuration source is
   evaluated; a matching `deny` rejects the call, including deny rules matched
   against individual segments of a chained shell command and paths touched by
   inline shell scripts. `allow` and `ask` rules are ignored (everything is
   already allowed; nothing ever prompts).

Everything else runs.

---

## Deny rules: the guardrail

Deny rules are for paths and commands you never want the agent to run, from
any source, merged with the same syntax as before:

```toml
# ~/.gbuild/config.toml or <project>/.gbuild/config.toml
[permission]
deny = [
  "Read(**/.env)",
  "Edit(**/.env)",
  "Bash(rm -rf *)",
  "Bash(git push*)",
  "MCPTool(sales__delete_*)",
]
```

```bash
gbuild -p "Deploy the service" --deny 'Bash(rm -rf *)'
```

Deny rules are checked per segment of chained commands (`&&`, `||`, `;`,
pipes), with environment prefixes and common wrappers (`timeout`, `nice`,
`env`, …) peeled, and against file paths that shell commands touch. One denied
segment rejects the entire command.

`.claude/settings.json` `permissions.deny` entries are also read (see below).

### What deny rules do not cover

- **`allow` / `ask` rules are inert.** Do not rely on `dontAsk`,
  `acceptEdits`, or `defaultMode` from Claude-compatible settings to restrict
  anything; only `deny` entries have an effect.
- **They are a model-level check, not OS enforcement.** A deny rule stops the
  *agent's* tool call; a process the agent already spawned, or code it wrote
  and you later run, is outside its reach. For OS-level enforcement use the
  sandbox or a container.
- **Hooks fail open.** A hook used as a boundary must handle its own errors.

### Rule matching reference

`Bash(...)` patterns match by prefix (character-for-character; `Bash(git *)`
requires the whole word) or by glob against the whole command. A trailing `:*`
suffix is stripped to a plain prefix. Path patterns for `Read`/`Edit`/`Grep`
are globs matched against the path as given (`*` and `?` do not cross `/`;
`**` does). `MCPTool(server__tool)` matches MCP tools with glob support.
`WebFetch(domain:example.com)` matches a host and its subdomains.

Tool names: `Bash`, `Read` (and `NotebookRead`), `Edit` (and `Write`,
`NotebookEdit`), `Grep` (and `Glob`), `MCPTool`, `WebFetch`, `WebSearch`. A
bare `*` matches every tool.

### Claude Code compatibility

gBuild reads `~/.claude/settings.json`, `~/.claude/settings.local.json`, and
project-level `<project>/.claude/settings.json` walking up to the repo root.
Only the `deny` entries affect behavior:

```json
{
  "permissions": {
    "deny": ["Bash(rm -rf *)", "Read(**/.env)"]
  }
}
```

---

## Hooks as a boundary

A `PreToolUse` hook can enforce an allowlist on any tool. Hooks are evaluated
before deny rules. Example: allow only `git` and `gh` shell commands.

**`~/.gbuild/hooks/git-gh-only.json`**

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "git-gh-only.sh", "timeout": 5 }]
      }
    ]
  }
}
```

**`~/.gbuild/hooks/git-gh-only.sh`**

```bash
#!/bin/sh
set -eu

deny() {
  echo '{"decision": "deny", "reason": "'"$1"'"}'
  exit 2
}

INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.toolInput.command // empty')
[ -n "$CMD" ] || deny "Empty command is not allowed"

CMD=$(echo "$CMD" | sed 's/&&/;/g; s/||/;/g')
case "$CMD" in
  *'$('*|*'`'*|*'&'*|*'>'*|*'<'*) deny "Substitution, background, and redirection are not permitted" ;;
esac

echo "$CMD" | tr ';|' '\n\n' | while IFS= read -r SEGMENT; do
  SEGMENT=$(echo "$SEGMENT" | sed 's/^[[:space:]]*//')
  [ -n "$SEGMENT" ] || continue
  case "$SEGMENT" in
    git\ *|git|gh\ *|gh) ;;
    *) deny "Only git and gh commands are permitted. Blocked segment: $SEGMENT" ;;
  esac
done
```

See [10-hooks.md](10-hooks.md) for the hook format and lifecycle events.

---

## Sandboxing and containers

For anything you would not run yourself in a shell, put an OS boundary around
the whole agent rather than relying on in-process rules:

- `--sandbox strict` (or a custom profile) for Landlock/Seatbelt isolation of
  the gBuild process ([18-sandbox.md](18-sandbox.md)).
- A container, VM, or remote sandbox with only the files and credentials the
  task needs, for untrusted repositories or unattended automation. Mount the
  workspace read-only where possible, pass the minimum API keys, and restrict
  network access when the task does not need it.

## Best practices

1. **Use deny rules for the few things that must never run** — credential
   files, force pushes, destructive commands.
2. **Review project configuration from unfamiliar sources before starting.**
   `.gbuild/config.toml`, `.claude/settings.json`, project hooks, plugins, and
   `.envrc` all take effect immediately.
3. **Containerize unattended runs.** Headless mode auto-approves everything;
   give automation its own credentials and filesystem view.
4. **Treat hooks as conveniences that fail open**, not as your only boundary.

## See also

- [Hooks](10-hooks.md) — PreToolUse and other lifecycle scripts
- [Headless mode](14-headless-mode.md) — One-shot CLI and automation flags
- [Agent mode](15-agent-mode.md) — ACP, stdio, and agent servers
- [Sandbox](18-sandbox.md) — OS-level isolation profiles
- [Configuration](05-configuration.md) — Native `config.toml` structure
