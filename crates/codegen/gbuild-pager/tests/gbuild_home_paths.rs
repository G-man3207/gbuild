//! `GBUILD_HOME` override tests in an isolated binary so `gbuild_home()`'s
//! process-wide `OnceLock` initializes from the overridden env var.

use std::path::PathBuf;

#[test]
fn gbuild_home_override_path_helpers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let gbuild_home = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("GBUILD_HOME", &gbuild_home);
    }

    assert_eq!(
        gbuild_pager::util::pager_toml_path(),
        gbuild_home.join("pager.toml")
    );
    assert_eq!(
        gbuild_pager::util::display_gbuild_home_prefix(),
        "$GBUILD_HOME"
    );
    assert_eq!(
        gbuild_pager::util::display_user_gbuild_path("config.toml"),
        "$GBUILD_HOME/config.toml"
    );

    let memory_path = gbuild_home.join("memory/MEMORY.md");
    assert_eq!(
        gbuild_pager::util::abbreviate_path(&memory_path.display().to_string()),
        "$GBUILD_HOME/memory/MEMORY.md"
    );

    // Copy-toast paths follow the same abbreviation convention, so a custom
    // $GBUILD_HOME outside $HOME still displays short.
    assert_eq!(
        gbuild_pager::clipboard::display_copy_path(&gbuild_home.join("last-copy.txt")),
        "$GBUILD_HOME/last-copy.txt"
    );

    assert!(gbuild_pager::util::is_under_user_gbuild_home(&memory_path));
    assert!(!gbuild_pager::util::is_under_user_gbuild_home(
        PathBuf::from("/tmp/other").as_path()
    ));
}
