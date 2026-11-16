//! Canonical completion values for the shared Clap command.
use clap::{CommandFactory, builder::PossibleValuesParser};
use kubectl_view_storage::cli::Cli;

pub fn command() -> clap::Command {
    // FromStr parsers accept aliases; completion offers their canonical spellings.
    Cli::command()
        .mut_arg("group_by", |argument| {
            argument.value_parser(PossibleValuesParser::new([
                "storageclass",
                "namespace",
                "pvc",
                "pv",
                "driver",
                "pod",
                "node",
                "accessmode",
                "volumemode",
                "phase",
                "statefulset",
                "workload",
            ]))
        })
        .mut_arg("units", |argument| {
            argument.value_parser(PossibleValuesParser::new([
                "auto", "B", "KB", "MB", "GB", "TB", "PB", "KiB", "MiB", "GiB", "TiB", "PiB",
            ]))
        })
        .mut_arg("sort", |argument| {
            argument.value_parser(PossibleValuesParser::new([
                "name",
                "requested",
                "provisioned",
                "used",
                "available",
                "used-percent",
                "inode-percent",
                "severity",
                "name:asc",
                "name:desc",
                "requested:asc",
                "requested:desc",
                "provisioned:asc",
                "provisioned:desc",
                "used:asc",
                "used:desc",
                "available:asc",
                "available:desc",
                "used-percent:asc",
                "used-percent:desc",
                "inode-percent:asc",
                "inode-percent:desc",
                "severity:asc",
                "severity:desc",
            ]))
        })
}
