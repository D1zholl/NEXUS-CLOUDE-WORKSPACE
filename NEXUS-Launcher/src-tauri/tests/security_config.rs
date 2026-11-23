//! Guards the desktop security posture against accidental capability creep.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &str) -> Value {
    let path = project_root().join(path);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", path.display());
    });
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("invalid JSON in {}: {error}", path.display()))
}

#[test]
fn capability_is_minimal_and_has_no_shell_fs_or_http_access() {
    let capability = read_json("capabilities/main.json");

    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default"]),
        "the WebView must not receive filesystem, shell, HTTP or updater permissions"
    );

    let serialized = capability.to_string().to_ascii_lowercase();
    for forbidden in [
        "shell:",
        "fs:",
        "http:",
        "allowed-commands",
        "shell-allow",
        "process:",
        "updater:",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "capability unexpectedly contains {forbidden:?}"
        );
    }
}

#[test]
fn tauri_config_hardens_csp_and_disables_remote_navigation_surface() {
    let config = read_json("tauri.conf.json");
    let security = &config["app"]["security"];

    assert_eq!(security["freezePrototype"], true);
    assert_eq!(
        security["capabilities"],
        serde_json::json!(["main-capability"])
    );

    let csp = security["csp"].as_str().expect("CSP must be configured");
    for directive in [
        "default-src 'self'",
        "object-src 'none'",
        "frame-ancestors 'none'",
        "base-uri 'self'",
        "form-action 'self'",
        "connect-src 'self' ipc: http://ipc.localhost",
    ] {
        assert!(csp.contains(directive), "CSP is missing {directive:?}");
    }

    assert!(
        config.get("plugins").is_none(),
        "no Tauri plugins should be registered until explicitly reviewed"
    );
    assert_ne!(
        config["app"]["withGlobalTauri"],
        Value::Bool(true),
        "the global Tauri object must not be exposed to page scripts"
    );

    // Development server is not configured: packaged assets are the only
    // navigation target for the main window.
    assert!(
        config["build"].get("devUrl").is_none(),
        "a remote devUrl must not be baked into the release configuration"
    );
}

#[test]
fn frontend_cannot_invoke_shell_or_arbitrary_fs_through_registered_commands() {
    let source = fs::read_to_string(project_root().join("src/lib.rs")).unwrap();
    let command_files = fs::read_dir(project_root().join("src/commands"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    let combined = format!("{source}\n{command_files}");
    for forbidden in [
        "tauri_plugin_shell",
        "tauri_plugin_fs",
        "tauri_plugin_http",
        "Command::new",
        "std::process::Command",
        "shell().",
    ] {
        assert!(
            !combined.contains(forbidden),
            "unexpected process/shell surface: {forbidden:?}"
        );
    }
}
