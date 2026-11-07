use std::{
    collections::{HashMap, HashSet}, env, fs::{self, File, OpenOptions}, io::{self, Read, Write},
    path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::book::{Book, Cover, Result};

const DROP_CHUNK: usize = 192 * 1024;
const COVER_LIMIT: u64 = 12 * 1024 * 1024;

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct ReadingState {
    settings: Value,
    last_book: Option<PathBuf>,
    positions: HashMap<String, Position>,
    library: Vec<LibraryEntry>,
    folders: Vec<String>,
    folder_images: HashMap<String, CustomCover>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Position {
    chapter: usize,
    offset: usize,
    fraction: f64,
}

#[derive(Deserialize, Serialize)]
struct LibraryEntry {
    id: String,
    title: String,
    author: String,
    path: PathBuf,
    cover: Option<Cover>,
    chapters: usize,
    #[serde(default)]
    managed: Option<bool>,
    #[serde(default)]
    front_matter: Vec<usize>,
    #[serde(default)]
    edits: BookEdits,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct BookEdits {
    title: Option<String>,
    author: Option<String>,
    cover: Option<CustomCover>,
    folder: Option<String>,
    tags: Vec<String>,
    archived: bool,
    numbering: HashMap<usize, bool>,
}

#[derive(Clone, Deserialize, Serialize)]
struct CustomCover {
    file: String,
    format: CoverFormat,
}

impl CustomCover {
    fn path(&self, dir: &Path) -> Result<PathBuf> {
        if self.file.is_empty() || !self.file.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
            return Err("Invalid custom cover filename.".into());
        }
        Ok(dir.join("Covers").join(&self.file).with_extension(self.format.extension()))
    }

    fn read(&self, dir: &Path) -> Result<String> {
        let bytes = read_cover_file(&self.path(dir)?)?;
        Ok(self.format.data_url(&bytes))
    }

    fn remove(&self, dir: &Path) {
        if let Ok(path) = self.path(dir) { let _ = fs::remove_file(path); }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
enum CoverFormat { Png, Jpeg, Webp, Gif }

impl CoverFormat {
    fn detect(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Ok(Self::Png); }
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) { return Ok(Self::Jpeg); }
        if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") { return Ok(Self::Webp); }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { return Ok(Self::Gif); }
        Err("Choose a PNG, JPEG, WebP, or GIF image.".into())
    }

    fn extension(self) -> &'static str {
        match self { Self::Png => "png", Self::Jpeg => "jpeg", Self::Webp => "webp", Self::Gif => "gif" }
    }

    fn data_url(self, bytes: &[u8]) -> String {
        format!("data:image/{};base64,{}", self.extension(), STANDARD.encode(bytes))
    }
}

#[derive(PartialEq, Eq)]
enum ImageTarget {
    Book(String),
    Folder(String),
}

struct PendingCover {
    target: ImageTarget,
    format: CoverFormat,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverEdit { Keep, Replace, Restore }

#[derive(Deserialize)]
pub struct BookDetails {
    title: String,
    author: String,
    cover: CoverEdit,
    folder: Option<String>,
    tags: Vec<String>,
    archived: bool,
}

fn read_cover_file(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(COVER_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > COVER_LIMIT { return Err("Cover images must be 12 MiB or smaller.".into()); }
    Ok(bytes)
}

impl LibraryEntry {
    fn new(book: &Book, path: PathBuf, managed: bool) -> Self {
        Self {
            id: book.id.clone(), title: book.title.clone(), author: book.author.clone(),
            path, cover: book.cover.clone(), chapters: book.sections.len(), front_matter: front_matter(book),
            edits: BookEdits::default(), managed: Some(managed),
        }
    }

    fn title(&self) -> &str { self.edits.title.as_deref().unwrap_or(&self.title) }
    fn author(&self) -> &str { self.edits.author.as_deref().unwrap_or(&self.author) }

    fn numbered(&self, index: usize) -> bool {
        self.edits.numbering.get(&index).copied().unwrap_or_else(|| !self.front_matter.contains(&index))
    }

    fn managed_copy(&self) -> bool {
        self.managed != Some(false) && self.id.split_once('-').is_some_and(|(size, hash)| {
            !size.is_empty() && !hash.is_empty()
                && size.bytes().chain(hash.bytes()).all(|byte| byte.is_ascii_hexdigit())
        }) && self.path == Path::new("Books").join(format!("{}.epub", self.id))
    }
}

fn front_matter(book: &Book) -> Vec<usize> {
    book.sections.iter().enumerate().filter_map(|(index, section)| section.front_matter.then_some(index)).collect()
}

pub struct Reader {
    state_file: PathBuf,
    state: ReadingState,
    book: Option<Book>,
    startup_book: Option<PathBuf>,
    dropped: Option<PendingBook>,
    pending_cover: Option<PendingCover>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    Init,
    Import { #[serde(default)] folder: Option<String> },
    DropBegin { name: String, length: u64 },
    DropChunk { data: String },
    DropFinish { #[serde(default)] folder: Option<String> },
    DropAbort,
    OpenBook { book_id: String },
    CloseBook,
    Cover { book_id: String, #[serde(default)] embedded: bool },
    PickCover { book_id: String },
    DiscardCover,
    EditBook { book_id: String, details: BookDetails },
    RemoveBook { book_id: String },
    CreateFolder { name: String },
    FolderImage { name: String },
    PickFolderImage { name: String },
    SetFolderImage { name: String, change: CoverEdit },
    DeleteFolder { name: String },
    SetNumbered { book_id: String, index: usize, numbered: bool },
    Chapter { book_id: String, index: usize },
    Image { book_id: String, path: String },
    Fullscreen { enabled: bool },
    Save { book_id: Option<String>, position: Option<Position>, settings: Value },
}

impl Reader {
    pub fn new(state_file: PathBuf) -> Self {
        let state = fs::read(&state_file).ok().and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        Self { state_file, state, book: None, startup_book: env::args_os().nth(1).map(PathBuf::from), dropped: None, pending_cover: None }
    }

    fn dir(&self) -> &Path {
        self.state_file.parent().expect("state file lives beside executable")
    }

    fn save(&self) -> Result<()> {
        let temporary = self.state_file.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(&self.state)?)?;
        fs::rename(temporary, &self.state_file)?;
        Ok(())
    }

    fn store_entry(&mut self, book: &Book, path: PathBuf, managed: bool, folder: Option<&str>) {
        let mut entry = LibraryEntry::new(book, path, managed);
        entry.edits.folder = folder.map(str::to_owned);
        if let Some(existing) = self.state.library.iter_mut().find(|item| item.id == book.id) {
            entry.edits = std::mem::take(&mut existing.edits);
            *existing = entry;
        } else {
            self.state.library.push(entry);
        }
    }

    fn store_link(&mut self, path: &Path, book: &Book) {
        if self.state.library.iter().any(|entry| entry.id == book.id) { return; }
        let full_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let portable = fs::canonicalize(self.dir()).ok()
            .and_then(|dir| full_path.strip_prefix(dir).ok().map(Path::to_owned));
        self.store_entry(book, portable.unwrap_or(full_path), false, None);
    }

    fn catalog(&self) -> Value {
        Value::Array(self.state.library.iter().map(|entry| {
            let position = self.state.positions.get(&entry.id);
            let numbered_chapters = (0..entry.chapters).filter(|&index| entry.numbered(index)).count();
            let resume_number = position.filter(|p| p.chapter < entry.chapters && entry.numbered(p.chapter))
                .map(|p| (0..=p.chapter).filter(|&index| entry.numbered(index)).count());
            json!({
                "id": entry.id, "title": entry.title(), "author": entry.author(),
                "cover": entry.cover.is_some() || entry.edits.cover.is_some(),
                "custom_cover": entry.edits.cover.is_some(), "embedded_cover": entry.cover.is_some(),
                "chapters": numbered_chapters, "position": position, "resume_number": resume_number,
                "folder": entry.edits.folder, "tags": entry.edits.tags, "archived": entry.edits.archived,
                "managed": entry.managed_copy(),
            })
        }).collect())
    }

    fn activate(&mut self, book: Book) -> Value {
        let position = self.state.positions.get(&book.id).cloned().unwrap_or_default();
        if let Some(entry) = self.state.library.iter_mut().find(|entry| entry.id == book.id) {
            entry.front_matter = front_matter(&book);
        }
        let entry = self.entry(&book.id).ok();
        let response = json!({
            "id": book.id, "title": entry.map_or(book.title.as_str(), LibraryEntry::title),
            "author": entry.map_or(book.author.as_str(), LibraryEntry::author),
            "sections": book.sections, "position": position, "numbering": entry.map(|entry| &entry.edits.numbering),
        });
        self.book = Some(book);
        response
    }

    fn current(&mut self, book_id: &str) -> Result<&mut Book> {
        self.book.as_mut().filter(|book| book.id == book_id)
            .ok_or_else(|| "The requested book is no longer open.".into())
    }

    fn entry(&self, book_id: &str) -> Result<&LibraryEntry> {
        self.state.library.iter().find(|book| book.id == book_id)
            .ok_or_else(|| "That book is no longer in the library.".into())
    }

    fn validate_folder(&self, name: &str) -> Result<()> {
        if self.state.folders.iter().any(|folder| folder == name) { return Ok(()); }
        Err("Choose a folder from your library.".into())
    }

    fn pick_image(&mut self, target: ImageTarget, selected: &[PathBuf]) -> Result<Value> {
        let Some(path) = selected.first() else { return Ok(Value::Null); };
        let bytes = read_cover_file(path)?;
        let format = CoverFormat::detect(&bytes)?;
        let preview = format.data_url(&bytes);
        self.pending_cover = Some(PendingCover { target, format, bytes });
        Ok(json!(preview))
    }

    fn save_pending_image(&self, target: ImageTarget) -> Result<CustomCover> {
        let pending = self.pending_cover.as_ref().filter(|pending| pending.target == target)
            .ok_or("Choose an image before saving.")?;
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let cover = CustomCover { file: format!("cover-{}-{time}", std::process::id()), format: pending.format };
        let path = cover.path(self.dir())?;
        fs::create_dir_all(self.dir().join("Covers"))?;
        let mut file = OpenOptions::new().create_new(true).write(true).open(&path)?;
        let result = file.write_all(&pending.bytes);
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(path);
            return Err(error.into());
        }
        Ok(cover)
    }

    fn edit_book(&mut self, id: &str, details: BookDetails) -> Result<Value> {
        let BookDetails { title, author, cover, folder, tags, archived } = details;
        let title = title.trim().to_owned();
        let author = author.trim().to_owned();
        if title.is_empty() { return Err("Enter a title for the book.".into()); }
        if let Some(name) = folder.as_deref() { self.validate_folder(name)?; }
        let mut seen_tags = HashSet::new();
        let tags: Vec<_> = tags.iter().map(|tag| tag.trim()).filter(|tag| {
            !tag.is_empty() && seen_tags.insert(tag.to_lowercase())
        }).map(str::to_owned).collect();
        if tags.len() > 32 || tags.iter().any(|tag| tag.chars().count() > 64) {
            return Err("Use up to 32 tags with at most 64 characters each.".into());
        }
        let index = self.state.library.iter().position(|entry| entry.id == id).ok_or("That book is no longer in the library.")?;
        let entry = &self.state.library[index];
        let old = entry.edits.clone();
        let created = if matches!(cover, CoverEdit::Replace) {
            Some(self.save_pending_image(ImageTarget::Book(id.into()))?)
        } else { None };
        let next_cover = match cover {
            CoverEdit::Keep => old.cover.clone(),
            CoverEdit::Restore => None,
            CoverEdit::Replace => created.clone(),
        };
        self.state.library[index].edits = BookEdits {
            title: (title != entry.title).then_some(title),
            author: (author != entry.author).then_some(author),
            cover: next_cover,
            folder,
            tags,
            archived,
            numbering: old.numbering.clone(),
        };
        if let Err(error) = self.save() {
            self.state.library[index].edits = old;
            if let Some(image) = created { image.remove(self.dir()); }
            return Err(error);
        }
        if !matches!(cover, CoverEdit::Keep) && let Some(old_cover) = old.cover {
            old_cover.remove(self.dir());
        }
        self.pending_cover = None;
        Ok(self.catalog())
    }

    fn create_folder(&mut self, name: &str) -> Result<Value> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("Enter a folder name up to 80 characters long.".into());
        }
        if self.state.folders.iter().any(|folder| folder.to_lowercase() == name.to_lowercase()) {
            return Err("A folder with that name already exists.".into());
        }
        self.state.folders.push(name.to_owned());
        if let Err(error) = self.save() {
            self.state.folders.pop();
            return Err(error);
        }
        Ok(json!(self.state.folders))
    }

    fn set_folder_image(&mut self, name: &str, change: CoverEdit) -> Result<Value> {
        self.validate_folder(name)?;
        let images = &self.state.folder_images;
        let created = match change {
            CoverEdit::Keep => return Ok(json!(images.keys().collect::<Vec<_>>())),
            CoverEdit::Restore => None,
            CoverEdit::Replace => Some(self.save_pending_image(ImageTarget::Folder(name.into()))?),
        };
        let old = if let Some(image) = created.clone() {
            self.state.folder_images.insert(name.to_owned(), image)
        } else {
            self.state.folder_images.remove(name)
        };
        if let Err(error) = self.save() {
            if let Some(image) = old {
                self.state.folder_images.insert(name.to_owned(), image);
            } else {
                self.state.folder_images.remove(name);
            }
            if let Some(image) = created { image.remove(self.dir()); }
            return Err(error);
        }
        if let Some(image) = old { image.remove(self.dir()); }
        self.pending_cover = None;
        Ok(json!(self.state.folder_images.keys().collect::<Vec<_>>()))
    }

    fn delete_folder(&mut self, name: &str) -> Result<Value> {
        let index = self.state.folders.iter().position(|folder| folder == name)
            .ok_or("That folder is no longer in your library.")?;
        let removed = self.state.folders.remove(index);
        let image = self.state.folder_images.remove(name);
        let mut unfiled = Vec::new();
        for (index, entry) in self.state.library.iter_mut().enumerate() {
            if entry.edits.folder.as_deref() == Some(name) {
                entry.edits.folder = None;
                unfiled.push(index);
            }
        }
        if let Err(error) = self.save() {
            self.state.folders.insert(index, removed.clone());
            for index in unfiled {
                self.state.library[index].edits.folder = Some(removed.clone());
            }
            if let Some(image) = image {
                self.state.folder_images.insert(removed, image);
            }
            return Err(error);
        }
        if let Some(image) = image { image.remove(self.dir()); }
        if self.pending_cover.as_ref().is_some_and(|pending| {
            matches!(&pending.target, ImageTarget::Folder(folder) if folder == name)
        }) {
            self.pending_cover = None;
        }
        Ok(json!({
            "folders": self.state.folders, "library": self.catalog(),
            "folder_images": self.state.folder_images.keys().collect::<Vec<_>>(),
            "unfiled": unfiled.len(),
        }))
    }

    fn set_numbered(&mut self, id: &str, index: usize, numbered: bool) -> Result<Value> {
        let section = self.current(id)?.sections.get(index).ok_or("Chapter is outside the reading order.")?;
        let default = !section.front_matter;
        let entry = self.state.library.iter_mut().find(|entry| entry.id == id).ok_or("Book is outside the library.")?;
        let old = entry.edits.numbering.clone();
        if numbered == default { entry.edits.numbering.remove(&index); }
        else { entry.edits.numbering.insert(index, numbered); }
        if let Err(error) = self.save() {
            if let Some(entry) = self.state.library.iter_mut().find(|entry| entry.id == id) {
                entry.edits.numbering = old;
            }
            return Err(error);
        }
        Ok(json!(self.entry(id)?.edits.numbering))
    }

    fn remove_book(&mut self, id: &str) -> Result<Value> {
        if self.book.as_ref().is_some_and(|book| book.id == id) { return Err("Close the book before removing it.".into()); }
        let index = self.state.library.iter().position(|entry| entry.id == id).ok_or("That book is no longer in the library.")?;
        let entry = self.state.library.remove(index);
        let last_book = self.state.last_book.clone();
        if self.state.library.is_empty() || self.state.last_book.as_ref() == Some(&entry.path) {
            self.state.last_book = None;
        }
        if let Err(error) = self.save() {
            self.state.last_book = last_book;
            self.state.library.insert(index, entry);
            return Err(error);
        }
        let epub = entry.managed_copy().then(|| self.dir().join(&entry.path));
        let cover = entry.edits.cover.and_then(|cover| cover.path(self.dir()).ok());
        let mut warning = None;
        for path in [epub, cover].into_iter().flatten() {
            if let Err(error) = fs::remove_file(path) && error.kind() != io::ErrorKind::NotFound {
                warning = Some(format!("Book removed from My Library. Folio could not delete a stored copy. {error}"));
            }
        }
        Ok(json!({"library": self.catalog(), "warning": warning}))
    }

    fn import(&mut self, mut staged: PendingBook, folder: Option<&str>) -> Result<()> {
        if let Some(name) = folder { self.validate_folder(name)?; }
        staged.close()?;
        let fallback = Path::new(&staged.name).file_stem().unwrap_or_default().to_string_lossy();
        let book = Book::open_named(&staged.path, &fallback)?;
        let path = Path::new("Books").join(format!("{}.epub", book.id));
        fs::rename(&staged.path, self.dir().join(&path))?;
        self.store_entry(&book, path, true, folder);
        self.save()?;
        Ok(())
    }

    pub fn handle(&mut self, request: Request, selected: Vec<PathBuf>) -> Result<Value> {
        match request {
            Request::Init => {
                if self.state.library.is_empty()
                    && let Some(path) = self.state.last_book.as_ref().map(|path| self.dir().join(path))
                    && let Ok(book) = Book::open(&path)
                {
                    self.store_link(&path, &book);
                    self.save()?;
                }
                let (book, notice) = match self.startup_book.take() {
                    Some(path) => match Book::open(&path) {
                        Ok(book) => {
                            self.store_link(&path, &book);
                            self.save()?;
                            (self.activate(book), None)
                        }
                        Err(error) => (Value::Null, Some(format!("Could not open {}. {error}", path.display()))),
                    },
                    None => (Value::Null, None),
                };
                Ok(json!({"settings": self.state.settings, "library": self.catalog(), "folders": self.state.folders,
                    "folder_images": self.state.folder_images.keys().collect::<Vec<_>>(), "book": book, "notice": notice}))
            }
            Request::Import { folder } => {
                let mut added = 0;
                let mut errors = Vec::new();
                for path in selected {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let result = (|| {
                        let mut source = File::open(&path)?;
                        let length = source.metadata()?.len();
                        let mut staged = PendingBook::new(self.dir(), name.clone(), length)?;
                        staged.written = io::copy(&mut source, staged.file.as_mut().expect("staging file is open"))?;
                        self.import(staged, folder.as_deref())
                    })();
                    match result {
                        Ok(()) => added += 1,
                        Err(error) => errors.push(format!("{name}: {error}")),
                    }
                }
                Ok(json!({"library": self.catalog(), "added": added, "errors": errors}))
            }
            Request::DropBegin { name, length } => {
                self.dropped = Some(PendingBook::new(self.dir(), name, length)?);
                Ok(Value::Null)
            }
            Request::DropChunk { data } => {
                let bytes = STANDARD.decode(data)?;
                if bytes.len() > DROP_CHUNK { return Err("The EPUB import chunk is too large.".into()); }
                let staged = self.dropped.as_mut().ok_or("No drop import is active.")?;
                if staged.written + bytes.len() as u64 > staged.length {
                    return Err("The EPUB size changed while importing.".into());
                }
                staged.file.as_mut().expect("staging file is open").write_all(&bytes)?;
                staged.written += bytes.len() as u64;
                Ok(Value::Null)
            }
            Request::DropFinish { folder } => {
                let staged = self.dropped.take().ok_or("No drop import is active.")?;
                self.import(staged, folder.as_deref())?;
                Ok(self.catalog())
            }
            Request::DropAbort => {
                self.dropped = None;
                Ok(Value::Null)
            }
            Request::OpenBook { book_id } => {
                let entry = self.entry(&book_id)?;
                let path = self.dir().join(&entry.path);
                let book = Book::open_named(&path, &entry.title).map_err(|error| {
                    format!("Could not open “{}”. {error}", entry.title)
                })?;
                if book.id != book_id { return Err("The book file changed. Add it again to the library.".into()); }
                self.state.last_book = Some(entry.path.clone());
                Ok(self.activate(book))
            }
            Request::CloseBook => {
                self.book = None;
                Ok(self.catalog())
            }
            Request::Cover { book_id, embedded } => {
                let entry = self.entry(&book_id)?;
                if !embedded && let Some(cover) = entry.edits.cover.as_ref()
                    && let Ok(image) = cover.read(self.dir())
                {
                    return Ok(json!(image));
                }
                let cover = entry.cover.as_ref().ok_or("This EPUB has no cover.")?;
                Ok(json!(cover.read(&self.dir().join(&entry.path))?))
            }
            Request::PickCover { book_id } => {
                self.entry(&book_id)?;
                self.pick_image(ImageTarget::Book(book_id), &selected)
            }
            Request::DiscardCover => {
                self.pending_cover = None;
                Ok(Value::Null)
            }
            Request::EditBook { book_id, details } => self.edit_book(&book_id, details),
            Request::RemoveBook { book_id } => self.remove_book(&book_id),
            Request::CreateFolder { name } => self.create_folder(&name),
            Request::FolderImage { name } => {
                let image = self.state.folder_images.get(&name).ok_or("This folder uses the default icon.")?;
                Ok(json!(image.read(self.dir())?))
            }
            Request::PickFolderImage { name } => {
                self.validate_folder(&name)?;
                self.pick_image(ImageTarget::Folder(name), &selected)
            }
            Request::SetFolderImage { name, change } => self.set_folder_image(&name, change),
            Request::DeleteFolder { name } => self.delete_folder(&name),
            Request::SetNumbered { book_id, index, numbered } => self.set_numbered(&book_id, index, numbered),
            Request::Chapter { book_id, index } => Ok(json!(self.current(&book_id)?.chapter(index)?)),
            Request::Image { book_id, path } => Ok(json!(self.current(&book_id)?.image(&path)?)),
            Request::Save { book_id, position, settings } => {
                self.state.settings = settings;
                if let (Some(book_id), Some(mut position)) = (book_id, position)
                    && let Some(book) = self.book.as_ref().filter(|book| book.id == book_id)
                {
                    position.chapter = position.chapter.min(book.sections.len() - 1);
                    position.fraction = position.fraction.clamp(0.0, 1.0);
                    self.state.positions.insert(book_id, position);
                }
                self.save().map_err(|error| format!("Could not save reading progress beside the executable. {error}"))?;
                Ok(Value::Null)
            }
            Request::Fullscreen { .. } => unreachable!("native window request"),
        }
    }
}

struct PendingBook {
    name: String,
    path: PathBuf,
    file: Option<File>,
    length: u64,
    written: u64,
}

impl PendingBook {
    fn new(dir: &Path, name: String, length: u64) -> Result<Self> {
        if !Path::new(&name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("epub")) {
            return Err("Choose a file with an .epub extension.".into());
        }
        let dir = dir.join("Books");
        fs::create_dir_all(&dir)?;
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = dir.join(format!(".import-{}-{time}.part", std::process::id()));
        let file = OpenOptions::new().create_new(true).write(true).open(&path)?;
        Ok(Self { name, path, file: Some(file), length, written: 0 })
    }

    fn close(&mut self) -> Result<()> {
        self.file.take();
        if self.written != self.length { return Err("The EPUB changed size while importing.".into()); }
        Ok(())
    }
}

impl Drop for PendingBook {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_folder_unfiles_books_and_preserves_their_data() -> Result<()> {
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = env::temp_dir().join(format!("folio-folder-{}-{time}", std::process::id()));
        fs::create_dir_all(dir.join("Books"))?;
        fs::create_dir(dir.join("Covers"))?;
        let state_file = dir.join("reading-state.json");
        let epub = dir.join("Books/1-a.epub");
        let book_cover = dir.join("Covers/book-image.png");
        let folder_cover = dir.join("Covers/folder-image.png");
        fs::write(&epub, b"book data")?;
        fs::write(&book_cover, b"book cover")?;
        fs::write(&folder_cover, b"folder image")?;
        fs::write(&state_file, serde_json::to_vec(&json!({
            "folders": ["Novels", "Essays"],
            "folder_images": {"Novels": {"file": "folder-image", "format": "Png"}},
            "positions": {"1-a": {"chapter": 0, "offset": 17}},
            "library": [
                {"id": "1-a", "title": "One", "author": "", "path": "Books/1-a.epub", "chapters": 1,
                 "edits": {"folder": "Novels", "tags": ["Fantasy"], "cover": {"file": "book-image", "format": "Png"}}},
                {"id": "1-b", "title": "Two", "author": "", "path": "Books/1-b.epub", "chapters": 1,
                 "edits": {"folder": "Novels", "archived": true}},
                {"id": "1-c", "title": "Three", "author": "", "path": "Books/1-c.epub", "chapters": 1,
                 "edits": {"folder": "Essays"}},
            ],
        }))?)?;

        let mut reader = Reader::new(state_file.clone());
        reader.state_file = dir.join("missing").join("reading-state.json");
        assert!(reader.delete_folder("Novels").is_err());
        assert_eq!(reader.entry("1-a")?.edits.folder.as_deref(), Some("Novels"));
        assert!(reader.state.folder_images.contains_key("Novels"));
        assert!(folder_cover.exists());
        reader.state_file = state_file.clone();
        let result = reader.handle(Request::DeleteFolder { name: "Novels".into() }, Vec::new())?;
        assert_eq!(result["unfiled"], 2);
        assert_eq!(result["folders"], json!(["Essays"]));
        assert_eq!(result["folder_images"], json!([]));
        assert!(!folder_cover.exists());
        assert_eq!(fs::read(&epub)?, b"book data");
        assert_eq!(fs::read(&book_cover)?, b"book cover");
        let reader = Reader::new(state_file.clone());
        let books = reader.catalog();
        assert_eq!(books[0]["folder"], Value::Null);
        assert_eq!(books[0]["tags"], json!(["Fantasy"]));
        assert_eq!(books[0]["position"]["offset"], 17);
        assert_eq!(books[1]["folder"], Value::Null);
        assert_eq!(books[1]["archived"], true);
        assert_eq!(books[2]["folder"], "Essays");
        assert!(reader.validate_folder("Novels").is_err());

        for path in [epub, book_cover, state_file] { fs::remove_file(path)?; }
        fs::remove_dir(dir.join("Books"))?;
        fs::remove_dir(dir.join("Covers"))?;
        fs::remove_dir(dir)?;
        Ok(())
    }

    fn details(cover: CoverEdit) -> BookDetails {
        BookDetails {
            title: "My title".into(), author: "My author".into(), cover,
            folder: Some("Novels".into()), tags: vec![" Fantasy ".into(), "fantasy".into(), "Favorites".into()],
            archived: true,
        }
    }

    #[test]
    fn edits_survive_reopening_a_legacy_library() -> Result<()> {
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = env::temp_dir().join(format!("folio-edit-{}-{time}", std::process::id()));
        fs::create_dir(&dir)?;
        let state = dir.join("reading-state.json");
        let original_cover = dir.join("chosen.png");
        let bytes = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a3ioAAAAASUVORK5CYII=")?;
        fs::write(&original_cover, &bytes)?;
        fs::write(&state, serde_json::to_vec(&json!({
            "library": [{"id": "book", "title": "Original", "author": "Original author",
                         "path": "Books/book.epub", "cover": null, "chapters": 3}],
            "positions": {"book": {"chapter": 1, "offset": 21, "fraction": 0.5}},
        }))?)?;

        let mut reader = Reader::new(state.clone());
        reader.create_folder(" Novels ")?;
        assert!(reader.create_folder("novels").is_err());
        let folder_preview = reader.handle(Request::PickFolderImage { name: "Novels".into() }, vec![original_cover.clone()])?;
        assert!(reader.edit_book("book", details(CoverEdit::Replace)).is_err());
        assert_eq!(reader.set_folder_image("Novels", CoverEdit::Replace)?, json!(["Novels"]));
        let preview = reader.handle(Request::PickCover { book_id: "book".into() }, vec![original_cover.clone()])?;
        let cards = reader.edit_book("book", details(CoverEdit::Replace))?;
        assert_eq!(cards[0]["title"], "My title");
        assert_eq!(cards[0]["position"]["offset"], 21);
        assert_eq!(cards[0]["tags"], json!(["Fantasy", "Favorites"]));
        drop(reader);

        let mut reader = Reader::new(state.clone());
        let folder_image_path = reader.state.folder_images["Novels"].path(&dir)?;
        let stored_folder_image = reader.handle(Request::FolderImage { name: "Novels".into() }, Vec::new())?;
        assert_eq!(stored_folder_image, folder_preview);
        assert_eq!(reader.set_folder_image("Novels", CoverEdit::Restore)?, json!([]));
        assert!(!folder_image_path.exists());
        let entry = reader.entry("book")?;
        assert_eq!((entry.title(), entry.author()), ("My title", "My author"));
        assert_eq!((&*entry.title, &*entry.author), ("Original", "Original author"));
        assert_eq!(entry.edits.folder.as_deref(), Some("Novels"));
        assert!(entry.edits.archived);
        let cover_path = entry.edits.cover.as_ref().unwrap().path(&dir)?;
        let stored = reader.handle(Request::Cover { book_id: "book".into(), embedded: false }, Vec::new())?;
        assert_eq!(stored, preview);
        assert_eq!(fs::read(&cover_path)?, bytes);

        let mut restored = details(CoverEdit::Restore);
        restored.archived = false;
        let cards = reader.edit_book("book", restored)?;
        assert_eq!(cards[0]["custom_cover"], false);
        assert_eq!(cards[0]["archived"], false);
        assert!(!cover_path.exists());
        assert_eq!(fs::read(&original_cover)?, bytes);

        fs::remove_file(original_cover)?;
        fs::remove_file(state)?;
        fs::remove_dir(dir.join("Covers"))?;
        fs::remove_dir(dir)?;
        Ok(())
    }

    #[test]
    fn numbering_and_removal_preserve_the_source_and_reading_position() -> Result<()> {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = env::temp_dir().join(format!("folio-remove-{}-{time}", std::process::id()));
        fs::create_dir(&dir)?;
        let state = dir.join("reading-state.json");
        let source = dir.join("Novel.epub");
        let mut zip = ZipWriter::new(File::create(&source)?);
        for (path, content) in [
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#),
            ("book.opf", r#"<package><metadata><title>Novel</title></metadata><manifest>
                <item id="cover" href="front.xhtml" media-type="application/xhtml+xml"/>
                <item id="toc" href="list.xhtml" media-type="application/xhtml+xml"/>
                <item id="one" href="one.xhtml" media-type="application/xhtml+xml"/>
                <item id="two" href="two.xhtml" media-type="application/xhtml+xml"/>
                </manifest><spine><itemref idref="cover"/><itemref idref="toc"/><itemref idref="one"/><itemref idref="two"/></spine>
                <guide><reference type="cover" href="front.xhtml"/></guide></package>"#),
            ("front.xhtml", "<html><body><h1>Novel</h1></body></html>"),
            ("list.xhtml", "<html><body><h1>Chapter 0: Table of Contents</h1></body></html>"),
            ("one.xhtml", "<html><body><h1>Chapter 1: The beginning</h1></body></html>"),
            ("two.xhtml", "<html><body><h1>Chapter 2: The journey</h1></body></html>"),
        ] {
            zip.start_file(path, SimpleFileOptions::default())?;
            zip.write_all(content.as_bytes())?;
        }
        zip.finish()?;
        let original_bytes = fs::read(&source)?;
        let mut reader = Reader::new(state.clone());
        reader.create_folder("Novels")?;
        reader.create_folder("Other")?;
        let result = reader.handle(Request::Import { folder: Some("Novels".into()) }, vec![source.clone()])?;
        assert_eq!(result["errors"], json!([]));
        assert_eq!(result["library"][0]["folder"], "Novels");
        let id = result["library"][0]["id"].as_str().unwrap().to_owned();
        let duplicate = reader.handle(Request::Import { folder: Some("Other".into()) }, vec![source.clone()])?;
        assert_eq!(duplicate["errors"], json!([]));
        assert_eq!(duplicate["library"][0]["folder"], "Novels");
        let imported = dir.join(&reader.entry(&id)?.path);
        assert_eq!(fs::read(&imported)?, original_bytes);
        let opened = reader.handle(Request::OpenBook { book_id: id.clone() }, Vec::new())?;
        assert_eq!(opened["sections"][0]["front_matter"], true);
        assert_eq!(opened["sections"][1]["front_matter"], true);
        assert_eq!(opened["sections"][2]["front_matter"], false);
        assert_eq!(reader.set_numbered(&id, 0, true)?["0"], true);
        assert_eq!(reader.set_numbered(&id, 3, false)?["3"], false);
        assert!(reader.remove_book(&id).is_err());
        reader.handle(Request::Save {
            book_id: Some(id.clone()), position: Some(Position { chapter: 2, offset: 9, fraction: 0.25 }),
            settings: Value::Null,
        }, Vec::new())?;
        drop(reader);

        let mut reader = Reader::new(state.clone());
        let opened = reader.handle(Request::OpenBook { book_id: id.clone() }, Vec::new())?;
        assert_eq!(opened["numbering"], json!({"0": true, "3": false}));
        assert_eq!(reader.catalog()[0]["resume_number"], 2);
        assert_eq!(reader.set_numbered(&id, 0, false)?, json!({"3": false}));
        reader.handle(Request::CloseBook, Vec::new())?;
        assert_eq!(reader.remove_book(&id)?["library"], json!([]));
        assert!(!imported.exists());
        assert_eq!(fs::read(&source)?, original_bytes);

        let mut reader = Reader::new(state.clone());
        reader.startup_book = None;
        assert_eq!(reader.handle(Request::Init, Vec::new())?["library"], json!([]));
        reader.store_link(&source, &Book::open(&source)?);
        assert_eq!(reader.catalog()[0]["position"]["offset"], 9);
        assert!(!reader.entry(&id)?.managed_copy());
        reader.remove_book(&id)?;
        assert_eq!(fs::read(&source)?, original_bytes);
        fs::remove_file(source)?;
        fs::remove_file(state)?;
        fs::remove_dir(dir.join("Books"))?;
        fs::remove_dir(dir)?;
        Ok(())
    }
}
