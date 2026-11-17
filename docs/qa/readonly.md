# Read-Only Exports — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Read-only exports (`:export-read-only:`, `docs/prd/PRD-AdocFoundry_READONLY.md`, epic #685) where automated tests cannot reach: how Microsoft Word, LibreOffice and PDF readers treat the documents. The tests check what is written and every refusal; they cannot run Word or a PDF reader |

Run each item in Microsoft Word (desktop, Microsoft 365 or 2021, on Windows and macOS) and LibreOffice Writer (7.6 or later). Record the build, the application and version, the platform and the result in the table at the end; record a difference as a difference.

## The documents

```sh
mkdir -p /tmp/ro && cp -R tests/fixtures/docx/reference /tmp/ro/reference
# Every page of the manual, version 2.0, asks to be read-only:
for page in /tmp/ro/reference/manual/modules/*/pages/*.adoc; do
  python3 -c 'import sys; p=sys.argv[1]; t=open(p).read().split("\n",1); open(p,"w").write(t[0]+"\n:export-read-only:\n"+t[1])' "$page"
done
cargo run -p adocfoundry-cli -- docx /tmp/ro/reference/manual --root /tmp/ro/reference --output /tmp/ro/manual.docx
printf '= Approved Procedure\n:export-read-only:\n\nThe procedure.\n' > /tmp/ro/page.adoc
cargo run -p adocfoundry-cli -- docx /tmp/ro/page.adoc --output /tmp/ro/page.docx
```

Both commands print `protected: read-only`.

## Automated coverage, for reference

- The header (READONLY 1/5, `crates/adoc-engine/tests/read_only.rs`): values, repeated and unset declarations, body, includes, `antora.yml` and the playbook ignored, the unsaved buffer honoured, and the export's own parser as the oracle for where the header ends.
- The decision (READONLY 2/5, `crates/adoc-engine/tests/protection.rs`): articles and books, mixed books, HTML, PDF/A, profiles and an export's attributes, existing output kept, unprotected output unchanged.
- Word (READONLY 3/5): `docx_readonly_settings_enforced`, `docx_readonly_preserves_relationships`, and the package checks of `w:documentProtection`.
- The front ends (READONLY 5/5): `protection_cli_exit_codes`, `protection_mcp_errors`, `protection_app_keeps_editors_editable`.

## Checks

1. **Opens read-only, without repair.** `page.docx` and `manual.docx` open with no repair or "unreadable content" prompt. Word shows the document as restricted (Restrict Editing pane: "Only this type of editing: No changes (Read only)"); LibreOffice says the document is read-only or protected.
2. **Edits are refused.** Typing, deleting, pasting, applying a style and editing a table cell, a footnote, a header and a footer are refused. Selecting and copying text still work.
3. **Stop Protection.** Word's Review › Restrict Editing › Stop Protection removes it with no password (this is a deterrent, as documented). LibreOffice lets the protection be switched off in the same way.
4. **Contents and fields.** In `manual.docx`, Word's offer to update fields on opening fills in the contents' page numbers, or note what it does instead (the contents may keep blank page numbers while the document is protected: record which).
5. **Links and pictures.** Links still work (Ctrl-click in Word). Pictures, formulas, the logo and covers show.
6. **Save as.** Save a copy in the application's own Word format and reopen it: still protected, no repair prompt.
7. **Accessibility.** Word's Check Accessibility and a screen reader (Narrator or VoiceOver) still read the protected document.
8. **The source.** The pages in AdocFoundry stay editable in the source and visual editors after exporting.
9. **Refusals.** In the app, Export Page to PDF… on `page.adoc` says why at once and asks for no file; Export Page to HTML… likewise. A mixed book (one page without the attribute) to Word is refused, naming both lists of pages.

## Known limitations

- **No password.** Word protection without a password deters editing; anyone who knows how can remove it. It does not encrypt or hide the content.
- **Mixed Word books are refused.** Leaving some pages editable needs Word's permission ranges, which wait until they can be verified in Word.
- **PDF.** A read-only PDF needs the Standard Security Handler written in Rust (READONLY 4/5); until its feasibility gate passes, a PDF export of a page that asks is refused. PDF/A is always refused with protection.

## Results

| Build | Application | Platform | Result | Notes |
|---|---|---|---|---|
| `readonly-3-word` | LibreOffice 26.8.3.1 (headless) | macOS 26 | Pass, partly | A protected page converts to PDF; saved back to Word, it keeps `w:documentProtection` read-only and enforced, so LibreOffice read the protection. Items 1 to 9 in the desktop applications remain to be run. |
| | Microsoft Word | Windows | | |
| | Microsoft Word | macOS | | |
| | LibreOffice Writer | Windows | | |
| | LibreOffice Writer | Ubuntu | | |
