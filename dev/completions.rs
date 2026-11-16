//! Generate and check completion assets from the application's Clap definitions.
use clap_complete::{Shell, generate};
use std::{error::Error, fs, path::PathBuf};

mod completion_generator;

fn main() -> Result<(), Box<dyn Error>> {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("completions");
    for (shell, name) in [
        (Shell::Bash, "kubectl-view_storage.bash"),
        (Shell::Zsh, "_kubectl-view_storage"),
        (Shell::Fish, "kubectl-view_storage.fish"),
    ] {
        let mut command = completion_generator::command();
        let mut generated = vec![];
        generate(shell, &mut command, "kubectl-view_storage", &mut generated);
        let path = directory.join(name);
        if check {
            if fs::read(&path)? != generated {
                return Err(format!("{} is stale; run make completions", path.display()).into());
            }
        } else {
            fs::write(path, generated)?;
        }
    }
    Ok(())
}
