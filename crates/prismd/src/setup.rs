//! First-run configuration.
//!
//! A new install must work without the operator writing TOML by hand. Before
//! this existed, a fresh daemon started with no file roots and no facets, so
//! Files was empty and Facets was empty, with only a log line to explain why —
//! which nobody reads.
//!
//! Everything here is a *detection with a default*, never a requirement. The
//! host is inspected for the things Prism can usefully manage, a config is
//! written, and the operator can edit it afterwards because it is their own
//! plain TOML rather than a hidden database.
//!
//! Nothing detected here is fabricated: a facet is only written if its command
//! actually exists on this machine, and a root only if the directory is real.
//! Offering to launch something that is not installed is worse than offering
//! nothing.

use prism_core::config::{
    BindMode, Facet, FacetLimits, FileRoot, FilesConfig, GovernorConfig, HostConfig, Profile,
    ServerConfig, TerminalConfig,
};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"))
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// Directories worth exposing, in the order they should appear.
///
/// Home comes first and is read-only: it is the useful default and the one
/// where an accidental delete would hurt most. Output directories are writable
/// because that is where the operator actually manages files from a phone.
fn detect_roots() -> Vec<FileRoot> {
    let h = home();
    let mut roots = vec![FileRoot {
        name: "home".into(),
        path: h.clone(),
        writable: false,
    }];

    // Generated-image output, under whichever layout is installed.
    let outputs: &[(&str, PathBuf)] = &[
        (
            "comfy-output",
            h.join("ComfyUI-Easy-Install/ComfyUI-Easy-Install/ComfyUI/output"),
        ),
        ("comfy-output", h.join("ComfyUI/output")),
        ("a1111-output", h.join("stable-diffusion-webui/outputs")),
    ];
    for (name, path) in outputs {
        if exists(path) && !roots.iter().any(|r| r.name == *name) {
            roots.push(FileRoot {
                name: (*name).into(),
                path: path.clone(),
                // The point of reaching these remotely is to sort and delete.
                writable: true,
            });
        }
    }

    for (name, path) in [
        ("downloads", h.join("Downloads")),
        ("documents", h.join("Documents")),
    ] {
        if exists(&path) {
            roots.push(FileRoot {
                name: name.into(),
                path,
                writable: true,
            });
        }
    }

    roots
}

/// Workloads present on this machine that Prism can start and contain.
fn detect_facets() -> Vec<Facet> {
    let h = home();
    let mut facets = Vec::new();

    // ComfyUI. The Easy-Install launcher prompts, so it needs a pty.
    let comfy_easy = h.join("ComfyUI-Easy-Install/ComfyUI-Easy-Install");
    for (dir, script) in [
        (comfy_easy.clone(), comfy_easy.join("run_nvidia_gpu.sh")),
        (h.join("ComfyUI"), h.join("ComfyUI/main.py")),
    ] {
        if exists(&script) {
            let interactive = script.extension().is_some_and(|e| e == "sh");
            facets.push(Facet {
                id: "comfyui".into(),
                name: "ComfyUI".into(),
                command: if interactive {
                    vec![script.display().to_string()]
                } else {
                    vec!["python".into(), script.display().to_string()]
                },
                cwd: Some(dir),
                limits: FacetLimits {
                    memory_high: None,
                    memory_max: None,
                    // The failure this guards against is a swap runaway, so swap
                    // is capped while RAM is left alone — the operator routinely
                    // uses nearly the whole machine and a RAM ceiling would
                    // truncate legitimate work.
                    swap_max: Some("6G".into()),
                },
                enabled_if: Default::default(),
                pty: interactive,
            });
            break;
        }
    }

    // llama.cpp, however it was built.
    for candidate in [
        h.join("llama.cpp/build/bin/llama-server"),
        h.join("llama.cpp/build-new/bin/llama-server"),
        PathBuf::from("/usr/bin/llama-server"),
    ] {
        if exists(&candidate) {
            facets.push(Facet {
                id: "llama".into(),
                name: "llama.cpp".into(),
                command: vec![candidate.display().to_string(), "--host".into(), "127.0.0.1".into()],
                cwd: None,
                limits: FacetLimits {
                    memory_high: None,
                    memory_max: None,
                    swap_max: Some("4G".into()),
                },
                enabled_if: Default::default(),
                pty: false,
            });
            break;
        }
    }

    if which("ollama").is_some() {
        facets.push(Facet {
            id: "ollama".into(),
            name: "Ollama".into(),
            command: vec!["ollama".into(), "serve".into()],
            cwd: None,
            limits: FacetLimits {
                memory_high: None,
                memory_max: None,
                swap_max: Some("4G".into()),
            },
            enabled_if: prism_core::Gate {
                binary: Some("ollama".into()),
                ..Default::default()
            },
            pty: false,
        });
    }

    facets
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Bind to the tailnet when Tailscale is up, otherwise loopback.
///
/// Never a wildcard: an install that silently published itself to the local
/// network would be a poor default however convenient.
fn detect_bind() -> BindMode {
    let up = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if up { BindMode::Tailscale } else { BindMode::Localhost }
}

pub struct Detected {
    pub host: HostConfig,
    pub profile: Profile,
}

pub fn detect() -> Detected {
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "prism".into());

    Detected {
        host: HostConfig {
            server: ServerConfig {
                port: prism_core::config::DEFAULT_PORT,
                bind: detect_bind(),
            },
            files: FilesConfig {
                roots: detect_roots(),
            },
            terminal: TerminalConfig::default(),
        },
        profile: Profile {
            name: hostname,
            governor: GovernorConfig::default(),
            storm: prism_core::watchdog::storm::builtin_rules(),
            facet: detect_facets(),
        },
    }
}

/// Write the detected configuration, leaving any existing files alone.
///
/// Returns whether anything was written. Never overwrites: a re-run of the
/// installer must not discard the operator's own edits.
pub fn write_if_absent(config_dir: &Path) -> anyhow::Result<bool> {
    let host_path = config_dir.join("prism.toml");
    let profile_path = config_dir.join("profile.toml");
    if host_path.exists() || profile_path.exists() {
        return Ok(false);
    }

    let d = detect();
    std::fs::create_dir_all(config_dir)?;
    prism_core::config::save(&host_path, &d.host)?;
    prism_core::config::save(&profile_path, &d.profile)?;
    Ok(true)
}

/// `prismd setup` — report what was found, and write config if there is none.
pub fn command(config_dir: &Path) -> anyhow::Result<()> {
    let d = detect();

    println!();
    println!("Detected on this machine");
    println!();
    println!("  bind        {}", match &d.host.server.bind {
        BindMode::Tailscale => "tailnet (Tailscale is up)".to_string(),
        BindMode::Localhost => "localhost (Tailscale not detected)".to_string(),
        BindMode::Address(a) => a.clone(),
    });

    println!("  file roots");
    for r in &d.host.files.roots {
        println!(
            "    {:<14} {}{}",
            r.name,
            r.path.display(),
            if r.writable { "" } else { "  (read-only)" }
        );
    }

    if d.profile.facet.is_empty() {
        println!("  workloads     none found — add them from the Facets app");
    } else {
        println!("  workloads");
        for f in &d.profile.facet {
            println!(
                "    {:<14} {}{}",
                f.name,
                f.command.first().map(String::as_str).unwrap_or(""),
                if f.pty { "  (interactive)" } else { "" }
            );
        }
    }
    println!();

    let wrote = write_if_absent(config_dir)?;
    if wrote {
        println!("Written to {}", config_dir.display());
        println!("Edit those files directly to change anything.");
    } else {
        println!("Configuration already exists at {}", config_dir.display());
        println!("Nothing was changed. Delete those files to re-detect.");
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_always_a_root() {
        let roots = detect_roots();
        assert_eq!(roots[0].name, "home");
    }

    #[test]
    fn home_is_read_only_by_default() {
        // The most useful root is also the one where a stray delete hurts most.
        assert!(!detect_roots()[0].writable);
    }

    #[test]
    fn only_directories_that_exist_are_offered() {
        for r in detect_roots() {
            assert!(r.path.exists(), "{} does not exist", r.path.display());
        }
    }

    #[test]
    fn root_names_are_unique() {
        // Duplicates would make one unreachable, since lookup is by name.
        let roots = detect_roots();
        let mut names: Vec<_> = roots.iter().map(|r| r.name.clone()).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }

    #[test]
    fn only_workloads_that_exist_are_offered() {
        // Offering to launch something absent is worse than offering nothing.
        for f in detect_facets() {
            let program = &f.command[0];
            if program.starts_with('/') {
                assert!(Path::new(program).exists(), "{program} does not exist");
            } else {
                assert!(which(program).is_some(), "{program} is not on PATH");
            }
        }
    }

    #[test]
    fn facet_ids_are_unique() {
        let facets = detect_facets();
        let mut ids: Vec<_> = facets.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before);
    }

    #[test]
    fn detected_workloads_cap_swap_but_not_ram() {
        // The failure being guarded against is a swap runaway; a RAM ceiling
        // would truncate legitimate work on a machine the operator uses fully.
        for f in detect_facets() {
            assert!(f.limits.swap_max.is_some(), "{} has no swap cap", f.name);
            assert!(f.limits.memory_max.is_none(), "{} caps RAM", f.name);
        }
    }

    #[test]
    fn the_bind_default_is_never_a_wildcard() {
        assert!(!detect_bind().is_wildcard());
    }

    #[test]
    fn existing_configuration_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("prism-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(write_if_absent(&dir).unwrap(), "should write on a clean dir");
        let original = std::fs::read_to_string(dir.join("prism.toml")).unwrap();

        std::fs::write(dir.join("prism.toml"), "# hand-edited\n").unwrap();
        assert!(!write_if_absent(&dir).unwrap(), "should not write twice");
        assert_ne!(
            std::fs::read_to_string(dir.join("prism.toml")).unwrap(),
            original,
            "a re-run must not discard the operator's edits"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_written_can_be_read_back() {
        let dir = std::env::temp_dir().join(format!("prism-setup-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_if_absent(&dir).unwrap();

        let host: HostConfig =
            prism_core::config::load_or_default(&dir.join("prism.toml")).unwrap();
        let profile: Profile =
            prism_core::config::load_or_default(&dir.join("profile.toml")).unwrap();
        assert!(!host.files.roots.is_empty());
        assert!(!profile.storm.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
