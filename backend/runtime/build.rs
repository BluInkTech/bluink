fn main() {
    println!("cargo:rerun-if-env-changed=BLUINK_NATIVE_BUILD_INFO");
    let info = std::env::var("BLUINK_NATIVE_BUILD_INFO")
        .unwrap_or_else(|_| "{\"sourceDigest\":\"unverified\"}".to_owned());
    println!("cargo:rustc-env=BLUINK_NATIVE_BUILD_INFO={info}");
    napi_build::setup();
}
