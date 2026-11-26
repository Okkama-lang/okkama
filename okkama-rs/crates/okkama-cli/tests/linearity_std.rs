//! #972: the ownership linearity check (OKKAMA_VERIFY_OWNERSHIP=1) settles on
//! the std functions with the most resources. A fixed bound of 64 visits per
//! block reported "the data flow does not settle" for these three modules.
mod tree;
use std::path::Path;

#[test]
fn http_modules_with_many_resources_pass_the_linearity_check() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for module in ["std/http/redirect.ok", "std/http/cache.ok", "std/http/http2_server.ok"] {
        let output = tree::okkama()
            .args(["check", root.join(module).to_str().unwrap()])
            .env("OKKAMA_VERIFY_OWNERSHIP", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{module}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
