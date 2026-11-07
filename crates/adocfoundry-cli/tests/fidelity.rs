//! The fidelity harness: every page in `tests/fixtures/render/` is rendered by
//! `adocfoundry-cli render` and by `asciidoctor`, and the two compared after normalising both to
//! their block structure, presentation, text, links, anchors and footnotes. Everything must match
//! but the differences in `ALLOWED`, each with its reason. Skipped, with a message, when no working
//! `asciidoctor` is on the `PATH`.

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

/// How blocks are shown, where a block's attributes and options decide it: each ordered
/// list's classes and `type`, `start` and `reversed`; each table's classes and width; each
/// table cell's alignment; each image block's classes and link; each collapsible block; and
/// each font icon's classes and title.
fn presentation(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        let classes = attribute(tag, "class");
        let first = classes.split_whitespace().next().unwrap_or("");
        let shown = match name {
            "ol" => {
                let reversed = tag.split_whitespace().any(|word| word == "reversed");
                Some(format!(
                    "ol class={classes:?} type={:?} start={:?} reversed={reversed}",
                    attribute(tag, "type"),
                    attribute(tag, "start")
                ))
            }
            // A diagram is a listing to Asciidoctor without its diagram extension.
            "div" if classes.split_whitespace().any(|class| class == "diagram") => None,
            "div" if matches!(first, "olist" | "imageblock") => Some(format!("{name} {classes}")),
            "table" => Some(format!(
                "table {classes} style={:?}",
                attribute(tag, "style")
            )),
            "td" | "th" => Some(format!("{name} {classes}")),
            "a" if first == "image" => Some(format!("a image href={:?}", attribute(tag, "href"))),
            // A font icon (`icons=font`), an admonition's or an inline one.
            "i" => Some(format!("i {classes} title={:?}", attribute(tag, "title"))),
            "details" | "summary" => Some(name.to_owned()),
            _ => None,
        };
        found.extend(shown);
    }
    found
}

/// The value of `name` in an element's opening tag, or empty.
fn attribute<'a>(tag: &'a str, name: &str) -> &'a str {
    tag.split_once(&format!(" {name}=\""))
        .and_then(|(_, value)| value.split_once('"'))
        .map_or("", |(value, _)| value)
}

/// The text of every element named one of `names`, or with one of `classes` as its class, tags
/// removed. Whitespace is collapsed unless `exact`. The element ends at the first closing tag of
/// its name, so names here should be ones that do not nest in the pages compared.
fn text_of(html: &str, names: &[&str], classes: &[&str], exact: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        let class = attribute(tag, "class");
        let first_class = class.split_whitespace().next().unwrap_or("");
        if !names.contains(&name) && !(name == "div" && classes.contains(&first_class)) {
            continue;
        }
        let body = &rest[end + 1..];
        let Some(close) = body.find(&format!("</{name}>")) else {
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
        out.push(if exact {
            text
        } else {
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        });
    }
    out
}

/// The text of every `<p>`.
fn paragraphs(html: &str) -> Vec<String> {
    text_of(html, &["p"], &[], false)
}

/// The text of every heading, list term and block title.
fn headings(html: &str) -> Vec<String> {
    text_of(
        html,
        &["h1", "h2", "h3", "h4", "h5", "h6", "dt", "caption"],
        &["title"],
        false,
    )
}

/// The text of every table cell.
fn cells(html: &str) -> Vec<String> {
    text_of(html, &["th", "td"], &[], false)
}

/// The text of every `<pre>`, exactly: its spaces and line breaks are what it shows.
fn preformatted(html: &str) -> Vec<String> {
    text_of(html, &["pre"], &[], true)
}

/// Each footnote's text, from the list at the end of the page.
fn footnotes(html: &str) -> Vec<String> {
    text_of(html, &[], &["footnote"], false)
}

/// Each link, with its destination and text.
fn links(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<a ") {
        rest = &rest[start..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        let href = attribute(tag, "href");
        let body = &rest[end + 1..];
        if !href.is_empty() {
            let text = text_of(&format!("<a>{body}"), &["a"], &[], false)
                .into_iter()
                .next()
                .unwrap_or_default();
            out.push(format!("{} -> {text}", decode_entities(href)));
        }
        rest = body;
    }
    out
}

/// Every id, generated or written, in document order, but the page's own wrappers, which a whole
/// page here and an embedded one from Asciidoctor differ in.
fn anchors(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let id = attribute(&rest[..end], "id");
        if !id.is_empty() && !matches!(id, "header" | "content" | "preamble") {
            out.push(id.to_owned());
        }
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

/// Reads one kind of item from a page's HTML.
type Reader = fn(&str) -> Vec<String>;

/// What is compared, with how to read it from a page's HTML.
const COMPARED: &[(&str, Reader)] = &[
    ("block", structure),
    ("paragraph", paragraphs),
    ("heading", headings),
    ("table cell", cells),
    ("preformatted text", preformatted),
    ("footnote", footnotes),
    ("link", links),
    ("anchor", anchors),
];

/// A difference that is meant: an item of `kind` that only this renderer, only Asciidoctor, or
/// each in its own way writes, with the reason.
struct Allowed {
    page: &'static str,
    kind: &'static str,
    ours: Option<&'static str>,
    theirs: Option<&'static str>,
    #[allow(dead_code)] // Read here, by whoever wonders why.
    why: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        page: "blocks.adoc",
        kind: "preformatted text",
        ours: Some("fn main() {\n    println!(\"hi\"); (1)\n}"),
        theirs: Some("fn main() {\n    println!(\"hi\"); // (1)\n}"),
        why:
            "the comment holding a callout goes with it, as with Asciidoctor's source highlighters",
    },
    Allowed {
        page: "blocks.adoc",
        kind: "preformatted text",
        ours: None,
        theirs: Some("graph TD\n  A --> B"),
        why: "a diagram is drawn here; without its extension, Asciidoctor lists its source",
    },
];

/// `items` without the first equal to `item`; `None` when none is.
fn without(items: &mut Vec<String>, item: Option<&str>) -> bool {
    let Some(item) = item else {
        return true;
    };
    match items.iter().position(|found| found == item) {
        Some(index) => {
            items.remove(index);
            true
        }
        None => false,
    }
}

/// Every difference of `COMPARED` between two renderings of `page`, but those in `allowed`. Each
/// allowance that applied is marked in `used`, so that one that no longer does can be reported.
fn differences_against(
    page: &str,
    ours: &str,
    theirs: &str,
    allowed: &[Allowed],
    used: &mut [bool],
) -> Vec<String> {
    let mut found = Vec::new();
    for (kind, read) in COMPARED {
        let (mut our_items, mut their_items) = (read(ours), read(theirs));
        for (index, allowance) in allowed.iter().enumerate() {
            if allowance.page == page
                && allowance.kind == *kind
                && our_items
                    .iter()
                    .any(|item| Some(item.as_str()) == allowance.ours)
                    == allowance.ours.is_some()
                && their_items
                    .iter()
                    .any(|item| Some(item.as_str()) == allowance.theirs)
                    == allowance.theirs.is_some()
            {
                without(&mut our_items, allowance.ours);
                without(&mut their_items, allowance.theirs);
                used[index] = true;
            }
        }
        if our_items == their_items {
            continue;
        }
        if our_items.len() != their_items.len() {
            found.push(format!(
                "{page}: {} {kind} items here, {} from asciidoctor\n  ours:   {our_items:?}\n  theirs: {their_items:?}",
                our_items.len(),
                their_items.len()
            ));
            continue;
        }
        for (index, (our, their)) in our_items.iter().zip(&their_items).enumerate() {
            if our != their {
                found.push(format!(
                    "{page}: {kind} {index} differs\n  ours:   {our}\n  theirs: {their}"
                ));
            }
        }
    }
    found
}

#[test]
fn a_changed_word_or_a_lost_item_is_a_difference() {
    let theirs = "<h2 id=\"s\">Title</h2><div class=\"paragraph\"><p>Some words</p></div>\
        <a href=\"https://example.com\">a link</a><table class=\"tableblock\"><tr><td>cell</td></tr></table>\
        <pre>  code</pre><div class=\"footnote\" id=\"f\">a note</div>";
    let differences = |ours: &str| differences_against("page.adoc", ours, theirs, &[], &mut []);
    assert!(differences(theirs).is_empty());
    for (word, changed) in [
        ("Some words", "Some other words"),
        ("Title", "Heading"),
        ("https://example.com", "https://example.org"),
        (">a link<", ">the link<"),
        ("id=\"s\"", "id=\"t\""),
        (">cell<", ">other<"),
        ("  code", " code"),
        ("a note", "a remark"),
        ("<div class=\"paragraph\"><p>Some words</p></div>", ""),
    ] {
        assert!(theirs.contains(word), "{word}");
        let found = differences(&theirs.replace(word, changed));
        assert!(!found.is_empty(), "{word:?} to {changed:?} went unnoticed");
    }
}

#[test]
fn an_allowance_excuses_only_what_it_names() {
    let allowed = [Allowed {
        page: "page.adoc",
        kind: "paragraph",
        ours: Some("ours"),
        theirs: Some("theirs"),
        why: "a test",
    }];
    let theirs = "<p>theirs</p><p>same</p>";
    let mut used = [false];
    let found = differences_against(
        "page.adoc",
        "<p>ours</p><p>same</p>",
        theirs,
        &allowed,
        &mut used,
    );
    assert!(found.is_empty(), "{found:?}");
    assert!(used[0]);

    // On another page, or with another word, it does not apply.
    let mut used = [false];
    let found = differences_against(
        "other.adoc",
        "<p>ours</p><p>same</p>",
        theirs,
        &allowed,
        &mut used,
    );
    assert!(!found.is_empty());
    assert!(!used[0]);
    let mut used = [false];
    let found = differences_against(
        "page.adoc",
        "<p>else</p><p>same</p>",
        theirs,
        &allowed,
        &mut used,
    );
    assert!(!found.is_empty());
    assert!(!used[0]);
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
        presentation(
            "<div class=\"olist loweralpha\"><ol class=\"loweralpha\" type=\"a\" start=\"3\" reversed>\
             <table class=\"tableblock frame-none\" style=\"width: 50%;\"><td class=\"tableblock valign-top\">\
             <details><summary class=\"title\"><i class=\"fa icon-note\" title=\"Note\"></i>"
        ),
        [
            "div olist loweralpha",
            "ol class=\"loweralpha\" type=\"a\" start=\"3\" reversed=true",
            "table tableblock frame-none style=\"width: 50%;\"",
            "td tableblock valign-top",
            "details",
            "summary",
            "i fa icon-note title=\"Note\"",
        ]
    );
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
    let mut used = vec![false; ALLOWED.len()];
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
            .args(["-s", "-a", "showtitle", "-o", "-"])
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
        let (our_shown, their_shown) = (presentation(&ours), presentation(&theirs));
        if our_shown != their_shown {
            let differences: Vec<String> = our_shown
                .iter()
                .zip(&their_shown)
                .filter(|(our, their)| our != their)
                .map(|(our, their)| format!("  ours:   {our}\n  theirs: {their}"))
                .collect();
            mismatched.push(format!(
                "{name}: presentation differs ({} here, {} from asciidoctor)\n{}",
                our_shown.len(),
                their_shown.len(),
                differences.join("\n")
            ));
        }
        mismatched.extend(differences_against(
            &name, &ours, &theirs, ALLOWED, &mut used,
        ));
    }
    for (allowed, used) in ALLOWED.iter().zip(&used) {
        if !*used {
            mismatched.push(format!(
                "{}: the allowed {} difference no longer happens ({}): {:?}",
                allowed.page, allowed.kind, allowed.why, allowed.theirs
            ));
        }
    }
    assert!(mismatched.is_empty(), "{}", mismatched.join("\n"));
}
