# Getting Started

gBuild is a provider-neutral terminal agent harness based on the upstream xAI codebase. It runs as a TUI (Terminal User Interface) that understands your codebase, executes shell commands, edits files, searches the web, and manages tasks.

You can use it interactively as a full-screen TUI, run it headlessly for scripting and CI/CD, or integrate it into editors via the Agent Client Protocol (ACP).

---

## Installation

gBuild currently ships from source only. From the repository root:

```bash
cargo build -p gbuild-pager-bin --bin gbuild --release
```

Install the resulting executable somewhere on your `PATH`:

```bash
install -m 755 target/release/gbuild ~/.local/bin/gbuild
```

On Windows PowerShell, build the same target and copy `gbuild.exe` to a
directory on your user `PATH`:

```powershell
cargo build -p gbuild-pager-bin --bin gbuild --release
Copy-Item target\release\gbuild.exe $HOME\bin\gbuild.exe
```

Verify the installation:

```bash
gbuild --version
```

---

## First Launch

Start gBuild by running:

```bash
gbuild
```

On first launch, gBuild lands on the welcome screen — no login flow is forced. Type `/login` and choose a provider to sign in: xAI browser sign-in, a ChatGPT Codex or GitHub Copilot subscription, an OpenRouter account, or an API key for Anthropic, OpenAI, Google, OpenCode, Kimi, or GLM. See [Providers](25-providers.md) for the full list and headless options.

The quickest start is an environment variable:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."   # or OPENAI_API_KEY, XAI_API_KEY, ...
gbuild
```

gBuild stores credentials in `~/.gbuild/auth.json`, refreshes session tokens automatically, and only prompts you to sign in again when a session token can no longer be renewed.

See [Authentication](02-authentication.md) for the full set of auth options including OIDC, external auth providers, and device code flow.

---

## Basic Interaction

Once authenticated, gBuild presents a full-screen TUI with two main areas:

- **Scrollback** -- the conversation history showing your prompts, gBuild's responses, tool calls, file edits, and more.
- **Prompt** -- the input area at the bottom where you type messages.

Type a message and press `Enter` to send it. gBuild reads files, runs commands, and edits code as needed. Each tool run streams into the scrollback in real time.

Press `Tab` to move focus between the prompt and the scrollback. While a turn is running, `Esc` cancels it (the exception is fullscreen vim scrollback mode, where mid-turn `Esc` is a no-op; minimal mode cancels even with vim on); `Ctrl+C` cancels once the composer is empty — with a draft, the first press only clears it. Idle, press `Esc` twice within 800ms to clear a non-empty prompt, or (with an empty prompt and conversation messages) to open rewind — see [Keyboard Shortcuts](03-keyboard-shortcuts.md#escape). With the scrollback focused, use the arrow keys to select entries and to collapse or expand them. To navigate with `j`/`k` and fold with `h`/`l` instead, enable Vim mode.

### File References

Use `@` in your prompt to attach files:

```
@src/main.rs              # Attach a file
@src/main.rs:10-50        # Attach lines 10-50
@src/                     # Browse a directory
```

The `@` operator opens a fuzzy file picker. By default it respects `.gitignore` and hides dotfiles. Prefix with `!` to search hidden files:

```
@!.github                 # Search hidden files
@!.env                    # Attach a .env file
```

### Permissions

By default, gBuild asks for permission before executing shell commands or editing files. You can approve individually or toggle always-approve mode:

- Press `Ctrl+O` to toggle always-approve mode
- Use the `--yolo` flag at launch: `gbuild --yolo`
- Type `/always-approve` in the prompt to toggle the mode

---

## Key Concepts

### Sessions

Every conversation is a **session**. Sessions are automatically saved to `~/.gbuild/sessions/` and can be resumed later. Each session tracks the full conversation history, tool calls, file edits, and task state.

- Start a new session: `Ctrl+N` or `/new`
- Resume a previous session: `/resume` in the TUI, or `--resume <ID>` from the CLI
- Continue the most recent session: `gbuild -c`

### Scrollback

The scrollback is the main display area. It shows:

- **User prompts** -- your messages, rendered as sticky headers
- **Agent messages** -- gBuild's responses with full markdown rendering and syntax highlighting
- **Thinking blocks** -- gBuild's reasoning process (collapsible)
- **Tool calls** -- file edits (with inline diffs), command executions, search results, and more
- **Task lists** -- TODO items tracking progress

Collapse or expand the selected entry with the `Left`/`Right` arrow keys (or `h`/`l` and `e` in Vim mode). In Vim mode, press `y` to copy its content and `Y` to copy its metadata (for example, the command that ran). Press `Enter` to open it in the fullscreen viewer (in any mode).

### Tools

gBuild has built-in tools for:

| Tool | Description |
|------|-------------|
| `read_file` / `search_replace` | Read and edit files with line-precise changes |
| `grep` | Regex search across your codebase (powered by ripgrep) |
| `list_dir` | List directory contents |
| `run_terminal_command` | Execute shell commands |
| `web_search` / `web_fetch` | Search the web and fetch URLs |
| `todo_write` | Create and manage task lists |
| `spawn_subagent` | Spawn parallel subagent sessions |
| `memory_search` | Search cross-session memory |

Tools can be extended with [MCP servers](05-configuration.md#mcp-servers) for integrations like GitHub, databases, and more.

### Slash Commands

Type `/` in the prompt to access commands. These provide quick actions without writing a full prompt:

```
/model grok-build                 # Switch model
/compact                          # Compress conversation history
/always-approve                   # Toggle always-approve mode
/new                              # Start a new session
```

See [Slash Commands](04-slash-commands.md) for the complete reference.

---

## Common Launch Options

```bash
# Launch the interactive TUI and submit an initial prompt as the first turn
gbuild "fix the failing auth test and run it"

# Initial prompt in a new git worktree. Use --worktree=<name> (with `=`) so the
# prompt isn't swallowed as the worktree name — `gbuild -w "refactor module X"`
# would treat "refactor module X" as the worktree label, not the prompt.
gbuild --worktree=feat "refactor module X"

# Base the worktree on a specific branch (e.g. main) instead of the current HEAD:
gbuild -w --ref main "implement feature from main"


# Start in a specific project directory
gbuild --cwd ~/projects/my-app

# Add project-specific rules
gbuild --rules "Always use TypeScript. Prefer functional components."

# Auto-approve all tool executions
gbuild --yolo

# Use a specific model
gbuild -m grok-build

# Resume a previous session
gbuild --resume <session-id>

# Continue the most recent session
gbuild -c

# Experimental scrollback-native render mode. Sticky: plain `gbuild` reopens in
# the mode last chosen via --minimal/--fullscreen (or /minimal//fullscreen).
gbuild --minimal

# Back to the standard fullscreen TUI (and make it sticky again)
gbuild --fullscreen

# Headless mode (for scripts)
gbuild -p "Explain this codebase"
```

---

## Headless Mode

Run gBuild non-interactively for scripting, CI/CD, and automation:

```bash
gbuild -p "Your prompt here"
```

Output formats:

| Format | Flag | Description |
|--------|------|-------------|
| `plain` | (default) | Human-readable text |
| `json` | `--output-format json` | Single JSON object with `text`, `stopReason`, `sessionId`, and `requestId` |
| `streaming-json` | `--output-format streaming-json` | NDJSON event stream for real-time processing |

Example CI/CD usage:

```bash
gbuild -p "Review changes for bugs" --output-format json --yolo | jq -r '.text'
```

---

## Project Rules (AGENTS.md)

Add per-project instructions by creating an `AGENTS.md` file in your repository. gBuild reads these files and injects their contents as a project-instructions message at the start of the conversation:

```
~/.gbuild/AGENTS.md           # Global rules (apply to all projects)
<repo-root>/AGENTS.md       # Repository-level rules
<cwd>/AGENTS.md             # Directory-level rules (highest priority)
```

Deeper files take precedence. gBuild also reads `CLAUDE.md` files for compatibility.

---

## Where to Go Next

| Document | What You Will Learn |
|----------|-------------------|
| [Authentication](02-authentication.md) | Browser login, API keys, OIDC, external auth, device code flow |
| [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Complete reference for all key bindings |
| [Slash Commands](04-slash-commands.md) | All available `/` commands |
| [Configuration](05-configuration.md) | config.toml, pager.toml, environment variables |
