use paradl_platform::{Platform, current};
use std::path::Path;

#[test]
fn current_is_a_usable_platform_in_every_configuration() {
    let p = current(); // &'static dyn Platform
    assert!(
        p.set_origin_metadata(Path::new("/definitely/not/a/file"), &[])
            .is_ok()
    );
    // Use an account that cannot exist in any real keychain: the brief's
    // original `"hf"` account is seeded by other tests on macOS hosts, which
    // would make this interface test host-dependent (and racy in a full run).
    assert_eq!(
        p.load_secret("pdl", "c1-current-absent-account")
            .expect("load")
            .is_none(),
        true
    );
}
