use {
    std::{
        env,
        fs::File,
        io::prelude::*,
        path::Path,
    },
    git2::Repository,
    itertools::Itertools as _,
};

fn main() {
    println!("cargo:rerun-if-changed=nonexistent.foo"); // check a nonexistent file to make sure build script is always run (see https://github.com/rust-lang/cargo/issues/4213 and https://github.com/rust-lang/cargo/issues/5663)
    let mut f = File::create(Path::new(&env::var_os("OUT_DIR").unwrap()).join("version.rs")).unwrap();
    let commit_hash = Repository::open(&env::var_os("CARGO_MANIFEST_DIR").unwrap()).unwrap().head().unwrap().peel_to_commit().unwrap().id();
    writeln!(&mut f, "pub const GIT_COMMIT_HASH: [u8; 20] = [{:#x}];", commit_hash.as_bytes().iter().format(", ")).unwrap();
}
