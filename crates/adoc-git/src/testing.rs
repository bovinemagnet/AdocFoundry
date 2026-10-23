//! Temporary repositories for tests, built with `gix`, never a developer's checkout.

use std::{
    fs,
    path::{Path, PathBuf},
};

use gix::object::tree::EntryKind;

/// A repository in a test's temporary directory, with `HEAD` on `main` and an identity set.
pub struct TestRepo {
    repo: gix::Repository,
    dir: PathBuf,
}

impl TestRepo {
    /// Initialises an empty repository in `dir`.
    ///
    /// # Panics
    /// When the repository cannot be created, as any test set-up failure should.
    #[must_use]
    pub fn init(dir: &Path) -> Self {
        let mut repo = gix::init(dir).expect("init a test repository");
        // Whatever `init.defaultBranch` says, the first branch is `main`.
        fs::write(repo.git_dir().join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        {
            let mut config = repo.config_snapshot_mut();
            config
                .set_raw_value("user.name", "Test Writer")
                .expect("set user.name");
            config
                .set_raw_value("user.email", "writer@example.com")
                .expect("set user.email");
        }
        Self {
            repo,
            dir: dir.to_path_buf(),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Writes `text` to `path` (relative to the working tree), creating directories.
    ///
    /// # Panics
    /// When the file cannot be written.
    pub fn write(&self, path: &str, text: &str) {
        let file = self.dir.join(path);
        fs::create_dir_all(file.parent().expect("a parent")).expect("create directories");
        fs::write(file, text).expect("write a file");
    }

    /// Deletes `path` from the working tree.
    ///
    /// # Panics
    /// When the file cannot be removed.
    pub fn remove(&self, path: &str) {
        fs::remove_file(self.dir.join(path)).expect("remove a file");
    }

    /// Commits every file of the working tree, as `git add -A && git commit` would, and makes
    /// the index match. Returns the commit's id.
    ///
    /// # Panics
    /// When the commit cannot be written.
    pub fn commit_all(&self, message: &str) -> String {
        let mut files = Vec::new();
        collect_files(&self.dir, &self.dir, &mut files);
        let mut editor = self
            .repo
            .edit_tree(gix::ObjectId::empty_tree(self.repo.object_hash()))
            .expect("a tree editor");
        for (path, bytes) in files {
            let blob = self.repo.write_blob(bytes).expect("write a blob").detach();
            editor
                .upsert(path.as_str(), EntryKind::Blob, blob)
                .expect("add to the tree");
        }
        let tree = editor.write().expect("write the tree").detach();
        let parents: Vec<gix::ObjectId> = self
            .repo
            .head_id()
            .ok()
            .map(gix::Id::detach)
            .into_iter()
            .collect();
        let id = self
            .repo
            .commit("HEAD", message, tree, parents)
            .expect("commit")
            .detach();
        self.write_index(tree);
        id.to_string()
    }

    /// Stages the working tree's `path` (added, changed or, when missing, deleted) on top of
    /// `HEAD`, as `git add <path>` would in a repository with nothing else staged.
    ///
    /// # Panics
    /// When the index cannot be written.
    pub fn stage(&self, path: &str) {
        let head_tree = self
            .repo
            .head_tree_id()
            .map(gix::Id::detach)
            .unwrap_or_else(|_| gix::ObjectId::empty_tree(self.repo.object_hash()));
        let mut editor = self.repo.edit_tree(head_tree).expect("a tree editor");
        match fs::read(self.dir.join(path)) {
            Ok(bytes) => {
                let blob = self.repo.write_blob(bytes).expect("write a blob").detach();
                editor
                    .upsert(path, EntryKind::Blob, blob)
                    .expect("add to the tree");
            }
            Err(_) => {
                editor.remove(path).expect("remove from the tree");
            }
        }
        let tree = editor.write().expect("write the tree").detach();
        self.write_index(tree);
    }

    /// Points `HEAD` at the current commit, detached from any branch. Returns its id.
    ///
    /// # Panics
    /// When there is no commit.
    pub fn detach(&self) -> String {
        let id = self
            .repo
            .head_id()
            .expect("a commit to detach at")
            .to_string();
        fs::write(self.repo.git_dir().join("HEAD"), format!("{id}\n")).expect("write HEAD");
        id
    }

    /// Creates branch `name` at the current commit when it does not exist, and points `HEAD` at
    /// it. The working tree is left as it is.
    ///
    /// # Panics
    /// When there is no commit or the branch cannot be written.
    pub fn switch_branch(&self, name: &str) {
        let branch = self.repo.git_dir().join("refs/heads").join(name);
        if !branch.exists() {
            let id = self
                .repo
                .head_id()
                .expect("a commit to branch from")
                .to_string();
            fs::create_dir_all(branch.parent().expect("a parent")).expect("create directories");
            fs::write(&branch, format!("{id}\n")).expect("write the branch");
        }
        fs::write(
            self.repo.git_dir().join("HEAD"),
            format!("ref: refs/heads/{name}\n"),
        )
        .expect("write HEAD");
    }

    /// Checks out branch `name`, as `git checkout` would with nothing uncommitted: the working
    /// tree and index become the branch's tree, and `HEAD` points at it.
    ///
    /// # Panics
    /// When the branch does not exist or the files cannot be written.
    pub fn checkout(&self, name: &str) {
        let mut reference = self
            .repo
            .find_reference(format!("refs/heads/{name}").as_str())
            .expect("the branch to check out");
        let tree = reference
            .peel_to_commit()
            .expect("a commit")
            .tree()
            .expect("its tree");
        let mut current = Vec::new();
        collect_files(&self.dir, &self.dir, &mut current);
        for (path, _) in current {
            fs::remove_file(self.dir.join(&path)).expect("remove a file");
        }
        let mut recorder = gix::traverse::tree::Recorder::default();
        tree.traverse()
            .breadthfirst(&mut recorder)
            .expect("walk the tree");
        for entry in recorder.records {
            if !entry.mode.is_blob() {
                continue;
            }
            let object = self.repo.find_object(entry.oid).expect("a blob");
            let path = entry.filepath.to_string();
            let file = self.dir.join(&path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("create directories");
            fs::write(file, &object.data).expect("write a file");
        }
        self.write_index(tree.id);
        fs::write(
            self.repo.git_dir().join("HEAD"),
            format!("ref: refs/heads/{name}\n"),
        )
        .expect("write HEAD");
    }

    /// Adds remote `name` with fetch URL `url` to the repository's configuration.
    ///
    /// # Panics
    /// When the configuration cannot be written.
    pub fn add_remote(&self, name: &str, url: &str) {
        let config = self.repo.git_dir().join("config");
        let mut text = fs::read_to_string(&config).unwrap_or_default();
        text.push_str(&format!(
            "[remote \"{name}\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/{name}/*\n"
        ));
        fs::write(config, text).expect("write the configuration");
    }

    /// Records the current branch as pushed to `remote`, as a push and fetch would: its
    /// remote-tracking branch points at the current commit.
    ///
    /// # Panics
    /// When `HEAD` is not on a branch with a commit.
    pub fn track_remote(&self, remote: &str) {
        let head = fs::read_to_string(self.repo.git_dir().join("HEAD")).expect("read HEAD");
        let branch = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .expect("HEAD on a branch");
        let id = self.repo.head_id().expect("a commit").to_string();
        let tracking = self
            .repo
            .git_dir()
            .join("refs/remotes")
            .join(remote)
            .join(branch);
        fs::create_dir_all(tracking.parent().expect("a parent")).expect("create directories");
        fs::write(tracking, format!("{id}\n")).expect("write the remote-tracking branch");
    }

    fn write_index(&self, tree: gix::ObjectId) {
        let mut index = self.repo.index_from_tree(&tree).expect("an index");
        index.write(Default::default()).expect("write the index");
    }
}

/// Every file under `dir` but `.git`, as `/`-separated paths relative to `root`.
fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read a directory")
        .map(|entry| entry.expect("a directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, fs::read(&path).expect("read a file")));
        }
    }
}
