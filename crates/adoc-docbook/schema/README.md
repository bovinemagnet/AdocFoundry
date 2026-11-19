# DocBook 5.2.1 schema

`docbook.rng` is the DocBook 5.2.1 RELAX NG schema in XML syntax, unchanged, from
<https://cdn.docbook.org/schema/5.2.1/rng/docbook.rng> (SHA-256
`cc6f70b53d7242a991b0cf885f978f2c4b3c9491c194130da9b212e17ac0adb5`). Its copyright notice and
permission are at the top of the file and travel with it.

It is a test fixture: `adoc_docbook::testing` (the `test-support` feature) checks every document the tests
write against it in process, through `oxixml-relaxng`. Nothing downloads it, no build or export reads
it, and no shipped binary contains it.

The Schematron rules embedded in it are not run (no Rust Schematron engine is used); the
export's own check (`adoc_docbook::check`) covers the ones that apply to what it writes.
