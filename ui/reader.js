"use strict";

const $ = id => document.getElementById(id);
const viewport = $("viewport");
const flow = $("flow");
const content = $("text");
const rail = $("rail");
const bridge = window.chrome?.webview;
const pending = new Map();
let sequence = 0;

bridge?.addEventListener("message", ({data}) => {
  const reply = pending.get(data.id);
  if (!reply) return;
  pending.delete(data.id);
  clearTimeout(reply.timeout);
  if (data.error) reply.reject(new Error(data.error));
  else reply.resolve(data.value);
});

function call(action, fields = {}) {
  return new Promise((resolve, reject) => {
    if (!bridge) { reject(new Error("Open Folio Reader.exe to read EPUB files.")); return; }
    const id = ++sequence;
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error("The reader did not respond."));
    }, ["import", "drop_finish", "pick_cover", "pick_folder_image"].includes(action) ? 600000 : 30000);
    pending.set(id, {resolve, reject, timeout});
    bridge.postMessage({id, request: {action, ...fields}});
  });
}

const fonts = {
  georgia: "Georgia, serif", cambria: "Cambria, Georgia, serif",
  times: '"Times New Roman", serif', palatino: '"Palatino Linotype", Georgia, serif',
  segoe: '"Segoe UI", sans-serif', arial: "Arial, sans-serif", consolas: "Consolas, monospace",
};
const settings = {
  font: "georgia", size: 19, theme: "dark", mode: "scroll", sidebar: true, footer: "always",
  lineSpacing: 1.64, paragraphSpacing: 1, trimBlankLines: false, searchMarkers: true,
};
let book = null;
let chapter = -1;
let chapterNumbers = [];
let chapterTotal = 0;
let loadToken = 0;
let opening = false;
let importing = false;
let fullscreen = false;
let resizingWindow = false;
let restoreFind = false;
let findLocked = false;
let stride = 1;
let spreads = 1;
let anchor = {offset: 0, fraction: 0};
let textRuns = [];
let runOffsets = new WeakMap();
let plain = "";
let matches = [];
let currentMatch = -1;
let markerFrame = 0;
let layoutFrame = 0;
let scrollFrame = 0;
let saveTimer = 0;
let findTimer = 0;
let noticeTimer = 0;
let lastSaved = "";
let loadedSettings = false;

const isSpread = () => settings.mode === "spread";
const clamp = (value, min, max) => Math.min(max, Math.max(min, value));
const pageIndex = () => clamp(Math.round(viewport.scrollLeft / stride), 0, spreads - 1);

function notice(message) {
  clearTimeout(noticeTimer);
  $("notice").textContent = message;
  $("notice").hidden = !message;
  if (message) noticeTimer = setTimeout(() => { $("notice").hidden = true; }, 6500);
}

function persist() {
  clearTimeout(saveTimer);
  saveTimer = 0;
  if (!loadedSettings) return;
  const payload = {
    book_id: book?.id ?? null,
    position: chapter >= 0 ? {chapter, ...anchor} : null,
    settings: {...settings, searchPresets: [...searchPresets.values()]},
  };
  const serialized = JSON.stringify(payload);
  if (serialized === lastSaved) return;
  lastSaved = serialized;
  call("save", payload).catch(error => {
    if (lastSaved === serialized) lastSaved = "";
    notice(error.message);
  });
}

function saveSoon() {
  if (!saveTimer) saveTimer = setTimeout(persist, 180);
}

function applySettings(patch = {}, save = true) {
  const position = viewport.hidden ? anchor : readPosition();
  if (Object.hasOwn(fonts, patch.font)) settings.font = patch.font;
  if (Number.isFinite(patch.size)) settings.size = clamp(Math.round(patch.size), 12, 42);
  if (["light", "dark", "black"].includes(patch.theme)) settings.theme = patch.theme;
  if (["scroll", "spread"].includes(patch.mode)) settings.mode = patch.mode;
  if (["always", "fullscreen", "never"].includes(patch.footer)) settings.footer = patch.footer;
  if ([1.2, 1.4, 1.64, 1.9, 2.15].includes(patch.lineSpacing)) settings.lineSpacing = patch.lineSpacing;
  if ([0, 0.35, 0.65, 1, 1.5].includes(patch.paragraphSpacing)) settings.paragraphSpacing = patch.paragraphSpacing;
  if (typeof patch.trimBlankLines === "boolean") settings.trimBlankLines = patch.trimBlankLines;
  if (typeof patch.searchMarkers === "boolean") settings.searchMarkers = patch.searchMarkers;
  if (typeof patch.sidebar === "boolean") settings.sidebar = patch.sidebar;
  document.documentElement.dataset.theme = settings.theme;
  document.documentElement.style.setProperty("--reading-font", fonts[settings.font]);
  document.documentElement.style.setProperty("--reading-size", settings.size + "px");
  document.documentElement.style.setProperty("--reading-line-height", settings.lineSpacing);
  document.documentElement.style.setProperty("--reading-paragraph-gap", settings.paragraphSpacing + "em");
  document.body.classList.toggle("spread", isSpread());
  document.body.classList.toggle("trim-blank-lines", settings.trimBlankLines);
  $("font").value = settings.font;
  $("size").value = settings.size;
  $("line-spacing").value = settings.lineSpacing;
  $("paragraph-spacing").value = settings.paragraphSpacing;
  $("trim-blank-lines").checked = settings.trimBlankLines;
  $("search-markers-toggle").checked = settings.searchMarkers;
  $("sidebar").hidden = !settings.sidebar;
  $("sidebar-toggle").setAttribute("aria-expanded", settings.sidebar);
  $("scroll-mode").setAttribute("aria-pressed", !isSpread());
  $("spread-mode").setAttribute("aria-pressed", isSpread());
  $("light-mode").setAttribute("aria-pressed", settings.theme === "light");
  $("dark-mode").setAttribute("aria-pressed", settings.theme === "dark");
  $("black-mode").setAttribute("aria-pressed", settings.theme === "black");
  $("footer-mode").value = settings.footer;
  updateFooterVisibility();
  layout(position);
  if (save) saveSoon();
}

function updateFooterVisibility() {
  $("reader-footer").hidden = settings.footer === "never" || (settings.footer === "fullscreen" && fullscreen);
}

let coverQueue = Promise.resolve();
const coverObserver = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue;
    const cover = entry.target;
    coverObserver.unobserve(cover);
    coverQueue = coverQueue.then(async () => {
      if (!cover.isConnected || $("library").hidden) return;
      const name = cover.dataset.folderName;
      const src = name ? await call("folder_image", {name}) : await call("cover", {book_id: cover.dataset.bookId});
      if (!cover.isConnected) return;
      const image = new Image();
      image.alt = "";
      image.addEventListener("load", () => cover.replaceChildren(image), {once: true});
      image.src = src;
    }).catch(() => {});
  }
}, {root: $("library"), rootMargin: "150px"});

let libraryEntries = new Map();
let libraryFolders = [];
let folderImages = new Set();
let libraryTags = new Map();
const tagFilters = new Map();
const searchPresets = new Map();
const presetEditor = $("preset-editor");
let editEntry = null;
let editTags = [];
let editCover = "keep";
let editBusy = false;
let previewRequest = 0;
const editor = $("book-editor");
const removeDialog = $("remove-dialog");
const folderDialog = $("folder-dialog");
const folderImageEditor = $("folder-image-editor");
const deleteFolderDialog = $("delete-folder-dialog");
const chapterDialog = $("chapter-dialog");
let folderBusy = false;
let editFolderName = null;
let folderImageChange = "keep";
let folderImageBusy = false;
let folderImageRequest = 0;
let chapterEdit = null;
let chapterEditBusy = false;

function selectOption([value, label]) {
  const option = document.createElement("option");
  option.value = value;
  option.textContent = label;
  return option;
}

function selectOptions(select, choices, preferred = select.value) {
  select.replaceChildren(...choices.map(selectOption));
  select.value = choices.some(([value]) => value === preferred) ? preferred : choices[0][0];
}

function folderOptions() {
  return [...libraryFolders].sort((a, b) => a.localeCompare(b)).map(name => ["folder:" + name, name]);
}

function libraryFolder() {
  const value = $("library-folder").value;
  return value.startsWith("folder:") ? value.slice(7) : null;
}

function currentLibraryFilters() {
  return {
    query: $("library-search").value.trim(),
    status: $("library-status").value,
    folder: $("library-folder").value,
  };
}

function matchesLibrary(entry, {query, status, folder, tags}) {
  if (status !== "all" && entry.archived !== (status === "archived")) return false;
  if (folder === "unfiled" && entry.folder) return false;
  if (folder.startsWith("folder:") && entry.folder !== folder.slice(7)) return false;
  if (tags.size) {
    const names = new Set(entry.tags.map(name => name.toLowerCase()));
    for (const [key, rule] of tags) {
      if (names.has(key) !== (rule.mode === "include")) return false;
    }
  }
  return !query || [entry.title, entry.author, entry.folder, ...entry.tags]
    .some(value => value?.toLowerCase().includes(query));
}

function filterLibrary() {
  const filters = {...currentLibraryFilters(), tags: tagFilters};
  filters.query = filters.query.toLowerCase();
  const browsingRoot = !filters.folder && !filters.query && !filters.tags.size;
  const folderGrid = $("folder-grid");
  const visibleFolders = browsingRoot ? folderGrid.children.length : 0;
  folderGrid.hidden = visibleFolders === 0;
  if (visibleFolders) {
    const folderCounts = new Map();
    for (const entry of libraryEntries.values()) {
      if (entry.folder && (filters.status === "all" || entry.archived === (filters.status === "archived"))) {
        folderCounts.set(entry.folder, (folderCounts.get(entry.folder) || 0) + 1);
      }
    }
    for (const tile of folderGrid.children) {
      const count = folderCounts.get(tile.dataset.folderName) || 0;
      tile.querySelector(".card-position").textContent = count + (count === 1 ? " book" : " books");
    }
  }
  let visible = 0;
  for (const card of $("book-grid").children) {
    const entry = libraryEntries.get(card.dataset.bookId);
    card.hidden = (browsingRoot && Boolean(entry.folder)) || !matchesLibrary(entry, filters);
    if (!card.hidden) visible++;
  }
  const folder = libraryFolder();
  $("library-breadcrumb").hidden = !folder && filters.folder !== "unfiled";
  $("library-location").textContent = folder || "Unfiled";
  const emptyLibrary = libraryEntries.size === 0 && libraryFolders.length === 0;
  $("library-empty").hidden = !emptyLibrary;
  const emptyFolder = folder && ![...libraryEntries.values()].some(entry => entry.folder === folder);
  $("library-no-matches").hidden = emptyLibrary || visible > 0 || visibleFolders > 0;
  $("library-no-matches").textContent = emptyFolder ? "This folder is empty." : "No books match these filters.";
  $("library-edit-hint").hidden = visible === 0 && visibleFolders === 0;
  const bookCount = visible + (visible === 1 ? " book" : " books");
  $("library-count").textContent = emptyLibrary ? "No books yet" : visibleFolders
    ? visibleFolders + (visibleFolders === 1 ? " folder" : " folders") + " · " + bookCount + " unfiled"
    : bookCount;
}

function browseLibrary(value) {
  const previous = libraryFolder();
  $("library-folder").value = value;
  filterLibrary();
  $("library").scrollTop = 0;
  const previousTile = [...$("folder-grid").children].find(tile => tile.dataset.folderName === previous);
  (value ? $("library-root") : previousTile && !$("folder-grid").hidden ? previousTile : $("library-search"))
    .focus({preventScroll: true});
}

function updateTagSummary() {
  const included = [...tagFilters.values()].filter(rule => rule.mode === "include").length;
  const excluded = tagFilters.size - included;
  const summary = [included && included + " included", excluded && excluded + " excluded"].filter(Boolean).join(" · ") || "All tags";
  $("library-tag-summary").textContent = summary;
  $("library-tag-toggle").setAttribute("aria-label", "Tags: " + summary);
  $("clear-tag-filters").disabled = !tagFilters.size;
}

function updateTagOption(button, name, mode) {
  button.dataset.mode = mode;
  button.firstElementChild.textContent = mode === "include" ? "✓" : mode === "exclude" ? "×" : "";
  const state = mode === "include" ? "Included. Activate to exclude."
    : mode === "exclude" ? "Excluded. Activate to clear." : "Clear. Activate to include.";
  button.setAttribute("aria-label", name + ": " + state);
}

function renderTagOptions() {
  const query = $("library-tag-search").value.trim().toLowerCase();
  const choices = new Map(libraryTags);
  for (const [key, rule] of tagFilters) {
    if (!choices.has(key)) choices.set(key, rule.name);
  }
  const buttons = [];
  for (const [key, name] of [...choices].sort((a, b) => a[1].localeCompare(b[1]))) {
    if (!key.includes(query)) continue;
    const button = document.createElement("button");
    button.type = "button";
    button.className = "tag-filter-option";
    const symbol = document.createElement("span");
    symbol.className = "tag-filter-symbol";
    symbol.setAttribute("aria-hidden", "true");
    const label = document.createElement("span");
    label.textContent = name;
    button.append(symbol, label);
    updateTagOption(button, name, tagFilters.get(key)?.mode || "");
    button.addEventListener("click", () => {
      const mode = tagFilters.get(key)?.mode;
      const next = mode === "include" ? "exclude" : mode === "exclude" ? "" : "include";
      if (next) tagFilters.set(key, {name, mode: next});
      else tagFilters.delete(key);
      updateTagOption(button, name, next);
      updateTagSummary();
      filterLibrary();
    });
    buttons.push(button);
  }
  $("library-tag-options").replaceChildren(...buttons);
  $("library-tag-empty").hidden = buttons.length > 0;
  $("library-tag-empty").textContent = choices.size ? "No matching tags." : "No tags yet. Add tags from Edit book.";
}

function showTagPicker(open) {
  $("library-tag-panel").hidden = !open;
  $("library-tag-toggle").setAttribute("aria-expanded", open);
  if (open) {
    $("library-tag-search").value = "";
    renderTagOptions();
    $("library-tag-search").focus({preventScroll: true});
  }
}

function updatePresetOptions(preferred) {
  const choices = [...searchPresets].sort((a, b) => a[1].name.localeCompare(b[1].name))
    .map(([key, preset]) => [key, preset.name]);
  selectOptions($("search-presets"), [["", "Choose a preset"], ...choices], preferred);
  $("search-presets").disabled = !choices.length;
  $("load-search-preset").disabled = !$("search-presets").value;
}

function updatePresetHint() {
  const preset = searchPresets.get($("preset-name").value.trim().toLowerCase());
  $("preset-hint").textContent = preset
    ? "Replace the saved filters for “" + preset.name + "” with the current filters."
    : "Save the current search, folder, tags, and status.";
  $("save-preset").textContent = preset ? "Overwrite preset" : "Save preset";
}

function saveSearchPreset(event) {
  event.preventDefault();
  const input = $("preset-name");
  const name = input.value.trim();
  if (!name) {
    input.value = "";
    input.reportValidity();
    return;
  }
  const key = name.toLowerCase();
  searchPresets.set(key, {name, ...currentLibraryFilters(), tags: [...tagFilters.values()]});
  updatePresetOptions(key);
  presetEditor.close();
  persist();
}

function loadSearchPreset() {
  const preset = searchPresets.get($("search-presets").value);
  if (!preset) return;
  $("library-search").value = preset.query;
  $("library-status").value = preset.status;
  $("library-folder").value = preset.folder;
  tagFilters.clear();
  for (const rule of preset.tags) tagFilters.set(rule.name.toLowerCase(), rule);
  updateTagSummary();
  showTagPicker(false);
  filterLibrary();
  if (preset.folder && !$("library-folder").value) notice("The preset’s folder is unavailable. Showing the main library.");
}

function updateLibraryOptions() {
  const select = $("library-folder");
  const selected = select.value;
  selectOptions(select, [["", "Organized view"], ["all-books", "Show all books"], ["unfiled", "Unfiled"]]);
  const folders = folderOptions();
  if (folders.length) {
    const group = document.createElement("optgroup");
    group.label = "Folders";
    const heading = document.createElement("legend");
    heading.textContent = group.label;
    group.append(heading, ...folders.map(selectOption));
    select.append(group);
    if (folders.some(([value]) => value === selected)) select.value = selected;
  }
  libraryTags = new Map();
  for (const entry of libraryEntries.values()) {
    for (const tag of entry.tags) {
      const key = tag.toLowerCase();
      if (!libraryTags.has(key)) libraryTags.set(key, tag);
    }
  }
  if (!$("library-tag-panel").hidden) renderTagOptions();
}

function renderFolderTiles() {
  const grid = $("folder-grid");
  for (const image of grid.querySelectorAll(".folder-art[data-folder-name]")) coverObserver.unobserve(image);
  const tiles = document.createDocumentFragment();
  for (const [, name] of folderOptions()) {
    const tile = document.createElement("button");
    tile.className = "folder-card";
    tile.dataset.folderName = name;
    const art = document.createElement("span");
    art.className = "book-cover folder-art";
    art.setAttribute("aria-hidden", "true");
    art.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
    if (folderImages.has(name)) {
      art.dataset.folderName = name;
      coverObserver.observe(art);
    }
    const title = document.createElement("span");
    title.className = "card-title";
    title.textContent = name;
    const count = document.createElement("span");
    count.className = "card-position";
    tile.append(art, title, count);
    tiles.append(tile);
  }
  grid.replaceChildren(tiles);
}

function renderLibrary(entries) {
  libraryEntries = new Map(entries.map(entry => [entry.id, entry]));
  coverObserver.disconnect();
  updateLibraryOptions();
  renderFolderTiles();
  const cards = document.createDocumentFragment();
  for (const entry of entries) {
    const card = document.createElement("button");
    card.className = "book-card";
    card.dataset.bookId = entry.id;
    const cover = document.createElement("span");
    cover.className = "book-cover";
    cover.setAttribute("aria-hidden", "true");
    cover.innerHTML = '<svg viewBox="0 0 24 24"><path d="M3 5q5-2 9 1.5Q16 3 21 5v14q-5-2-9 1.5Q8 17 3 19zM12 6.5v14"/></svg>';
    if (entry.cover) {
      cover.dataset.bookId = entry.id;
      coverObserver.observe(cover);
    }
    const title = document.createElement("span");
    title.className = "card-title";
    title.textContent = entry.title;
    const author = document.createElement("span");
    author.className = "card-author";
    author.textContent = entry.author || "Unknown author";
    const labels = document.createElement("span");
    labels.className = "card-labels";
    const markers = [entry.archived ? "Archived" : "", entry.folder, ...entry.tags.slice(0, 3)].filter(Boolean);
    if (entry.tags.length > 3) markers.push("+" + (entry.tags.length - 3));
    labels.textContent = markers.join(" · ");
    labels.hidden = markers.length === 0;
    const position = document.createElement("span");
    position.className = "card-position";
    position.textContent = entry.position
      ? entry.resume_number ? "Resume · Chapter " + entry.resume_number + " of " + entry.chapters : "Resume reading"
      : entry.chapters + (entry.chapters === 1 ? " chapter" : " chapters");
    card.append(cover, title, author, labels, position);
    cards.append(card);
  }
  $("book-grid").replaceChildren(cards);
  filterLibrary();
}

function showError(id, message = "") {
  $(id).textContent = message;
  $(id).hidden = !message;
}

const editError = message => showError("edit-error", message);

function setEditorBusy(busy) {
  editBusy = busy;
  for (const id of ["pick-cover", "restore-cover", "cancel-edit", "save-edit", "remove-book"]) $(id).disabled = busy;
}

async function previewCover(src) {
  const request = ++previewRequest;
  const preview = $("edit-cover-preview");
  if (!src) {
    preview.hidden = true;
    preview.removeAttribute("src");
    return;
  }
  const decoded = new Image();
  decoded.src = src;
  await decoded.decode();
  if (request !== previewRequest || !editor.open) return;
  preview.src = src;
  preview.hidden = false;
}

async function loadEditorCover(embedded = false) {
  const entry = editEntry;
  const request = ++previewRequest;
  if (embedded ? !entry.embedded_cover : !entry.cover) {
    await previewCover(null);
    return;
  }
  const src = await call("cover", {book_id: entry.id, embedded}).catch(() => null);
  if (request !== previewRequest || editEntry !== entry || !editor.open) return;
  await previewCover(src).catch(() => {});
}

function renderTagChips() {
  const chips = document.createDocumentFragment();
  editTags.forEach((tag, index) => {
    const chip = document.createElement("span");
    chip.className = "tag-chip";
    const label = document.createElement("span");
    label.textContent = tag;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "×";
    remove.setAttribute("aria-label", "Remove tag " + tag);
    const removeTag = () => {
      editTags.splice(index, 1);
      renderTagChips();
      $("edit-tags").focus();
    };
    remove.addEventListener("click", removeTag);
    remove.addEventListener("keydown", event => {
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        removeTag();
      }
    });
    chip.append(label, remove);
    chips.append(chip);
  });
  $("tag-chips").replaceChildren(chips);
}

function addDraftTag() {
  const input = $("edit-tags");
  const tag = input.value.trim();
  if (!tag) { input.value = ""; return true; }
  if (!editTags.some(existing => existing.toLowerCase() === tag.toLowerCase())) {
    if (editTags.length >= 32 || [...tag].length > 64) {
      editError("Use up to 32 tags with at most 64 characters each.");
      return false;
    }
    editTags.push(tag);
    renderTagChips();
  }
  input.value = "";
  editError();
  return true;
}

function openEditor(id) {
  if (editor.open || opening || importing) return;
  const entry = libraryEntries.get(id);
  if (!entry) return;
  editEntry = entry;
  editCover = "keep";
  $("edit-title").value = entry.title;
  $("edit-author").value = entry.author;
  selectOptions($("edit-folder"), [["", "Unfiled"], ...folderOptions()], entry.folder ? "folder:" + entry.folder : "");
  editTags = [...entry.tags];
  $("edit-tags").value = "";
  renderTagChips();
  $("edit-archived").checked = entry.archived;
  $("restore-cover").hidden = !entry.custom_cover;
  editError();
  previewCover(null);
  editor.showModal();
  $("edit-title").focus();
  loadEditorCover();
}

async function chooseCover() {
  if (editBusy || !editEntry) return;
  setEditorBusy(true);
  editError();
  try {
    const src = await call("pick_cover", {book_id: editEntry.id});
    if (!src) return;
    try {
      await previewCover(src);
      editCover = "replace";
      $("restore-cover").hidden = false;
    } catch {
      editCover = "keep";
      $("restore-cover").hidden = !editEntry.custom_cover;
      await loadEditorCover();
      throw new Error("This image could not be opened. Choose another cover.");
    }
  } catch (error) { editError(error.message); }
  finally { setEditorBusy(false); }
}

async function saveBookEdit(event) {
  event.preventDefault();
  if (!editEntry || editBusy || !addDraftTag()) return;
  setEditorBusy(true);
  editError();
  const id = editEntry.id;
  try {
    const entries = await call("edit_book", {
      book_id: id,
      details: {
        title: $("edit-title").value, author: $("edit-author").value, cover: editCover,
        folder: $("edit-folder").value.slice(7) || null,
        tags: editTags, archived: $("edit-archived").checked,
      },
    });
    editor.close();
    renderLibrary(entries);
    const visible = Array.from($("book-grid").children).find(card => card.dataset.bookId === id && !card.hidden);
    (visible || $("library-status")).focus({preventScroll: true});
  } catch (error) { editError(error.message); }
  finally { setEditorBusy(false); }
}

function setFolderImageBusy(busy) {
  folderImageBusy = busy;
  for (const id of ["pick-folder-image", "restore-folder-image", "cancel-folder-image", "save-folder-image", "delete-folder"]) {
    $(id).disabled = busy;
  }
}

function showFolderImage(src) {
  const image = $("folder-image-preview");
  image.hidden = !src;
  if (src) image.src = src;
  else image.removeAttribute("src");
}

async function loadFolderImage() {
  const name = editFolderName;
  const request = ++folderImageRequest;
  if (!folderImages.has(name)) { showFolderImage(null); return; }
  const src = await call("folder_image", {name}).catch(() => null);
  if (request === folderImageRequest && folderImageEditor.open) showFolderImage(src);
}

function openFolderImageEditor(name) {
  if (folderImageEditor.open || opening || importing || !libraryFolders.includes(name)) return;
  editFolderName = name;
  folderImageChange = "keep";
  $("folder-image-name").textContent = name;
  $("restore-folder-image").hidden = !folderImages.has(name);
  showFolderImage(null);
  showError("folder-image-error");
  folderImageEditor.showModal();
  loadFolderImage();
}

async function chooseFolderImage() {
  if (folderImageBusy || !editFolderName) return;
  setFolderImageBusy(true);
  showError("folder-image-error");
  try {
    const src = await call("pick_folder_image", {name: editFolderName});
    if (!src) return;
    try {
      const decoded = new Image();
      decoded.src = src;
      await decoded.decode();
    } catch {
      folderImageChange = "keep";
      $("restore-folder-image").hidden = !folderImages.has(editFolderName);
      await loadFolderImage();
      throw new Error("This image could not be opened. Choose another image.");
    }
    ++folderImageRequest;
    folderImageChange = "replace";
    $("restore-folder-image").hidden = false;
    showFolderImage(src);
  } catch (error) { showError("folder-image-error", error.message); }
  finally { setFolderImageBusy(false); }
}

async function saveFolderImage() {
  if (folderImageBusy || !editFolderName) return;
  if (folderImageChange === "keep") { folderImageEditor.close(); return; }
  setFolderImageBusy(true);
  showError("folder-image-error");
  const name = editFolderName;
  try {
    folderImages = new Set(await call("set_folder_image", {name, change: folderImageChange}));
    folderImageEditor.close();
    renderFolderTiles();
    filterLibrary();
    [...$("folder-grid").children].find(tile => tile.dataset.folderName === name)?.focus({preventScroll: true});
  } catch (error) { showError("folder-image-error", error.message); }
  finally { setFolderImageBusy(false); }
}

function askDeleteFolder() {
  if (folderImageBusy || !editFolderName) return;
  $("delete-folder-description").textContent =
    'Delete “' + editFolderName + '”? Its books will move to My Library as unfiled.';
  showError("delete-folder-error");
  deleteFolderDialog.showModal();
}

async function deleteFolder() {
  if (folderImageBusy || !editFolderName || !deleteFolderDialog.open) return;
  setFolderImageBusy(true);
  $("confirm-delete-folder").disabled = $("cancel-delete-folder").disabled = true;
  showError("delete-folder-error");
  try {
    const result = await call("delete_folder", {name: editFolderName});
    libraryFolders = result.folders;
    folderImages = new Set(result.folder_images);
    deleteFolderDialog.close();
    folderImageEditor.close();
    renderLibrary(result.library);
    $("library-search").focus({preventScroll: true});
    const count = result.unfiled;
    notice(count === 0 ? "Folder deleted." : count + (count === 1 ? " book moved" : " books moved") + " to the main library.");
  } catch (error) { showError("delete-folder-error", error.message); }
  finally {
    setFolderImageBusy(false);
    $("confirm-delete-folder").disabled = $("cancel-delete-folder").disabled = false;
  }
}

function askRemoveBook() {
  if (editBusy || !editEntry) return;
  const detail = editEntry.managed
    ? " Folio will delete its imported copy. Your original file stays where it is."
    : "";
  $("remove-description").textContent = 'Remove “' + editEntry.title + '” from My Library?' + detail;
  showError("remove-error");
  removeDialog.showModal();
}

async function removeBook() {
  if (editBusy || !editEntry || !removeDialog.open) return;
  setEditorBusy(true);
  $("confirm-remove").disabled = $("cancel-remove").disabled = true;
  showError("remove-error");
  try {
    const result = await call("remove_book", {book_id: editEntry.id});
    removeDialog.close();
    editor.close();
    renderLibrary(result.library);
    $("library-search").focus({preventScroll: true});
    if (result.warning) notice(result.warning);
  } catch (error) { showError("remove-error", error.message); }
  finally {
    setEditorBusy(false);
    $("confirm-remove").disabled = $("cancel-remove").disabled = false;
  }
}

async function createFolder(event) {
  event.preventDefault();
  if (folderBusy) return;
  folderBusy = true;
  $("create-folder").disabled = $("cancel-folder").disabled = true;
  showError("folder-error");
  const name = $("folder-name").value.trim();
  try {
    libraryFolders = await call("create_folder", {name});
    updateLibraryOptions();
    renderFolderTiles();
    filterLibrary();
    folderDialog.close();
    notice('Created folder “' + name + '”. Right-click a book to move it there.');
  } catch (error) { showError("folder-error", error.message); }
  finally {
    folderBusy = false;
    $("create-folder").disabled = $("cancel-folder").disabled = false;
  }
}

function setView(reading) {
  $("library").hidden = reading;
  $("reader").hidden = !reading;
  $("brand").hidden = reading;
  for (const id of ["library-back", "sidebar-toggle", "fullscreen", "reading-title", "toolbar-chapters"]) $(id).hidden = !reading;
  $("toolbar-previous-chapter").disabled = $("toolbar-next-chapter").disabled = true;
  $("drop-overlay").hidden = true;
  showTagPicker(false);
  showSettings(false);
}

function showSettings(open) {
  $("reading-settings").hidden = !open;
  $("settings-toggle").setAttribute("aria-expanded", open);
  if (open) $("scroll-mode").focus();
}

async function setFullscreen(enabled) {
  if (resizingWindow || enabled === fullscreen || !book) return;
  resizingWindow = true;
  const position = readPosition();
  try {
    await call("fullscreen", {enabled});
    fullscreen = enabled;
    showSettings(false);
    document.body.classList.toggle("full-reader", fullscreen);
    $("fullscreen").title = fullscreen ? "Exit full reader view (F11 or Esc)" : "Full reader view (F11)";
    $("fullscreen").setAttribute("aria-label", fullscreen ? "Exit full reader view" : "Enter full reader view");
    $("exit-fullscreen").hidden = !fullscreen;
    updateFooterVisibility();
    if (fullscreen) {
      restoreFind = !$("find").hidden;
      $("find").hidden = true;
    } else if (restoreFind) {
      $("find").hidden = false;
      restoreFind = false;
    }
    layout(position);
    viewport.focus({preventScroll: true});
  } catch (error) {
    notice(error.message);
  } finally {
    resizingWindow = false;
  }
}

async function returnLibrary() {
  if (!book) return;
  if (fullscreen) await setFullscreen(false);
  anchor = readPosition();
  persist();
  ++loadToken;
  viewport.hidden = true;
  closeFind({focus: false, clear: true});
  content.replaceChildren();
  indexText();
  chapter = -1;
  book = null;
  anchor = {offset: 0, fraction: 0};
  setView(false);
  try {
    renderLibrary(await call("close_book"));
    $("add-books").focus({preventScroll: true});
  } catch (error) { notice(error.message); }
}

async function chooseBook(bookId) {
  if (opening || importing) return;
  opening = true;
  try { await useBook(await call("open_book", {book_id: bookId})); }
  catch (error) { notice(error.message); }
  finally { opening = false; }
}

function importStatus(errors) {
  if (errors.length) notice(errors.slice(0, 3).join(" · ") + (errors.length > 3 ? " · More files could not be added." : ""));
}

async function addFiles() {
  if (importing || opening) return;
  importing = true;
  $("add-books").disabled = true;
  try {
    if (book) await returnLibrary();
    const result = await call("import", {folder: libraryFolder()});
    renderLibrary(result.library);
    importStatus(result.errors);
  } catch (error) { notice(error.message); }
  finally {
    importing = false;
    $("add-books").disabled = false;
  }
}

function encodeSlice(slice) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("load", () => resolve(reader.result.split(",", 2)[1]), {once: true});
    reader.addEventListener("error", () => reject(reader.error), {once: true});
    reader.readAsDataURL(slice);
  });
}

async function addDroppedFiles(files) {
  if (book || importing || opening || !files.length) return;
  importing = true;
  $("add-books").disabled = true;
  const errors = [];
  const folder = libraryFolder();
  const chunk = 192 * 1024;
  try {
    for (const file of files) {
      if (!/\.epub$/i.test(file.name)) { errors.push(file.name + " is not an EPUB."); continue; }
      try {
        await call("drop_begin", {name: file.name, length: file.size});
        for (let offset = 0; offset < file.size; offset += chunk) {
          const data = await encodeSlice(file.slice(offset, offset + chunk));
          await call("drop_chunk", {data});
        }
        renderLibrary(await call("drop_finish", {folder}));
      } catch (error) {
        await call("drop_abort").catch(() => {});
        errors.push(file.name + " · " + error.message);
      }
    }
    importStatus(errors);
  } finally {
    importing = false;
    $("add-books").disabled = false;
  }
}

function calculateChapterNumbers() {
  chapterTotal = 0;
  chapterNumbers = book.sections.map((section, index) =>
    (book.numbering?.[index] ?? !section.front_matter) ? ++chapterTotal : null);
}

function chapterList(scrollActive = false) {
  if (!book) return;
  const query = $("chapter-filter").value.trim().toLowerCase();
  const number = query.match(/^(?:chapter\s*|#\s*)?0*(\d+)$/i)?.[1];
  const fragment = document.createDocumentFragment();
  let visible = 0;
  book.sections.forEach((section, index) => {
    const shownNumber = chapterNumbers[index];
    if (number !== undefined ? Number(number) !== shownNumber : !section.title.toLowerCase().includes(query)) return;
    visible++;
    const button = document.createElement("button");
    button.className = "chapter-row";
    button.dataset.index = index;
    const ordinal = document.createElement("span");
    ordinal.className = "ordinal";
    ordinal.textContent = shownNumber === null ? "" : String(shownNumber).padStart(2, "0");
    if (shownNumber === null) button.title = "Unnumbered · right-click to count as a chapter";
    const label = document.createElement("span");
    label.textContent = section.title;
    button.append(ordinal, label);
    if (index === chapter) button.setAttribute("aria-current", "location");
    fragment.append(button);
  });
  $("chapters").replaceChildren(fragment);
  const unnumbered = book.sections.length - chapterTotal;
  $("chapter-count").textContent = query
    ? visible + " of " + book.sections.length + " entries"
    : chapterTotal + (chapterTotal === 1 ? " chapter" : " chapters") +
      (unnumbered ? " · " + unnumbered + " unnumbered" : "");
  if (scrollActive) $("chapters").querySelector('[aria-current="location"]')?.scrollIntoView({block: "nearest"});
}

function openChapterEditor(index) {
  if (!book?.sections[index] || chapterDialog.open) return;
  const numbered = chapterNumbers[index] === null;
  chapterEdit = {book_id: book.id, index, numbered};
  $("chapter-dialog-title").textContent = book.sections[index].title;
  $("toggle-chapter-number").textContent = numbered ? "Count as chapter" : "Mark as non-chapter";
  showError("chapter-dialog-error");
  chapterDialog.showModal();
}

async function saveChapterNumber() {
  if (!chapterEdit || chapterEditBusy) return;
  chapterEditBusy = true;
  $("toggle-chapter-number").disabled = $("cancel-chapter-edit").disabled = true;
  showError("chapter-dialog-error");
  const index = chapterEdit.index;
  try {
    book.numbering = await call("set_numbered", chapterEdit);
    calculateChapterNumbers();
    chapterDialog.close();
    chapterList();
    updateFooter();
    ($("chapters").querySelector('[data-index="' + index + '"]') || $("chapter-filter")).focus({preventScroll: true});
  } catch (error) { showError("chapter-dialog-error", error.message); }
  finally {
    chapterEditBusy = false;
    $("toggle-chapter-number").disabled = $("cancel-chapter-edit").disabled = false;
  }
}

async function useBook(next) {
  ++loadToken;
  book = next;
  calculateChapterNumbers();
  chapter = -1;
  content.replaceChildren();
  viewport.hidden = true;
  $("reader-loading").textContent = "Loading chapter…";
  $("reader-loading").hidden = false;
  setView(true);
  $("reading-title").textContent = book.title;
  $("location").textContent = "Opening " + book.title + "…";
  $("book-title").textContent = book.title;
  $("book-author").textContent = book.author;
  $("chapter-filter").disabled = false;
  $("chapter-filter").value = "";
  closeFind({focus: false, clear: true});
  chapterList();
  const saved = book.position;
  await loadChapter(clamp(saved.chapter, 0, book.sections.length - 1), saved);
}

async function loadChapter(index, position = {offset: 0, fraction: 0}, fragment = null) {
  if (!book?.sections[index]) return;
  if (chapter >= 0 && index !== chapter) {
    anchor = readPosition();
    persist();
  }
  const token = ++loadToken;
  const bookId = book.id;
  const section = book.sections[index];
  try {
    const source = await call("chapter", {book_id: bookId, index});
    if (token !== loadToken) return;
    const clean = sanitize(source, section);
    markSpacers(clean);
    content.replaceChildren(clean);
    indexText();
    chapter = index;
    $("reader-loading").hidden = true;
    viewport.hidden = false;
    $("previous-chapter").disabled = $("toolbar-previous-chapter").disabled = index === 0;
    $("next-chapter").disabled = $("toolbar-next-chapter").disabled = index + 1 === book.sections.length;
    $("location").textContent = section.title;
    layout(position);
    if (fragment) jumpTo(elementAt(content, fragment));
    chapterList(true);
    if ($("find").hidden) viewport.focus({preventScroll: true});
    if (!$("find").hidden || $("query").value) refreshFind(false);
    persist();
    const images = Array.from(content.querySelectorAll("img[data-epub-src]")).slice(0, 100);
    for (const image of images) {
      if (token !== loadToken) break;
      try {
        const data = await call("image", {book_id: bookId, path: image.dataset.epubSrc});
        if (token !== loadToken) break;
        image.addEventListener("load", queueLayout, {once: true});
        image.src = data;
      } catch {
        image.replaceWith(document.createTextNode(image.alt || "[Image unavailable]"));
        indexText();
        queueLayout();
      }
    }
  } catch (error) {
    if (token === loadToken) {
      if (viewport.hidden) $("reader-loading").textContent = error.message;
      notice(error.message);
    }
  }
}

const safeTags = new Set("a abbr b bdi bdo blockquote br caption cite code dd del div dl dt em figcaption figure h1 h2 h3 h4 h5 h6 hr i img kbd li mark ol p pre q rp rt ruby s samp section small span strong sub sup table tbody td th thead tr u ul var wbr".split(" "));
const skipTags = new Set("audio base button canvas embed form head iframe input link meta noscript object script select style template textarea title video".split(" "));

function internalPath(base, href) {
  if (!href || /^[a-z][a-z\d+.-]*:|^\/\/|\\/i.test(href)) return null;
  try {
    const origin = "https://folio-epub.invalid/";
    const url = new URL(href, origin + base.split("/").map(encodeURIComponent).join("/"));
    if (url.origin !== new URL(origin).origin) return null;
    return {path: decodeURIComponent(url.pathname.slice(1)), fragment: decodeURIComponent(url.hash.slice(1))};
  } catch { return null; }
}

function elementAt(root, id) {
  return id ? Array.from(root.querySelectorAll("[data-epub-id]")).find(node => node.dataset.epubId === id) : null;
}

function sanitize(source, section) {
  const template = document.createElement("template");
  template.innerHTML = source;
  const clean = document.createDocumentFragment();
  function copy(from, to, depth = 0) {
    if (depth > 120) return;
    for (const node of from.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) { to.append(document.createTextNode(node.data)); continue; }
      if (node.nodeType !== Node.ELEMENT_NODE || skipTags.has(node.localName)) continue;
      const tag = node.localName === "image" ? "img" : node.localName;
      const id = node.getAttribute("id") || node.getAttribute("name");
      if (!safeTags.has(tag) && !id) { copy(node, to, depth + 1); continue; }
      const output = document.createElement(safeTags.has(tag) ? tag : "div");
      if (id) output.dataset.epubId = id;
      if (["ltr", "rtl", "auto"].includes(node.getAttribute("dir"))) output.dir = node.getAttribute("dir");
      if (node.hasAttribute("lang")) output.lang = node.getAttribute("lang");
      if (tag === "img") {
        const href = node.getAttribute("src") || node.getAttribute("href") || node.getAttribute("xlink:href");
        const asset = internalPath(section.path, href);
        if (!asset) continue;
        output.dataset.epubSrc = asset.path;
        output.alt = node.getAttribute("alt") || "";
        for (const dimension of ["width", "height"]) {
          const value = node.getAttribute(dimension);
          if (/^\d{1,5}$/.test(value || "")) output.setAttribute(dimension, value);
        }
      } else {
        if (tag === "a") {
          const dest = internalPath(section.path, node.getAttribute("href"));
          if (dest) {
            output.href = "#";
            output.dataset.epubPath = dest.path;
            output.dataset.epubFragment = dest.fragment;
          }
        }
        for (const attr of ["start", "value", "colspan", "rowspan"]) {
          const value = node.getAttribute(attr);
          if (/^-?\d{1,4}$/.test(value || "")) output.setAttribute(attr, value);
        }
        copy(node, output, depth + 1);
      }
      to.append(output);
    }
  }
  copy(template.content, clean);
  if (!section.start && !section.end) return clean;
  // Range boundaries refer to EPUB anchors without importing IDs into the app UI.
  const wrapper = document.createElement("div");
  wrapper.append(clean);
  const range = document.createRange();
  range.selectNodeContents(wrapper);
  const start = elementAt(wrapper, section.start);
  const end = elementAt(wrapper, section.end);
  if (start) range.setStartBefore(start);
  if (end) range.setEndBefore(end);
  return range.cloneContents();
}

function markSpacers(root) {
  const blank = text => /^[\s\u200B\u2060]*$/u.test(text);
  for (const block of root.querySelectorAll("p, div")) {
    if (blank(block.textContent) && !block.closest("pre, table") && !block.hasAttribute("data-epub-id")
        && !block.querySelector("img, hr, table, pre, [data-epub-id]")) {
      block.classList.add("epub-spacer");
    }
  }
  for (const line of root.querySelectorAll("br")) {
    if (line.closest("pre, table")) continue;
    let previous = line.previousSibling;
    while (previous?.nodeType === Node.TEXT_NODE && blank(previous.data)) previous = previous.previousSibling;
    if (previous?.localName === "br" && !line.hasAttribute("data-epub-id")) line.classList.add("epub-spacer");
  }
}

function indexText() {
  textRuns = [];
  runOffsets = new WeakMap();
  let priorBlock;
  let offset = 0;
  const chunks = [];
  const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!node.length) continue;
    const block = node.parentElement.closest("p,div,li,section,pre,blockquote,dt,dd,td,th,h1,h2,h3,h4,h5,h6") || content;
    if (priorBlock && block !== priorBlock) { chunks.push("\n"); offset++; }
    const run = {node, start: offset};
    textRuns.push(run);
    runOffsets.set(node, run);
    chunks.push(node.data);
    offset += node.length;
    priorBlock = block;
  }
  plain = chunks.join("");
}

function pointAt(offset) {
  if (!textRuns.length) return null;
  let lo = 0, hi = textRuns.length;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >>> 1;
    if (textRuns[mid].start <= offset) lo = mid;
    else hi = mid;
  }
  const {node, start} = textRuns[lo];
  return [node, clamp(offset - start, 0, node.length)];
}

function rangeAt(start, end) {
  const left = pointAt(start), right = pointAt(end);
  if (!left || !right) return null;
  const range = document.createRange();
  range.setStart(...left);
  range.setEnd(...right);
  return range;
}

function readPosition() {
  if (viewport.hidden) return anchor;
  const spread = isSpread();
  const maximum = spread ? spreads - 1 : viewport.scrollHeight - viewport.clientHeight;
  const current = spread ? pageIndex() : viewport.scrollTop;
  const fraction = maximum > 0 ? clamp(current / maximum, 0, 1) : 0;
  if (current < 1) return {offset: 0, fraction: 0};
  const bounds = viewport.getBoundingClientRect();
  for (const y of [12, 28, 47, 72, 105]) {
    if (y >= bounds.height - 5) break;
    for (const x of [.18, .33, .48, .62]) {
      const caret = document.caretRangeFromPoint(bounds.left + bounds.width * x, bounds.top + y);
      const run = runOffsets.get(caret?.startContainer);
      if (!run) continue;
      const rect = caret.getBoundingClientRect();
      if (rect.bottom < bounds.top || rect.top > bounds.top + Math.max(y + 36, 60)) continue;
      return {offset: run.start + caret.startOffset, fraction};
    }
  }
  return {offset: 0, fraction};
}

function jumpTo(item) {
  const rect = item?.getClientRects()[0];
  if (!rect) return false;
  const bounds = viewport.getBoundingClientRect();
  if (isSpread()) {
    const x = viewport.scrollLeft + rect.left - bounds.left;
    viewport.scrollLeft = clamp(Math.floor((x + 2) / stride), 0, spreads - 1) * stride;
  } else {
    viewport.scrollTop += rect.top - bounds.top - 14;
  }
  return true;
}

function layout(position = anchor) {
  if (viewport.hidden) {
    $("page-controls").hidden = true;
    queueSearchMarkers();
    return;
  }
  const spread = isSpread();
  const width = viewport.clientWidth;
  if (!width) return;
  rail.style.width = "";
  if (spread) {
    const gap = width < 540 ? 22 : 40;
    flow.style.setProperty("--sheet-width", width + "px");
    flow.style.setProperty("--sheet-gap", gap + "px");
    flow.style.setProperty("--image-height", Math.max(45, viewport.clientHeight - 52) + "px");
    stride = width + gap;
    spreads = Math.max(1, 1 + Math.ceil((flow.scrollWidth - width - 2) / stride));
    rail.style.width = (width + stride * (spreads - 1)) + "px";
    viewport.scrollTop = 0;
    viewport.scrollLeft = Math.round((position.fraction || 0) * (spreads - 1)) * stride;
  } else {
    spreads = 1;
    viewport.scrollLeft = 0;
    viewport.scrollTop = (position.fraction || 0) * (viewport.scrollHeight - viewport.clientHeight);
  }
  if (position.offset > 0) jumpTo(rangeAt(position.offset, position.offset + 1));
  anchor = readPosition();
  updateFooter();
  queueSearchMarkers();
}

function queueLayout() {
  if (layoutFrame) return;
  const position = anchor;
  layoutFrame = requestAnimationFrame(() => {
    layoutFrame = 0;
    layout(position);
  });
}

function updateFooter() {
  $("page-controls").hidden = !isSpread() || chapter < 0;
  if (chapter < 0) return;
  const page = pageIndex();
  $("pages").textContent = "Spread " + (page + 1) + " / " + spreads;
  $("page-back").disabled = $("margin-back").disabled = page === 0;
  $("page-forward").disabled = $("margin-forward").disabled = page === spreads - 1;
  const number = chapterNumbers[chapter];
  $("progress").textContent = number === null
    ? "Unnumbered · " + Math.round(anchor.fraction * 100) + "% of section"
    : number + " / " + chapterTotal + " chapters · " + Math.round(anchor.fraction * 100) + "% of chapter";
}

function turn(delta) {
  if (chapter < 0 || !isSpread()) return;
  viewport.scrollLeft = clamp(pageIndex() + delta, 0, spreads - 1) * stride;
  anchor = readPosition();
  updateFooter();
  persist();
}

function clearHighlights() {
  CSS.highlights.delete("folio-matches");
  CSS.highlights.delete("folio-active");
}

function searchMarkerAxis() {
  const bounds = viewport.getBoundingClientRect();
  return isSpread()
    ? {horizontal: true, origin: bounds.left + viewport.clientLeft - viewport.scrollLeft, length: viewport.scrollWidth}
    : {horizontal: false, origin: bounds.top + viewport.clientTop - viewport.scrollTop, length: viewport.scrollHeight};
}

function searchMarkerPosition(match, axis) {
  const rect = match?.getClientRects()[0];
  if (!rect || !axis.length) return null;
  const center = axis.horizontal ? rect.left + rect.width / 2 : rect.top + rect.height / 2;
  return Math.round(1000 * clamp((center - axis.origin) / axis.length, 0, 1));
}

function searchMarkerPath(position, horizontal) {
  if (position === null) return "";
  return horizontal ? "M" + position + " 0v1000" : "M0 " + position + "h1000";
}

function updateActiveSearchMarker(axis) {
  if ($("search-markers").hasAttribute("hidden")) return;
  axis ??= searchMarkerAxis();
  const position = searchMarkerPosition(matches[currentMatch], axis);
  $("search-active-mark").setAttribute("d", searchMarkerPath(position, axis.horizontal));
}

function queueSearchMarkers() {
  const track = $("search-markers");
  const hidden = !settings.searchMarkers || viewport.hidden || !matches.length;
  track.toggleAttribute("hidden", hidden);
  if (hidden || markerFrame) return;
  markerFrame = requestAnimationFrame(() => {
    markerFrame = 0;
    if (track.hasAttribute("hidden")) return;
    const axis = searchMarkerAxis();
    const positions = new Set(matches.map(match => searchMarkerPosition(match, axis)));
    $("search-match-marks").setAttribute("d", [...positions].map(position =>
      searchMarkerPath(position, axis.horizontal)).join(""));
    updateActiveSearchMarker(axis);
  });
}

function refreshFind(jump = true) {
  clearHighlights();
  matches = [];
  currentMatch = -1;
  const query = $("query").value.trim();
  if (query) {
    const pattern = query.split(/\s+/u).map(word => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("\\s+");
    const expression = new RegExp(pattern, "giu");
    for (const result of plain.matchAll(expression)) {
      const range = rangeAt(result.index, result.index + result[0].length);
      if (range) matches.push(range);
    }
    const highlight = new Highlight();
    for (const range of matches) highlight.add(range);
    CSS.highlights.set("folio-matches", highlight);
  }
  if (matches.length && jump) selectMatch(0);
  else matchLabel(query);
  queueSearchMarkers();
}

function matchLabel(query = $("query").value.trim()) {
  $("match-count").textContent = matches.length
    ? (currentMatch < 0 ? matches.length + " found" : (currentMatch + 1) + " / " + matches.length)
    : query ? "No matches" : "";
  $("prev-match").disabled = $("next-match").disabled = matches.length === 0;
}

function selectMatch(index) {
  if (!matches.length) return;
  currentMatch = (index % matches.length + matches.length) % matches.length;
  CSS.highlights.set("folio-active", new Highlight(matches[currentMatch]));
  jumpTo(matches[currentMatch]);
  updateActiveSearchMarker();
  anchor = readPosition();
  updateFooter();
  saveSoon();
  matchLabel();
}

function openFind() {
  if (chapter < 0) return;
  const position = readPosition();
  if (fullscreen) restoreFind = true;
  $("find").hidden = false;
  layout(position);
  $("query").focus();
  $("query").select();
}

function lockFind(locked) {
  findLocked = locked;
  $("lock-find").setAttribute("aria-pressed", locked);
  $("lock-find").textContent = locked ? "Locked" : "Lock";
}

function closeFind({focus = true, clear = !findLocked} = {}) {
  if ($("find").hidden && !$("query").value && !restoreFind && !findLocked) return;
  const position = readPosition();
  restoreFind = false;
  $("find").hidden = true;
  const needsRefresh = clear || findTimer;
  clearTimeout(findTimer);
  findTimer = 0;
  if (clear) {
    $("query").value = "";
    lockFind(false);
  }
  if (needsRefresh) refreshFind(false);
  layout(position);
  if (focus && !viewport.hidden) viewport.focus({preventScroll: true});
}

$("add-books").addEventListener("click", addFiles);
$("library-root").addEventListener("click", () => browseLibrary(""));
$("library-search").addEventListener("input", filterLibrary);
for (const id of ["library-folder", "library-status"]) $(id).addEventListener("change", filterLibrary);
$("library-tag-toggle").addEventListener("click", () => showTagPicker($("library-tag-panel").hidden));
$("library-tag-search").addEventListener("input", renderTagOptions);
$("clear-tag-filters").addEventListener("click", () => {
  tagFilters.clear();
  updateTagSummary();
  renderTagOptions();
  filterLibrary();
  $("library-tag-search").focus({preventScroll: true});
});
$("search-presets").addEventListener("change", () => {
  $("load-search-preset").disabled = !$("search-presets").value;
});
$("load-search-preset").addEventListener("click", loadSearchPreset);
$("open-preset-editor").addEventListener("click", () => {
  showTagPicker(false);
  $("preset-name").value = searchPresets.get($("search-presets").value)?.name || "";
  updatePresetHint();
  presetEditor.showModal();
  $("preset-name").select();
});
$("preset-name").addEventListener("input", updatePresetHint);
$("preset-form").addEventListener("submit", saveSearchPreset);
$("cancel-preset").addEventListener("click", () => presetEditor.close());
$("new-folder").addEventListener("click", () => {
  if (folderDialog.open || opening || importing) return;
  $("folder-name").value = "";
  showError("folder-error");
  folderDialog.showModal();
  $("folder-name").focus();
});
$("folder-form").addEventListener("submit", createFolder);
$("cancel-folder").addEventListener("click", () => folderDialog.close());
folderDialog.addEventListener("cancel", event => { if (folderBusy) event.preventDefault(); });
$("folder-grid").addEventListener("click", event => {
  const tile = event.target.closest(".folder-card");
  if (tile) browseLibrary("folder:" + tile.dataset.folderName);
});
$("folder-grid").addEventListener("contextmenu", event => {
  const tile = event.target.closest(".folder-card");
  if (!tile) return;
  event.preventDefault();
  openFolderImageEditor(tile.dataset.folderName);
});
$("pick-folder-image").addEventListener("click", chooseFolderImage);
$("restore-folder-image").addEventListener("click", () => {
  ++folderImageRequest;
  folderImageChange = "restore";
  $("restore-folder-image").hidden = true;
  showFolderImage(null);
  showError("folder-image-error");
});
$("save-folder-image").addEventListener("click", saveFolderImage);
$("cancel-folder-image").addEventListener("click", () => folderImageEditor.close());
$("delete-folder").addEventListener("click", askDeleteFolder);
$("cancel-delete-folder").addEventListener("click", () => deleteFolderDialog.close());
$("confirm-delete-folder").addEventListener("click", deleteFolder);
deleteFolderDialog.addEventListener("cancel", event => { if (folderImageBusy) event.preventDefault(); });
folderImageEditor.addEventListener("cancel", event => { if (folderImageBusy) event.preventDefault(); });
folderImageEditor.addEventListener("close", () => {
  ++folderImageRequest;
  editFolderName = null;
  showFolderImage(null);
  call("discard_cover").catch(() => {});
});
$("book-grid").addEventListener("click", event => {
  const card = event.target.closest(".book-card");
  if (card) chooseBook(card.dataset.bookId);
});
$("book-grid").addEventListener("contextmenu", event => {
  const card = event.target.closest(".book-card");
  if (!card) return;
  event.preventDefault();
  openEditor(card.dataset.bookId);
});
$("edit-form").addEventListener("submit", saveBookEdit);
$("edit-tags").addEventListener("keydown", event => {
  if (event.key === "Enter" && !event.isComposing) {
    event.preventDefault();
    if (!editBusy) addDraftTag();
  }
});
$("pick-cover").addEventListener("click", chooseCover);
$("restore-cover").addEventListener("click", () => {
  editCover = "restore";
  $("restore-cover").hidden = true;
  editError();
  loadEditorCover(true);
});
$("cancel-edit").addEventListener("click", () => editor.close());
$("remove-book").addEventListener("click", askRemoveBook);
$("cancel-remove").addEventListener("click", () => removeDialog.close());
$("confirm-remove").addEventListener("click", removeBook);
removeDialog.addEventListener("cancel", event => { if (editBusy) event.preventDefault(); });
editor.addEventListener("cancel", event => {
  if (editBusy) event.preventDefault();
});
editor.addEventListener("close", () => {
  ++previewRequest;
  editEntry = null;
  call("discard_cover").catch(() => {});
});
$("library-back").addEventListener("click", returnLibrary);
$("settings-toggle").addEventListener("click", () => showSettings($("reading-settings").hidden));
$("fullscreen").addEventListener("click", () => setFullscreen(!fullscreen));
$("exit-fullscreen").addEventListener("click", () => setFullscreen(false));
$("sidebar-toggle").addEventListener("click", () => applySettings({sidebar: !settings.sidebar}));
$("light-mode").addEventListener("click", () => applySettings({theme: "light"}));
$("dark-mode").addEventListener("click", () => applySettings({theme: "dark"}));
$("black-mode").addEventListener("click", () => applySettings({theme: "black"}));
$("footer-mode").addEventListener("change", event => applySettings({footer: event.target.value}));
$("font").addEventListener("change", event => applySettings({font: event.target.value}));
$("size").addEventListener("change", event => applySettings({size: event.target.valueAsNumber}));
$("line-spacing").addEventListener("change", event => applySettings({lineSpacing: Number(event.target.value)}));
$("paragraph-spacing").addEventListener("change", event => applySettings({paragraphSpacing: Number(event.target.value)}));
$("trim-blank-lines").addEventListener("change", event => applySettings({trimBlankLines: event.target.checked}));
$("search-markers-toggle").addEventListener("change", event => applySettings({searchMarkers: event.target.checked}));
$("smaller").addEventListener("click", () => applySettings({size: settings.size - 1}));
$("larger").addEventListener("click", () => applySettings({size: settings.size + 1}));
$("scroll-mode").addEventListener("click", () => applySettings({mode: "scroll"}));
$("spread-mode").addEventListener("click", () => applySettings({mode: "spread"}));
$("chapter-filter").addEventListener("input", () => chapterList());
$("chapters").addEventListener("click", event => {
  const button = event.target.closest("[data-index]");
  if (button) loadChapter(Number(button.dataset.index));
});
$("chapters").addEventListener("contextmenu", event => {
  const button = event.target.closest(".chapter-row");
  if (!button) return;
  event.preventDefault();
  openChapterEditor(Number(button.dataset.index));
});
$("toggle-chapter-number").addEventListener("click", saveChapterNumber);
$("cancel-chapter-edit").addEventListener("click", () => chapterDialog.close());
chapterDialog.addEventListener("cancel", event => { if (chapterEditBusy) event.preventDefault(); });
chapterDialog.addEventListener("close", () => { chapterEdit = null; });
for (const id of ["previous-chapter", "toolbar-previous-chapter"]) $(id).addEventListener("click", () => loadChapter(chapter - 1));
for (const id of ["next-chapter", "toolbar-next-chapter"]) $(id).addEventListener("click", () => loadChapter(chapter + 1));
for (const id of ["page-back", "margin-back"]) $(id).addEventListener("click", () => turn(-1));
for (const id of ["page-forward", "margin-forward"]) $(id).addEventListener("click", () => turn(1));
$("close-find").addEventListener("click", () => closeFind());
$("lock-find").addEventListener("click", () => lockFind(!findLocked));
$("prev-match").addEventListener("click", () => selectMatch(currentMatch < 0 ? matches.length - 1 : currentMatch - 1));
$("next-match").addEventListener("click", () => selectMatch(currentMatch + 1));
$("query").addEventListener("input", () => {
  clearTimeout(findTimer);
  findTimer = setTimeout(() => {
    findTimer = 0;
    refreshFind();
  }, 80);
});
$("query").addEventListener("keydown", event => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  if (findTimer) { clearTimeout(findTimer); findTimer = 0; refreshFind(false); }
  selectMatch(currentMatch < 0 ? (event.shiftKey ? matches.length - 1 : 0) : currentMatch + (event.shiftKey ? -1 : 1));
});
content.addEventListener("click", event => {
  const link = event.target.closest("[data-epub-path]");
  if (!link) return;
  event.preventDefault();
  const {epubPath, epubFragment} = link.dataset;
  if (book.sections[chapter].path === epubPath && jumpTo(elementAt(content, epubFragment))) return;
  let index = book.sections.findIndex(section => section.path === epubPath && section.fragment === epubFragment);
  if (index < 0) index = book.sections.findIndex(section => section.path === epubPath);
  if (index >= 0) loadChapter(index, undefined, epubFragment);
});

document.addEventListener("keydown", event => {
  if (editor.open || folderDialog.open || folderImageEditor.open || chapterDialog.open || presetEditor.open) return;
  if (event.ctrlKey && ["f", "o"].includes(event.key.toLowerCase())) {
    event.preventDefault();
    if (event.key.toLowerCase() === "f") openFind();
    else addFiles();
  } else if (event.key === "F11" && book) {
    event.preventDefault();
    setFullscreen(!fullscreen);
  } else if (event.key === "Escape" && !$("library-tag-panel").hidden) {
    event.preventDefault();
    showTagPicker(false);
    $("library-tag-toggle").focus();
  } else if (event.key === "Escape" && !$("find").hidden) {
    event.preventDefault();
    closeFind();
  } else if (event.key === "Escape" && !$("reading-settings").hidden) {
    showSettings(false);
    $("settings-toggle").focus();
  } else if (event.key === "Escape" && fullscreen) {
    event.preventDefault();
    setFullscreen(false);
  } else if (event.key === "F3" && chapter >= 0) {
    event.preventDefault();
    if ($("find").hidden && !findLocked) openFind();
    else selectMatch(currentMatch < 0 ? (event.shiftKey ? -1 : 0) : currentMatch + (event.shiftKey ? -1 : 1));
  } else if (isSpread() && !event.target.matches("input,select,textarea") &&
      ["ArrowLeft", "ArrowRight", "PageUp", "PageDown"].includes(event.key)) {
    event.preventDefault();
    turn(["ArrowLeft", "PageUp"].includes(event.key) ? -1 : 1);
  }
});

document.addEventListener("click", event => {
  if (!event.target.closest("#settings-toggle, #reading-settings")) showSettings(false);
  if (!$("library-tag-panel").hidden && !event.target.closest("#library-tag-toggle, #library-tag-panel")) showTagPicker(false);
});
document.addEventListener("dragover", event => {
  if (!event.dataTransfer.types.includes("Files")) return;
  event.preventDefault();
  const allowed = !book && !opening && !importing && !editor.open && !folderDialog.open &&
    !folderImageEditor.open && $("library").contains(event.target);
  event.dataTransfer.dropEffect = allowed ? "copy" : "none";
  $("drop-overlay").hidden = !allowed;
});
document.addEventListener("dragleave", event => {
  if (!$("library").contains(event.relatedTarget)) $("drop-overlay").hidden = true;
});
document.addEventListener("drop", event => {
  if (!event.dataTransfer.types.includes("Files")) return;
  event.preventDefault();
  $("drop-overlay").hidden = true;
  if (!editor.open && !folderDialog.open && !folderImageEditor.open && $("library").contains(event.target)) {
    addDroppedFiles(Array.from(event.dataTransfer.files));
  }
});

viewport.addEventListener("scroll", () => {
  if (scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = 0;
    anchor = readPosition();
    updateFooter();
    saveSoon();
  });
}, {passive: true});
viewport.addEventListener("scrollend", () => {
  anchor = readPosition();
  persist();
});
new ResizeObserver(queueLayout).observe(viewport);
window.addEventListener("blur", () => {
  if (chapter >= 0) anchor = readPosition();
  persist();
});

call("init").then(async initial => {
  applySettings(initial.settings || {}, false);
  const presets = initial.settings?.searchPresets;
  for (const preset of Array.isArray(presets) ? presets : []) {
    if (typeof preset?.name !== "string" || !preset.name.trim() ||
        typeof preset.query !== "string" || typeof preset.folder !== "string" ||
        !["active", "archived", "all"].includes(preset.status) || !Array.isArray(preset.tags)) continue;
    const tags = preset.tags.filter(rule => typeof rule?.name === "string" && rule.name.trim() &&
      ["include", "exclude"].includes(rule.mode));
    searchPresets.set(preset.name.toLowerCase(), {...preset, tags});
  }
  updatePresetOptions();
  loadedSettings = true;
  libraryFolders = initial.folders || [];
  folderImages = new Set(initial.folder_images || []);
  renderLibrary(initial.library || []);
  if (initial.book) await useBook(initial.book);
  if (initial.notice) notice(initial.notice);
}).catch(error => notice(error.message));
