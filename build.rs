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
    emit_servil_package(&manifest_dir, &lock);

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

/// Where a local checkout of the fork sits, relative to this crate's
/// manifest directory, when a `--config` patch replaces the git
/// dependency: the enclosing checkout, the one layout the tools use.
const SERVIL_CHECKOUT: &str = "..";

/// The fork's repository, as Cargo.toml names it and Cargo.lock records it.
const SERVIL_GIT: &str = "https://github.com/johnservil/BLAKE3";

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

/// Reads the git state of `repository`, leaving out the path `skip` (the
/// results directory, or this repository nested inside the fork's
/// checkout, where it is an untracked directory). A dirty tree is fingerprinted by hashing its status, its
/// diff against HEAD, and every untracked file.
fn git_state(repository: &Path, skip: Option<&str>) -> GitState {
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

    let exclude = skip.map(|path| format!(":(exclude){path}"));
    let pathspec: Vec<&str> = ["--", "."].into_iter().chain(exclude.as_deref()).collect();
    let status = git_bytes(
        repository,
        &[&["status", "--porcelain=v1", "-z", "--untracked-files=all"][..], &pathspec].concat(),
    );

    let clean_status = if status.is_empty() || only_servil_patched(repository, &status) {
        "clean".to_owned()
    } else {
        /*
         * `git diff --binary HEAD` captures staged and unstaged changes to
         * tracked files. Git does not include untracked-file contents in that
         * diff, so hash those contents explicitly as well.
         */
        let diff = git_bytes(
            repository,
            &[&["diff", "--binary", "HEAD"][..], &pathspec].concat(),
        );

        let untracked = git_bytes(
            repository,
            &[&["ls-files", "--others", "--exclude-standard", "-z"][..], &pathspec].concat(),
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

/*
 * A `--config` patch that points blake3-servil at a local checkout makes
 * Cargo drop that package's `source` line from Cargo.lock. The fork's own
 * provenance line then names the checkout and its state, so a tree whose
 * only change is that line (or another pinned fork commit there) counts
 * as clean.
 */
fn only_servil_patched(repository: &Path, status: &[u8]) -> bool {
    if status != b" M Cargo.lock\0" {
        return false;
    }
    let without_servil_source = |lock: &str| -> String {
        lock.lines()
            .filter(|line| !line.starts_with(&format!("source = \"git+{SERVIL_GIT}")))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let committed = git_text(repository, &["show", "HEAD:Cargo.lock"]);
    let current = fs::read_to_string(repository.join("Cargo.lock"))
        .expect("Cargo.lock must be valid UTF-8");
    without_servil_source(&committed) == without_servil_source(current.trim())
}

fn emit_git_metadata(repository: &Path) {
    watch_repository(repository);

    /* The run's own results are output, not source. */
    let state = git_state(repository, Some("benchmark-results"));

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
 * blake3-servil comes from the fork's repository at the commit Cargo.lock
 * pins, with a line such as
 *   source = "git+https://github.com/johnservil/BLAKE3?branch=servil#COMMIT"
 * A `--config` patch replaces it with the enclosing checkout at `..`, and
 * Cargo.lock then records no source; the checkout's own git state
 * identifies the code: origin URL, branch, exact commit, and a clean flag
 * or a hash of the uncommitted changes. Both emit the same fields.
 */
fn emit_servil_package(manifest_dir: &Path, lock: &str) {
    let block = package_block(lock, "blake3-servil")
        .expect("blake3-servil must be present in Cargo.lock");
    let version = quoted_field(block, "version")
        .expect("Cargo.lock package must have a version");

    let description = match quoted_field(block, "source") {
        Some(source) => {
            let pinned = source
                .strip_prefix(&format!("git+{SERVIL_GIT}?branch="))
                .unwrap_or_else(|| panic!("blake3-servil must come from {SERVIL_GIT} or a local patch; Cargo.lock says {source}"));
            let (branch, commit) = pinned
                .split_once('#')
                .expect("a git source in Cargo.lock names its commit after '#'");
            format!("blake3-servil {version}; source {SERVIL_GIT}; branch {branch}; commit {commit}; clean")
        }
        None => {
            let repository = manifest_dir.join(SERVIL_CHECKOUT);
            assert!(
                repository.join("Cargo.toml").is_file(),
                "blake3-servil is patched to a local checkout, which must be the enclosing one at {} (this repository inside it)",
                repository.display()
            );
            watch_repository(&repository);
            let own_name = manifest_dir
                .file_name()
                .and_then(|name| name.to_str())
                .expect("the manifest directory has a UTF-8 name");
            let state = git_state(&repository, Some(own_name));
            format!(
                "blake3-servil {version} (local checkout {SERVIL_CHECKOUT}); source {}; branch {}; commit {}; {}",
                state.source, state.branch, state.commit, state.clean_status
            )
        }
    };
    emit_env("BLAKE3_SERVIL_SOURCE_INFO", &description);
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

/// The `[[package]]` block of `name` in Cargo.lock.
fn package_block<'a>(lock: &'a str, name: &str) -> Option<&'a str> {
    lock.split("[[package]]")
        .skip(1)
        .find(|package| quoted_field(package, "name").as_deref() == Some(name))
}

fn package_description(
    lock: &str,
    name: &str,
) -> Option<String> {
    let package = package_block(lock, name)?;

    let version = quoted_field(package, "version")
        .expect("Cargo.lock package must have a version");

    let mut result = format!("{name} {version}");

    if let Some(checksum) = quoted_field(package, "checksum") {
        result.push_str("; crate archive SHA-256 ");
        result.push_str(&checksum);
    }

    if let Some(source) = quoted_field(package, "source") {
        result.push_str("; source ");
        result.push_str(&source);
    }

    Some(result)
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
