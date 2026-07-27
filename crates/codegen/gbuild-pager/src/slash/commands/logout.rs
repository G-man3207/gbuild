//! `/logout` -- remove auth credentials and return to the login screen,
//! or clear one provider's stored API key with `/logout <provider>`.

use crate::app::actions::Action;
use crate::slash::command::{ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct LogoutCommand;

impl SlashCommand for LogoutCommand {
    fn name(&self) -> &str {
        "logout"
    }

    fn description(&self) -> &str {
        "Log out and return to the login screen"
    }

    fn usage(&self) -> &str {
        "/logout [provider]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(
        &self,
        _ctx: &crate::slash::command::AppCtx,
        _args_query: &str,
    ) -> Option<Vec<ArgItem>> {
        Some(
            gbuild_shell::auth::provider_keys::PROVIDER_KEY_SPECS
                .iter()
                .map(|spec| ArgItem {
                    display: spec.id.to_string(),
                    match_text: format!("{} {}", spec.id, spec.display),
                    insert_text: spec.id.to_string(),
                    description: format!("Clear stored {} API key", spec.display),
                })
                .collect(),
        )
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let provider = args.trim();
        if provider.is_empty() {
            return CommandResult::Action(Action::Logout);
        }
        let Some(spec) = gbuild_shell::auth::provider_keys::spec_by_id(provider) else {
            return CommandResult::Error(format!(
                "Unknown provider '{provider}'. Expected one of: {}",
                gbuild_shell::auth::provider_keys::PROVIDER_KEY_SPECS
                    .iter()
                    .map(|s| s.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        let home = gbuild_shell::util::gbuild_home::gbuild_home();
        match gbuild_shell::auth::provider_keys::clear_provider_key(&home, spec.id) {
            Ok(()) => CommandResult::Message(format!(
                "Cleared {} API key. It applies to new sessions.",
                spec.display
            )),
            Err(e) => CommandResult::Error(format!("Failed to clear API key: {e}")),
        }
    }
}
