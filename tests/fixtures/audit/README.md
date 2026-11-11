# Audit fixture

Three component versions for the documentation quality audit's rules (`adoc-audit`):

- `product` 3.0: two navigation files (`ROOT` and `admin`), a page listed in both, a page listed through its alias, an unlisted page, an untitled page, a page whose title may come from an include, block images with and without alternative text and an inline one without, a partial and an example that are not pages, and a navigation entry that does not resolve.
- `product` 2.0: lists `orphan.adoc`, which 3.0 does not.
- `guide` (versionless): its navigation includes another file and has an entry naming an attribute, so its unlisted page cannot be asserted.
