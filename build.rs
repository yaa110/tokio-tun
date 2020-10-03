use std::env;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
fn get_wrapper_path() -> &'static str {
    "include/linux/wrapper.h"
}

#[cfg(not(any(target_os = "linux")))]
fn get_wrapper_path() -> &'static str {
    unimplemented!()
}

fn main() {
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    let bindings = bindgen::Builder::default()
        .header(get_wrapper_path())
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("unable to write bindings");
}
