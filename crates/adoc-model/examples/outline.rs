//! Prints a page's model: each node, whether it is editable, and its text as shown.

use adoc_dom::{AcdcParser, DocumentParser, ParseInput};
use adoc_model::{Access, Model};

fn main() {
    let path = std::path::PathBuf::from(std::env::args().nth(1).expect("a page"));
    let text = std::fs::read_to_string(&path).expect("readable");
    let dom = AcdcParser.parse(&ParseInput::new(&path, &text));
    let model = Model::build(&path, &text, &dom);
    for (path, node) in model.walk() {
        let access = match &node.access {
            Access::Editable => "edit".to_owned(),
            Access::ReadOnly(why) => format!("{why:?}")
                .split([' ', '{'])
                .next()
                .unwrap_or("")
                .to_owned(),
        };
        let shown = node
            .text
            .as_ref()
            .map(|text| format!("{:?}", text.display))
            .unwrap_or_default();
        println!(
            "{:indent$}{:?} {}..{} [{access}] {shown}",
            "",
            node.kind,
            node.range.start,
            node.range.end,
            indent = path.len() * 2
        );
    }
}
