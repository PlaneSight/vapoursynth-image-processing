use std::{env, fs, path::Path, process::ExitCode};

const PLUGINS: &[&str] = &[
    "masklab",
    "defect",
    "deconvolve",
    "flowfield",
    "grainlab",
    "phase",
    "edgeaware",
    "register",
    "lens",
    "tonelab",
    "residual",
    "segment",
];

fn main() -> ExitCode {
    let Some(command) = env::args().nth(1) else {
        eprintln!("usage: cargo xtask <check-tree|inventory|clean-dist|clean-site|clean-tmp>");
        return ExitCode::FAILURE;
    };
    match command.as_str() {
        "check-tree" => check_tree(),
        "inventory" => {
            PLUGINS.iter().for_each(|plugin| println!("{plugin}"));
            ExitCode::SUCCESS
        }
        "clean-dist" => clean("dist"),
        "clean-site" => clean("site"),
        "clean-tmp" => clean(".tmp"),
        _ => {
            eprintln!("unknown xtask command: {command}");
            ExitCode::FAILURE
        }
    }
}

fn check_tree() -> ExitCode {
    let required = [
        "Cargo.toml",
        "docs/architecture.md",
        "docs/artifacts.md",
        "crates/vsip-core/Cargo.toml",
        "crates/vsip-kernels/Cargo.toml",
        "crates/vsip-plugin-api/Cargo.toml",
    ];
    let mut missing = Vec::new();
    for path in required {
        if !Path::new(path).is_file() {
            missing.push(path.to_owned());
        }
    }
    for plugin in PLUGINS {
        let path = format!("plugins/{plugin}/Cargo.toml");
        if !Path::new(&path).is_file() {
            missing.push(path);
        }
    }
    if missing.is_empty() {
        println!("repository tree contract satisfied");
        ExitCode::SUCCESS
    } else {
        missing.iter().for_each(|path| eprintln!("missing: {path}"));
        ExitCode::FAILURE
    }
}

fn clean(path: &str) -> ExitCode {
    let target = Path::new(path);
    if !target.exists() {
        return ExitCode::SUCCESS;
    }
    match fs::remove_dir_all(target) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("failed to remove {path}: {error}");
            ExitCode::FAILURE
        }
    }
}

