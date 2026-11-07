# Pasting and dropping images: manual checks

The GPUI test platform has a clipboard and takes simulated drops, but not the operating system's own. These need real input on each platform.

Use an Antora workspace kept in Git, so the result can be read with `git status` and `git diff`.

## Pasting

- [ ] A screenshot taken to the clipboard (macOS `Cmd`-`Ctrl`-`Shift`-`4`, Windows `Win`-`Shift`-`S`, the Linux desktop's tool) pasted in the source editor asks for a name and shows the path; the file opens in an image tab and is the same picture; the preview shows it within a moment.
- [ ] The same in the visual editor: the dialog also asks for alt text, and the image is drawn after the caret's block.
- [ ] A picture copied from a browser pastes as a picture, or, if the browser puts text on the clipboard with it, as text. Note which, per browser.
- [ ] A selection of cells copied from a spreadsheet or text from a word processor, which come with a picture, paste as text.
- [ ] An image file copied in Finder, File Explorer or the Linux file manager pastes without asking for a name.
- [ ] On Linux, both X11 and Wayland.

## Dropping

- [ ] Type in the source editor, switch to the file manager without touching the app's window, and drag an image in: it goes in at the line under the pointer, and typing goes on in its alt text.
- [ ] The same in the visual editor, dropped on a paragraph: it goes after that paragraph and the image dialog opens on its alt text.
- [ ] A file with spaces and capitals in its name; several files at once; a file that is not an image beside an image.
- [ ] A screenshot dragged from the macOS thumbnail in the corner of the screen, and from the macOS Screenshot app's file on the Desktop.
- [ ] A drop on an image in the module's own `images` folder, from the file manager, copies nothing.

## Undo

- [ ] Undo removes the macro and the file, and `git status` is clean; redo brings both back.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
