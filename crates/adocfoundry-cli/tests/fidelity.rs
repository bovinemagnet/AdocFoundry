//! The fidelity harness: every page in `tests/fixtures/render/` is rendered by
//! `adocfoundry-cli render` and by `asciidoctor`, and the two compared after normalising both to
//! their block structure and paragraph text. Block structure must match; text differences are
//! reported. Skipped, with a message, when no working `asciidoctor` is on the `PATH`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// The first class of an element that is one of these is a block, as Asciidoctor names them.
const BLOCK_CLASSES: &[&str] = &[
    "sect1",
    "sect2",
    "sect3",
    "sect4",
    "sect5",
    "discrete",
    "paragraph",
    "ulist",
    "olist",
    "dlist",
    "qlist",
    "hdlist",
    "colist",
    "admonitionblock",
    "listingblock",
    "literalblock",
    "quoteblock",
    "verseblock",
    "sidebarblock",
    "exampleblock",
    "openblock",
    "tableblock",
    "imageblock",
    "stemblock",
    "audioblock",
    "videoblock",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn asciidoctor_available() -> bool {
    Command::new("asciidoctor")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Block kinds in document order. A diagram is compared as the listing block Asciidoctor
/// produces without its diagram extension.
fn structure(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        if !matches!(name, "div" | "table" | "h2" | "h3" | "h4" | "h5" | "h6") {
            continue;
        }
        let Some(classes) = tag
            .split_once("class=\"")
            .and_then(|(_, classes)| classes.split_once('"'))
            .map(|(classes, _)| classes)
        else {
            continue;
        };
        let mut classes = classes.split_whitespace();
        let first = classes.next().unwrap_or("");
        if BLOCK_CLASSES.contains(&first) {
            let diagram = classes.any(|class| class == "diagram");
            blocks.push(if diagram { "listingblock" } else { first }.to_owned());
        }
    }
    blocks
}

/// The text of every `<p>`, tags removed and whitespace collapsed.
fn paragraphs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<p") {
        rest = &rest[start..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        if !matches!(rest.as_bytes().get(2), Some(b'>' | b' ')) {
            rest = &rest[2..];
            continue;
        }
        let body = &rest[open_end + 1..];
        let Some(close) = body.find("</p>") else {
            break;
        };
        let mut text = String::new();
        let mut in_tag = false;
        for character in body[..close].chars() {
            match character {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => text.push(character),
                _ => {}
            }
        }
        let text = decode_entities(&text);
        out.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
        rest = &body[close..];
    }
    out
}

/// Character references as characters, so `&#169;` and `©` compare equal.
fn decode_entities(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let decoded = rest.find(';').and_then(|end| {
            let name = &rest[1..end];
            let character = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| {
                        name.strip_prefix('#')
                            .and_then(|decimal| decimal.parse().ok())
                    })
                    .and_then(char::from_u32),
            }?;
            Some((character, end))
        });
        match decoded {
            Some((character, end)) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn normalising_keeps_blocks_and_paragraph_text() {
    let html = "<div class=\"sect1\"><h2 id=\"x\">X</h2><div class=\"sectionbody\">\
        <div class=\"paragraph\"><p>Some <strong>bold</strong>\n text</p></div>\
        <div class=\"imageblock diagram\"><div class=\"content\"><img></div></div>\
        <table class=\"tableblock frame-all\"><tr><td><p class=\"tableblock\">1</p></td></tr></table>\
        <h3 class=\"discrete\">D</h3></div></div>";
    assert_eq!(
        structure(html),
        [
            "sect1",
            "paragraph",
            "listingblock",
            "tableblock",
            "discrete"
        ]
    );
    assert_eq!(paragraphs(html), ["Some bold text", "1"]);
    assert_eq!(
        decode_entities("&#169; &amp; &#x2192; &bogus"),
        "\u{a9} & \u{2192} &bogus"
    );
}

#[test]
fn fidelity_block_structure_matches_asciidoctor() {
    if !asciidoctor_available() {
        eprintln!("skipped: no working `asciidoctor` on the PATH");
        return;
    }
    let corpus = repository_root().join("tests/fixtures/render");
    let mut pages: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "adoc")
        })
        .collect();
    pages.sort();
    assert!(!pages.is_empty());

    let mut mismatched = Vec::new();
    for page in &pages {
        let ours = Command::new(env!("CARGO_BIN_EXE_adocfoundry-cli"))
            .arg("render")
            .arg(page)
            .output()
            .unwrap();
        assert!(
            ours.status.success(),
            "{}",
            String::from_utf8_lossy(&ours.stderr)
        );
        let theirs = Command::new("asciidoctor")
            .args(["-s", "-o", "-"])
            .arg(page)
            .output()
            .unwrap();
        assert!(
            theirs.status.success(),
            "{}",
            String::from_utf8_lossy(&theirs.stderr)
        );
        let ours = String::from_utf8(ours.stdout).unwrap();
        let theirs = String::from_utf8(theirs.stdout).unwrap();

        let name = page.file_name().unwrap().to_string_lossy();
        let (our_blocks, their_blocks) = (structure(&ours), structure(&theirs));
        if our_blocks != their_blocks {
            mismatched.push(format!(
                "{name}: block structure differs\n  ours:   {our_blocks:?}\n  theirs: {their_blocks:?}"
            ));
        }
        let (our_text, their_text) = (paragraphs(&ours), paragraphs(&theirs));
        for (index, (our, their)) in our_text.iter().zip(&their_text).enumerate() {
            if our != their {
                eprintln!(
                    "{name}: paragraph {index} text differs\n  ours:   {our}\n  theirs: {their}"
                );
            }
        }
        if our_text.len() != their_text.len() {
            eprintln!(
                "{name}: {} paragraphs here, {} from asciidoctor",
                our_text.len(),
                their_text.len()
            );
        }
    }
    assert!(mismatched.is_empty(), "{}", mismatched.join("\n"));
}
