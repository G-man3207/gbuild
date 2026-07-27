//! `/login` -- sign in or add a provider API key.
//!
//! Bare `/login` lists providers and what's configured. `/login grok` starts
//! the xAI browser sign-in. `/login <provider> <key>` stores an API key in
//! `~/.gbuild/auth.json`; `gbuild login --provider <id>` prompts for the key
//! with hidden input for better secrecy.

use crate::app::actions::Action;
use crate::slash::command::{ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct LoginCommand;

fn provider_menu() -> String {
    let mut out = String::from("Sign in with a provider:\n\n");
    out.push_str("  /login grok                 xAI browser sign-in (OAuth)\n");
    out.push_str("  /login codex                ChatGPT Codex subscription (browser OAuth)\n");
    out.push_str("  /login copilot              GitHub Copilot subscription (device code)\n");
    out.push_str("  /login openrouter           OpenRouter browser sign-in (OAuth)\n");
    for spec in gbuild_shell::auth::provider_keys::PROVIDER_KEY_SPECS {
        if matches!(spec.id, "openrouter" | "codex" | "copilot") {
            continue;
        }
        let configured = spec
            .env_vars
            .iter()
            .any(|v| gbuild_shell::auth::provider_keys::resolve_env_or_stored(v).is_some());
        let status = if configured { "configured" } else { "—" };
        out.push_str(&format!(
            "  /login {:<12}        {} [{}]\n",
            spec.id, spec.display, status
        ));
    }
    out.push_str(
        "\nStore a key with `/login <provider> <key>`, or run \
         `gbuild login --provider <id>` for a hidden-input prompt. \
         Keys are kept in ~/.gbuild/auth.json.",
    );
    out
}

impl SlashCommand for LoginCommand {
    fn name(&self) -> &str {
        "login"
    }

    fn description(&self) -> &str {
        "Sign in with a provider (xAI OAuth or API keys)"
    }

    fn usage(&self) -> &str {
        "/login [provider] [api-key]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(
        &self,
        _ctx: &crate::slash::command::AppCtx,
        _args_query: &str,
    ) -> Option<Vec<ArgItem>> {
        let mut items = vec![ArgItem {
            display: "grok".to_string(),
            match_text: "grok xai oauth browser".to_string(),
            insert_text: "grok".to_string(),
            description: "xAI browser sign-in (OAuth)".to_string(),
        }];
        for spec in gbuild_shell::auth::provider_keys::PROVIDER_KEY_SPECS {
            let configured = spec
                .env_vars
                .iter()
                .any(|v| gbuild_shell::auth::provider_keys::resolve_env_or_stored(v).is_some());
            items.push(ArgItem {
                display: spec.id.to_string(),
                match_text: format!("{} {}", spec.id, spec.display),
                insert_text: spec.id.to_string(),
                description: if configured {
                    format!("{} — key configured", spec.display)
                } else {
                    format!("{} — API key", spec.display)
                },
            });
        }
        Some(items)
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let parts: Vec<&str> = args.split_whitespace().collect();
        match parts.as_slice() {
            [] => CommandResult::Message(provider_menu()),
            ["grok"] | ["oauth"] => CommandResult::Action(Action::Login),
            [provider] => {
                let Some(spec) = gbuild_shell::auth::provider_keys::spec_by_id(provider) else {
                    return CommandResult::Error(format!(
                        "Unknown provider '{provider}'.\n\n{}",
                        provider_menu()
                    ));
                };
                if spec.id == "xai" {
                    // xAI without an inline key means the OAuth flow; a key can
                    // be given inline as /login xai <key>.
                    return CommandResult::Action(Action::Login);
                }
                if spec.id == "openrouter" {
                    return CommandResult::Message(
                        "Run `gbuild login --provider openrouter` in a shell for the browser \
                         sign-in (it stores the key for you), or paste a key here with \
                         `/login openrouter <key>`."
                            .to_string(),
                    );
                }
                if spec.id == "codex" {
                    return CommandResult::Message(
                        "Run `gbuild login --provider codex` in a shell for the ChatGPT \
                         subscription sign-in (browser OAuth). Then select gpt-5.3-codex or \
                         gpt-5.2-codex with /model."
                            .to_string(),
                    );
                }
                if spec.id == "copilot" {
                    return CommandResult::Message(
                        "Run `gbuild login --provider copilot` in a shell for the GitHub \
                         Copilot sign-in (device code). Then select a copilot model with /model."
                            .to_string(),
                    );
                }
                CommandResult::Message(format!(
                    "Store a {} API key with `/login {} <key>`, or run \
                     `gbuild login --provider {}` for a hidden-input prompt.",
                    spec.display, spec.id, spec.id
                ))
            }
            [provider, key] => {
                let Some(spec) = gbuild_shell::auth::provider_keys::spec_by_id(provider) else {
                    return CommandResult::Error(format!(
                        "Unknown provider '{provider}'.\n\n{}",
                        provider_menu()
                    ));
                };
                let home = gbuild_shell::util::gbuild_home::gbuild_home();
                match gbuild_shell::auth::provider_keys::store_provider_key(&home, spec.id, key) {
                    Ok(()) => CommandResult::Message(format!(
                        "Stored {} API key in ~/.gbuild/auth.json. It applies to new sessions; \
                         switch models with /model to use it now.",
                        spec.display
                    )),
                    Err(e) => CommandResult::Error(format!("Failed to store API key: {e}")),
                }
            }
            _ => CommandResult::Error(format!("Usage: {}\n\n{}", self.usage(), provider_menu())),
        }
    }
}
