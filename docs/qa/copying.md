# Manual QA: selecting and copying rendered text

Selecting and copying in the visual editor and the rendered preview (#794). The tests drive both with a simulated pointer and keyboard, and never touch the real clipboard; these checks are what they cannot cover.

Use a copy of a real documentation project, with a page that has an `include::` of another page, code from an `include::`, callouts, numbered and nested lists, a table and a formula.

## Selecting

- [ ] Dragging over included text, included code, callout explanations, verse, a block title and an AsciiDoc table cell highlights exactly what the pointer passes over, in a light and a dark theme.
- [ ] A drag from a paragraph, through an include, to the paragraph after it highlights all three, and back again shrinks it.
- [ ] Over selectable text the pointer is a text cursor; over a link it is a hand.
- [ ] Double-click selects a word and triple-click the whole text. One click alone still shows the source, and the wait before it does not feel slow.
- [ ] A line with a typeset formula is highlighted and copied with the formula's source.
- [ ] Select All on a long page, and scrolling while a selection is held, stay smooth.
- [ ] The same in the preview docked, and in its own window.

## Copying

- [ ] Copy from each, pasted into a plain text editor, has its line breaks, indentation, tabs and list markers, and nothing else.
- [ ] Selecting items 2 to 4 of a numbered list pastes `2.`, `3.` and `4.`.
- [ ] The Copy button on a code block pastes code that runs.
- [ ] Copy As Markdown pasted into a GitHub comment, and Copy As AsciiDoc into a new page, look as the selection did.
- [ ] The right-click menu opens where clicked, its Copy As submenu opens, and Escape closes it with the focus back where it was.

## Formatted text

Record the versions of Word and the browser.

- [ ] macOS: Copy As Formatted Text pastes into Word, Pages and Google Docs with headings, bold, italic, code, links, lists, tables and code blocks, and into a plain text editor as plain text.
- [ ] Windows: the same in Word and Google Docs.
- [ ] A numbered list copied from its second item is pasted counting from 2.
- [ ] Linux: Formatted Text is not offered in the right-click menu, the Edit menu or the palette.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
