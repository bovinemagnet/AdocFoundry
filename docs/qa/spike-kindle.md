# Kindle Spike (K1, #627) — Record

| | |
|---|---|
| Author | Paul Snow |
| Date | 2026-11-17 |
| Candidates | `ebook-rs` 0.18.6 (MIT), `epub3-kindle` 0.5.1 (MIT), `mobi` 0.8.0 (MIT, reader), `boko` 0.6.0 (GPL-3.0-or-later, not built) |
| Toolchain | Rust 1.99.1, as `rust-toolchain.toml` pins |
| Decision | **Go, both formats in-house.** `adoc-kindle` writes AZW3 (KF8) and KFX (KF10) itself, with no writer dependency. Not yet verified on a Kindle: the reader checks below are pending |

The PRD (`docs/prd/PRD-AdocFoundry_Kindle_Export.md`) makes this package a gate: a writer approach for each format, licences, prototypes read back and tried on readers, supported devices and resource budgets. This record holds what was measured, what was decided and what is still to be checked by hand.

## Method

A scratch Cargo project outside the workspace depended on the two writer candidates with `default-features = false`, and on `mobi` as a reader. It built a two-chapter EPUB 3 package in memory with `zip`: metadata, a nested navigation document, a PNG cover, bold text, a link from one chapter to an anchor in the other, Japanese text, a list, a code block and a table. It passed the bytes to each writer, wrote the results, and read them back. It ran twice, in two processes, and the outputs were compared byte for byte. Every `build.rs` was read before anything was compiled. No converter, reader application or other process was run.

## Findings

| | `epub3-kindle` 0.5.1 | `ebook-rs` 0.18.6 |
|---|---|---|
| Writes | AZW3, KF8 only (`BOOKMOBI`, MOBI header version 8, EXTH, FDST, skeleton and fragment indexes). A "dual" `.mobi` is a separate option | KFX (`CONT` container, version 2, 19 `ENTY` entities for the sample) |
| Input | EPUB bytes only (`memory::convert`); no in-memory model | EPUB, MOBI, PDF and other files only (`Book::from_bytes`); no in-memory model |
| Output | 16,829 bytes; the PNG was transcoded to JPEG | 3,309 bytes; the PNG kept as it was |
| Repeatable | Byte-identical across two processes | Byte-identical across two processes |
| Read back | By `mobi` 0.8.0, a separate project: title, author, language and text | Only by `ebook-rs`'s own reader (title, two spine items, two contents entries), which is not independent of its writer |
| Errors | Typed (`thiserror`), warnings as codes and messages, no source location | `Result<_, String>` |
| Dependencies | 87 crates. `zip` 2 with its default features compiles three C libraries through `cc` (`bzip2-sys`, `lzma-sys`, `zstd-sys`) and brings `aes`, `pbkdf2` and `time`. The `winresource` build dependency compiles on every platform. Cargo cannot turn a dependency's features off | 72 crates with default features off, among them `aes`, `cbc`, `regex`, `serde_json`, `whatlang` and `zip` 9 |
| `unsafe` | 2 occurrences | 28, in `book.rs`, `layout.rs`, `ffi.rs` and `server.rs` |
| Evidence on devices | The README shows photographs of a sample on a physical Kindle | None found |
| Maturity | 142 downloads; released 2026-11-13 | 2,223 downloads; released 2026-11-16, the day before this spike |

`boko` is GPL-3.0-or-later, as its crates.io metadata says. It was not built or read, and it is not added to the repository as a dependency or a test dependency.

## Decision

Both formats are written in-house, in `adoc-kindle`. That settles the PRD's "Writer per format" decision (§15):

- **Neither library takes an in-memory book.** Both take a packaged EPUB, so the exporter would have to write an EPUB package in memory to hand to them. Their warnings would then describe the EPUB rather than the AsciiDoc source. The exporter's own `KindleBook` maps to each format directly and keeps source locations for every diagnostic.
- **The dependency trees do not fit the workspace.** Three C libraries and a build script on every platform come with `epub3-kindle`. An unreviewed `unsafe` surface, a fast-moving multi-format crate and string errors come with `ebook-rs`. The DOCX exporter was written in-house for the same reasons, because `docx-rs` made output depend on process-wide state.
- **Both formats can be written in Rust.** Each prototype is a real container: the AZW3 is detected as KF8, and the KFX has a `CONT` signature whose entities resolve in its own reader. Neither format is blocked, so the "If KFX proves blocked" decision does not arise yet. It returns if KFX fails on the reader checks below, and that choice is the maintainer's.

### Where the format facts come from

The writers are written from documentation and permissively licensed code, never from GPL code. Calibre, KindleUnpack and `boko` are not consulted.

- **KF8 (AZW3):** the MIT source of `epub3-kindle` 0.5.1 (`src/kf8/`, `src/container/`: the PalmDB and MOBI headers, EXTH, the indexes, the FDST and the trailing entries), the writer with photographs from a device. Anything derived from it carries its MIT notice beside it.
- **KFX:** the MIT source of `ebook-rs` 0.18.6 (`src/kfx/`: the container, the shared `YJ_symbols` table and the fragments it writes) and the Amazon Ion binary specification. Anything derived from `ebook-rs` carries its MIT notice beside it.

### Independent readback

The PRD's "Independent readback" decision (§15) is settled like this:

- **AZW3** is read back by `mobi` 0.8.0, MIT and a dev-dependency only, which shares no code with the writer. It covers the PalmDB records, the MOBI header, EXTH metadata and the decompressed text. The KF8 indexes `mobi` does not read (skeleton, fragment, NCX) are checked by a reader in `adoc-kindle`'s `testing` module, written apart from the writer.
- **KFX** has no permissively licensed reader other than `ebook-rs`'s, whose tree is too large even as a dev-dependency. It is read back by an Ion and container reader in `adoc-kindle`'s `testing` module, written apart from the writer from the Ion specification. That reader decodes every entity and resolves every reference.

`mobi` adds 17 crates to the development tree, among them `encoding` 0.2, and none to any release build.

## Supported readers (to be confirmed in K8)

The promise, and so the checks in `docs/qa/kindle.md`, is limited to what K8 verifies by hand. It is proposed as:

| Format | Readers to check |
|---|---|
| AZW3 | One current Kindle e-reader (sideloaded over USB), and Kindle for iOS or Android |
| KFX | The same e-reader. Whether a Kindle app opens a sideloaded KFX is checked, not assumed |

Devices older than the Kindle Touch (4th generation) are out of scope, as they are for `epub3-kindle`. Legacy MOBI is out of scope in any case (PRD §3).

## Resource budgets (for K6 and K8)

These follow the DOCX exporter's (`docx_large_book_is_bounded`): a book of 600 pages, each with a section, a list, a code block and a table, plus 50 images, exported in each format by a debug build on a CI runner, within 90 s, with the command returning within 3 s, no pump longer than 3 s and the process's peak memory under 2 GB. Each embedded image is limited to 16 MiB and 40 megapixels, and a book to 256 MiB of embedded bytes. K8 measures against these and records what it finds.

## Pending manual checks

None of these can be run by the automated tests, and none has been run:

- [ ] Open the prototype AZW3 and KFX on a Kindle e-reader. Check that each appears in the library with its title, that the contents and the link work, and that the image shows.
- [ ] Repeat with the files `adoc-kindle` writes once K3 and K4 land (`docs/qa/kindle.md`).
- [ ] Confirm which Kindle apps open a sideloaded KFX.

## KDP

KFX and AZW3 are files for reading and sideloading on Kindle devices and apps. They are not manuscript formats for Kindle Direct Publishing, which takes EPUB, DOCX and KPF. KPF is a different format from KFX, and K9 (#635) researches it separately.
