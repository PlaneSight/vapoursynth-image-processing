//! Repository conformance and authored-artifact maintenance commands.

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
        "pyproject.toml",
        "uv.lock",
        ".python-version",
        "zensical.toml",
        "src/vsip_tools/__init__.py",
        "src/vsip_tools/cli.py",
        "src/vsip_tools/py.typed",
        "docs/architecture.md",
        "docs/artifacts.md",
        "docs/cli.md",
        "docs/development.md",
        "docs/filter-catalog.md",
        "docs/getting-started.md",
        "docs/index.md",
        "docs/vapoursynth.md",
        "tests/python/test_cli.py",
        "tests/python/test_metadata.py",
        "tests/vapoursynth/catalog_smoke.vpy",
        "tests/vapoursynth/masklab_smoke.vpy",
        "crates/vsip-core/Cargo.toml",
        "crates/vsip-kernels/Cargo.toml",
        "crates/vsip-plugin-api/Cargo.toml",
        "crates/vsip-vapoursynth/Cargo.toml",
    ];
    let mut problems = Vec::new();
    for path in required {
        if !Path::new(path).is_file() {
            problems.push(format!("missing: {path}"));
        }
    }
    for plugin in PLUGINS {
        check_plugin(plugin, &mut problems);
    }
    if problems.is_empty() {
        println!("repository tree, Python tooling, docs, and adapter catalogues satisfied");
        ExitCode::SUCCESS
    } else {
        problems.iter().for_each(|problem| eprintln!("{problem}"));
        ExitCode::FAILURE
    }
}

fn check_plugin(plugin: &str, problems: &mut Vec<String>) {
    let manifest = format!("plugins/{plugin}/Cargo.toml");
    let source_path = format!("plugins/{plugin}/src/lib.rs");
    let adapter_manifest = format!("adapters/{plugin}-vapoursynth/Cargo.toml");
    let adapter_path = format!("adapters/{plugin}-vapoursynth/src/lib.rs");

    for path in [&manifest, &source_path, &adapter_manifest, &adapter_path] {
        if !Path::new(path).is_file() {
            problems.push(format!("missing: {path}"));
        }
    }

    let Ok(source) = fs::read_to_string(&source_path) else {
        return;
    };
    if source.contains("#![allow(missing_docs)]") {
        problems.push(format!(
            "plugin suppresses public API documentation lint: {source_path}"
        ));
    }

    let Ok(adapter) = fs::read_to_string(&adapter_path) else {
        return;
    };
    for forbidden in ["unsafe {", "unsafe fn", "unsafe extern"] {
        if adapter.contains(forbidden) {
            problems.push(format!(
                "adapter {plugin} contains handwritten unsafe syntax: {forbidden}"
            ));
        }
    }
    for filter in catalogue_names(&source) {
        if !adapter.contains(&format!("\"{filter}\"")) {
            problems.push(format!(
                "adapter {plugin} does not register catalogue filter {filter}"
            ));
        }
    }
}

fn catalogue_names(source: &str) -> impl Iterator<Item = &str> {
    source.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("name: \"")
            .and_then(|remainder| remainder.split_once('"'))
            .map(|(name, _)| name)
    })
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
