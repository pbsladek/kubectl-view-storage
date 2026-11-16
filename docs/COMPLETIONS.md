# Shell completions

`kubectl-view_storage --completions bash|zsh|fish` writes completion source to
stdout without reading kubeconfig or contacting a cluster. These completions
apply to the standalone executable `kubectl-view_storage` on PATH. Installing
them does not register a kubectl plugin completion helper.

For Bash, source the generated file in your shell startup configuration:

```sh
kubectl-view_storage --completions bash > ~/.kubectl-view_storage.bash
source ~/.kubectl-view_storage.bash
```

For Zsh, put the generated function in a directory on `fpath` before `compinit`:

```sh
mkdir -p ~/.zsh/completions
kubectl-view_storage --completions zsh > ~/.zsh/completions/_kubectl-view_storage
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit && compinit
```

For Fish:

```fish
mkdir -p ~/.config/fish/completions
kubectl-view_storage --completions fish > ~/.config/fish/completions/kubectl-view_storage.fish
```

The generated choices include flags, views, providers, grouping dimensions, sort
fields and units. Completion stays offline: it does not list namespaces, claims,
contexts or other cluster objects.

Maintainers run `make completions` after changing CLI definitions. The pinned,
dev-only Clap generator writes the three checked-in assets. Cargo tests compare
them with fresh output, and `cargo run --locked --example completions -- --check`
checks freshness directly. Generation adds no runtime dependency.
