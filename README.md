# Folio Reader

Folio is a portable EPUB reader for 64-bit Windows. Extract the ZIP into a writable folder and open **Folio Reader.exe**.

**My Library** is the start screen. Click **Add EPUBs** or drag EPUB files onto the bookshelf to add them. Folio shows the title, author, and cover stored in each book. Click a book to read it, then use **Library** in the upper left to return.

- In **My Library**, use **New folder** to create a collection. Open a folder tile to see its books. New EPUBs added inside a folder go into that folder. Right-click a folder tile to choose an image or restore the default icon.
- To delete a folder, right-click its tile and choose **Delete folder**, then confirm. Its books move to the main library as unfiled, where your current filters still apply.
- Right-click a book to edit its title, author, or cover, assign it to a folder, or archive it. Type a tag and press **Enter** to add it. Click the tag's **×** to remove it. Choose **Restore EPUB cover** to remove your custom cover.
- The library search matches titles, authors, tags, and folder names. In the **Folder** dropdown, **Organized view** shows folder tiles and unfiled books in their usual places. Choose **Show all books** for a flat list or select a folder from the **Folders** group. Select **Archived** to find archived books or unarchive them from **Edit book**.
- Open **Tags** to search your tags. Click a tag once to require it, again to exclude it, and a third time to clear it. A book must have every included tag and none of the excluded tags.
- **Save preset** stores the current library search, folder, tags, and status under a name. Select a preset and click **Load** to reuse it. To update an existing preset, enter its name in the save dialog and click **Overwrite preset**.
- To remove a book, right-click it, choose **Remove book**, and confirm. Folio deletes its imported copy and keeps your saved reading position if you add the book again.
- Open **Reading settings** in the upper right for the layout, font, text size, theme, and bottom bar visibility. Dark is the default theme. The bottom bar can always show, hide in fullscreen, or always hide. In Two pages, click either outside margin or use the left and right arrow keys to turn spreads.
- **Line spacing** adjusts the distance between wrapped lines. **Paragraph spacing** adjusts the gap between paragraphs. Turn on **Trim extra blank lines** to collapse blank paragraphs and repeated line breaks inserted by the EPUB.
- **Ctrl+F** finds text in the current chapter and keeps your query active as you change chapters. Click **Lock**, then **×**, to hide the search box and keep highlighting matches. Press Ctrl+F to show it again. Unlock before closing the box to clear the search, or return to My Library.
- **Search markers** in Reading settings shows where matches occur along the reading scrollbar in Scroll mode or along the bottom edge in Two pages. The markers stay visible while a locked search is active.
- Search the sidebar by chapter title or displayed number. Recognized covers and tables of contents are unnumbered. Right-click any row to mark it as a non-chapter or count it as a chapter. Use the button beside **Library** to hide or show the sidebar.
- **Full reader view** or **F11** fills the monitor. Move the pointer to the top edge to reveal the toolbar in either layout. Use the fullscreen button, F11, or Escape to leave it. Previous and Next chapter are available in the toolbar and at the end of each chapter.
- Folio stores EPUB copies in `Books`, custom book and folder images in `Covers`, and the library, positions, and preferences in `reading-state.json`. Copy these with the portable app folder to bring them to another computer.

To upgrade from the earlier reader, replace the executable and DLL in the same folder or bring the existing `reading-state.json` to a fresh extraction. Folio adds the last opened book if it can still find the original file. Add earlier books again to bring them into My Library. Their saved positions are reused for the same EPUB files.

Folio supports unencrypted, reflowable EPUB 2 and EPUB 3 books. It uses the selected reading font and app styling for book content. Fixed-layout and DRM-protected books are outside its scope. The app runs offline and uses the Windows WebView2 Runtime that ships with Windows 11. If Windows reports that the runtime is missing, install the [Microsoft WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).

## Build from source

With Rust and a Windows linker available, run `cargo build --release --locked` in this folder. The released x64 binary was built with the `x86_64-pc-windows-gnullvm` Rust toolchain and its `rust-mingw` component. The release binary embeds the HTML, CSS, and JavaScript interface.
