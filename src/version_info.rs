const CI_TAG: &str = env!("GOVEE_CI_TAG");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn govee_version() -> &'static str {
    if CI_TAG.is_empty() {
        PKG_VERSION
    } else {
        CI_TAG
    }
}
