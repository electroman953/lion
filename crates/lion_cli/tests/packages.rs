//! Tests of projects and packages (spec §20.4, §25, step 8, C101): `lion new`, `lion
//! add`, `lion update`, `lion remove`, and `lion run` in a project, with git repositories
//! made in a temporary folder and a cache of their own. Skipped without git.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Place {
    folder: PathBuf,
    cache: PathBuf,
}

impl Place {
    fn new(name: &str) -> Place {
        let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let cache = folder.join("cache");
        Place { folder, cache }
    }

    /// `lion` with these arguments, in `folder`.
    fn lion(&self, folder: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lion"))
            .args(args)
            .current_dir(self.folder.join(folder))
            .env("LION_CACHE", &self.cache)
            .env("LION_UI", "headless")
            .output()
            .expect("the lion binary runs")
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.folder.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(&self, folder: &str, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.email=lion@test", "-c", "user.name=lion", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(self.folder.join(folder))
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    /// A package in a git repository, with a commit and a tag for each version, where
    /// `aire` multiplies by the major number of the version.
    fn repository(&self, name: &str, versions: &[&str]) {
        fs::create_dir_all(self.folder.join(name)).unwrap();
        self.git(name, &["init", "-q"]);
        self.repository_more(name, versions);
    }

    /// More versions of a repository made by `repository`.
    fn repository_more(&self, name: &str, versions: &[&str]) {
        for version in versions {
            let major = version.split('.').next().unwrap();
            self.write(
                &format!("{name}/lion.toml"),
                &format!("[project]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"0.1\"\n"),
            );
            self.write(
                &format!("{name}/{name}.lion"),
                &format!(
                    "use cercle\nfun aire(x in Int) in Int = x * {major}\nfun version() in Text = \"{version}\"\n\
                     fun rond(r in Int) in Int = cercle.surface(r)\n"
                ),
            );
            self.write(&format!("{name}/cercle.lion"), "fun surface(r in Int) in Int = 3 * r * r\n");
            self.git(name, &["add", "-A"]);
            self.git(name, &["commit", "-q", "-m", version]);
            self.git(name, &["tag", &format!("v{version}")]);
        }
    }
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn git_exists() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|output| output.status.success())
}

#[test]
fn a_package_is_installed_with_one_command() {
    if !git_exists() {
        eprintln!("git is not installed: the test of packages is skipped");
        return;
    }
    let place = Place::new("packages-install");
    place.repository("geometrie", &["1.0.0", "1.1.0"]);
    assert!(place.lion(".", &["new", "carnet"]).status.success());
    let added = place.lion("carnet", &["add", "--git", "../geometrie"]);
    assert!(added.status.success(), "{}", text(&added));
    assert!(text(&added).contains("added `geometrie` 1.1.0"), "{}", text(&added));
    let manifest = fs::read_to_string(place.folder.join("carnet/lion.toml")).unwrap();
    assert!(manifest.contains("version = \"1.1\""), "{manifest}");
    assert!(place.folder.join("carnet/lion.lock").is_file());
    place.write("carnet/main.lion", "use geometrie\nuse geometrie.cercle\nshow(geometrie.version())\nshow(geometrie.rond(2))\nshow(cercle.surface(1))\n");
    let run = place.lion("carnet", &["run"]);
    assert_eq!(text(&run), "1.1.0\n12\n3\n");

    // A new version within those accepted, then one that may break the program.
    place.repository_more("geometrie", &["1.2.0", "2.0.0"]);
    assert_eq!(text(&place.lion("carnet", &["run"])), "1.1.0\n12\n3\n", "the lock keeps the version");
    assert!(place.lion("carnet", &["update"]).status.success());
    assert_eq!(text(&place.lion("carnet", &["run"])), "1.2.0\n12\n3\n");

    // Without the repository, the cache and the lock are enough.
    fs::rename(place.folder.join("geometrie"), place.folder.join("away")).unwrap();
    assert_eq!(text(&place.lion("carnet", &["run"])), "1.2.0\n12\n3\n");
    fs::rename(place.folder.join("away"), place.folder.join("geometrie")).unwrap();

    // A package of a folder, which uses a package of git itself.
    place.write(
        "outils/lion.toml",
        "[project]\nname = \"outils\"\nversion = \"0.1.0\"\nedition = \"0.1\"\n\n[dependencies]\ngeometrie = { git = \"../geometrie\", version = \"1.2\" }\n",
    );
    let absolute = place.folder.join("geometrie").display().to_string();
    let manifest =
        fs::read_to_string(place.folder.join("outils/lion.toml")).unwrap().replace("../geometrie", &absolute);
    place.write("outils/lion.toml", &manifest);
    place.write("outils/outils.lion", "use geometrie\nfun double(x in Int) in Int = 2 * geometrie.aire(x)\n");
    let added = place.lion("carnet", &["add", "../outils"]);
    assert!(added.status.success(), "{}", text(&added));
    place.write("carnet/main.lion", "use outils\nshow(outils.double(5))\n");
    assert_eq!(text(&place.lion("carnet", &["run"])), "10\n");

    // Two versions of one package cannot meet.
    let manifest =
        fs::read_to_string(place.folder.join("outils/lion.toml")).unwrap().replace("\"1.2\"", "\"2.0\"");
    place.write("outils/lion.toml", &manifest);
    let conflict = place.lion("carnet", &["run"]);
    assert!(!conflict.status.success());
    assert!(text(&conflict).contains("two versions of `geometrie` are needed"), "{}", text(&conflict));

    let removed = place.lion("carnet", &["remove", "outils"]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert!(!fs::read_to_string(place.folder.join("carnet/lion.toml")).unwrap().contains("outils"));
}

#[test]
fn a_project_declares_a_known_edition() {
    let place = Place::new("packages-edition");
    assert!(place.lion(".", &["new", "vieux"]).status.success());
    let manifest =
        fs::read_to_string(place.folder.join("vieux/lion.toml")).unwrap().replace("\"0.1\"", "\"0.9\"");
    place.write("vieux/lion.toml", &manifest);
    let run = place.lion("vieux", &["run"]);
    assert!(!run.status.success());
    assert!(text(&run).contains("the project declares the edition `0.9`"), "{}", text(&run));
    let refused = place.lion(".", &["new", "Vieux"]);
    assert!(!refused.status.success());
    let outside = place.lion(".", &["add", "../nothing"]);
    assert!(text(&outside).contains("there is no project here"), "{}", text(&outside));
}
