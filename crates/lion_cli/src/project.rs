//! Projects and packages (spec §20.4, §25, step 8, C101).
//!
//! A project is a folder with a `lion.toml` file, which names the project, its edition
//! and its dependencies: packages from a git repository or a local folder. `lion.lock`
//! keeps the exact commit of each package from git, and a checksum of its files, so
//! that the same project gives the same program everywhere. Packages from git are
//! copied once to the cache of Lion and never changed.
//!
//! `use geometrie` reaches the module `geometrie.lion` of the package `geometrie`, and
//! `use geometrie.cercle` its module `cercle.lion`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The editions that this version of Lion knows (§25).
pub const EDITIONS: &[&str] = &["0.1"];

/// The file of a project.
pub const MANIFEST: &str = "lion.toml";
/// The versions of the packages, written by `lion`.
pub const LOCK: &str = "lion.lock";

/// What a project or a package declares in its `lion.toml`.
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub dependencies: Vec<(String, Spec)>,
}

/// Where a dependency comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Spec {
    /// A git repository, with the versions accepted (`"1.2"`: from 1.2.0 to 2.0.0), or
    /// any version.
    Git { url: String, version: Option<String> },
    /// A folder, relative to the project that names it.
    Path(PathBuf),
}

/// The packages that each file of a program may use: those of the project, and those
/// of each package, by the folder of the package.
#[derive(Default)]
pub struct Resolution {
    pub project: HashMap<String, PathBuf>,
    pub packages: HashMap<PathBuf, HashMap<String, PathBuf>>,
}

/// The folder of the project that holds `folder`: the nearest one with a `lion.toml`.
pub fn find_root(folder: &Path) -> Option<PathBuf> {
    let start = fs::canonicalize(folder).ok()?;
    start.ancestors().find(|candidate| candidate.join(MANIFEST).is_file()).map(Path::to_path_buf)
}

// ---------------------------------------------------------------------------------
// lion.toml and lion.lock: a small part of TOML (sections, `key = "text"`, and inline
// tables of texts).

#[derive(Clone, Debug, PartialEq)]
enum TomlValue {
    Text(String),
    Table(Vec<(String, String)>),
}

/// The entries of a TOML text: the section, the key and the value of each line.
fn parse_toml(text: &str, file: &str) -> Result<Vec<(String, String, TomlValue)>, String> {
    let mut entries = Vec::new();
    let mut section = String::new();
    for (number, line) in text.lines().enumerate() {
        let wrong = |what: &str| format!("{file}, line {}: {what}", number + 1);
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| wrong("expected `key = value`"))?;
        let key = key.trim().trim_matches('"').to_string();
        let value = value.trim();
        let value = if let Some(inner) = value.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
            let mut fields = Vec::new();
            for field in split_fields(inner) {
                let (name, text) =
                    field.split_once('=').ok_or_else(|| wrong("expected `name = \"text\"`"))?;
                let text =
                    parse_text(text.trim()).ok_or_else(|| wrong("a value is written between quotes"))?;
                fields.push((name.trim().to_string(), text));
            }
            TomlValue::Table(fields)
        } else {
            TomlValue::Text(parse_text(value).ok_or_else(|| wrong("a value is written between quotes"))?)
        };
        entries.push((section.clone(), key, value));
    }
    Ok(entries)
}

/// The line without its comment, which starts with `#` outside quotes.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The fields of an inline table, split at the commas outside quotes.
fn split_fields(inner: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (at, c) in inner.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(&inner[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    fields.push(&inner[start..]);
    fields.into_iter().filter(|field| !field.trim().is_empty()).collect()
}

fn parse_text(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                '"' => text.push('"'),
                '\\' => text.push('\\'),
                'n' => text.push('\n'),
                't' => text.push('\t'),
                _ => return None,
            }
        } else {
            text.push(c);
        }
    }
    Some(text)
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Reads the `lion.toml` of a project or a package.
pub fn read_manifest(root: &Path) -> Result<Manifest, String> {
    let path = root.join(MANIFEST);
    let file = path.display().to_string();
    let text = fs::read_to_string(&path).map_err(|error| format!("cannot read `{file}`: {error}"))?;
    let mut manifest = Manifest {
        name: String::new(),
        version: String::new(),
        edition: String::new(),
        dependencies: Vec::new(),
    };
    for (section, key, value) in parse_toml(&text, &file)? {
        match (section.as_str(), key.as_str(), value) {
            ("project", "name", TomlValue::Text(name)) => manifest.name = name,
            ("project", "version", TomlValue::Text(version)) => manifest.version = version,
            ("project", "edition", TomlValue::Text(edition)) => manifest.edition = edition,
            ("dependencies", name, TomlValue::Table(fields)) => {
                let field = |wanted: &str| {
                    fields.iter().find(|(key, _)| key == wanted).map(|(_, value)| value.clone())
                };
                let spec = match (field("git"), field("path")) {
                    (Some(url), None) => Spec::Git { url, version: field("version") },
                    (None, Some(folder)) => Spec::Path(PathBuf::from(folder)),
                    _ => {
                        return Err(format!(
                            "{file}: the dependency `{name}` gives either `git = \"...\"` or `path = \"...\"`"
                        ));
                    }
                };
                manifest.dependencies.push((name.to_string(), spec));
            }
            (section, key, _) => {
                return Err(format!("{file}: `{key}` is not an entry that Lion knows in [{section}]"));
            }
        }
    }
    if manifest.name.is_empty() {
        return Err(format!("{file}: the project has no name (`name = \"...\"` in [project])"));
    }
    if !EDITIONS.contains(&manifest.edition.as_str()) {
        let found = if manifest.edition.is_empty() {
            "no edition".to_string()
        } else {
            format!("the edition `{}`", manifest.edition)
        };
        return Err(format!(
            "{file}: the project declares {found}; this version of Lion knows the editions {} (§25)",
            EDITIONS.join(", ")
        ));
    }
    Ok(manifest)
}

/// A package of git, as `lion.lock` keeps it.
#[derive(Clone, Debug, PartialEq)]
struct Locked {
    url: String,
    version: String,
    commit: String,
    checksum: String,
}

fn read_lock(root: &Path) -> Result<BTreeMap<String, Locked>, String> {
    let path = root.join(LOCK);
    let Ok(text) = fs::read_to_string(&path) else { return Ok(BTreeMap::new()) };
    let mut lock = BTreeMap::new();
    for (_, name, value) in parse_toml(&text, &path.display().to_string())? {
        let TomlValue::Table(fields) = value else { continue };
        let field = |wanted: &str| {
            fields.iter().find(|(key, _)| key == wanted).map(|(_, value)| value.clone()).unwrap_or_default()
        };
        lock.insert(
            name,
            Locked {
                url: field("git"),
                version: field("version"),
                commit: field("commit"),
                checksum: field("checksum"),
            },
        );
    }
    Ok(lock)
}

fn write_lock(root: &Path, lock: &BTreeMap<String, Locked>) -> Result<(), String> {
    let mut text = String::from(
        "# Written by lion: the exact version of each package that comes from git.\n\n[packages]\n",
    );
    for (name, locked) in lock {
        text.push_str(&format!(
            "{name} = {{ git = {}, version = {}, commit = {}, checksum = {} }}\n",
            quote(&locked.url),
            quote(&locked.version),
            quote(&locked.commit),
            quote(&locked.checksum)
        ));
    }
    let path = root.join(LOCK);
    if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    fs::write(&path, text).map_err(|error| format!("cannot write `{}`: {error}", path.display()))
}

// ---------------------------------------------------------------------------------
// Versions: `major.minor.patch`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    /// `1.2.3`, `v1.2.3`, or a shorter form: `1.2` is 1.2.0.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let parts: Vec<u64> = text.split('.').map(|part| part.parse().ok()).collect::<Option<_>>()?;
        match parts.as_slice() {
            [major] => Some(Version(*major, 0, 0)),
            [major, minor] => Some(Version(*major, *minor, 0)),
            [major, minor, patch] => Some(Version(*major, *minor, *patch)),
            _ => None,
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Whether `version` is accepted by `wanted`: from `wanted` to the next version that
/// may break it, `1.2` accepting up to 2.0.0 excluded, and `0.3` up to 0.4.0 excluded.
pub fn accepts(wanted: &str, version: Version) -> bool {
    let Some(low) = Version::parse(wanted) else { return false };
    let parts = wanted.trim_start_matches('v').split('.').count();
    let high = match (low, parts) {
        (Version(0, 0, _), 3) => Version(0, 0, low.2 + 1),
        (Version(0, minor, _), 2 | 3) => Version(0, minor + 1, 0),
        (Version(major, ..), _) => Version(major + 1, 0, 0),
    };
    low <= version && version < high
}

// ---------------------------------------------------------------------------------
// git.

fn git(args: &[&str], folder: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(folder) = folder {
        command.current_dir(folder);
    }
    let output =
        command.output().map_err(|error| format!("cannot run `git`, which gets the packages: {error}"))?;
    if !output.status.success() {
        let problem = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("`git {}` failed: {problem}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The versions of a repository, from its tags `v1.2.0` or `1.2.0`, with their commits.
fn tags(url: &str) -> Result<Vec<(Version, String)>, String> {
    let listing = git(&["ls-remote", "--tags", url], None)?;
    let mut found: BTreeMap<Version, String> = BTreeMap::new();
    let mut peeled: BTreeMap<Version, String> = BTreeMap::new();
    for line in listing.lines() {
        let Some((commit, reference)) = line.split_once('\t') else { continue };
        let Some(tag) = reference.strip_prefix("refs/tags/") else { continue };
        let (tag, is_peeled) = match tag.strip_suffix("^{}") {
            Some(tag) => (tag, true),
            None => (tag, false),
        };
        let Some(version) = Version::parse(tag) else { continue };
        if is_peeled {
            peeled.insert(version, commit.to_string());
        } else {
            found.insert(version, commit.to_string());
        }
    }
    // An annotated tag names an object that names the commit.
    found.extend(peeled);
    Ok(found.into_iter().collect())
}

/// The commit of the default branch of a repository.
fn head(url: &str) -> Result<String, String> {
    let listing = git(&["ls-remote", url, "HEAD"], None)?;
    listing
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("the repository `{url}` has no commit"))
}

/// A checksum of the files of a folder, by their paths and contents.
fn checksum(folder: &Path) -> String {
    let mut files = Vec::new();
    collect_files(folder, folder, &mut files);
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for relative in files {
        let content = fs::read(folder.join(&relative)).unwrap_or_default();
        for byte in relative.bytes().chain([0]).chain(content) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

fn collect_files(root: &Path, folder: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The folder of the cache where Lion keeps the packages from git.
fn packages_folder() -> PathBuf {
    crate::native::cache_folder().join("packages")
}

/// The folder of the package at this commit, fetched first if needed.
fn fetch(name: &str, url: &str, commit: &str) -> Result<PathBuf, String> {
    let short = &commit[..commit.len().min(12)];
    let target = packages_folder().join(format!("{name}-{short}"));
    if target.join(MANIFEST).is_file() {
        return Ok(target);
    }
    let parent = packages_folder();
    fs::create_dir_all(&parent).map_err(|error| format!("cannot write `{}`: {error}", parent.display()))?;
    let temporary = parent.join(format!(".{name}-{short}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temporary);
    let temporary_text = temporary.to_string_lossy().into_owned();
    eprintln!("fetching `{name}` from {url}");
    git(&["clone", "--quiet", url, &temporary_text], None)?;
    git(&["checkout", "--quiet", commit], Some(&temporary))?;
    let _ = fs::remove_dir_all(temporary.join(".git"));
    if !temporary.join(MANIFEST).is_file() {
        let _ = fs::remove_dir_all(&temporary);
        return Err(format!("the repository {url} has no `{MANIFEST}`: it is not a Lion package"));
    }
    if fs::rename(&temporary, &target).is_err() {
        // Another `lion` put it there first.
        let _ = fs::remove_dir_all(&temporary);
    }
    Ok(target)
}

// ---------------------------------------------------------------------------------
// Resolution.

/// A project or a package whose dependencies are to be found: its folder, its name,
/// whether it comes from git, and its dependencies.
type Pending = (PathBuf, String, bool, Vec<(String, Spec)>);

/// Finds the package of each dependency of the project, and of each package, fetching
/// those that are missing; `lion.lock` gives the versions of the packages from git,
/// except those of `update` (every package for `Some("")`), which take the latest
/// version accepted.
pub fn resolve(root: &Path, update: Option<&str>) -> Result<Resolution, String> {
    let manifest = read_manifest(root)?;
    let old_lock = read_lock(root)?;
    let mut lock = BTreeMap::new();
    let mut resolution = Resolution::default();
    // Each package once: its folder, who asked for it first, and which one it is.
    let mut chosen: HashMap<String, (PathBuf, String, String)> = HashMap::new();
    let mut queue: Vec<Pending> =
        vec![(root.to_path_buf(), manifest.name.clone(), false, manifest.dependencies)];
    let mut index = 0;
    while index < queue.len() {
        let (folder, owner, from_git, dependencies) = queue[index].clone();
        index += 1;
        let mut reached = HashMap::new();
        for (name, spec) in dependencies {
            if lion_std::module(&name).is_some() {
                return Err(format!(
                    "the package `{name}` of `{owner}` has the name of a module of the standard library"
                ));
            }
            let (package, is_git, label) = match &spec {
                Spec::Path(path) => {
                    if from_git {
                        return Err(format!(
                            "`{owner}` comes from git, so its dependency `{name}` cannot be a local folder"
                        ));
                    }
                    let path = folder.join(path);
                    let package = fs::canonicalize(&path).map_err(|_| {
                        format!(
                            "the dependency `{name}` of `{owner}` names `{}`, which does not exist",
                            path.display()
                        )
                    })?;
                    let label = format!("the folder {}", package.display());
                    (package, false, label)
                }
                Spec::Git { url, version } => {
                    let updating = update.is_some_and(|wanted| wanted.is_empty() || wanted == name);
                    let kept = old_lock.get(&name).filter(|locked| {
                        !updating
                            && locked.url == *url
                            && match (version, Version::parse(&locked.version)) {
                                (Some(wanted), Some(found)) => accepts(wanted, found),
                                (None, _) => true,
                                _ => false,
                            }
                    });
                    let locked = match kept {
                        Some(locked) => locked.clone(),
                        None => {
                            let (found, commit) = choose(url, version.as_deref(), &name)?;
                            Locked { url: url.clone(), version: found, commit, checksum: String::new() }
                        }
                    };
                    let package = fetch(&name, url, &locked.commit)?;
                    let sum = checksum(&package);
                    if !locked.checksum.is_empty() && locked.checksum != sum {
                        return Err(format!(
                            "the files of `{name}` {} differ from those that `{LOCK}` recorded: remove `{}` and try again",
                            locked.version,
                            package.display()
                        ));
                    }
                    let label = if locked.version.is_empty() {
                        format!("the commit {} of {url}", &locked.commit[..locked.commit.len().min(12)])
                    } else {
                        format!("{} from {url}", locked.version)
                    };
                    lock.insert(name.clone(), Locked { checksum: sum, ..locked });
                    (package, true, label)
                }
            };
            let own = read_manifest(&package)?;
            if own.name != name {
                return Err(format!(
                    "the dependency `{name}` of `{owner}` is the package `{}`: write `{} = ...` in `{MANIFEST}`",
                    own.name, own.name
                ));
            }
            match chosen.get(&name) {
                Some((known, first, first_label)) if *known != package => {
                    return Err(format!(
                        "two versions of `{name}` are needed: `{first}` uses {first_label}, and `{owner}` uses {label}"
                    ));
                }
                Some(_) => {}
                None => {
                    chosen.insert(name.clone(), (package.clone(), owner.clone(), label));
                    queue.push((package.clone(), name.clone(), is_git, own.dependencies));
                }
            }
            reached.insert(name, package);
        }
        if folder == root {
            resolution.project = reached;
        } else {
            resolution.packages.insert(folder, reached);
        }
    }
    if !lock.is_empty() || root.join(LOCK).exists() {
        write_lock(root, &lock)?;
    }
    Ok(resolution)
}

/// The latest version of the repository that `wanted` accepts, and its commit; the last
/// commit of the default branch when the repository has no version.
fn choose(url: &str, wanted: Option<&str>, name: &str) -> Result<(String, String), String> {
    let versions = tags(url)?;
    if versions.is_empty() {
        if let Some(wanted) = wanted {
            return Err(format!(
                "`{name}` asks for version {wanted}, but {url} has no version (no tag `v1.0.0`)"
            ));
        }
        return Ok((String::new(), head(url)?));
    }
    versions
        .into_iter()
        .rev()
        .find(|(version, _)| wanted.is_none_or(|wanted| accepts(wanted, *version)))
        .map(|(version, commit)| (version.to_string(), commit))
        .ok_or_else(|| format!("no version of {url} fits `{}` for `{name}`", wanted.unwrap_or("")))
}

// ---------------------------------------------------------------------------------
// The commands.

/// A name of a project or a package: a lowercase name of Lion (§4.2).
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `lion new name`: a project with a script.
pub fn new_project(name: &str) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!(
            "`{name}` cannot name a project: a lowercase letter, then letters, digits or `_` (§4.2)"
        ));
    }
    let folder = Path::new(name);
    if folder.exists() {
        return Err(format!("`{name}` exists already"));
    }
    fs::create_dir_all(folder).map_err(|error| format!("cannot create `{name}`: {error}"))?;
    let manifest = format!(
        "[project]\nname = {}\nversion = \"0.1.0\"\nedition = {}\n\n[dependencies]\n",
        quote(name),
        quote(EDITIONS[EDITIONS.len() - 1])
    );
    let write = |file: &str, text: &str| {
        fs::write(folder.join(file), text).map_err(|error| format!("cannot write `{name}/{file}`: {error}"))
    };
    write(MANIFEST, &manifest)?;
    write("main.lion", "show(\"Bonjour !\")\n")?;
    Ok(format!("created the project `{name}`: `cd {name}`, then `lion run`"))
}

fn project_here() -> Result<PathBuf, String> {
    find_root(Path::new(".")).ok_or_else(|| {
        format!("there is no project here: no `{MANIFEST}` in this folder or above it; `lion new name` creates one")
    })
}

/// `lion add source [name]`: a dependency on a git repository or a folder.
pub fn add(source: &str, name: Option<&str>, force_git: bool) -> Result<String, String> {
    let root = project_here()?;
    let manifest = read_manifest(&root)?;
    let is_url = source.contains("://") || source.starts_with("git@");
    let (spec, found_name, shown) = if !is_url && !force_git && Path::new(source).is_dir() {
        let folder = fs::canonicalize(source).map_err(|error| format!("`{source}`: {error}"))?;
        let own = read_manifest(&folder)?;
        let relative = relative_path(&root, &folder);
        (Spec::Path(relative.clone()), own.name, format!("from the folder `{}`", relative.display()))
    } else {
        let url = if is_url {
            source.to_string()
        } else {
            fs::canonicalize(source).map_or(source.to_string(), |path| path.display().to_string())
        };
        let versions = tags(&url)?;
        let (version, commit) = match versions.last() {
            Some((version, commit)) => (Some(*version), commit.clone()),
            None => (None, head(&url)?),
        };
        // The last part of the address names the copy, until the package says its name.
        let guess =
            url.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or("package").trim_end_matches(".git");
        let guess: String = guess
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
            .collect();
        let probe = fetch(name.unwrap_or(&guess), &url, &commit)?;
        let own = read_manifest(&probe)?;
        let wanted = version.map(|version| {
            if version.0 == 0 { format!("0.{}", version.1) } else { format!("{}.{}", version.0, version.1) }
        });
        let shown = match version {
            Some(version) => format!("{version} from {url}"),
            None => format!("from {url} (no version: its last commit)"),
        };
        (Spec::Git { url, version: wanted }, own.name, shown)
    };
    let name = name.unwrap_or(&found_name).to_string();
    if name != found_name {
        return Err(format!("the package is named `{found_name}`, not `{name}`"));
    }
    if lion_std::module(&name).is_some() {
        return Err(format!("`{name}` is a module of the standard library: a package cannot take its name"));
    }
    if name == manifest.name {
        return Err(format!("`{name}` is the name of this project"));
    }
    if manifest.dependencies.iter().any(|(known, _)| *known == name) {
        return Err(format!(
            "the project already uses `{name}`; `lion update {name}` takes its latest version"
        ));
    }
    let line = match &spec {
        Spec::Git { url, version: Some(version) } => {
            format!("{name} = {{ git = {}, version = {} }}", quote(url), quote(version))
        }
        Spec::Git { url, version: None } => format!("{name} = {{ git = {} }}", quote(url)),
        Spec::Path(path) => format!("{name} = {{ path = {} }}", quote(&path.to_string_lossy())),
    };
    let path = root.join(MANIFEST);
    let text =
        fs::read_to_string(&path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    fs::write(&path, with_dependency(&text, &line))
        .map_err(|error| format!("cannot write `{}`: {error}", path.display()))?;
    if let Err(problem) = resolve(&root, None) {
        // The project stays as it was.
        let _ = fs::write(&path, text);
        return Err(problem);
    }
    Ok(format!("added `{name}` {shown}"))
}

/// `lion remove name`.
pub fn remove(name: &str) -> Result<String, String> {
    let root = project_here()?;
    let path = root.join(MANIFEST);
    let text =
        fs::read_to_string(&path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let mut section = String::new();
    let mut removed = false;
    let mut kept = String::new();
    for line in text.lines() {
        let content = strip_comment(line).trim();
        if let Some(name) = content.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.trim().to_string();
        }
        let key = content.split_once('=').map(|(key, _)| key.trim().trim_matches('"'));
        if section == "dependencies" && key == Some(name) {
            removed = true;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    if !removed {
        return Err(format!("the project does not use `{name}`"));
    }
    fs::write(&path, kept).map_err(|error| format!("cannot write `{}`: {error}", path.display()))?;
    resolve(&root, None)?;
    Ok(format!("removed `{name}`"))
}

/// `lion update [name]`: the latest versions that `lion.toml` accepts.
pub fn update(name: Option<&str>) -> Result<String, String> {
    let root = project_here()?;
    resolve(&root, Some(name.unwrap_or("")))?;
    Ok(match name {
        Some(name) => format!("updated `{name}`"),
        None => "updated the packages".to_string(),
    })
}

/// The text of `lion.toml` with one more line in `[dependencies]`.
fn with_dependency(text: &str, line: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|line| strip_comment(line).trim() == "[dependencies]");
    match start {
        None => {
            let mut text = text.trim_end().to_string();
            text.push_str(&format!("\n\n[dependencies]\n{line}\n"));
            text
        }
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|line| strip_comment(line).trim().starts_with('['))
                .map_or(lines.len(), |offset| start + 1 + offset);
            // After the last entry of the section.
            let mut at = end;
            while at > start + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.insert(at, line);
            let mut text = lines.join("\n");
            text.push('\n');
            text
        }
    }
}

/// `to` as seen from `from`, with `..` when needed.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in common..from.len() {
        path.push("..");
    }
    for part in &to[common..] {
        path.push(part);
    }
    if path.as_os_str().is_empty() { PathBuf::from(".") } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_accepted_as_cargo_does() {
        let v = |text: &str| Version::parse(text).unwrap();
        assert!(accepts("1.2", v("1.2.0")));
        assert!(accepts("1.2", v("1.9.3")));
        assert!(!accepts("1.2", v("2.0.0")));
        assert!(!accepts("1.2", v("1.1.9")));
        assert!(accepts("0.3", v("0.3.7")));
        assert!(!accepts("0.3", v("0.4.0")));
        assert!(accepts("1", v("1.5.0")));
        assert_eq!(v("v2.1").to_string(), "2.1.0");
        assert!(Version::parse("beta").is_none());
    }

    #[test]
    fn toml_entries_are_read() {
        let text = "[project] # the project\nname = \"carnet\"\n\n[dependencies]\ngeo = { git = \"https://x/y\", version = \"1.2\" }\n";
        let entries = parse_toml(text, "lion.toml").unwrap();
        assert_eq!(
            entries[0],
            ("project".to_string(), "name".to_string(), TomlValue::Text("carnet".to_string()))
        );
        assert_eq!(
            entries[1].2,
            TomlValue::Table(vec![
                ("git".to_string(), "https://x/y".to_string()),
                ("version".to_string(), "1.2".to_string())
            ])
        );
        assert!(parse_toml("name = carnet", "lion.toml").is_err());
    }

    #[test]
    fn a_dependency_joins_its_section() {
        let text = "[project]\nname = \"a\"\n\n[dependencies]\nb = { path = \"../b\" }\n\n[other]\n";
        assert_eq!(
            with_dependency(text, "c = { path = \"../c\" }"),
            "[project]\nname = \"a\"\n\n[dependencies]\nb = { path = \"../b\" }\nc = { path = \"../c\" }\n\n[other]\n"
        );
        assert_eq!(with_dependency("[project]\n", "c = 1"), "[project]\n\n[dependencies]\nc = 1\n");
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/c/d")), PathBuf::from("../c/d"));
    }
}
