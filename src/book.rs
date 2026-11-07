use std::{collections::HashMap, fs::File, io::Read, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};
use roxmltree::{Document, Node, ParsingOptions};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TEXT_LIMIT: u64 = 12 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 12 * 1024 * 1024;

#[derive(Serialize)]
pub struct Section {
    title: String,
    path: String,
    fragment: Option<String>,
    start: Option<String>,
    end: Option<String>,
    pub front_matter: bool,
}

pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub cover: Option<Cover>,
    pub sections: Vec<Section>,
    archive: ZipArchive<File>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Cover {
    path: String,
    mime: String,
}

impl Cover {
    pub fn read(&self, book: &Path) -> Result<String> {
        let mut archive = ZipArchive::new(File::open(book)?)?;
        image_url(&mut archive, &self.path, &self.mime)
    }
}

struct Item<'a> {
    href: &'a str,
    kind: &'a str,
    properties: &'a str,
}

struct Heading {
    path: String,
    fragment: Option<String>,
    title: String,
}

impl Book {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_named(path, path.file_stem().unwrap_or_default().to_string_lossy().as_ref())
    }

    pub fn open_named(path: &Path, fallback_title: &str) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        let mut archive = ZipArchive::new(file).map_err(|_| "This file is not a readable EPUB archive.")?;
        let container = read_text(&mut archive, "META-INF/container.xml")
            .map_err(|_| "The EPUB container is missing or unreadable.")?;
        let container = xml(&container)?;
        let package = container.descendants().find(|n| n.has_tag_name("rootfile"))
            .and_then(|n| n.attribute("full-path")).ok_or("The EPUB has no package document.")?;
        let package = target("", package).ok_or("Invalid EPUB package path.")?.0;
        let opf = read_text(&mut archive, &package)?;
        let doc = xml(&opf)?;
        let manifest = doc.descendants().find(|n| n.has_tag_name("manifest")).ok_or("Missing EPUB manifest.")?;
        let spine = doc.descendants().find(|n| n.has_tag_name("spine")).ok_or("Missing EPUB reading order.")?;
        let items: HashMap<_, _> = manifest.children().filter_map(|n| {
            Some((n.attribute("id")?, Item {
                href: n.attribute("href")?,
                kind: n.attribute("media-type").unwrap_or_default(),
                properties: n.attribute("properties").unwrap_or_default(),
            }))
        }).collect();
        let mut order: Vec<_> = spine.children().filter_map(|n| {
            let item = items.get(n.attribute("idref")?)?;
            if !matches!(item.kind, "application/xhtml+xml" | "text/html") { return None; }
            let (path, _) = target(&package, item.href)?;
            Some((path, n.attribute("linear") != Some("no")))
        }).collect();
        if order.iter().any(|(_, linear)| *linear) {
            order.retain(|(_, linear)| *linear);
        }
        if order.is_empty() {
            return Err("This EPUB has no readable text chapters.".into());
        }

        let nav = items.values().find(|item| item.properties.split_whitespace().any(|p| p == "nav"))
            .and_then(|item| target(&package, item.href)).map(|(path, _)| path);
        let mut front_pages: Vec<_> = doc.descendants().find(|node| node.has_tag_name("guide"))
            .into_iter().flat_map(|guide| guide.children())
            .filter(|node| matches!(node.attribute("type"), Some("cover" | "toc")))
            .filter_map(|node| target(&package, node.attribute("href")?)).collect();
        if let Some(nav) = nav.as_deref() {
            front_pages.extend(navigation_landmarks(&mut archive, nav));
        }
        let ncx = spine.attribute("toc").and_then(|id| items.get(id))
            .or_else(|| items.values().find(|item| item.kind == "application/x-dtbncx+xml"))
            .and_then(|item| target(&package, item.href)).map(|(path, _)| path);
        let mut headings = nav.as_deref().map(|p| read_headings(&mut archive, p, false)).unwrap_or_default();
        if headings.is_empty() {
            headings = ncx.as_deref().map(|p| read_headings(&mut archive, p, true)).unwrap_or_default();
        }

        let mut sections = Vec::new();
        let mut fingerprint: u64 = 0xcbf29ce484222325;
        for (path, _) in order {
            if let Ok(file) = archive.by_name(&path) {
                for byte in file.crc32().to_le_bytes().into_iter().chain(path.bytes()) {
                    fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            } else {
                continue;
            }
            let mut labels = Vec::<&Heading>::new();
            for heading in headings.iter().filter(|heading| heading.path == path) {
                if !labels.iter().any(|prior| prior.fragment == heading.fragment) {
                    labels.push(heading);
                }
            }
            // Several EPUB chapters may share one XHTML file. Only use split points
            // that exist in its DOM, and use document order for the section bounds.
            if labels.iter().filter(|h| h.fragment.is_some()).count() > 1
                && let Ok(source) = read_text(&mut archive, &path)
                && let Ok(document) = xml(&source)
            {
                let mut splits = Vec::new();
                for node in document.descendants().filter(Node::is_element) {
                    let id = node.attribute("id").or_else(|| node.attribute("name"));
                    if id.is_some() && let Some(heading) = labels.iter().find(|h| h.fragment.as_deref() == id) {
                        splits.push(*heading);
                    }
                }
                if splits.len() > 1 {
                    for (index, heading) in splits.iter().enumerate() {
                        sections.push(Section {
                            title: heading.title.clone(), path: path.clone(),
                            fragment: heading.fragment.clone(),
                            start: (index != 0).then(|| heading.fragment.clone()).flatten(),
                            end: splits.get(index + 1).and_then(|next| next.fragment.clone()),
                            front_matter: is_front_matter(&heading.title, &path, heading.fragment.as_deref(), &front_pages),
                        });
                    }
                    continue;
                }
            }
            let title = labels.first().map(|heading| heading.title.clone()).or_else(|| {
                let chapter = read_text(&mut archive, &path).ok()?;
                let chapter = xml(&chapter).ok()?;
                chapter.descendants().find(|n| n.has_tag_name("h1") || n.has_tag_name("h2"))
                    .or_else(|| chapter.descendants().find(|n| n.has_tag_name("title")))
                    .map(text).filter(|title| !title.is_empty())
            }).unwrap_or_else(|| format!("Chapter {}", sections.len() + 1));
            let fragment = labels.first().and_then(|h| h.fragment.clone());
            let file = path.rsplit('/').next().unwrap_or(&path);
            let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            let front_matter = nav.as_deref() == Some(path.as_str())
                || matches!(stem.to_ascii_lowercase().as_str(), "cover" | "cover-page" | "toc")
                || is_front_matter(&title, &path, fragment.as_deref(), &front_pages);
            sections.push(Section { title, path, fragment, start: None, end: None, front_matter });
        }
        if sections.is_empty() {
            return Err("The EPUB's chapter files are missing.".into());
        }
        let metadata = doc.descendants().find(|n| n.has_tag_name("metadata"));
        let titles = metadata.into_iter().flat_map(|node| node.children()).filter(|n| n.has_tag_name("title"));
        let title = titles.clone().find(|node| refinements(metadata, *node, "title-type").any(|value| value == "main"))
            .map(text).filter(|title| !title.is_empty())
            .or_else(|| titles.map(text).find(|title| !title.is_empty()))
            .unwrap_or_else(|| fallback_title.to_owned());
        let creators = metadata.into_iter().flat_map(|node| node.children()).filter(|n| n.has_tag_name("creator"));
        let authors: Vec<_> = creators.clone().filter(|node| {
            node.attributes().any(|attr| attr.name() == "role" && attr.value() == "aut")
                || refinements(metadata, *node, "role").any(|value| value == "aut" || value.ends_with("/aut"))
        }).map(text).filter(|name| !name.is_empty()).collect();
        let author = if authors.is_empty() {
            creators.map(text).filter(|name| !name.is_empty()).collect::<Vec<_>>().join(", ")
        } else {
            authors.join(", ")
        };
        let cover = find_cover(&mut archive, &doc, &package, &items, nav.as_deref());
        Ok(Self { id: format!("{size:x}-{fingerprint:016x}"), title, author, cover, sections, archive })
    }

    pub fn chapter(&mut self, index: usize) -> Result<String> {
        let section = self.sections.get(index).ok_or("Chapter is outside the reading order.")?;
        read_text(&mut self.archive, &section.path)
    }

    pub fn image(&mut self, path: &str) -> Result<String> {
        let mime = image_mime(path).ok_or("Unsupported EPUB image format.")?;
        image_url(&mut self.archive, path, mime)
    }
}

fn is_front_matter(title: &str, path: &str, fragment: Option<&str>, declared: &[(String, Option<String>)]) -> bool {
    if declared.iter().any(|(file, anchor)| file == path && (anchor.is_none() || anchor.as_deref() == fragment)) {
        return true;
    }
    let label = title.rsplit([':', '–', '—']).next().unwrap_or(title).trim();
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    matches!(label.as_str(), "cover" | "cover page" | "front cover" | "table of contents" | "contents" | "toc")
}

fn navigation_landmarks(archive: &mut ZipArchive<File>, path: &str) -> Vec<(String, Option<String>)> {
    let Ok(source) = read_text(archive, path) else { return Vec::new() };
    let Ok(doc) = xml(&source) else { return Vec::new() };
    doc.descendants().filter(|node| node.has_tag_name("a") && node.attributes().any(|attr| {
        attr.name() == "type" && attr.value().split_whitespace().any(|value| matches!(value, "cover" | "toc"))
    })).filter_map(|node| target(path, node.attribute("href")?)).collect()
}

fn refinements<'a, 'input: 'a>(
    metadata: Option<Node<'a, 'input>>, node: Node<'a, 'input>, property: &'a str,
) -> impl Iterator<Item = String> + 'a {
    let id = node.attribute("id");
    metadata.into_iter().flat_map(|node| node.children()).filter(move |meta| {
        id.is_some() && meta.has_tag_name("meta") && meta.attribute("property") == Some(property)
            && meta.attribute("refines").and_then(|value| value.strip_prefix('#')) == id
    }).map(text)
}

fn find_cover(
    archive: &mut ZipArchive<File>, doc: &Document<'_>, package: &str,
    items: &HashMap<&str, Item<'_>>, nav: Option<&str>,
) -> Option<Cover> {
    let declared = items.values().find(|item| item.properties.split_whitespace().any(|p| p == "cover-image"));
    let legacy = doc.descendants().find(|n| n.has_tag_name("meta") && n.attribute("name") == Some("cover"))
        .and_then(|n| n.attribute("content")).and_then(|id| items.get(id));
    for item in [declared, legacy].into_iter().flatten() {
        if let Some((path, _)) = target(package, item.href)
            && let Some(cover) = cover_at(archive, path, Some(item.kind))
        {
            return Some(cover);
        }
    }
    let guide = doc.descendants().find(|node| node.has_tag_name("guide"))
        .and_then(|node| node.children().find(|n| n.attribute("type") == Some("cover")))
        .and_then(|node| node.attribute("href"))
        .and_then(|href| target(package, href)).map(|(path, _)| path);
    let landmark = nav.and_then(|path| {
        let source = read_text(archive, path).ok()?;
        let document = xml(&source).ok()?;
        let href = document.descendants().find(|node| node.has_tag_name("a") && node.attributes().any(|attr| {
            attr.name() == "type" && attr.value().split_whitespace().any(|value| value == "cover")
        }))?.attribute("href")?;
        target(path, href).map(|(path, _)| path)
    });
    for path in [guide, landmark].into_iter().flatten() {
        if let Some(cover) = cover_at(archive, path, None) { return Some(cover); }
    }
    // Older books occasionally omit the cover declaration but name the image.
    items.values().filter(|item| {
        item.href.rsplit('/').next().is_some_and(|file| {
            file.rsplit_once('.').is_some_and(|(stem, _)| stem.eq_ignore_ascii_case("cover"))
        }) && valid_image_mime(item.kind)
    }).find_map(|item| cover_at(archive, target(package, item.href)?.0, Some(item.kind)))
}

fn cover_at(archive: &mut ZipArchive<File>, path: String, declared_mime: Option<&str>) -> Option<Cover> {
    if let Some(mime) = declared_mime.filter(|mime| valid_image_mime(mime)).or_else(|| image_mime(&path)) {
        let file = archive.by_name(&path).ok()?;
        return (file.size() <= IMAGE_LIMIT).then(|| Cover { path, mime: mime.to_owned() });
    }
    let source = read_text(archive, &path).ok()?;
    let doc = xml(&source).ok()?;
    doc.descendants().filter(|n| n.has_tag_name("img") || n.has_tag_name("image")).find_map(|node| {
        let href = node.attribute("src").or_else(|| {
            node.attributes().find(|attr| attr.name() == "href").map(|attr| attr.value())
        })?;
        let (path, _) = target(&path, href)?;
        let mime = image_mime(&path)?;
        let file = archive.by_name(&path).ok()?;
        (file.size() <= IMAGE_LIMIT).then(|| Cover { path, mime: mime.to_owned() })
    })
}

fn valid_image_mime(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml" | "image/avif")
}

fn image_mime(path: &str) -> Option<&'static str> {
    match path.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

fn image_url(archive: &mut ZipArchive<File>, path: &str, mime: &str) -> Result<String> {
    if !valid_image_mime(mime) { return Err("Unsupported EPUB image format.".into()); }
    let bytes = read_entry(archive, path, IMAGE_LIMIT)?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

fn xml(source: &str) -> std::result::Result<Document<'_>, roxmltree::Error> {
    Document::parse_with_options(source, ParsingOptions { allow_dtd: true, ..Default::default() })
}

fn text(node: Node<'_, '_>) -> String {
    node.descendants().filter(Node::is_text).filter_map(|n| n.text()).collect::<String>()
        .split_whitespace().collect::<Vec<_>>().join(" ")
}

fn read_headings(archive: &mut ZipArchive<File>, path: &str, ncx: bool) -> Vec<Heading> {
    let Ok(source) = read_text(archive, path) else { return Vec::new() };
    let Ok(doc) = xml(&source) else { return Vec::new() };
    if ncx {
        doc.descendants().filter(|n| n.has_tag_name("navPoint")).filter_map(|n| {
            let label = n.children().find(|child| child.has_tag_name("navLabel")).map(text)?;
            let href = n.children().find(|child| child.has_tag_name("content"))?.attribute("src")?;
            heading(path, href, label)
        }).collect()
    } else {
        let toc = doc.descendants().filter(|n| n.has_tag_name("nav")).find(|n| {
            n.attributes().any(|a| {
                (a.name() == "type" && a.value().split_whitespace().any(|value| value == "toc"))
                    || (a.name() == "role" && a.value() == "doc-toc")
            })
        }).or_else(|| doc.descendants().find(|n| n.has_tag_name("nav")));
        toc.into_iter().flat_map(|n| n.descendants()).filter(|n| n.has_tag_name("a"))
            .filter_map(|n| heading(path, n.attribute("href")?, text(n))).collect()
    }
}

fn heading(path: &str, href: &str, title: String) -> Option<Heading> {
    let (path, fragment) = target(path, href)?;
    (!title.is_empty()).then_some(Heading { path, fragment, title })
}

fn target(base: &str, href: &str) -> Option<(String, Option<String>)> {
    let (relative, fragment) = href.split_once('#').unwrap_or((href, ""));
    let fragment = (!fragment.is_empty()).then(|| decode(fragment));
    let relative = decode(relative.split('?').next().unwrap_or_default());
    if relative.is_empty() { return Some((base.to_owned(), fragment)); }
    if relative.contains([':', '\\', '\0']) || relative.starts_with("//") { return None; }
    let directory = base.rsplit_once('/').map(|(dir, _)| dir).unwrap_or_default();
    let mut parts: Vec<_> = if relative.starts_with('/') || directory.is_empty() { Vec::new() }
        else { directory.split('/').collect() };
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop()?; }
            _ => parts.push(part),
        }
    }
    Some((parts.join("/"), fragment))
}

fn decode(input: &str) -> String {
    let mut output = Vec::with_capacity(input.len());
    let mut remaining = input.as_bytes();
    while let Some((&first, rest)) = remaining.split_first() {
        if first == b'%' && rest.len() >= 2
            && let Ok(pair) = std::str::from_utf8(&rest[..2])
            && let Ok(byte) = u8::from_str_radix(pair, 16)
        {
            output.push(byte);
            remaining = &rest[2..];
        } else {
            output.push(first);
            remaining = rest;
        }
    }
    String::from_utf8(output).unwrap_or_else(|_| input.to_owned())
}

fn read_entry(archive: &mut ZipArchive<File>, path: &str, limit: u64) -> Result<Vec<u8>> {
    let file = archive.by_name(path)?;
    if file.size() > limit { return Err("An EPUB entry exceeds the reader's size limit.".into()); }
    let mut data = Vec::with_capacity(file.size() as usize);
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit { return Err("An EPUB entry exceeds the reader's size limit.".into()); }
    Ok(data)
}

fn read_text(archive: &mut ZipArchive<File>, path: &str) -> Result<String> {
    let bytes = read_entry(archive, path, TEXT_LIMIT)?;
    if let Some((rest, little_endian)) = bytes.strip_prefix(&[255, 254]).map(|rest| (rest, true))
        .or_else(|| bytes.strip_prefix(&[254, 255]).map(|rest| (rest, false)))
    {
        let words: Vec<_> = rest.chunks_exact(2).map(|pair| {
            let pair = [pair[0], pair[1]];
            if little_endian { u16::from_le_bytes(pair) } else { u16::from_be_bytes(pair) }
        }).collect();
        return Ok(String::from_utf16_lossy(&words));
    }
    Ok(String::from_utf8(bytes)?.trim_start_matches('\u{feff}').to_owned())
}
