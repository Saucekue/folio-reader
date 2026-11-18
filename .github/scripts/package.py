"""Assemble the portable release ZIP the same way earlier releases were laid out.

Usage: package.py VERSION WEBVIEW2_DIR OUT_DIR
  VERSION       version from Cargo.toml, e.g. 1.14.0
  WEBVIEW2_DIR  extracted Microsoft.Web.WebView2 NuGet package
  OUT_DIR       where the ZIP is written

Run from the repository root after `cargo build --release --locked`.
"""
import hashlib, json, pathlib, re, subprocess, sys, zipfile

TARGET = "x86_64-pc-windows-gnullvm"
# WebView2Loader.dll shipped with 1.9.0 through 1.10.2 (NuGet Microsoft.Web.WebView2 1.0.4314.35).
KNOWN_LOADER_SHA256 = "d3fce90bdeecb08596e377dea6a316354dd8949e32ff09b89d51be00c14e21b6"

version, webview2_dir, out_dir = sys.argv[1], pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
root = pathlib.Path.cwd()


def read_me():
    """Read Me.txt is the README with its build section replaced by the loader note."""
    readme = (root / "README.md").read_text(encoding="utf-8")
    cut = readme.index("## Build from source")
    return readme[:cut] + "Keep WebView2Loader.dll next to Folio Reader.exe.\n"


def version_key(text):
    return tuple(int(part) if part.isdigit() else 0 for part in re.split(r"[.+-]", text))


def rust_notices():
    """Every crate compiled into the app, each followed by its license files."""
    metadata = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        check=True, capture_output=True, text=True).stdout)
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    root_id = metadata["resolve"]["root"]
    seen, stack = set(), [root_id]
    while stack:
        for dep in nodes[stack.pop()]["deps"]:
            if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])
    crates = sorted((packages[id] for id in seen), key=lambda p: (p["name"], version_key(p["version"])))
    blocks = []
    for crate in crates:
        folder = pathlib.Path(crate["manifest_path"]).parent
        files = sorted(path for path in folder.iterdir()
                       if path.is_file() and path.name.lower().startswith(("license", "copying")))
        if not files:
            sys.exit(f"No license file found for {crate['name']} {crate['version']}")
        texts = []
        for path in files:
            text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
            texts.append(path.name + "\n" + (text if text.endswith("\n") else text + "\n"))
        blocks.append(f"{crate['name']}-{crate['version']}\n" + "\n".join(texts))
    return "\n".join(blocks)


def find(name):
    matches = [path for path in webview2_dir.rglob("*") if path.name.lower() == name.lower()]
    if name == "WebView2Loader.dll":
        matches = [path for path in matches if path.parent.name.lower() == "x64"]
    if len(matches) != 1:
        sys.exit(f"Expected one {name} in the WebView2 package, found {len(matches)}")
    return matches[0].read_bytes()


loader = find("WebView2Loader.dll")
digest = hashlib.sha256(loader).hexdigest()
if digest != KNOWN_LOADER_SHA256:
    print(f"::warning::WebView2Loader.dll differs from earlier releases (sha256 {digest})")

files = [
    ("Folio Reader.exe", (root / "target" / "release" / "folio-reader.exe").read_bytes()),
    ("Read Me.txt", read_me().encode("utf-8")),
    ("WebView2Loader.dll", loader),
    ("Third-party notices/Rust dependencies.txt", rust_notices().encode("utf-8")),
    ("Third-party notices/WebView2-LICENSE.txt", find("LICENSE.txt")),
    ("Third-party notices/WebView2-NOTICE.txt", find("NOTICE.txt")),
]
out_dir.mkdir(parents=True, exist_ok=True)
archive = out_dir / f"Folio-Reader-{version}-Windows-x64-Portable.zip"
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
    for name, data in files:
        bundle.writestr(name, data)
print(archive)
