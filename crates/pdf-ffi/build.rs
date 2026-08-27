fn main() {
    println!("cargo:rerun-if-changed=tests/c/compose_invoice.c");
    println!("cargo:rerun-if-changed=tests/c/header_surface.c");
    println!("cargo:rerun-if-changed=include/prismpdf.h");

    // The C acceptance objects are linked into the unit-test binary only, so they are built only
    // when the `c-acceptance` feature asks for them. CI enables it everywhere (`--all-features`),
    // which is where the header is compiled by a real C compiler and the invoice journey is
    // exercised from C. Leaving it off by default keeps a plain `cargo build -p prismpdf-ffi`
    // free of any C toolchain requirement — the release pipeline cross-compiles this crate to
    // sixteen targets (`docs/native-artifacts.md`), and a build script that needs a matching
    // cross C compiler for each one would be the only reason it did.
    if std::env::var_os("CARGO_FEATURE_C_ACCEPTANCE").is_none() {
        return;
    }

    cc::Build::new()
        .file("tests/c/compose_invoice.c")
        .file("tests/c/header_surface.c")
        .include("include")
        .warnings(true)
        .extra_warnings(true)
        .compile("prismpdf_c_acceptance");
}
