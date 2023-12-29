fn main() {
    let mut ci_tag = String::new();

    if let Ok(env) = std::env::var("GOVEE_CI_TAG") {
        ci_tag = env.trim().to_string();
    } else if let Ok(tag) = std::fs::read(".tag") {
        if let Ok(s) = String::from_utf8(tag) {
            ci_tag = s.trim().to_string();
        }
    } else if let Ok(output) = std::process::Command::new("git")
        .args(&[
            "-c",
            "core.abbrev=8",
            "show",
            "-s",
            "--format=%cd-%h",
            "--date=format:%Y.%m.%d",
        ])
        .output()
    {
        let info = String::from_utf8_lossy(&output.stdout);
        ci_tag = info.trim().to_string();
    }

    println!("cargo:rerun-if-changed=.tag");
    println!("cargo:rustc-env=GOVEE_CI_TAG={ci_tag}");
}
