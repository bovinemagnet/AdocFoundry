use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    let dom = AcdcParser.parse(&ParseInput::new(&path, &text));
    for block in dom.walk() {
        println!(
            "{:<18} line {:>3} {}",
            block.kind_name(),
            block.meta.span.line,
            block.meta.span.path.display()
        );
    }
    for warning in &dom.warnings {
        println!("WARN {}", warning.message);
    }
}
