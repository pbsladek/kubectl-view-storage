//! Generated with the dev-only Clap completion tool; no runtime dependency.
use kubectl_view_storage::cli::CompletionShell;

pub fn script(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => include_str!("../completions/kubectl-view_storage.bash"),
        CompletionShell::Zsh => include_str!("../completions/_kubectl-view_storage"),
        CompletionShell::Fish => include_str!("../completions/kubectl-view_storage.fish"),
    }
}
