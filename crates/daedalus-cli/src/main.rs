fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut lib = daedalus_core::frontend::Library::new();
    let text = std::fs::read_to_string(&args[1]).unwrap();
    let errs = lib.set_document(&args[1], &text);
    for e in errs { eprintln!("{e}"); }
    let t = std::time::Instant::now();
    match lib.flatten(&args[2]) {
        Ok(m) => {
            eprintln!("flatten {:?}", t.elapsed());
            for w in &m.warnings { eprintln!("warn: {w}"); }
            match daedalus_core::backend::compile(m, &Default::default()) {
                Ok(s) => { println!("{}", s.dump()); for w in &s.warnings { eprintln!("warn: {w}"); } }
                Err(e) => eprintln!("error: {e:#}"),
            }
            eprintln!("total {:?}", t.elapsed());
        }
        Err(e) => eprintln!("error: {e:#}"),
    }
}
