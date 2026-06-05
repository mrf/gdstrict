//! Format .gd files and print the result: `cargo run -p gdstrict-format --example fmt -- <file>...`
fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: fmt <file.gd>...");
        std::process::exit(2);
    }
    for f in files {
        let src = std::fs::read_to_string(&f).expect("read source file");
        print!("{}", gdstrict_format::format(&src));
    }
}
