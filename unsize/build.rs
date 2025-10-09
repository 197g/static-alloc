fn main() {
    let ac = autocfg::new();
    ac.emit_rustc_version(1, 51);
    // support for Weak::into_raw
    ac.emit_rustc_version(1, 45);
}
