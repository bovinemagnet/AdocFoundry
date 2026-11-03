//! What a tool answers: its JSON as text, as every client reads it, and the same JSON as
//! structured content for a session whose protocol version has it (2025-06-18 on); a failure's
//! message as text, with a stable [`ErrorCode`] in its structured content. And the cursors that
//! page long lists: opaque, bound to one query and one workspace revision, refused when either
//! differs, so a page never mixes two states of the workspace.

use adoc_engine::EngineError;
use rmcp::model::{CallToolResult, ContentBlock, ProtocolVersion};
use serde::Serialize;
use serde_json::{json, Value};

/// At most this many items in a page of a list, unless a tool's `limit` asks for fewer.
pub const PAGE: usize = 500;

/// A failure's kind, a name a client can act on. The names are stable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// An argument is missing, malformed, ambiguous or contradicts another, or a cursor
    /// belongs to another query or an older workspace revision.
    InvalidArgument,
    /// The page, resource id, component or file named does not exist.
    NotFound,
    /// A path lies outside every workspace root, or reaches outside through a link.
    OutsideWorkspace,
    /// The server does not allow this, whatever the request or the documents say.
    PermissionDenied,
    /// The text or file changed from what the request expected.
    Conflict,
    /// A staged change was made against revisions that have since changed.
    StalePreview,
    /// The workspace is not yet, or not wholly, indexed.
    IndexIncomplete,
    /// Something the request needs is not there: a tool, a provider, a free slot.
    Unavailable,
    /// The work, or the wait for the engine, took longer than allowed.
    Timeout,
    /// The answer would be larger than a response may be.
    ResultTooLarge,
    /// The server failed; the request may be fine.
    InternalError,
}

/// Why a tool could not answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }

    /// The failure as structured content.
    pub fn to_value(&self) -> Value {
        json!({ "error": { "code": self.code, "message": self.message } })
    }
}

impl From<EngineError> for Failure {
    fn from(error: EngineError) -> Self {
        let code = match &error {
            EngineError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                ErrorCode::NotFound
            }
            EngineError::Io { .. } | EngineError::UnknownCommand(_) => ErrorCode::InternalError,
            EngineError::UnknownDocument(_) | EngineError::NotFound(_) => ErrorCode::NotFound,
            EngineError::InvalidArguments { .. }
            | EngineError::InvalidEdit(_)
            | EngineError::InvalidRequest(_)
            | EngineError::Settings(_) => ErrorCode::InvalidArgument,
            EngineError::Refused(_)
            | EngineError::SaveConflict { .. }
            | EngineError::GroupBlocked { .. } => ErrorCode::Conflict,
            EngineError::PreviewStale { .. } => ErrorCode::StalePreview,
        };
        Self::new(code, error.to_string())
    }
}

/// A tool's answer: on success the JSON as text and as structured content, on failure the
/// message as text and the code and message as structured content. [`for_session`] takes the
/// structured content away again for a session that cannot read it.
pub fn answer(result: Result<Value, Failure>) -> CallToolResult {
    match result {
        Ok(value) => {
            let mut result = CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&value).unwrap_or_default(),
            )]);
            result.structured_content = Some(value);
            result
        }
        Err(failure) => {
            let mut result = CallToolResult::error(vec![ContentBlock::text(&failure.message)]);
            result.structured_content = Some(failure.to_value());
            result
        }
    }
}

/// Whether a session at `version` reads structured content and output schemas, which the
/// protocol has from 2025-06-18. A session with no version is given text only.
pub fn structured(version: Option<&ProtocolVersion>) -> bool {
    // ISO dates compare as text the same as in time.
    version.is_some_and(|version| version.as_str() >= ProtocolVersion::V_2025_06_18.as_str())
}

/// `result` as a session at `version` reads it: without structured content before 2025-06-18.
pub fn for_session(
    mut result: CallToolResult,
    version: Option<&ProtocolVersion>,
) -> CallToolResult {
    if !structured(version) {
        result.structured_content = None;
    }
    result
}

/// Where a page of a list resumes: the query it pages, the workspace revision it was made at
/// and how many items came before it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Cursor {
    query: String,
    revision: u64,
    offset: usize,
}

impl Cursor {
    /// Hex, so it reads as one opaque token; nothing in it is secret.
    fn encode(&self) -> String {
        format!("1:{}:{}:{}", self.revision, self.offset, self.query)
            .bytes()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn decode(cursor: &str) -> Option<Self> {
        if !cursor.len().is_multiple_of(2) || !cursor.is_ascii() {
            return None;
        }
        let bytes: Option<Vec<u8>> = (0..cursor.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&cursor[at..at + 2], 16).ok())
            .collect();
        let text = String::from_utf8(bytes?).ok()?;
        let mut parts = text.splitn(4, ':');
        if parts.next()? != "1" {
            return None;
        }
        Some(Self {
            revision: parts.next()?.parse().ok()?,
            offset: parts.next()?.parse().ok()?,
            query: parts.next()?.to_owned(),
        })
    }
}

/// One page of a list.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Where the next page starts; `None` on the last.
    pub next_cursor: Option<String>,
    /// How many items the whole list has.
    pub total: usize,
}

/// The page of `items`, in the order the caller made deterministic, that `cursor` names, or
/// the first; at most `limit` items, and never more than [`PAGE`]. `query` names everything
/// that chose the items, and `revision` the workspace they were chosen from: a cursor made for
/// another query or revision is refused, and the caller lists again from the start.
pub fn page<T>(
    items: Vec<T>,
    query: &str,
    revision: u64,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<Page<T>, Failure> {
    let limit = limit.unwrap_or(PAGE).clamp(1, PAGE);
    let offset = match cursor {
        None => 0,
        Some(cursor) => {
            let cursor = Cursor::decode(cursor)
                .ok_or_else(|| Failure::invalid("the cursor is not one this server made"))?;
            if cursor.query != query {
                return Err(Failure::invalid(
                    "the cursor belongs to another query; pass the arguments it was made with",
                ));
            }
            if cursor.revision != revision {
                return Err(Failure::invalid(format!(
                    "the cursor is stale: the workspace has changed since it was made (revision \
                     {} then, {revision} now); list again from the start",
                    cursor.revision
                )));
            }
            cursor.offset
        }
    };
    let total = items.len();
    let items: Vec<T> = items.into_iter().skip(offset).take(limit).collect();
    let end = offset + items.len();
    let next_cursor = (end < total).then(|| {
        Cursor {
            query: query.to_owned(),
            revision,
            offset: end,
        }
        .encode()
    });
    Ok(Page {
        items,
        next_cursor,
        total,
    })
}

/// A page of a tool's list as JSON: the items under `key`, `total`, `next_cursor` and the
/// `workspace_revision` the cursor holds to, and, while more follow, `truncated` as the tools
/// gave it before they paged, so a client that reads only that still sees the list is cut.
pub fn paged<T: Serialize>(key: &str, page: Page<T>, revision: u64) -> Value {
    let shown = page.items.len();
    let mut value = json!({
        key: page.items,
        "total": page.total,
        "next_cursor": page.next_cursor,
        "workspace_revision": revision,
    });
    if value["next_cursor"].is_string() {
        value["truncated"] = json!({ "shown": shown, "total": page.total });
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_named_stably() {
        let codes = [
            ErrorCode::InvalidArgument,
            ErrorCode::NotFound,
            ErrorCode::OutsideWorkspace,
            ErrorCode::PermissionDenied,
            ErrorCode::Conflict,
            ErrorCode::StalePreview,
            ErrorCode::IndexIncomplete,
            ErrorCode::Unavailable,
            ErrorCode::Timeout,
            ErrorCode::ResultTooLarge,
            ErrorCode::InternalError,
        ];
        assert_eq!(
            serde_json::to_value(codes).unwrap(),
            json!([
                "invalid_argument",
                "not_found",
                "outside_workspace",
                "permission_denied",
                "conflict",
                "stale_preview",
                "index_incomplete",
                "unavailable",
                "timeout",
                "result_too_large",
                "internal_error",
            ])
        );
    }

    #[test]
    fn cursors_walk_a_list_once_and_hold_to_their_query_and_revision() {
        let items: Vec<usize> = (0..1_234).collect();
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = page(items.clone(), "q", 7, cursor.as_deref(), None).unwrap();
            assert!(page.items.len() <= PAGE);
            assert_eq!(page.total, items.len());
            seen.extend(page.items);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, items, "every item once, in order");

        let first = page(items.clone(), "q", 7, None, Some(10)).unwrap();
        assert_eq!(first.items, (0..10).collect::<Vec<_>>());
        let next = first.next_cursor.unwrap();
        assert_eq!(
            page(items.clone(), "q", 7, Some(&next), Some(10))
                .unwrap()
                .items,
            (10..20).collect::<Vec<_>>()
        );
        let stale = page(items.clone(), "q", 8, Some(&next), Some(10)).unwrap_err();
        assert_eq!(stale.code, ErrorCode::InvalidArgument);
        assert!(stale.message.contains("stale"), "{}", stale.message);
        let other = page(items.clone(), "r", 7, Some(&next), Some(10)).unwrap_err();
        assert!(other.message.contains("another query"), "{}", other.message);
        for forged in ["", "zz", "31", "not a cursor"] {
            assert!(page(items.clone(), "q", 7, Some(forged), None).is_err());
        }
        assert_eq!(
            page(Vec::<u8>::new(), "q", 7, None, Some(0)).unwrap().total,
            0
        );
    }

    #[test]
    fn structured_content_is_given_from_2025_06_18() {
        assert!(!structured(None));
        assert!(!structured(Some(&ProtocolVersion::V_2024_11_05)));
        assert!(!structured(Some(&ProtocolVersion::V_2025_03_26)));
        assert!(structured(Some(&ProtocolVersion::V_2025_06_18)));
        assert!(structured(Some(&ProtocolVersion::LATEST)));

        let result = answer(Ok(json!({ "a": 1 })));
        assert_eq!(result.structured_content, Some(json!({ "a": 1 })));
        let text = result.content[0].as_text().unwrap().text.clone();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({ "a": 1 })
        );
        let old = for_session(result, Some(&ProtocolVersion::V_2025_03_26));
        assert_eq!(old.structured_content, None);
        assert_eq!(old.content.len(), 1, "the text stays");

        let failed = answer(Err(Failure::not_found("no such page")));
        assert_eq!(failed.is_error, Some(true));
        assert_eq!(
            failed.structured_content,
            Some(json!({ "error": { "code": "not_found", "message": "no such page" } }))
        );
    }
}
