# Providers

gBuild is provider-neutral. It ships a built-in catalog of providers and
models, picks a working default automatically from the credentials it finds,
and lets you add any OpenAI-compatible, Responses, or Anthropic Messages
endpoint on top.

---

## Built-in providers

| Provider | Models | Credential |
| -------- | ------ | ---------- |
| xAI | Grok 4.5 | `XAI_API_KEY`, or browser sign-in (`/login grok`) |
| Anthropic | Claude Sonnet 4.6, Claude Opus 4.6 | `ANTHROPIC_API_KEY` |
| OpenAI | GPT-5.5 | `OPENAI_API_KEY` |
| ChatGPT Codex | GPT-5.3 Codex, GPT-5.2 Codex | browser sign-in (`gbuild login --provider codex`) |
| GitHub Copilot | GPT-5.3 Codex, Claude Sonnet 4.6 | device-code sign-in (`gbuild login --provider copilot`) |
| Google | Gemini 3.1 Pro | `GEMINI_API_KEY` or `GOOGLE_API_KEY` |
| OpenRouter | OpenRouter Auto (add specific slugs via config) | `OPENROUTER_API_KEY` or browser sign-in (`gbuild login --provider openrouter`) |
| OpenCode Go | GLM 5.1 | `OPENCODE_API_KEY` |
| Kimi (Moonshot) | Kimi K2.7 Code | `KIMI_API_KEY` |
| GLM (Z.AI) | GLM 5.1 (coding plan) | `ZHIPU_API_KEY` or `ZAI_API_KEY` |

Set the environment variable for the provider you want and start gBuild —
no login flow, no config file required:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
gbuild
```

**Default model selection.** gBuild sorts the built-in catalog so providers
with a resolvable credential come first and uses the first entry as the
default model. Override anytime with `-m <model>`, `GBUILD_DEFAULT_MODEL`,
`[models] default` in `~/.gbuild/config.toml`, or `/model` in the TUI.

## Storing keys with /login

Instead of environment variables, you can store provider keys in
`~/.gbuild/auth.json` (owner-only `0600` permissions):

```bash
gbuild login --provider anthropic            # hidden-input prompt
gbuild login --provider openai --api-key sk-...
gbuild login --provider openrouter           # browser sign-in (OAuth, stores the key)
```

Or in the TUI: `/login` shows the provider menu; `/login <provider> <key>`
stores a key; `/logout <provider>` clears one (`gbuild logout --provider <id>`
on the CLI). Stored keys behave exactly like the corresponding environment
variables and take precedence over them.

xAI is one provider with a browser sign-in (`/login grok`,
`gbuild login`, or `gbuild login --device-auth` for headless machines).
Two more browser sign-ins exist for subscription accounts:

- **OpenRouter** (`gbuild login --provider openrouter`) mints an API key
  billed from your OpenRouter credits.
- **ChatGPT Codex** (`gbuild login --provider codex`) signs in with your
  ChatGPT subscription via OpenAI OAuth and unlocks the `gpt-5.3-codex` /
  `gpt-5.2-codex` models. The sign-in opens a browser (loopback port 1455,
  with a paste-the-URL fallback for remote machines); the access token,
  refresh token, and account id are stored in `~/.gbuild/auth.json` and
  refreshed automatically at the start of each turn.
- **GitHub Copilot** (`gbuild login --provider copilot`) signs in with a
  GitHub device code (works headless — no loopback needed) and unlocks the
  Copilot-routed models. The long-lived GitHub token stays on disk; the
  short-lived Copilot API token is re-exchanged automatically when stale.

Stored keys and environment variables satisfy the startup gate, so the login
screen only appears when nothing at all is configured.

## Custom providers

Any endpoint that speaks OpenAI Chat Completions, OpenAI Responses, or
Anthropic Messages can be added in `~/.gbuild/config.toml`:

```toml
[model.my-model]
model = "model-id"
base_url = "https://my-provider.example/v1"
env_key = "MY_PROVIDER_API_KEY"
context_window = 200000
# api_backend = "chat_completions"  # default; or "responses", "messages"
```

Shared blocks for several models on one provider live under
`[model_providers.<id>]`. See [Custom models](11-custom-models.md).

## Feature availability by provider

Some features depend on a specific API family:

- **Web search** uses the Responses API's built-in search tool and works on
  xAI and OpenAI. It follows your session model when `[models] web_search`
  is unset and stays unavailable on providers without a Responses endpoint.
- **Voice, image generation, video generation** are xAI-only services and
  are disabled.
- **Session summaries, image descriptions, prompt suggestions** use your
  session model unless pinned via `[models]`.

## Credential isolation

gBuild never sends ambient xAI credentials (session token, `XAI_API_KEY`)
to non-xAI origins, and never sends `x-grok-*` identity headers to
third-party endpoints. A custom provider only receives the credentials you
configured for it (`api_key`, `env_key`, or an `auth_provider` helper).

## See also

- [Authentication](02-authentication.md) — xAI OAuth, device code, OIDC SSO, external auth providers
- [Custom models](11-custom-models.md) — `[model.*]`, `[model_providers.*]`, backends, headers
- [Configuration](05-configuration.md) — `[models]` defaults and model selection
