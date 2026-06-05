//! Dump the s-expression of a .gd file: `cargo run -p gdstrict-syntax --example sexp -- <file>`
fn main() {
    let path = std::env::args().nth(1).expect("usage: sexp <file.gd>");
    let src = std::fs::read_to_string(&path).unwrap();
    let tree = gdstrict_syntax::parse(&src);
    println!("{}", tree.root_node().to_sexp());
    let defects = gdstrict_syntax::defects(&tree);
    eprintln!("defects: {}", defects.len());
    for d in defects {
        eprintln!("  {d:?}");
    }
}
