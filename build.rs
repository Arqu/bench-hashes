use blake3::Hasher;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn hash_framed(
    hasher: &mut Hasher,
    label: &[u8],
    contents: &[u8],
) {
    hasher.update(&(label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update(&(contents.len() as u64).to_le_bytes());
    hasher.update(contents);
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );

    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=README.md");
    println!("cargo:rerun-if-changed=src");

    let lock_path = manifest_dir.join("Cargo.lock");
    assert!(
        lock_path.is_file(),
        "Cargo.lock must exist so benchmark source versions are reproducible"
    );

    let lock = fs::read_to_string(&lock_path)
        .expect("Cargo.lock must be valid UTF-8");

    emit_required_package(
        "BLAKE3_SOURCE_INFO",
        &lock,
        "blake3",
    );
    emit_required_package(
        "SHA2_SOURCE_INFO",
        &lock,
        "sha2",
    );
    emit_required_package(
        "SHA1_CHECKED_SOURCE_INFO",
        &lock,
        "sha1-checked",
    );
    emit_required_package(
        "RING_SOURCE_INFO",
        &lock,
        "ring",
    );
    emit_required_package(
        "AB_BLAKE3_SOURCE_INFO",
        &lock,
        "ab-blake3",
    );
    emit_path_package(
        "BLAKE3_SERVIL_SOURCE_INFO",
        &manifest_dir,
        "blake3-servil",
        BLAKE3_SME2_PATH,
    );

    emit_git_metadata(&manifest_dir);

    let rustc = env::var_os("RUSTC")
        .expect("Cargo must provide RUSTC");

    let rustc_output = Command::new(rustc)
        .arg("--version")
        .output()
        .expect("rustc --version must run successfully");

    assert!(
        rustc_output.status.success(),
        "rustc --version must succeed"
    );

    let rustc_version = String::from_utf8(rustc_output.stdout)
        .expect("rustc version must be UTF-8")
        .trim()
        .to_owned();

    let target = env::var("TARGET")
        .expect("Cargo must provide TARGET");

    let target_features = env::var("CARGO_CFG_TARGET_FEATURE")
        .expect("Cargo must provide CARGO_CFG_TARGET_FEATURE");

    emit_env("BENCH_RUSTC_VERSION", &rustc_version);
    emit_env("BENCH_BUILD_TARGET", &target);
    emit_env("BENCH_TARGET_FEATURES", &target_features);
}

fn normalize_git_source(source: &str) -> String {
    let source = source.trim().trim_end_matches(".git");

    if let Some(rest) = source.strip_prefix("git@") {
        let (host, path) = rest
            .split_once(':')
            .expect("scp-style Git source must contain ':'");

        return format!("https://{host}/{path}");
    }

    if let Some(rest) = source.strip_prefix("ssh://git@") {
        let (host, path) = rest
            .split_once('/')
            .expect("SSH Git source must contain a repository path");

        return format!("https://{host}/{path}");
    }

    source.to_owned()
}

/*
 * A repository can legitimately have no reachable release tags, so a
 * describe failure is a soft result rather than a contract violation.
 */
fn git_text_allow_failure(
    repository: &Path,
    arguments: &[&str],
) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("Git must be installed");

    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8(output.stdout)
            .expect("Git output must be UTF-8")
            .trim()
            .to_owned(),
    )
}

/// Where Cargo.toml points the `blake3-servil` path dependency, relative to
/// this crate's manifest directory: the enclosing fork checkout. Cargo.toml
/// and this constant agree.
const BLAKE3_SME2_PATH: &str = "..";

/// What `git` reports about a checkout: its origin URL, HEAD commit,
/// nearest release tag, current branch, and whether the tree is clean.
struct GitState {
    source: String,
    commit: String,
    tag: String,
    branch: String,
    clean_status: String,
}

/// Registers every file git tracks in `repository` (and the refs that
/// name HEAD) as build-script inputs, so the embedded provenance follows
/// each commit and each edit.
fn watch_repository(repository: &Path) {
    let git_directory = git_text(
        repository,
        &["rev-parse", "--git-dir"],
    );

    let git_directory = {
        let path = PathBuf::from(git_directory);

        if path.is_absolute() {
            path
        } else {
            repository.join(path)
        }
    };

    println!(
        "cargo:rerun-if-changed={}",
        git_directory.join("HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        git_directory.join("index").display()
    );
    // HEAD names a branch, so a commit changes the branch's ref and leaves
    // HEAD itself alone (and the index too, when the commit follows a
    // build of the staged tree, as a pre-commit check does). The reflog
    // gains a line on every commit, checkout, and reset.
    println!(
        "cargo:rerun-if-changed={}",
        git_directory.join("logs/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        git_directory.join("refs/tags").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        git_directory.join("packed-refs").display()
    );

    let tracked_files = git_bytes(
        repository,
        &["ls-files", "-z"],
    );

    for path in tracked_files
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = String::from_utf8(path.to_vec())
            .expect("tracked Git paths must be UTF-8");

        println!(
            "cargo:rerun-if-changed={}",
            repository.join(path).display()
        );
    }
}

/// Reads the git state of `repository`. A dirty tree is fingerprinted by
/// hashing its status, its diff against HEAD, and every untracked file.
fn git_state(repository: &Path) -> GitState {
    let source = normalize_git_source(&git_text(
        repository,
        &["remote", "get-url", "origin"],
    ));

    let commit = git_text(
        repository,
        &["rev-parse", "HEAD"],
    );

    let branch = git_text(
        repository,
        &["rev-parse", "--abbrev-ref", "HEAD"],
    );

    let tags = match git_text_allow_failure(
        repository,
        &["describe", "--tags", "--long", "--match", "v*"],
    ) {
        Some(description) => {
            /*
             * git describe --long output has the form TAG-N-gHASH, where
             * N is the number of commits since TAG. The tag itself may
             * contain '+' but never '-', so splitting from the right
             * twice is unambiguous.
             */
            let (rest, _short_hash) = description
                .rsplit_once('-')
                .expect("git describe --long output must contain '-'");

            let (tag, commits_since) = rest
                .rsplit_once('-')
                .expect("git describe --long output must contain two '-'");

            if commits_since == "0" {
                tag.to_owned()
            } else {
                format!("{tag} (+{commits_since} commits)")
            }
        }
        None => "(no reachable release tag)".to_owned(),
    };

    let status = git_bytes(
        repository,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
    );

    let clean_status = if status.is_empty() {
        "clean".to_owned()
    } else {
        /*
         * `git diff --binary HEAD` captures staged and unstaged changes to
         * tracked files. Git does not include untracked-file contents in that
         * diff, so hash those contents explicitly as well.
         */
        let diff = git_bytes(
            repository,
            &["diff", "--binary", "HEAD", "--"],
        );

        let untracked = git_bytes(
            repository,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        );

        let mut hasher = Hasher::new();

        hash_framed(
            &mut hasher,
            b"format",
            b"bench-hashes dirty working tree v1",
        );
        hash_framed(&mut hasher, b"status", &status);
        hash_framed(&mut hasher, b"diff", &diff);

        for path_bytes in untracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8(path_bytes.to_vec())
                .expect("untracked Git paths must be UTF-8");

            /* A nested checkout (a directory) contributes its own provenance;
               its contents are not hashed here. */
            if repository.join(&path).is_dir() {
                continue;
            }

            let contents = fs::read(repository.join(&path))
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to read untracked file {path:?}: {error}"
                    )
                });

            hash_framed(
                &mut hasher,
                b"untracked path",
                path.as_bytes(),
            );
            hash_framed(
                &mut hasher,
                b"untracked contents",
                &contents,
            );
        }

        format!("dirty-{}", hasher.finalize().to_hex())
    };

    GitState {
        source,
        commit,
        tag: tags,
        branch,
        clean_status,
    }
}

fn emit_git_metadata(repository: &Path) {
    watch_repository(repository);

    let state = git_state(repository);

    emit_env("BENCH_GIT_SOURCE", &state.source);
    emit_env("BENCH_GIT_COMMIT", &state.commit);
    emit_env("BENCH_GIT_TAG", &state.tag);
    emit_env("BENCH_GIT_CLEAN_STATUS", &state.clean_status);
}

fn git_text(repository: &Path, arguments: &[&str]) -> String {
    let output = git_output(repository, arguments);

    String::from_utf8(output.stdout)
        .expect("Git text output must be UTF-8")
        .trim()
        .to_owned()
}

fn git_bytes(repository: &Path, arguments: &[&str]) -> Vec<u8> {
    git_output(repository, arguments).stdout
}

fn git_output(repository: &Path, arguments: &[&str]) -> Output {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("Git must be installed");

    assert!(
        output.status.success(),
        "git {} failed:\n{}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );

    output
}

/*
 * A path dependency has no registry checksum and no Cargo.lock source;
 * the checkout's own git state identifies it: origin URL, branch, exact
 * commit, and a clean flag or a hash of the uncommitted changes. Emit
 * those in the same shape this repository uses to identify itself. The
 * checkout must be a git repository with an `origin` remote.
 */
fn emit_path_package(
    environment_variable: &str,
    manifest_dir: &Path,
    package_name: &str,
    relative_path: &str,
) {
    let repository = manifest_dir.join(relative_path);

    assert!(
        repository.join("Cargo.toml").is_file(),
        "{package_name} must be checked out at {} (this repository lives inside that checkout); clone github.com/johnservil/BLAKE3 there and check out its sme2-bench branch",
        repository.display()
    );

    watch_repository(&repository);

    let state = git_state(&repository);

    emit_env(
        environment_variable,
        &format!(
            "{package_name} (path dependency {relative_path}); source {}; branch {}; commit {}; {}",
            state.source, state.branch, state.commit, state.clean_status
        ),
    );
}

fn emit_required_package(
    environment_variable: &str,
    lock: &str,
    package_name: &str,
) {
    let description = package_description(lock, package_name)
        .unwrap_or_else(|| {
            panic!(
                "{package_name} must be present in Cargo.lock"
            )
        });

    emit_env(environment_variable, &description);
}

fn package_description(
    lock: &str,
    requested_name: &str,
) -> Option<String> {
    for package in lock.split("[[package]]").skip(1) {
        let Some(name) = quoted_field(package, "name") else {
            continue;
        };

        if name != requested_name {
            continue;
        }

        let version = quoted_field(package, "version")
            .expect("Cargo.lock package must have a version");

        let mut result = format!("{name} {version}");

        if let Some(checksum) =
            quoted_field(package, "checksum")
        {
            result.push_str("; crate archive SHA-256 ");
            result.push_str(&checksum);
        }

        if let Some(source) = quoted_field(package, "source") {
            result.push_str("; source ");
            result.push_str(&source);
        }

        return Some(result);
    }

    None
}

fn quoted_field(block: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} = ");

    block.lines().find_map(|line| {
        let value = line.trim().strip_prefix(&prefix)?;

        assert!(
            value.len() >= 2
                && value.starts_with('"')
                && value.ends_with('"'),
            "{key} must be a quoted Cargo.lock field"
        );

        Some(value[1..value.len() - 1].to_owned())
    })
}

fn emit_env(name: &str, value: &str) {
    assert!(
        !value.contains('\n') && !value.contains('\r'),
        "embedded metadata must occupy one line"
    );

    println!("cargo:rustc-env={name}={value}");
}
