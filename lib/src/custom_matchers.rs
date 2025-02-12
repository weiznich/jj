use crate::matchers::{GlobsMatcher, Matcher, PathGlobPattern, Visit};
use crate::repo_path::{RepoPath, RepoPathTree};
use crate::tree::Tree;

#[derive(Clone, Debug)]
struct IgnoreWrapper(ignore::gitignore::Gitignore);

impl Default for IgnoreWrapper {
    fn default() -> Self {
        Self(ignore::gitignore::Gitignore::empty())
    }
}

/// Matches file paths with glob patterns.
///
/// Patterns are provided as `(dir, pattern)` pairs, where `dir` should be the
/// longest directory path that contains no glob meta characters, and `pattern`
/// will be evaluated relative to `dir`.
#[derive(Clone, Debug)]
pub struct GitAttributesMatcher {
    tree: RepoPathTree<IgnoreWrapper>,
}

#[derive(Debug, thiserror::Error)]
pub enum GitAttributesMatcherError {
    #[error("failed to parse .gitattributes file")]
    GitIgnoreError(#[from] ignore::Error),
    #[error("failed to read .gitattributes file")]
    ReadError(#[from] std::io::Error),
    #[error("invalid pattern in .gitattributes: {0}")]
    InvalidPattern(String),
}

fn parse_gitattributes(
    f: std::fs::File,
    builder: &mut ignore::gitignore::GitignoreBuilder,
) -> Result<(), GitAttributesMatcherError> {
    use std::io::BufRead;

    let reader = std::io::BufReader::new(f);

    for line in reader.lines() {
        let line = line?;
        let mut parts = line.split_whitespace();
        let pattern = match parts.next() {
            Some(l) => l,
            None => continue,
        };

        // lines starting with # are ignored
        if pattern.starts_with('#') {
            continue;
        }

        // Simple heuristic to detect LFS files
        let is_lfs = parts.any(|p| p == "filter=lfs");
        if !is_lfs {
            // We only want to ignore LFS files
            continue;
        }

        // See https://git-scm.com/docs/gitattributes, these are the two
        // differences from a standard .gitignore file as far as patterns go.
        if pattern.starts_with("!") {
            return Err(GitAttributesMatcherError::InvalidPattern(
                "negative patterns are not allowed in .gitattributes files".to_string(),
            ));
        }

        // git somehow accepts this but ignores it
        if pattern.ends_with("/") {
            continue;
        }
        // hack to workaround https://github.com/BurntSushi/ripgrep/issues/2962
        let pattern = pattern.replace("[[:space:]]", " ");

        builder.add_line(None, &pattern)?;
    }

    Ok(())
}

impl GitAttributesMatcher {
    /// Create a new matcher for the contents of a .gitattributes file
    pub fn new(
        tree: &Tree,
        repo_root: &std::path::Path,
    ) -> Result<Self, GitAttributesMatcherError> {
        let git_attributes_glob = globset::Glob::new("**/.gitattributes")
            .expect("This is a statically known valid pattern");
        let mut git_attributes_matcher = GlobsMatcher::builder();
        let path_glob_pattern = PathGlobPattern(git_attributes_glob);
        git_attributes_matcher.add(tree.dir(), &path_glob_pattern);
        let git_attributes_matcher = git_attributes_matcher.build();
        let entries = tree.entries_matching(&git_attributes_matcher);
        let mut tree = RepoPathTree::default();
        for (path, _) in entries {
            let gitattributes_path = path
                .to_fs_path(repo_root)
                .expect("The file matcher only returns valid paths");

            if gitattributes_path.exists() {
                let relative_path = path
                    .parent()
                    .expect("There is always a parent directory for files")
                    .to_owned();
                let location = repo_root.join(relative_path.to_internal_dir_string());
                let mut builder = ignore::gitignore::GitignoreBuilder::new(location);
                let file = std::fs::File::open(&gitattributes_path)?;

                parse_gitattributes(file, &mut builder)?;
                *tree.add(&relative_path).value_mut() = IgnoreWrapper(builder.build()?);
            }
        }
        Ok(GitAttributesMatcher { tree })
    }
}

impl Matcher for GitAttributesMatcher {
    fn matches(&self, file: &RepoPath) -> bool {
        self.tree
            .walk_to(file)
            .take_while(|(_, tail_path)| !tail_path.is_root())
            .any(|(sub, tail_path)| {
                let Ok(path) = tail_path.to_fs_path(sub.value().0.path()) else {
                    return false;
                };
                matches!(sub.value().0.matched(path, false), ignore::Match::Ignore(_))
            })
    }

    fn visit(&self, _dir: &RepoPath) -> Visit {
        Visit::Nothing
    }
}
