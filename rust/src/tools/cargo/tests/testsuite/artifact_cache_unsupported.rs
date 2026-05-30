//! Tests for the unsupported-host `-Zartifact-cache` boundary.

use crate::prelude::*;
use cargo_test_support::{paths, prelude::*, project};

#[cargo_test(nightly, reason = "-Zartifact-cache is unstable")]
fn configured_cache_is_not_published_on_unsupported_hosts() {
    let cache = paths::root().join("shared-cache");
    let p = project()
        .file(
            ".cargo/config.toml",
            &format!(
                r#"
                [build]
                artifact-cache-dir = "{}"
                "#,
                cache.display()
            ),
        )
        .file("src/lib.rs", "pub fn value() -> u32 { 42 }\n")
        .build();

    p.cargo("-Zartifact-cache build --lib")
        .masquerade_as_nightly_cargo(&["artifact-cache"])
        .run();

    assert!(
        !cache.exists(),
        "unsupported hosts must not publish restorable artifacts"
    );
}
