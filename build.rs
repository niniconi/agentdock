// Build-time shell completion generation.
//
// The completions are produced from the same derive definitions the binary
// uses, so they cannot describe a command that no longer exists. They land in
// OUT_DIR for packaging to pick up; see the note on the install path below.

use clap::{CommandFactory, ValueEnum};
use clap_complete::{Generator, Shell, generate_to};
use std::env;
use std::io::Error;

// `args.rs` imports only clap and std, so it can be pulled in here without the
// rest of the crate. Deriving `Parser` gives the `command()` constructor that
// walks the whole tree.
include!("src/cli/args.rs");

fn main() -> Result<(), Error> {
    println!("cargo:rerun-if-changed=src/cli/args.rs");

    let outdir = match env::var_os("OUT_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => return Ok(()),
    };
    let completions = outdir.join("completions");
    std::fs::create_dir_all(&completions)?;

    let mut cmd = Cli::command();
    for shell in Shell::value_variants() {
        generate_to(*shell, &mut cmd, "agentdock", &completions)?;
    }

    // Packaging needs a stable path rather than a hashed one, so mirror the
    // tree where a deb or rpm build can pick it up.
    let published = PathBuf::from("target/completions");
    std::fs::create_dir_all(&published)?;
    for shell in Shell::value_variants() {
        let file = shell.file_name("agentdock");
        let src = completions.join(&file);
        let dst = published.join(&file);
        std::fs::copy(&src, &dst)?;
    }

    Ok(())
}
