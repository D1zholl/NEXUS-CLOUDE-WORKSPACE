//! Integration tests for the filesystem layer on the real Windows filesystem.

use std::fs;
use std::path::Path;
use std::process::Command;

use nexus_core::ErrorCode;
use nexus_platform::security::{ExpectedFile, SafeFs, SafeRelativePath, Sha256Digest};

fn safe_fs() -> (tempfile::TempDir, SafeFs) {
    let temp = tempfile::tempdir().unwrap();
    let fs = SafeFs::open(temp.path()).unwrap();
    (temp, fs)
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) -> bool {
    Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[test]
fn path_traversal_and_absolute_paths_are_rejected_before_io() {
    let (_temp, fs) = safe_fs();
    for raw in [
        "../outside.txt",
        "a/../../outside.txt",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
        "\\\\server\\share\\file",
        "safe/../../outside.txt",
    ] {
        assert!(
            SafeRelativePath::parse(raw).is_err(),
            "{raw:?} must not become a capability path"
        );
    }
    assert!(!fs.root().join("outside.txt").exists());
}

#[cfg(windows)]
#[test]
fn junction_cannot_be_used_to_escape_or_delete_outside_data() {
    let (temp, fs) = safe_fs();
    let real = temp.path().join("real");
    let outside = temp.path().join("outside");
    fs::create_dir(&real).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(real.join("secret.txt"), b"inside").unwrap();
    fs::write(outside.join("keep.txt"), b"keep").unwrap();

    let link = temp.path().join("link");
    if !create_junction(&link, &outside) {
        eprintln!("skipping junction test: mklink /J is unavailable");
        return;
    }

    let error = fs
        .resolve_existing(&SafeRelativePath::parse("link/keep.txt").unwrap())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ReparsePointRejected);

    let error = fs
        .write_atomic(
            &SafeRelativePath::parse("link/evil.txt").unwrap(),
            b"must not be written",
            true,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ReparsePointRejected);
    assert!(!outside.join("evil.txt").exists());

    // Deleting the junction removes the link itself, never the target.
    fs.remove_dir_all(&SafeRelativePath::parse("link").unwrap())
        .unwrap();
    assert!(!link.exists());
    assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(fs::read(real.join("secret.txt")).unwrap(), b"inside");
}

#[cfg(windows)]
#[test]
fn junction_below_instance_directory_is_rejected() {
    let (temp, fs) = safe_fs();
    let instance = SafeRelativePath::parse("instances/demo").unwrap();
    fs.ensure_dir(&instance).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let link = temp.path().join("instances/demo/mods");
    if !create_junction(&link, &outside) {
        return;
    }

    let error = fs
        .write_atomic(
            &SafeRelativePath::parse("instances/demo/mods/evil.jar").unwrap(),
            b"payload",
            true,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ReparsePointRejected);
    assert!(!outside.join("evil.jar").exists());
}

#[test]
fn checksum_verification_detects_substituted_installation_file() {
    let (_temp, fs) = safe_fs();
    let relative = SafeRelativePath::parse("libraries/example.jar").unwrap();
    fs.write_atomic(&relative, b"trusted-bytes", false).unwrap();

    let expected =
        Sha256Digest::from_hex("0b1e0a1a60e0f16f8b8a54c1f4c6f6a2d4f54b6d4b9f0f8f0d3a1c6e07a4a2b1")
            .unwrap();
    let error = fs
        .verify(&relative, &ExpectedFile::sha256(expected), 1024)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ChecksumMismatch);

    let digest = fs.hash(&relative, 1024).unwrap();
    fs.verify(
        &relative,
        &ExpectedFile::new(digest.sha256, digest.size),
        1024,
    )
    .unwrap();
}
