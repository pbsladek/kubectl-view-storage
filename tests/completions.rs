//! Completion assets must match the current Clap command, including enum values.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../dev/completion_generator.rs"]
mod completion_generator;

#[test]
fn checked_in_shell_completions_match_the_current_cli() {
    use clap_complete::{Shell, generate};
    for (shell, contents) in [
        (
            Shell::Bash,
            include_bytes!("../completions/kubectl-view_storage.bash").as_slice(),
        ),
        (
            Shell::Zsh,
            include_bytes!("../completions/_kubectl-view_storage").as_slice(),
        ),
        (
            Shell::Fish,
            include_bytes!("../completions/kubectl-view_storage.fish").as_slice(),
        ),
    ] {
        let mut generated = vec![];
        generate(
            shell,
            &mut completion_generator::command(),
            "kubectl-view_storage",
            &mut generated,
        );
        assert_eq!(contents, generated, "{shell}: run make completions");
    }
}
