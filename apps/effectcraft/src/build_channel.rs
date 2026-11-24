//! The build-time nightly identity shared by the build script and its pure regression tests.
pub fn is_nightly_version(version: &str) -> bool {
    let Some((base, suffix)) = version.split_once("-nightly.") else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let base: Vec<_> = base.split('.').collect();
    let parts: Vec<_> = suffix.split('.').collect();
    base.len() == 3
        && base.iter().all(|s| digits(s))
        && parts.len() == 4
        && parts[0].len() == 8
        && digits(parts[0])
        && digits(parts[1])
        && digits(parts[2])
        && parts[3].strip_prefix('g').is_some_and(|sha| sha.len() == 12 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
/// The prepared version, source commit and run date must agree: the version names this commit and
/// the original run's UTC date (the packaging scripts check the same pair).
pub fn identity_matches(version: &str, sha: &str, date: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !is_nightly_version(version) || sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return false;
    }
    if !version.ends_with(&format!(".g{}", &sha[..12])) {
        return false;
    }
    let Some((suffix, _)) = version.split_once("-nightly.") else { return false };
    let date_digits = suffix.split('.').next().unwrap_or("");
    date.len() == 10 && digits(date_digits) && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-' && date.replace('-', "") == date_digits
}

pub fn windows_icon_path(nightly: bool) -> &'static str {
    if nightly { "../../assets/app-icon/effectcraft-nightly.ico" } else { "../../assets/app-icon/effectcraft.ico" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_resource_uses_the_flavor_specific_icon_without_changing_stable() {
        assert_eq!(windows_icon_path(false), "../../assets/app-icon/effectcraft.ico");
        assert_eq!(windows_icon_path(true), "../../assets/app-icon/effectcraft-nightly.ico");
    }

    #[test]
    fn explicit_feed_identity_is_required_for_a_nightly_build() {
        assert!(is_nightly_version("0.23.1-nightly.20261120.17.1.gabcdef012345"));
        for invalid in [
            "",
            "0.23.1",
            "0.23.1-nightly",
            "0.23.1-nightly.20261120",
            "0.23.1-nightly.20261120.17.1.gABCDEF012345",
            "0.23.1-nightly.20261120.17.1.gabcdef012345\n",
        ] {
            assert!(!is_nightly_version(invalid), "{invalid:?}");
        }
    }
    #[test]
    fn the_prepared_identity_requires_a_matching_commit_and_run_date() {
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        let version = "0.25.1-nightly.20261124.41.1.gabcdef012345";
        assert!(identity_matches(version, sha, "2026-11-24"));
        for (v, s, d) in [
            (version, "abcdef0123456789abcdef0123456789abcdef02", "2026-11-24"),
            (version, sha, "2026-11-23"),
            (
                version,
                sha,
                "2026-11-24
",
            ),
            ("0.25.1-nightly.20261124.41.1.gABCDEF012345", sha, "2026-11-24"),
            ("0.25.1", sha, "2026-11-24"),
            (version, "abc", "2026-11-24"),
        ] {
            assert!(!identity_matches(v, s, d), "{v} {s} {d}");
        }
    }

    #[test]
    fn version_cannot_inject_a_path_or_an_extra_build_directive() {
        for invalid in [
            "../0.23.1-nightly.20261120.1.1.gabcdef012345",
            "0.23.1-nightly.20261120.1.1.gabcdef012345/other",
            "0.23.1-nightly.20261120.1.1.gabcdef012345\ncargo:rustc-cfg=other",
        ] {
            assert!(!is_nightly_version(invalid));
        }
    }
}
