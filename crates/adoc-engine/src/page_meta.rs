//! A page's metadata file (`docs/prd/PRD-AdocFoundry_Comments.md` §5): `install.adoc` has
//! `.install.adoc.afd` beside it, a TOML file whose `[[comment]]` tables are the page's
//! comments. The leading dot keeps it out of Antora's catalog and publishing.
//!
//! The file may come from a repository someone else controls, so it is read as untrusted: held to
//! [`MAX_FILE`] and the other limits before anything else, never panicking, and a file this
//! version cannot use is reported and never rewritten. Whatever it holds that this version does
//! not know, at any level, is kept as it is when the file is written again, so later kinds of
//! metadata, and other tools' data, survive. The same limits hold when writing, so a file the
//! engine writes is always one it reads.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

/// The extension of a page's metadata file.
pub const SIDECAR_EXTENSION: &str = "afd";
/// The version of the file this engine writes.
pub const FORMAT: i64 = 1;
/// The largest metadata file read or written.
pub const MAX_FILE: usize = 4 * 1024 * 1024;
/// The most comments a page holds.
pub const MAX_COMMENTS: usize = 500;
/// The most replies a comment holds.
pub const MAX_REPLIES: usize = 100;
/// The longest text of a comment or reply, in bytes.
pub const MAX_TEXT: usize = 8 * 1024;
/// The longest author's name, in bytes: as long as a participant's name in a session.
pub const MAX_AUTHOR: usize = 64;
/// The longest e-mail address, in bytes.
pub const MAX_EMAIL: usize = 254;
/// The longest quoted text kept, in bytes: a longer range is found by its first part.
pub const MAX_QUOTE: usize = 4 * 1024;
/// The most text kept on either side of a comment's range, in bytes.
pub const MAX_CONTEXT: usize = 32;
/// The longest page revision read.
const MAX_REVISION: usize = 128;

const HEADER: &str = "# AdocFoundry page metadata\n";

/// The metadata file of the page at `page`: `.` + its file name + `.afd`, in its folder.
#[must_use]
pub fn sidecar_path(page: &Path) -> Option<PathBuf> {
    let name = page.file_name()?.to_str()?;
    Some(page.with_file_name(format!(".{name}.{SIDECAR_EXTENSION}")))
}

/// The page whose metadata file is at `path`, if `path` is one: the inverse of [`sidecar_path`].
#[must_use]
pub fn page_of_sidecar(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let page = name
        .strip_prefix('.')?
        .strip_suffix(SIDECAR_EXTENSION)?
        .strip_suffix('.')?;
    let page = path.with_file_name(page);
    (!page.as_os_str().is_empty() && adoc_index::is_asciidoc_path(&page)).then_some(page)
}

/// The revision a comment's offsets refer to: the SHA-256 of the page's text.
#[must_use]
pub fn revision_of(text: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(text.as_bytes())))
}

/// The SHA-256 of a file's bytes, which a write expects to find on disk.
#[must_use]
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// The quoted text of `text[start..end]` and the text either side of it, each cut at a
/// character boundary to its limit: the quote keeps its first [`MAX_QUOTE`] bytes, the text
/// before its last [`MAX_CONTEXT`], the text after its first.
#[must_use]
pub fn context(text: &str, start: usize, end: usize) -> (String, String, String) {
    let quote_end = floor_boundary(text, end.min(start + MAX_QUOTE));
    let before_start = ceil_boundary(text, start.saturating_sub(MAX_CONTEXT));
    let after_end = floor_boundary(text, (end + MAX_CONTEXT).min(text.len()));
    (
        text.get(start..quote_end).unwrap_or_default().to_owned(),
        text.get(before_start..start).unwrap_or_default().to_owned(),
        text.get(end..after_end).unwrap_or_default().to_owned(),
    )
}

fn floor_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

/// `text` cut to at most `limit` bytes at a character boundary.
#[must_use]
pub fn cut(text: &str, limit: usize) -> &str {
    &text[..floor_boundary(text, limit)]
}

/// A comment as the file keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredComment {
    pub id: u64,
    pub author: String,
    pub email: Option<String>,
    pub created: SystemTime,
    pub edited: Option<SystemTime>,
    /// Byte offsets into the text `revision` names.
    pub start: usize,
    pub end: usize,
    pub revision: String,
    pub quote: String,
    pub before: String,
    pub after: String,
    pub text: String,
    pub resolved: Option<Resolution>,
    /// Its text was not found: `quote` is what it was on.
    pub detached: bool,
    pub replies: Vec<StoredReply>,
}

/// A reply to a comment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredReply {
    pub id: u64,
    pub author: String,
    pub email: Option<String>,
    pub created: SystemTime,
    pub edited: Option<SystemTime>,
    pub text: String,
}

/// Who resolved a comment, and when.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub by: String,
    pub at: SystemTime,
}

/// Why a metadata file cannot be used, or a change to it cannot be written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetaProblem {
    /// Larger than [`MAX_FILE`].
    TooLarge,
    /// It holds a Git merge conflict.
    Conflict,
    /// Not TOML, or not UTF-8.
    NotToml(String),
    /// TOML, but not a metadata file this version reads.
    Invalid(String),
    /// Written by a newer version, in a shape this one does not read.
    NewerFormat(i64),
    /// Writing would take the page past [`MAX_COMMENTS`] or a comment past [`MAX_REPLIES`],
    /// or the file past [`MAX_FILE`].
    TooMany,
    /// A comment or reply longer than [`MAX_TEXT`].
    TooLong,
}

impl fmt::Display for MetaProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => write!(
                formatter,
                "The comments file is larger than {} MiB.",
                MAX_FILE / (1024 * 1024)
            ),
            Self::Conflict => formatter.write_str(
                "The comments file has a merge conflict; resolve it to see this page's comments.",
            ),
            Self::NotToml(problem) => write!(formatter, "The comments file is not TOML: {problem}"),
            Self::Invalid(problem) => {
                write!(formatter, "The comments file cannot be read: {problem}")
            }
            Self::NewerFormat(format) => write!(
                formatter,
                "The comments file was written by a newer version of AdocFoundry (format {format})."
            ),
            Self::TooMany => formatter.write_str("This page has as many comments as it can hold."),
            Self::TooLong => formatter.write_str("This comment is too long."),
        }
    }
}

/// A metadata file as read: the TOML document, kept for what this version does not know, and
/// its comments.
#[derive(Clone, Debug)]
pub struct PageMeta {
    document: DocumentMut,
    pub comments: Vec<StoredComment>,
    /// The file's `format`, when newer than [`FORMAT`]: read, but never to be written.
    pub newer: Option<i64>,
}

impl Default for PageMeta {
    fn default() -> Self {
        let document: DocumentMut = format!("{HEADER}format = {FORMAT}\n")
            .parse()
            .expect("the header is TOML");
        Self {
            document,
            comments: Vec::new(),
            newer: None,
        }
    }
}

/// Reads a metadata file's bytes.
pub fn parse(bytes: &[u8]) -> Result<PageMeta, MetaProblem> {
    if bytes.len() > MAX_FILE {
        return Err(MetaProblem::TooLarge);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| MetaProblem::NotToml("it is not UTF-8".to_owned()))?;
    if text
        .lines()
        .any(|line| line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>"))
    {
        return Err(MetaProblem::Conflict);
    }
    let document: DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| MetaProblem::NotToml(error.message().to_owned()))?;
    let format = document
        .get("format")
        .and_then(Item::as_integer)
        .ok_or_else(|| invalid("it has no format"))?;
    let read = read_comments(&document);
    match (format, read) {
        (FORMAT, Ok(comments)) => Ok(PageMeta {
            document,
            comments,
            newer: None,
        }),
        (format, Ok(comments)) if format > FORMAT => Ok(PageMeta {
            document,
            comments,
            newer: Some(format),
        }),
        (format, Err(_)) if format > FORMAT => Err(MetaProblem::NewerFormat(format)),
        (format, _) if format < 1 => Err(invalid(&format!("format {format} is not one"))),
        (_, Err(problem)) => Err(problem),
        (format, Ok(_)) => Err(invalid(&format!("format {format} is not one"))),
    }
}

fn invalid(problem: &str) -> MetaProblem {
    MetaProblem::Invalid(problem.to_owned())
}

fn read_comments(document: &DocumentMut) -> Result<Vec<StoredComment>, MetaProblem> {
    let Some(item) = document.get("comment") else {
        return Ok(Vec::new());
    };
    let tables = item
        .as_array_of_tables()
        .ok_or_else(|| invalid("`comment` is not a list of tables"))?;
    if tables.len() > MAX_COMMENTS {
        return Err(invalid(&format!(
            "it has more than {MAX_COMMENTS} comments"
        )));
    }
    let mut ids = BTreeSet::new();
    let mut comments = Vec::with_capacity(tables.len());
    for table in tables {
        let comment = read_comment(table)?;
        for id in std::iter::once(comment.id).chain(comment.replies.iter().map(|reply| reply.id)) {
            if !ids.insert(id) {
                return Err(invalid(&format!("the id {id:016x} is used twice")));
            }
        }
        comments.push(comment);
    }
    Ok(comments)
}

fn read_comment(table: &Table) -> Result<StoredComment, MetaProblem> {
    let start = offset(table, "start")?;
    let end = offset(table, "end")?;
    if start > end {
        return Err(invalid("a comment ends before it starts"));
    }
    let replies = match table.get("reply") {
        None => Vec::new(),
        Some(item) => {
            let tables = item
                .as_array_of_tables()
                .ok_or_else(|| invalid("`reply` is not a list of tables"))?;
            if tables.len() > MAX_REPLIES {
                return Err(invalid(&format!(
                    "a comment has more than {MAX_REPLIES} replies"
                )));
            }
            tables.iter().map(read_reply).collect::<Result<_, _>>()?
        }
    };
    let resolved = match table.get("resolved").map(|item| item.as_bool()) {
        None | Some(Some(false)) => None,
        Some(Some(true)) => Some(Resolution {
            by: optional_text(table, "resolved_by", MAX_AUTHOR)?.unwrap_or_default(),
            at: optional_time(table, "resolved_at")?.unwrap_or(UNIX_EPOCH),
        }),
        Some(None) => return Err(invalid("`resolved` is not true or false")),
    };
    let detached = match table.get("detached").map(|item| item.as_bool()) {
        None => false,
        Some(Some(detached)) => detached,
        Some(None) => return Err(invalid("`detached` is not true or false")),
    };
    Ok(StoredComment {
        id: id(table)?,
        author: text(table, "author", MAX_AUTHOR)?,
        email: optional_text(table, "email", MAX_EMAIL)?,
        created: time(table, "created")?,
        edited: optional_time(table, "edited")?,
        start,
        end,
        revision: text(table, "revision", MAX_REVISION)?,
        quote: text(table, "quote", MAX_QUOTE)?,
        before: optional_text(table, "before", MAX_CONTEXT)?.unwrap_or_default(),
        after: optional_text(table, "after", MAX_CONTEXT)?.unwrap_or_default(),
        text: text(table, "text", MAX_TEXT)?,
        resolved,
        detached,
        replies,
    })
}

fn read_reply(table: &Table) -> Result<StoredReply, MetaProblem> {
    Ok(StoredReply {
        id: id(table)?,
        author: text(table, "author", MAX_AUTHOR)?,
        email: optional_text(table, "email", MAX_EMAIL)?,
        created: time(table, "created")?,
        edited: optional_time(table, "edited")?,
        text: text(table, "text", MAX_TEXT)?,
    })
}

fn id(table: &Table) -> Result<u64, MetaProblem> {
    let text = text(table, "id", 16)?;
    if text.len() != 16 {
        return Err(invalid("an id is not 16 hexadecimal digits"));
    }
    u64::from_str_radix(&text, 16).map_err(|_| invalid("an id is not 16 hexadecimal digits"))
}

fn offset(table: &Table, key: &str) -> Result<usize, MetaProblem> {
    table
        .get(key)
        .and_then(Item::as_integer)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid(&format!("`{key}` is not an offset")))
}

fn text(table: &Table, key: &str, limit: usize) -> Result<String, MetaProblem> {
    optional_text(table, key, limit)?.ok_or_else(|| invalid(&format!("`{key}` is missing")))
}

fn optional_text(table: &Table, key: &str, limit: usize) -> Result<Option<String>, MetaProblem> {
    let Some(item) = table.get(key) else {
        return Ok(None);
    };
    let value = item
        .as_str()
        .ok_or_else(|| invalid(&format!("`{key}` is not text")))?;
    if value.len() > limit {
        return Err(invalid(&format!("`{key}` is longer than {limit} bytes")));
    }
    Ok(Some(value.to_owned()))
}

fn time(table: &Table, key: &str) -> Result<SystemTime, MetaProblem> {
    optional_time(table, key)?.ok_or_else(|| invalid(&format!("`{key}` is missing")))
}

fn optional_time(table: &Table, key: &str) -> Result<Option<SystemTime>, MetaProblem> {
    let Some(item) = table.get(key) else {
        return Ok(None);
    };
    let datetime = item
        .as_datetime()
        .ok_or_else(|| invalid(&format!("`{key}` is not a time")))?;
    from_datetime(datetime)
        .map(Some)
        .ok_or_else(|| invalid(&format!("`{key}` is not a time with a date and offset")))
}

/// Days from 1970-01-01 to `year-month-day`, by the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of the day `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u8, day as u8)
}

fn from_datetime(datetime: &toml_edit::Datetime) -> Option<SystemTime> {
    let date = datetime.date?;
    let time = datetime.time?;
    let offset_minutes = match datetime.offset? {
        toml_edit::Offset::Z => 0,
        toml_edit::Offset::Custom { minutes } => i64::from(minutes),
    };
    if !(1..=12).contains(&date.month) || !(1..=31).contains(&date.day) {
        return None;
    }
    let days = days_from_civil(i64::from(date.year), date.month, date.day);
    let seconds = days * 86_400
        + i64::from(time.hour) * 3_600
        + i64::from(time.minute) * 60
        + i64::from(time.second.unwrap_or(0))
        - offset_minutes * 60;
    let seconds = u64::try_from(seconds).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

fn to_datetime(time: SystemTime) -> toml_edit::Datetime {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let seconds = i64::try_from(seconds).unwrap_or(0);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let in_day = seconds.rem_euclid(86_400);
    toml_edit::Datetime {
        date: Some(toml_edit::Date {
            year: u16::try_from(year).unwrap_or(9999),
            month,
            day,
        }),
        time: Some(toml_edit::Time {
            hour: (in_day / 3_600) as u8,
            minute: (in_day % 3_600 / 60) as u8,
            second: Some((in_day % 60) as u8),
            nanosecond: None,
        }),
        offset: Some(toml_edit::Offset::Z),
    }
}

/// The keys this version writes on a comment; any other key is kept as it was.
const COMMENT_KEYS: &[&str] = &[
    "id",
    "author",
    "email",
    "created",
    "edited",
    "start",
    "end",
    "revision",
    "quote",
    "before",
    "after",
    "text",
    "resolved",
    "resolved_by",
    "resolved_at",
    "detached",
    "reply",
];

impl PageMeta {
    /// The file's text with `comments` in place of those it held, keeping whatever else it
    /// holds; `None` when nothing would be left but the format, so the file is removed. Refuses
    /// what would break a limit, and a file of a newer format.
    pub fn render(&self, comments: &[StoredComment]) -> Result<Option<String>, MetaProblem> {
        if let Some(format) = self.newer {
            return Err(MetaProblem::NewerFormat(format));
        }
        if comments.len() > MAX_COMMENTS
            || comments
                .iter()
                .any(|comment| comment.replies.len() > MAX_REPLIES)
        {
            return Err(MetaProblem::TooMany);
        }
        if comments.iter().any(|comment| {
            comment.text.len() > MAX_TEXT
                || comment
                    .replies
                    .iter()
                    .any(|reply| reply.text.len() > MAX_TEXT)
        }) {
            return Err(MetaProblem::TooLong);
        }
        let old: Vec<Table> = self
            .document
            .get("comment")
            .and_then(Item::as_array_of_tables)
            .map(|tables| tables.iter().cloned().collect())
            .unwrap_or_default();
        let mut ordered: Vec<&StoredComment> = comments.iter().collect();
        ordered.sort_by_key(|comment| (comment.detached, comment.start, comment.end, comment.id));
        let mut tables = ArrayOfTables::new();
        for comment in ordered {
            let kept = old
                .iter()
                .find(|table| id(table).ok() == Some(comment.id))
                .cloned()
                .unwrap_or_default();
            tables.push(write_comment(kept, comment));
        }
        let mut document = self.document.clone();
        if tables.is_empty() {
            document.remove("comment");
        } else {
            document.insert("comment", Item::ArrayOfTables(tables));
        }
        set(document.as_table_mut(), "format", FORMAT);
        if document.iter().all(|(key, _)| key == "format") {
            return Ok(None);
        }
        let text = document.to_string();
        if text.len() > MAX_FILE {
            return Err(MetaProblem::TooMany);
        }
        Ok(Some(text))
    }
}

/// Whether two values are the same, however each is written.
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::String(left), Value::String(right)) => left.value() == right.value(),
        (Value::Integer(left), Value::Integer(right)) => left.value() == right.value(),
        (Value::Boolean(left), Value::Boolean(right)) => left.value() == right.value(),
        (Value::Datetime(left), Value::Datetime(right)) => {
            from_datetime(left.value()) == from_datetime(right.value())
        }
        _ => false,
    }
}

fn set(table: &mut Table, key: &str, value: impl Into<Value>) {
    let value = value.into();
    match table.get_mut(key) {
        // Left as it is written when its value is the same, so a diff shows only what changed.
        Some(Item::Value(existing)) if same(existing, &value) => {}
        // Kept where it was, with its own comment, when only its value changes.
        Some(Item::Value(existing)) => {
            let decor = existing.decor().clone();
            *existing = value;
            *existing.decor_mut() = decor;
        }
        _ => {
            table.insert(key, Item::Value(value));
        }
    }
}

fn set_or_remove(table: &mut Table, key: &str, value: Option<impl Into<Value>>) {
    match value {
        Some(value) => set(table, key, value),
        None => {
            table.remove(key);
        }
    }
}

fn written_id(id: u64) -> String {
    format!("{id:016x}")
}

fn write_comment(mut table: Table, comment: &StoredComment) -> Table {
    set(&mut table, "id", written_id(comment.id));
    set(&mut table, "author", comment.author.as_str());
    set_or_remove(&mut table, "email", comment.email.as_deref());
    set(&mut table, "created", to_datetime(comment.created));
    set_or_remove(&mut table, "edited", comment.edited.map(to_datetime));
    set(&mut table, "start", comment.start as i64);
    set(&mut table, "end", comment.end as i64);
    set(&mut table, "revision", comment.revision.as_str());
    set(&mut table, "quote", comment.quote.as_str());
    set(&mut table, "before", comment.before.as_str());
    set(&mut table, "after", comment.after.as_str());
    set(&mut table, "text", comment.text.as_str());
    set(&mut table, "resolved", comment.resolved.is_some());
    set_or_remove(
        &mut table,
        "resolved_by",
        comment
            .resolved
            .as_ref()
            .map(|resolution| resolution.by.as_str()),
    );
    set_or_remove(
        &mut table,
        "resolved_at",
        comment
            .resolved
            .as_ref()
            .map(|resolution| to_datetime(resolution.at)),
    );
    set_or_remove(&mut table, "detached", comment.detached.then_some(true));
    let old: Vec<Table> = table
        .get("reply")
        .and_then(Item::as_array_of_tables)
        .map(|tables| tables.iter().cloned().collect())
        .unwrap_or_default();
    let mut replies = ArrayOfTables::new();
    for reply in &comment.replies {
        let kept = old
            .iter()
            .find(|table| id(table).ok() == Some(reply.id))
            .cloned()
            .unwrap_or_default();
        replies.push(write_reply(kept, reply));
    }
    if replies.is_empty() {
        table.remove("reply");
    } else {
        table.insert("reply", Item::ArrayOfTables(replies));
    }
    // The known keys in their order, then whatever else the table held.
    table.sort_values_by(|left, _, right, _| {
        let rank = |key: &toml_edit::Key| {
            COMMENT_KEYS
                .iter()
                .position(|known| *known == key.get())
                .unwrap_or(COMMENT_KEYS.len())
        };
        rank(left).cmp(&rank(right))
    });
    table
}

fn write_reply(mut table: Table, reply: &StoredReply) -> Table {
    set(&mut table, "id", written_id(reply.id));
    set(&mut table, "author", reply.author.as_str());
    set_or_remove(&mut table, "email", reply.email.as_deref());
    set(&mut table, "created", to_datetime(reply.created));
    set_or_remove(&mut table, "edited", reply.edited.map(to_datetime));
    set(&mut table, "text", reply.text.as_str());
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn comment(id: u64, start: usize, end: usize) -> StoredComment {
        StoredComment {
            id,
            author: "Ada Lovelace".to_owned(),
            email: Some("ada@example.org".to_owned()),
            created: at(1_795_000_000),
            edited: None,
            start,
            end,
            revision: revision_of("= Page\n\nSome text.\n"),
            quote: "text".to_owned(),
            before: "Some ".to_owned(),
            after: ".\n".to_owned(),
            text: "Is this right?\nI think so.".to_owned(),
            resolved: None,
            detached: false,
            replies: vec![StoredReply {
                id: id + 1,
                author: "Grace".to_owned(),
                email: None,
                created: at(1_795_000_060),
                edited: Some(at(1_795_000_120)),
                text: "Yes.".to_owned(),
            }],
        }
    }

    fn written(comments: &[StoredComment]) -> String {
        PageMeta::default().render(comments).unwrap().unwrap()
    }

    #[test]
    fn sidecar_paths_name_the_page() {
        let page = Path::new("/docs/modules/ROOT/pages/install.adoc");
        let sidecar = sidecar_path(page).unwrap();
        assert_eq!(
            sidecar,
            Path::new("/docs/modules/ROOT/pages/.install.adoc.afd")
        );
        assert_eq!(page_of_sidecar(&sidecar).as_deref(), Some(page));
        for other in [
            ".afd",
            "..afd",
            ".install.adoc",
            "install.adoc.afd",
            ".notes.txt.afd",
        ] {
            assert_eq!(page_of_sidecar(Path::new(other)), None, "{other}");
        }
    }

    #[test]
    fn a_sidecar_round_trips() {
        let mut resolved = comment(0x10, 13, 17);
        resolved.resolved = Some(Resolution {
            by: "Grace".to_owned(),
            at: at(1_795_000_500),
        });
        let mut detached = comment(0x20, 0, 0);
        detached.detached = true;
        detached.replies.clear();
        let comments = vec![comment(0x1, 8, 12), resolved, detached];
        let text = written(&comments);
        assert!(text.starts_with(HEADER), "{text}");
        let read = parse(text.as_bytes()).unwrap();
        assert_eq!(read.comments, comments);
        assert_eq!(read.render(&read.comments).unwrap().unwrap(), text);
    }

    #[test]
    fn unknown_data_is_kept() {
        let mut text = written(&[comment(0x1, 8, 12)]);
        text = text.replace("text = ", "colour = \"amber\" # someone's tool\ntext = ");
        text = text.replace(
            "[[comment.reply]]\n",
            "[[comment.reply]]\nmood = \"glad\"\n",
        );
        text.push_str("\n[suggestion]\nkind = \"later\"\n");
        let read = parse(text.as_bytes()).unwrap();
        let mut changed = read.comments.clone();
        changed[0].text = "Changed.".to_owned();
        let again = read.render(&changed).unwrap().unwrap();
        assert!(
            again.contains("colour = \"amber\" # someone's tool"),
            "{again}"
        );
        assert!(again.contains("mood = \"glad\""), "{again}");
        assert!(again.contains("[suggestion]\nkind = \"later\""), "{again}");
        assert_eq!(parse(again.as_bytes()).unwrap().comments, changed);
    }

    #[test]
    fn render_orders_by_start_and_detached_last() {
        let mut detached = comment(0x30, 0, 0);
        detached.detached = true;
        let text = written(&[comment(0x20, 9, 10), detached, comment(0x10, 2, 3)]);
        let ids: Vec<u64> = parse(text.as_bytes())
            .unwrap()
            .comments
            .iter()
            .map(|comment| comment.id)
            .collect();
        assert_eq!(ids, [0x10, 0x20, 0x30]);
    }

    #[test]
    fn limits_are_held_reading_and_writing() {
        let mut long = comment(0x1, 0, 1);
        long.text = "x".repeat(MAX_TEXT + 1);
        assert_eq!(
            PageMeta::default().render(&[long]),
            Err(MetaProblem::TooLong)
        );
        let mut many = comment(0x1, 0, 1);
        many.replies = (0..=MAX_REPLIES as u64)
            .map(|n| StoredReply {
                id: 0x1000 + n,
                ..comment(0x1, 0, 1).replies[0].clone()
            })
            .collect();
        assert_eq!(
            PageMeta::default().render(&[many]),
            Err(MetaProblem::TooMany)
        );
        let crowd: Vec<StoredComment> = (0..=MAX_COMMENTS as u64)
            .map(|n| {
                let mut comment = comment(n * 2, 0, 1);
                comment.replies.clear();
                comment
            })
            .collect();
        assert_eq!(
            PageMeta::default().render(&crowd),
            Err(MetaProblem::TooMany)
        );

        assert_eq!(
            parse(&vec![b' '; MAX_FILE + 1]).err(),
            Some(MetaProblem::TooLarge)
        );
        let text = written(&[comment(0x1, 0, 1)]);
        for (from, to) in [
            (
                "author = \"Ada Lovelace\"",
                format!("author = \"{}\"", "a".repeat(MAX_AUTHOR + 1)),
            ),
            (
                "before = \"Some \"",
                format!("before = \"{}\"", "b".repeat(MAX_CONTEXT + 1)),
            ),
            (
                "quote = \"text\"",
                format!("quote = \"{}\"", "q".repeat(MAX_QUOTE + 1)),
            ),
        ] {
            let broken = text.replace(from, &to);
            assert!(
                matches!(parse(broken.as_bytes()), Err(MetaProblem::Invalid(_))),
                "{from}"
            );
        }
    }

    #[test]
    fn unusable_files_are_named() {
        let text = written(&[comment(0x1, 0, 1)]);
        let conflicted = format!("<<<<<<< HEAD\n{text}=======\n{text}>>>>>>> theirs\n");
        assert_eq!(
            parse(conflicted.as_bytes()).err(),
            Some(MetaProblem::Conflict)
        );
        assert!(MetaProblem::Conflict.to_string().contains("merge conflict"));
        assert!(matches!(
            parse(b"format = [").err(),
            Some(MetaProblem::NotToml(_))
        ));
        assert!(matches!(
            parse(&[0xff, 0xfe]).err(),
            Some(MetaProblem::NotToml(_))
        ));
        assert!(matches!(
            parse(b"x = 1\n").err(),
            Some(MetaProblem::Invalid(_))
        ));
        let wrong = text.replace("start = 0", "start = \"zero\"");
        assert!(matches!(
            parse(wrong.as_bytes()).err(),
            Some(MetaProblem::Invalid(_))
        ));
        let backwards = text.replace("start = 0", "start = 5");
        assert!(matches!(
            parse(backwards.as_bytes()).err(),
            Some(MetaProblem::Invalid(_))
        ));
    }

    #[test]
    fn duplicate_ids_are_invalid() {
        let mut twin = comment(0x1, 2, 3);
        twin.replies.clear();
        let text = written(&[comment(0x1, 0, 1)])
            + &written(&[twin])
                .replace(HEADER, "")
                .replace("format = 1\n", "");
        assert!(matches!(
            parse(text.as_bytes()).err(),
            Some(MetaProblem::Invalid(_))
        ));
    }

    #[test]
    fn a_newer_format_is_read_but_never_written() {
        let text = written(&[comment(0x1, 0, 1)]).replace("format = 1", "format = 2");
        let read = parse(text.as_bytes()).unwrap();
        assert_eq!(read.newer, Some(2));
        assert_eq!(read.comments.len(), 1);
        assert_eq!(read.render(&[]), Err(MetaProblem::NewerFormat(2)));
        let unreadable = "format = 2\ncomment = \"elsewhere\"\n";
        assert_eq!(
            parse(unreadable.as_bytes()).err(),
            Some(MetaProblem::NewerFormat(2))
        );
    }

    #[test]
    fn the_last_comment_gone_removes_the_file() {
        let read = parse(written(&[comment(0x1, 0, 1)]).as_bytes()).unwrap();
        assert_eq!(read.render(&[]), Ok(None));
        let with_more = written(&[comment(0x1, 0, 1)]) + "\n[suggestion]\nkind = \"later\"\n";
        let read = parse(with_more.as_bytes()).unwrap();
        let left = read.render(&[]).unwrap().unwrap();
        assert!(!left.contains("[[comment]]"), "{left}");
        assert!(left.contains("[suggestion]"), "{left}");
    }

    #[test]
    fn context_is_cut_at_character_boundaries() {
        let text = format!("{}é{}", "a".repeat(31), "😀".repeat(20));
        let start = 32 + 1; // after the é
        let end = start + 8; // two emoji
        let (quote, before, after) = context(&text, start, end);
        assert_eq!(quote, "😀😀");
        assert_eq!(
            before,
            format!("{}é", "a".repeat(30)),
            "31 bytes, the first a cut"
        );
        assert_eq!(after, "😀".repeat(8), "32 bytes");
        let long = "x".repeat(MAX_QUOTE + 10);
        assert_eq!(context(&long, 0, long.len()).0.len(), MAX_QUOTE);
        assert_eq!(cut("é", 1), "");
    }

    #[test]
    fn times_are_written_in_utc() {
        for (seconds, written) in [
            (0, "1970-01-01T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_795_134_645, "2026-11-20T00:30:45Z"),
        ] {
            let datetime = to_datetime(at(seconds));
            assert_eq!(datetime.to_string(), written);
            assert_eq!(from_datetime(&datetime), Some(at(seconds)));
        }
        let offset: toml_edit::Datetime = "2026-11-20T10:30:45+10:00".parse().unwrap();
        assert_eq!(from_datetime(&offset), Some(at(1_795_134_645)));
        let local: toml_edit::Datetime = "2026-11-21T08:30:45".parse().unwrap();
        assert_eq!(from_datetime(&local), None);
    }
}
