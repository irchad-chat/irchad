fn main() {
    // libwebrtc's desktop capture (X11 here) brings its portal code along,
    // which calls into GIO; webrtc-sys only compiles against the headers.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for lib in ["gio-2.0", "gobject-2.0", "glib-2.0"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
