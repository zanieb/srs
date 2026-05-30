//! Tests for `-Zartifact-cache`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::prelude::*;
use crate::utils::tools;
use cargo_test_support::{Project, paths, prelude::*, project_in, sleep_ms};

fn project_with_cache(name: &str, cache: &Path, materialization: &str, value: u32) -> Project {
    let cache = cache.to_string_lossy().replace('\\', "\\\\");
    project_in(name)
        .file(
            ".cargo/config.toml",
            &format!(
                r#"
                [build]
                artifact-cache-dir = "{cache}"
                artifact-cache-materialization = "{materialization}"
                "#,
            ),
        )
        .file(
            "src/lib.rs",
            &format!("pub fn value() -> u32 {{ {value} }}\n"),
        )
        .build()
}

fn cached_rlib(root: &Path) -> PathBuf {
    let mut rlibs = Vec::new();
    visit_rlibs(root, &mut rlibs);
    assert_eq!(
        rlibs.len(),
        1,
        "expected one cached rlib under {}",
        root.display()
    );
    rlibs.pop().unwrap()
}

fn cached_rlibs(root: &Path) -> Vec<PathBuf> {
    let mut rlibs = Vec::new();
    visit_rlibs(root, &mut rlibs);
    rlibs
}

fn all_rlibs(root: &Path) -> Vec<PathBuf> {
    let mut rlibs = Vec::new();
    visit_all_rlibs(root, &mut rlibs);
    rlibs
}

fn directory_size(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directory_size(&path)
            } else {
                fs::metadata(path).unwrap().len()
            }
        })
        .sum()
}

fn recorded_cache_size(path: &Path) -> u64 {
    fs::read_to_string(path.join(".cargo-artifact-cache-size"))
        .unwrap()
        .trim()
        .strip_prefix("v1 ")
        .unwrap()
        .parse()
        .unwrap()
}

fn contains_cache_entry_with_marker(path: &Path, marker: &str) -> bool {
    fs::read_dir(path).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().contains(marker))
            || (path.is_dir() && contains_cache_entry_with_marker(&path, marker))
    })
}

fn visit_rlibs(path: &Path, rlibs: &mut Vec<PathBuf>) {
    if !path.exists() {
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            visit_rlibs(&path, rlibs);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rlib")
        {
            rlibs.push(path);
        }
    }
}

fn visit_all_rlibs(path: &Path, rlibs: &mut Vec<PathBuf>) {
    if !path.exists() {
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit_all_rlibs(&path, rlibs);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rlib")
        {
            rlibs.push(path);
        }
    }
}

fn isolated_loader_path() -> PathBuf {
    paths::root().join("empty-compiler-loader-path")
}

fn build(project: &Project) {
    project
        .cargo("-Zartifact-cache build --lib")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .run();
}

fn build_in_target(project: &Project, target_dir: &Path) {
    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(target_dir)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .run();
}

fn build_in_target_with_env(project: &Project, target_dir: &Path, key: &str, value: &str) {
    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(target_dir)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env(key, value)
        .run();
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn rustc_wrappers_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_WRAPPER", tools::echo_wrapper())
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn explicitly_configured_rustc_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let rustc = cargo_util::paths::resolve_executable(Path::new("rustc")).unwrap();

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC", rustc)
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn self_profile_requests_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let self_profile = project.root().join("self-profile");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_BOOTSTRAP", "1")
        .env(
            "RUSTFLAGS",
            format!("-Zself-profile={}", self_profile.display()),
        )
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn rustc_tracing_requests_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let trace_output = project.root().join("rustc-trace.log");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_LOG", "rustc_codegen_ssa=info")
        .env("RUSTC_LOG_OUTPUT_TARGET", trace_output)
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn rustc_bootstrap_requests_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_BOOTSTRAP", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn rustc_forced_version_inputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_FORCE_RUSTC_VERSION", "artifact-cache-test-version")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

fn project_without_cache_config(name: &str, value: u32) -> Project {
    project_in(name)
        .file(
            "src/lib.rs",
            &format!("pub fn value() -> u32 {{ {value} }}\n"),
        )
        .build()
}

fn build_manual_extern_dependency() -> PathBuf {
    let dependency = project_in("dependency")
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "extern-dep"
            version = "0.0.1"
            edition = "2024"
            "#,
        )
        .file("src/lib.rs", "pub fn value() -> u32 { 7 }\n")
        .build();
    let target = dependency.root().join("target-dir");
    build_in_target(&dependency, &target);
    cached_rlib(&target.join("debug").join("deps"))
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn hardlink_restore_detaches_before_rebuild() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    build_in_target(&project, &consumer_target);

    let stored = cached_rlib(&cache);
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    let stored_before = fs::read(&stored).unwrap();

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &consumer_target);

    let rebuilt = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_ne!(stored_before, fs::read(&rebuilt).unwrap());
    assert!(
        all_rlibs(&cache)
            .iter()
            .any(|path| fs::read(path).unwrap() == stored_before)
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn cache_hit_is_fresh_after_rebuilding_non_cacheable_dependency() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let producer_target = paths::root().join("producer-build");
    let consumer_target = paths::root().join("consumer-build");
    let project = project_in("project")
        .file(
            ".cargo/config.toml",
            &format!(
                r#"
                [build]
                artifact-cache-dir = "{}"
                artifact-cache-materialization = "hardlink"
                "#,
                cache.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .file(
            "Cargo.toml",
            r#"
            [package]
            name = "project"
            version = "0.0.1"
            edition = "2024"

            [dependencies]
            dependency = { path = "dependency" }
            "#,
        )
        .file(
            "src/lib.rs",
            "pub fn value() -> u32 { dependency::value() }\n",
        )
        .file(
            "dependency/Cargo.toml",
            r#"
            [package]
            name = "dependency"
            version = "0.0.1"
            edition = "2024"
            build = "build.rs"
            "#,
        )
        .file("dependency/build.rs", "fn main() {}\n")
        .file("dependency/src/lib.rs", "pub fn value() -> u32 { 42 }\n")
        .build();

    build_in_target(&project, &producer_target);
    sleep_ms(1100);
    build_in_target(&project, &consumer_target);

    let stored = cached_rlib(&cache);
    let restored = cached_rlibs(&consumer_target.join("debug").join("deps"))
        .into_iter()
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("libproject-")
        })
        .unwrap();
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    let invoked_timestamp = fs::read_dir(consumer_target.join("debug").join(".fingerprint"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("project-")
        })
        .unwrap()
        .join("invoked.timestamp");
    let before = fs::metadata(&invoked_timestamp)
        .unwrap()
        .modified()
        .unwrap();
    sleep_ms(1100);

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&consumer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .with_stderr_does_not_contain("[COMPILING] project [..]")
        .run();
    assert_eq!(
        before,
        fs::metadata(invoked_timestamp).unwrap().modified().unwrap()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn copy_restore_does_not_share_inodes() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "copy", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    build_in_target(&project, &consumer_target);

    let stored = cached_rlib(&cache);
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_ne!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    assert_eq!(fs::read(&stored).unwrap(), fs::read(&restored).unwrap());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn copy_restore_detaches_existing_hardlink() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let before_target = project.root().join("before-target");
    let after_target = project.root().join("after-target");
    let consumer_target = project.root().join("consumer-target");
    project.change_file(
        "src/lib.rs",
        r#"pub fn value() -> &'static str { env!("ARTIFACT_CACHE_TEST_VALUE") }"#,
    );

    build_in_target_with_env(
        &project,
        &before_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "before",
    );
    build_in_target_with_env(
        &project,
        &after_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "after",
    );
    build_in_target_with_env(
        &project,
        &consumer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "before",
    );
    let before_output = cached_rlib(&consumer_target.join("debug").join("deps"));
    let before_bytes = fs::read(&before_output).unwrap();
    let before_cached = cached_rlibs(&cache)
        .into_iter()
        .find(|path| fs::read(path).unwrap() == before_bytes)
        .unwrap();
    assert_eq!(
        fs::metadata(&before_cached).unwrap().ino(),
        fs::metadata(&before_output).unwrap().ino()
    );

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "copy"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    build_in_target_with_env(
        &project,
        &consumer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "after",
    );

    let after_output = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(fs::read(&before_cached).unwrap(), before_bytes);
    assert_ne!(
        fs::metadata(&before_cached).unwrap().ino(),
        fs::metadata(&after_output).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn same_source_different_target_directories_restore_by_hardlink() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    build_in_target(&project, &consumer_target);

    let stored = cached_rlib(&cache);
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn target_paths_inside_semantic_rustflags_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let producer_value = producer_target.join("debug").to_string_lossy().to_string();
    let consumer_value = consumer_target.join("debug").to_string_lossy().to_string();
    project.change_file(
        "src/lib.rs",
        &format!(
            r#"
            #![allow(unexpected_cfgs)]
            #[cfg(cache_variant = "{producer_value}")]
            pub fn value() -> u32 {{ 42 }}
            #[cfg(cache_variant = "{consumer_value}")]
            pub fn value() -> u32 {{ 43 }}
            "#,
        ),
    );

    let config = |value: &str| {
        format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["--cfg", 'cache_variant="{value}"']
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        )
    };

    project.change_file(".cargo/config.toml", &config(&producer_value));
    build_in_target(&project, &producer_target);
    let producer_bytes =
        fs::read(cached_rlib(&producer_target.join("debug").join("deps"))).unwrap();

    project.change_file(".cargo/config.toml", &config(&consumer_value));
    build_in_target(&project, &consumer_target);
    let consumer_bytes =
        fs::read(cached_rlib(&consumer_target.join("debug").join("deps"))).unwrap();

    assert_eq!(cached_rlibs(&cache).len(), 2);
    assert_ne!(producer_bytes, consumer_bytes);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn concurrent_restores_share_the_cached_entry() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let delayed_target = project.root().join("delayed-target");
    let concurrent_target = project.root().join("concurrent-target");
    let ready = project.root().join("restore-ready");
    if ready.exists() {
        fs::remove_file(&ready).unwrap();
    }

    build_in_target(&project, &producer_target);
    let stored = cached_rlib(&cache);

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&delayed_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_DELAY_MS", "5000")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_READY_FILE", &ready)
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(ready.exists(), "cargo did not reach the restore test hook");

    build_in_target(&project, &concurrent_target);
    let concurrent = cached_rlib(&concurrent_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&concurrent).unwrap().ino()
    );

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "delayed restore failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let delayed = cached_rlib(&delayed_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&delayed).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn environment_configuration_restores_by_hardlink() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_without_cache_config("project", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--target-dir")
            .arg(target)
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env(
                cargo_util::paths::dylib_path_envvar(),
                isolated_loader_path(),
            )
            .env("CARGO_BUILD_ARTIFACT_CACHE_DIR", &cache)
            .env("CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION", "hardlink")
            .env("CARGO_INCREMENTAL", "1")
            .run();
    }

    let stored = cached_rlib(&cache);
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn distinct_inherited_dynamic_library_search_paths_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--target-dir")
            .arg(target)
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env_remove(cargo_util::paths::dylib_path_envvar())
            .env("CARGO_INCREMENTAL", "1")
            .env(
                cargo_util::paths::dylib_path_envvar(),
                target.join("debug").join("compiler-libs"),
            )
            .run();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn changed_inherited_dynamic_library_inputs_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let inherited_path = project.root().join("compiler-libs");
    let input = inherited_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&inherited_path).unwrap();
    fs::write(&input, b"before").unwrap();

    build_in_target_with_env(
        &project,
        &producer_target,
        cargo_util::paths::dylib_path_envvar(),
        inherited_path.to_str().unwrap(),
    );
    fs::write(&input, b"after!").unwrap();
    build_in_target_with_env(
        &project,
        &consumer_target,
        cargo_util::paths::dylib_path_envvar(),
        inherited_path.to_str().unwrap(),
    );

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn changed_relative_dynamic_library_inputs_use_rustc_cwd() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("parent/project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let inherited_path = project.root().join("compiler-libs");
    let input = inherited_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&inherited_path).unwrap();
    fs::write(&input, b"before").unwrap();

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--manifest-path")
            .arg(project.root().join("Cargo.toml"))
            .arg("--target-dir")
            .arg(target)
            .cwd("..")
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env("CARGO_BUILD_ARTIFACT_CACHE_DIR", &cache)
            .env("CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION", "hardlink")
            .env(cargo_util::paths::dylib_path_envvar(), "compiler-libs")
            .env("CARGO_INCREMENTAL", "1")
            .run();
        fs::write(&input, b"after!").unwrap();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn changed_cargo_injected_dynamic_library_inputs_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let library_name = if cfg!(target_os = "linux") {
        "libcompiler_input.so"
    } else {
        "libcompiler_input.dylib"
    };
    let producer_input = producer_target.join("debug/deps").join(library_name);
    let consumer_input = consumer_target.join("debug/deps").join(library_name);
    fs::create_dir_all(producer_input.parent().unwrap()).unwrap();
    fs::create_dir_all(consumer_input.parent().unwrap()).unwrap();
    fs::write(&producer_input, b"before").unwrap();
    fs::write(&consumer_input, b"after!").unwrap();

    build_in_target(&project, &producer_target);
    build_in_target(&project, &consumer_target);

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn linux_hwcaps_dynamic_library_inputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let inherited_path = project.root().join("compiler-libs");
    let hwcaps_path = inherited_path.join("glibc-hwcaps/x86-64-v3");
    let input = hwcaps_path.join("libcompiler_input.so");
    fs::create_dir_all(&hwcaps_path).unwrap();
    fs::write(&input, b"input").unwrap();

    build_in_target_with_env(
        &project,
        &target,
        cargo_util::paths::dylib_path_envvar(),
        inherited_path.to_str().unwrap(),
    );

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn linux_legacy_hwcaps_dynamic_library_inputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let inherited_path = project.root().join("compiler-libs");
    let input = inherited_path.join("tls/libcompiler_input.so");
    fs::create_dir_all(input.parent().unwrap()).unwrap();
    fs::write(&input, b"input").unwrap();

    build_in_target_with_env(
        &project,
        &target,
        cargo_util::paths::dylib_path_envvar(),
        inherited_path.to_str().unwrap(),
    );

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn cargo_injected_linux_hwcaps_inputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let input = target
        .join("debug/deps/glibc-hwcaps/x86-64-v3")
        .join("libcompiler_input.so");
    fs::create_dir_all(input.parent().unwrap()).unwrap();
    fs::write(&input, b"input").unwrap();

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn unrelated_linux_loader_symlink_cycle_is_cacheable() {
    use std::os::unix::fs::symlink;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let inherited_path = project.root().join("compiler-libs");
    fs::create_dir_all(&inherited_path).unwrap();
    symlink(".", inherited_path.join("loop")).unwrap();

    build_in_target_with_env(
        &project,
        &target,
        cargo_util::paths::dylib_path_envvar(),
        inherited_path.to_str().unwrap(),
    );

    assert_eq!(cached_rlibs(&cache).len(), 1);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn changed_default_macos_dynamic_library_inputs_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let home = project.root().join("home");
    let loader_path = home.join("lib");
    let input = loader_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--target-dir")
            .arg(target)
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env_remove(cargo_util::paths::dylib_path_envvar())
            .env("HOME", &home)
            .env("CARGO_INCREMENTAL", "1")
            .run();
        fs::write(&input, b"after!").unwrap();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn changed_relative_default_macos_dynamic_library_inputs_use_rustc_cwd() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("parent/project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let loader_path = project.root().join("home/lib");
    let input = loader_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--manifest-path")
            .arg(project.root().join("Cargo.toml"))
            .arg("--target-dir")
            .arg(target)
            .cwd("..")
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env("CARGO_BUILD_ARTIFACT_CACHE_DIR", &cache)
            .env("CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION", "hardlink")
            .env_remove(cargo_util::paths::dylib_path_envvar())
            .env("HOME", "home")
            .env("CARGO_INCREMENTAL", "1")
            .run();
        fs::write(&input, b"after!").unwrap();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn changed_priority_macos_dynamic_library_inputs_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let loader_path = project.root().join("compiler-libs");
    let input = loader_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--target-dir")
            .arg(target)
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env(
                cargo_util::paths::dylib_path_envvar(),
                isolated_loader_path(),
            )
            .env("DYLD_LIBRARY_PATH", &loader_path)
            .env("CARGO_INCREMENTAL", "1")
            .run();
        fs::write(&input, b"after!").unwrap();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn changed_configured_macos_dynamic_library_inputs_keep_distinct_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let loader_path = project.root().join("compiler-libs");
    let input = loader_path.join("libcompiler_input.dylib");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"

            [env]
            DYLD_LIBRARY_PATH = "{}"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\"),
            loader_path.to_string_lossy().replace('\\', "\\\\"),
        ),
    );

    for target in [&producer_target, &consumer_target] {
        project
            .cargo("-Zartifact-cache build --lib")
            .arg("--target-dir")
            .arg(target)
            .masquerade_as_nightly_cargo(&["artifact-cache"])
            .env(
                cargo_util::paths::dylib_path_envvar(),
                isolated_loader_path(),
            )
            .env("CARGO_INCREMENTAL", "1")
            .run();
        fs::write(&input, b"after!").unwrap();
    }

    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn tokenized_macos_dynamic_loader_path_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("DYLD_LIBRARY_PATH", "@loader_path/compiler-libs")
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "macos")]
fn unmodeled_macos_dynamic_loader_override_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("DYLD_ARTIFACT_CACHE_TEST_OVERRIDE", "1")
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn unmodeled_unix_dynamic_loader_override_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("LD_ARTIFACT_CACHE_TEST_OVERRIDE", "1")
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn configured_unmodeled_unix_dynamic_loader_override_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"

            [env]
            LD_ARTIFACT_CACHE_TEST_OVERRIDE = "1"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\"),
        ),
    );

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn tokenized_linux_dynamic_loader_path_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            "$ORIGIN/compiler-libs",
        )
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(target_os = "linux")]
fn glibc_tunables_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("GLIBC_TUNABLES", "glibc.cpu.hwcaps=-AVX2")
        .env("CARGO_INCREMENTAL", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn changed_loader_input_after_compilation_is_not_published() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let loader_path = project.root().join("compiler-libs");
    let input = loader_path.join("libcompiler_input.dylib");
    let ready = project.root().join("loader-digest-ready");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&producer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(cargo_util::paths::dylib_path_envvar(), &loader_path)
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS", "5000")
        .env(
            "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE",
            &ready,
        )
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(
        ready.exists(),
        "cargo did not reach the loader-input digest test hook"
    );
    fs::write(&input, b"after!").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "producer build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(cached_rlibs(&cache).is_empty());
    build_in_target_with_env(
        &project,
        &consumer_target,
        cargo_util::paths::dylib_path_envvar(),
        loader_path.to_str().unwrap(),
    );
    assert_eq!(cached_rlibs(&cache).len(), 1);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn changed_loader_input_during_restore_forces_compile() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");
    let loader_path = project.root().join("compiler-libs");
    let input = loader_path.join("libcompiler_input.dylib");
    let ready = project.root().join("loader-restore-ready");
    fs::create_dir_all(&loader_path).unwrap();
    fs::write(&input, b"before").unwrap();

    build_in_target_with_env(
        &project,
        &producer_target,
        cargo_util::paths::dylib_path_envvar(),
        loader_path.to_str().unwrap(),
    );
    let stored = cached_rlib(&cache);

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&consumer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(cargo_util::paths::dylib_path_envvar(), &loader_path)
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_DELAY_MS", "5000")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_READY_FILE", &ready)
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(ready.exists(), "cargo did not reach the restore test hook");
    fs::write(&input, b"after!").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "consumer build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let consumer = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_ne!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&consumer).unwrap().ino()
    );
    assert_eq!(cached_rlibs(&cache).len(), 1);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn corrupt_cache_entry_is_removed_and_rebuilt() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    let stored = cached_rlib(&cache);
    fs::write(&stored, b"not a valid cached rlib").unwrap();

    build_in_target(&project, &consumer_target);

    let rebuilt = cached_rlib(&consumer_target.join("debug").join("deps"));
    let stored = cached_rlib(&cache);
    assert_ne!(fs::read(&stored).unwrap(), b"not a valid cached rlib");
    assert_ne!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&rebuilt).unwrap().ino()
    );
    assert!(!contains_cache_entry_with_marker(&cache, ".rejected-"));
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn unreadable_cache_metadata_is_removed_and_rebuilt() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    let stored = cached_rlib(&cache);
    let entry = stored.parent().unwrap().parent().unwrap();
    fs::write(entry.join("complete"), [0xff, 0xfe]).unwrap();

    build_in_target(&project, &consumer_target);

    let rebuilt = cached_rlib(&consumer_target.join("debug").join("deps"));
    let stored = cached_rlib(&cache);
    assert_eq!(fs::read(&stored).unwrap(), fs::read(&rebuilt).unwrap());
    assert!(!contains_cache_entry_with_marker(&cache, ".rejected-"));
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn cache_store_failure_does_not_fail_build() {
    let cache = paths::root().join("cache-is-a-file");
    fs::write(&cache, b"not a directory").unwrap();
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    build_in_target(&project, &target);

    assert!(cached_rlib(&target.join("debug").join("deps")).exists());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn cache_store_failure_after_staging_is_cleaned_up() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env(
            "__CARGO_TEST_ARTIFACT_CACHE_STORE_FAILURE_AFTER_STAGING",
            "1",
        )
        .run();

    assert!(!contains_cache_entry_with_marker(&cache, ".publishing-"));
    assert!(cached_rlib(&target.join("debug").join("deps")).exists());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn abandoned_transients_from_old_variant_are_cleaned_up() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let first_target = project.root().join("first-target");
    let second_target = project.root().join("second-target");

    build_in_target(&project, &first_target);
    let stored = cached_rlib(&cache);
    let entry_root = stored.parent().unwrap().parent().unwrap().parent().unwrap();
    fs::create_dir_all(
        entry_root
            .join(".old-input.publishing-abandoned")
            .join("files"),
    )
    .unwrap();
    fs::create_dir_all(
        entry_root
            .join(".old-input.rejected-abandoned")
            .join("files"),
    )
    .unwrap();
    fs::write(cache.join(".cargo-artifact-cache-size"), b"dirty\n").unwrap();

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &second_target);

    assert!(!contains_cache_entry_with_marker(&cache, ".publishing-"));
    assert!(!contains_cache_entry_with_marker(&cache, ".rejected-"));
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn retained_transient_keeps_size_state_dirty_until_cleanup_succeeds() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let first_target = project.root().join("first-target");
    let second_target = project.root().join("second-target");
    let third_target = project.root().join("third-target");

    build_in_target(&project, &first_target);
    let stored = cached_rlib(&cache);
    let entry_root = stored.parent().unwrap().parent().unwrap().parent().unwrap();
    fs::create_dir_all(
        entry_root
            .join(".old-input.rejected-abandoned")
            .join("files"),
    )
    .unwrap();
    fs::write(cache.join(".cargo-artifact-cache-size"), b"dirty\n").unwrap();

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&second_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_TRANSIENT_REMOVE_FAILURE", "1")
        .run();

    assert!(contains_cache_entry_with_marker(&cache, ".rejected-"));
    assert_eq!(
        fs::read_to_string(cache.join(".cargo-artifact-cache-size")).unwrap(),
        "dirty\n"
    );

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 44 }\n");
    build_in_target(&project, &third_target);

    assert!(!contains_cache_entry_with_marker(&cache, ".rejected-"));
    assert!(recorded_cache_size(&cache) > 0);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn max_size_evicts_old_completed_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let measuring_first_target = project.root().join("measuring-first-target");
    let measuring_second_target = project.root().join("measuring-second-target");
    let limited_first_target = project.root().join("limited-first-target");
    let limited_second_target = project.root().join("limited-second-target");

    build_in_target(&project, &measuring_first_target);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &measuring_second_target);
    let max_size = cached_rlibs(&cache)
        .iter()
        .map(|rlib| directory_size(rlib.parent().unwrap().parent().unwrap()))
        .max()
        .unwrap();
    fs::remove_dir_all(&cache).unwrap();

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "{max_size}B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 42 }\n");
    build_in_target(&project, &limited_first_target);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &limited_second_target);

    assert_eq!(cached_rlibs(&cache).len(), 1);
    assert_eq!(
        fs::read(cached_rlib(&cache)).unwrap(),
        fs::read(cached_rlib(
            &limited_second_target.join("debug").join("deps")
        ))
        .unwrap()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn lowered_max_size_is_enforced_by_a_cache_hit() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let first_target = project.root().join("first-target");
    let second_target = project.root().join("second-target");
    let restored_target = project.root().join("restored-target");

    build_in_target(&project, &first_target);
    sleep_ms(1100);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &second_target);
    let second_bytes = fs::read(cached_rlib(&second_target.join("debug").join("deps"))).unwrap();
    let newest_cached = cached_rlibs(&cache)
        .into_iter()
        .find(|rlib| fs::read(rlib).unwrap() == second_bytes)
        .unwrap();
    let max_size = cached_rlibs(&cache)
        .iter()
        .map(|rlib| directory_size(rlib.parent().unwrap().parent().unwrap()))
        .max()
        .unwrap();

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "{max_size}B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    build_in_target(&project, &restored_target);

    assert_eq!(cached_rlibs(&cache).len(), 1);
    assert!(newest_cached.exists());
    let restored = cached_rlib(&restored_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&newest_cached).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    assert!(recorded_cache_size(&cache) <= max_size);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn lowered_max_size_during_concurrent_restore_causes_a_miss_until_maintenance_can_run() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let first_target = project.root().join("first-target");
    let second_target = project.root().join("second-target");
    let delayed_target = project.root().join("delayed-target");
    let contended_target = project.root().join("contended-target");
    let restored_target = project.root().join("restored-target");
    let ready = project.root().join("restore-ready");

    build_in_target(&project, &first_target);
    sleep_ms(1100);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &second_target);
    let second_bytes = fs::read(cached_rlib(&second_target.join("debug").join("deps"))).unwrap();
    let newest_cached = cached_rlibs(&cache)
        .into_iter()
        .find(|rlib| fs::read(rlib).unwrap() == second_bytes)
        .unwrap();
    let max_size = cached_rlibs(&cache)
        .iter()
        .map(|rlib| directory_size(rlib.parent().unwrap().parent().unwrap()))
        .max()
        .unwrap();

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&delayed_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_DELAY_MS", "5000")
        .env("__CARGO_TEST_ARTIFACT_CACHE_RESTORE_READY_FILE", &ready)
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(ready.exists(), "cargo did not reach the restore test hook");

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "{max_size}B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    build_in_target(&project, &contended_target);
    let contended = cached_rlib(&contended_target.join("debug").join("deps"));
    let contended_was_restored =
        fs::metadata(&newest_cached).unwrap().ino() == fs::metadata(&contended).unwrap().ino();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "delayed restore failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !contended_was_restored,
        "a contended hit restored an entry before enforcing the lowered limit"
    );

    build_in_target(&project, &restored_target);
    assert_eq!(cached_rlibs(&cache).len(), 1);
    assert!(newest_cached.exists());
    let restored = cached_rlib(&restored_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&newest_cached).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    assert!(recorded_cache_size(&cache) <= max_size);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn unversioned_size_state_is_reconciled_before_restore() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let first_target = project.root().join("first-target");
    let second_target = project.root().join("second-target");
    let restored_target = project.root().join("restored-target");

    build_in_target(&project, &first_target);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    build_in_target(&project, &second_target);
    let max_size = cached_rlibs(&cache)
        .iter()
        .map(|rlib| directory_size(rlib.parent().unwrap().parent().unwrap()))
        .max()
        .unwrap();
    fs::write(cache.join(".cargo-artifact-cache-size"), b"1\n").unwrap();

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "{max_size}B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    build_in_target(&project, &restored_target);

    assert_eq!(cached_rlibs(&cache).len(), 1);
    assert!(recorded_cache_size(&cache) <= max_size);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn admitted_lower_limit_restore_excludes_larger_limit_publication() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let delayed_target = project.root().join("delayed-target");
    let contended_writer_target = project.root().join("contended-writer-target");
    let retried_writer_target = project.root().join("retried-writer-target");
    let ready = project.root().join("restore-admitted-ready");

    project.change_file(
        "src/lib.rs",
        r#"pub fn value() -> &'static str { env!("ARTIFACT_CACHE_TEST_VALUE") }"#,
    );
    build_in_target_with_env(
        &project,
        &producer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "alpha",
    );
    let stored = cached_rlib(&cache);
    let max_size = directory_size(stored.parent().unwrap().parent().unwrap());
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "{max_size}B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&delayed_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("ARTIFACT_CACHE_TEST_VALUE", "alpha")
        .env(
            "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_DELAY_MS",
            "5000",
        )
        .env(
            "__CARGO_TEST_ARTIFACT_CACHE_RESTORE_ADMITTED_READY_FILE",
            &ready,
        )
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(
        ready.exists(),
        "cargo did not reach the restore admission hook"
    );

    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    build_in_target_with_env(
        &project,
        &contended_writer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "bravo",
    );
    assert_eq!(
        cached_rlibs(&cache).len(),
        1,
        "a larger-limit writer published while an admitted low-limit restore held the lock"
    );

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "delayed restore failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let restored = cached_rlib(&delayed_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );

    build_in_target_with_env(
        &project,
        &retried_writer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "bravo",
    );
    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn oversized_entry_is_not_published() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            artifact-cache-max-size = "1B"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
    assert!(cached_rlib(&target.join("debug").join("deps")).exists());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn cache_key_failure_does_not_fail_build() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("__CARGO_TEST_ARTIFACT_CACHE_KEY_FAILURE", "1")
        .run();

    assert!(cached_rlibs(&cache).is_empty());
    assert!(cached_rlib(&target.join("debug").join("deps")).exists());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn cache_restore_failure_falls_back_to_compile() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-target");
    let consumer_target = project.root().join("consumer-target");

    build_in_target(&project, &producer_target);
    let cached = cached_rlib(&cache);
    let entry_root = cached
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::remove_dir_all(&entry_root).unwrap();
    fs::write(&entry_root, b"not a directory").unwrap();

    build_in_target(&project, &consumer_target);

    assert!(cached_rlib(&consumer_target.join("debug").join("deps")).exists());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn compact_native_rustflags_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let native = project.root().join("native");
    fs::create_dir_all(&native).unwrap();
    fs::write(native.join("libfoo.a"), b"!<arch>\n").unwrap();
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["-Lnative={}", "-lstatic=foo"]
            "#,
            cache.to_string_lossy().replace('\\', "\\\\"),
            native.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn mixed_library_crate_types_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        "Cargo.toml",
        r#"
        [package]
        name = "project"
        version = "0.0.1"
        edition = "2024"

        [lib]
        crate-type = ["lib", "cdylib"]
        "#,
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn compact_additional_crate_type_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache rustc --lib")
        .arg("--target-dir")
        .arg(&target)
        .arg("--")
        .arg("--crate-type=cdylib")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn additional_emit_outputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache rustc --lib")
        .arg("--target-dir")
        .arg(&target)
        .arg("--")
        .arg("--emit=link,asm")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn saved_temporary_outputs_are_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");

    project
        .cargo("-Zartifact-cache rustc --lib")
        .arg("--target-dir")
        .arg(&target)
        .arg("--")
        .arg("-Csave-temps")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .run();

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn compact_link_arg_rustflag_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["-Clink-arg=--artifact-cache-test"]
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn response_file_rustflag_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    let argfile = project.root().join("rustc.args");
    fs::write(&argfile, "--cfg\nartifact_cache_test\n").unwrap();
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["@{}"]
            "#,
            cache.to_string_lossy().replace('\\', "\\\\"),
            argfile.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn compact_extern_rustflag_is_not_cacheable() {
    let extern_rlib = build_manual_extern_dependency();
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        "src/lib.rs",
        "pub fn value() -> u32 { extern_dep::value() }\n",
    );
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["--extern=extern_dep={}"]
            "#,
            cache.to_string_lossy().replace('\\', "\\\\"),
            extern_rlib.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn pathless_extern_rustflag_is_not_cacheable() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let target = project.root().join("target-dir");
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            rustflags = ["--extern", "extern_dep"]
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );

    build_in_target(&project, &target);

    assert!(cached_rlibs(&cache).is_empty());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn distinct_package_roots_do_not_share_path_sensitive_artifacts() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let source = r#"pub fn value() -> &'static str { env!("CARGO_MANIFEST_DIR") }"#;
    let producer = project_with_cache("producer", &cache, "hardlink", 42);
    let consumer = project_with_cache("consumer", &cache, "hardlink", 42);
    producer.change_file("src/lib.rs", source);
    consumer.change_file("src/lib.rs", source);

    build(&producer);
    build(&consumer);

    assert_eq!(cached_rlibs(&cache).len(), 2);
    assert_ne!(
        fs::metadata(cached_rlib(&producer.target_debug_dir().join("deps")))
            .unwrap()
            .ino(),
        fs::metadata(cached_rlib(&consumer.target_debug_dir().join("deps")))
            .unwrap()
            .ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn source_directory_named_target_keeps_distinct_input_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");
    project.change_file(
        "src/lib.rs",
        "mod target;\npub fn value() -> u32 { target::VALUE }\n",
    );
    fs::create_dir_all(project.root().join("src/target")).unwrap();
    fs::write(
        project.root().join("src/target/mod.rs"),
        "pub const VALUE: u32 = 42;\n",
    )
    .unwrap();

    build_in_target(&project, &producer_target);
    let stored_before = fs::read(cached_rlib(&cache)).unwrap();
    fs::write(
        project.root().join("src/target/mod.rs"),
        "pub const VALUE: u32 = 43;\n",
    )
    .unwrap();
    build_in_target(&project, &consumer_target);

    assert_eq!(cached_rlibs(&cache).len(), 2);
    assert_ne!(
        stored_before,
        fs::read(cached_rlib(&consumer_target.join("debug").join("deps"))).unwrap()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn external_include_change_keeps_distinct_input_variants() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("external-input/pkg", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");
    let input = project.root().parent().unwrap().join("shared.bin");
    project.change_file(
        "src/lib.rs",
        "pub fn value() -> &'static [u8] { include_bytes!(\"../../shared.bin\") }\n",
    );
    fs::write(&input, b"before").unwrap();

    build_in_target(&project, &producer_target);
    let stored_before = fs::read(cached_rlib(&cache)).unwrap();
    fs::write(&input, b"after").unwrap();
    build_in_target(&project, &consumer_target);

    assert_ne!(
        stored_before,
        fs::read(cached_rlib(&consumer_target.join("debug").join("deps"))).unwrap()
    );
    assert_eq!(cached_rlibs(&cache).len(), 2);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn input_changed_after_compilation_is_not_published() {
    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");
    let ready = project.root().join("input-digest-ready");
    if ready.exists() {
        fs::remove_file(&ready).unwrap();
    }

    let mut command = project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&producer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .env("CARGO_INCREMENTAL", "1")
        .env("__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_DELAY_MS", "5000")
        .env(
            "__CARGO_TEST_ARTIFACT_CACHE_INPUT_DIGEST_READY_FILE",
            &ready,
        )
        .build_command();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().unwrap();
    for _ in 0..100 {
        if ready.exists() {
            break;
        }
        sleep_ms(50);
    }
    assert!(
        ready.exists(),
        "cargo did not reach the input-digest test hook"
    );
    sleep_ms(1100);
    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "producer build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(cached_rlibs(&cache).is_empty());
    build_in_target(&project, &consumer_target);
    assert_eq!(cached_rlibs(&cache).len(), 1);
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn tracked_environment_values_restore_independent_cached_variants() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");
    let original_target = project.root().join("original-value-build");
    project.change_file(
        "src/lib.rs",
        r#"pub fn value() -> &'static str { env!("ARTIFACT_CACHE_TEST_VALUE") }"#,
    );

    build_in_target_with_env(
        &project,
        &producer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "before",
    );
    let stored_before = fs::read(cached_rlib(&cache)).unwrap();
    build_in_target_with_env(
        &project,
        &consumer_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "after",
    );

    assert_eq!(cached_rlibs(&cache).len(), 2);
    build_in_target_with_env(
        &project,
        &original_target,
        "ARTIFACT_CACHE_TEST_VALUE",
        "before",
    );
    let original_cached = cached_rlibs(&cache)
        .into_iter()
        .find(|path| fs::read(path).unwrap() == stored_before)
        .unwrap();
    let restored = cached_rlib(&original_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&original_cached).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn restored_warning_is_still_denied() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("warning-project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");
    project.change_file(
        ".cargo/config.toml",
        &format!(
            r#"
            [build]
            artifact-cache-dir = "{}"
            artifact-cache-materialization = "hardlink"
            warnings = "deny"
            "#,
            cache.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    project.change_file(
        "src/lib.rs",
        "pub fn value() -> u32 { let unused = 1; 42 }\n",
    );

    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&producer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .with_status(101)
        .with_stderr_contains("[ERROR] warnings are denied by `build.warnings` configuration")
        .run();
    let stored = cached_rlib(&cache);
    project
        .cargo("-Zartifact-cache build --lib")
        .arg("--target-dir")
        .arg(&consumer_target)
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .with_status(101)
        .with_stderr_contains("[ERROR] warnings are denied by `build.warnings` configuration")
        .run();
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
#[cfg(unix)]
fn disabling_cache_still_detaches_previously_restored_hardlink() {
    use std::os::unix::fs::MetadataExt;

    let cache = paths::root().join("shared-cache");
    let project = project_with_cache("project", &cache, "hardlink", 42);
    let producer_target = project.root().join("producer-build");
    let consumer_target = project.root().join("consumer-build");

    build_in_target(&project, &producer_target);
    build_in_target(&project, &consumer_target);
    let stored = cached_rlib(&cache);
    let restored = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_eq!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&restored).unwrap().ino()
    );
    let stored_before = fs::read(&stored).unwrap();

    project.change_file("src/lib.rs", "pub fn value() -> u32 { 43 }\n");
    project
        .cargo("build --lib")
        .arg("--target-dir")
        .arg(&consumer_target)
        .env("CARGO_INCREMENTAL", "1")
        .run();

    let rebuilt = cached_rlib(&consumer_target.join("debug").join("deps"));
    assert_ne!(
        fs::metadata(&stored).unwrap().ino(),
        fs::metadata(&rebuilt).unwrap().ino()
    );
    assert_eq!(stored_before, fs::read(&stored).unwrap());
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn materialization_requires_cache_directory() {
    let p = project_in("missing-cache-directory")
        .file(
            ".cargo/config.toml",
            r#"
            [build]
            artifact-cache-materialization = "copy"
            "#,
        )
        .file("src/lib.rs", "pub fn value() -> u32 { 42 }\n")
        .build();

    p.cargo("-Zartifact-cache build --lib")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .with_status(101)
        .with_stderr_data(cargo_test_support::str![[r#"
[ERROR] build.artifact-cache-materialization requires build.artifact-cache-dir

"#]])
        .run();
}

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn max_size_requires_cache_directory() {
    let p = project_in("missing-cache-directory")
        .file(
            ".cargo/config.toml",
            r#"
            [build]
            artifact-cache-max-size = "1GB"
            "#,
        )
        .file("src/lib.rs", "pub fn value() -> u32 { 42 }\n")
        .build();

    p.cargo("-Zartifact-cache build --lib")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .env(
            cargo_util::paths::dylib_path_envvar(),
            isolated_loader_path(),
        )
        .with_status(101)
        .with_stderr_data(cargo_test_support::str![[r#"
[ERROR] build.artifact-cache-max-size requires build.artifact-cache-dir

"#]])
        .run();
}
