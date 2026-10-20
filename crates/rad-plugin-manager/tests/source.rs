use rad_plugin_manager::{Error, GithubSource};

#[test]
fn rejects_plain_http_before_any_transport_or_storage() {
    assert!(matches!(
        GithubSource::parse("http://github.com/example/plugin/releases/tag/v1"),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn accepts_public_repository_and_pinned_release_sources() {
    assert!(GithubSource::parse("https://github.com/KroxiDev/rad-plugin-fixture").is_ok());
    assert!(
        GithubSource::parse("https://github.com/KroxiDev/rad-plugin-fixture/releases/tag/v1.0.0")
            .is_ok()
    );
}

#[test]
fn github_repository_shortcuts_preserve_exact_origin() {
    for suffix in [
        ".git",
        "/releases",
        "/releases/latest",
        ".git/releases/latest",
    ] {
        assert!(
            rad_plugin_manager::GithubSource::parse(&format!(
                "https://github.com/owner/repo{suffix}"
            ))
            .is_ok(),
            "{suffix}"
        );
    }
    for source in [
        "https://github.com.evil/o/r/releases",
        "https://github.com/o/r/issues",
        "https://github.com/o/.git",
        "https://github.com/o/r/releases/latest/extra",
    ] {
        assert!(rad_plugin_manager::GithubSource::parse(source).is_err());
    }
}
