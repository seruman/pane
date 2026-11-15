//! Async filesystem access for trusted scripts. Paths are not sandboxed: absolute
//! paths are accepted, and relative paths are resolved against the script root.
//! Tokio filesystem operations use blocking work internally, so cancellation of
//! the surrounding job is best effort and does not roll back filesystem changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use tokio::fs;
use tokio_util::sync::CancellationToken;

use super::streams::{Reader, Writer};
use super::{Error, Reply, Resource, Result, Scope};

const MAX_DIRECTORY_ENTRIES: usize = 10_000;

fn resolve(scope: &Scope, path: &Path) -> PathBuf {
    scope.root.join(path)
}

fn file_type(kind: std::fs::FileType) -> &'static str {
    if kind.is_symlink() {
        "symlink"
    } else if kind.is_file() {
        "file"
    } else if kind.is_dir() {
        "directory"
    } else {
        "other"
    }
}

pub async fn open(
    scope: Arc<Scope>,
    path: PathBuf,
    mode: String,
    cancel: CancellationToken,
) -> Result<Reply> {
    let mut options = fs::OpenOptions::new();
    match mode.as_str() {
        "read" => {
            options.read(true);
        }
        "write" => {
            options.write(true).create(true).truncate(true);
        }
        "append" => {
            options.append(true).create(true);
        }
        _ => {
            return Err(Error::new(
                "invalid_argument",
                "mode must be read, write, or append",
            ));
        }
    }

    // Reserve before opening: a resource-limit failure must not truncate a file.
    let permit = scope.reserve()?;
    let file = options.open(resolve(&scope, &path)).await?;
    let resource = if mode == "read" {
        Resource::Reader(Reader::new(file, cancel, Some(permit)))
    } else {
        Resource::Writer(Writer::new(file, cancel, Some(permit)))
    };
    Ok(Reply::Handle(resource))
}

pub async fn stat(scope: Arc<Scope>, path: PathBuf) -> Result<Reply> {
    // Do not follow links, including dangling links.
    let metadata = fs::symlink_metadata(resolve(&scope, &path)).await?;
    let mut fields = vec![
        ("type", Reply::Text(file_type(metadata.file_type()).into())),
        (
            "size",
            Reply::Number(metadata.len().min(i64::MAX as u64) as i64),
        ),
    ];
    if let Ok(modified) = metadata.modified() {
        let milliseconds = match modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
            Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
        };
        fields.push(("modifiedMs", Reply::Number(milliseconds)));
    }
    Ok(Reply::map(fields))
}

pub async fn read_dir(scope: Arc<Scope>, path: PathBuf) -> Result<Reply> {
    let mut directory = fs::read_dir(resolve(&scope, &path)).await?;
    let mut entries = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err(Error::new("limit", "directory entry limit exceeded"));
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = file_type(entry.file_type().await?);
        entries.push((name, kind));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(Reply::List(
        entries
            .into_iter()
            .map(|(name, kind)| {
                Reply::map([
                    ("name", Reply::Text(name)),
                    ("type", Reply::Text(kind.into())),
                ])
            })
            .collect(),
    ))
}

pub async fn mkdir(scope: Arc<Scope>, path: PathBuf, recursive: bool) -> Result<Reply> {
    let path = resolve(&scope, &path);
    if recursive {
        fs::create_dir_all(path).await?;
    } else {
        fs::create_dir(path).await?;
    }
    Ok(Reply::Bool(true))
}

pub async fn rename(scope: Arc<Scope>, from: PathBuf, to: PathBuf) -> Result<Reply> {
    fs::rename(resolve(&scope, &from), resolve(&scope, &to)).await?;
    Ok(Reply::Bool(true))
}

pub async fn remove(scope: Arc<Scope>, path: PathBuf) -> Result<Reply> {
    let path = resolve(&scope, &path);
    if fs::symlink_metadata(&path).await?.is_dir() {
        fs::remove_dir(path).await?;
    } else {
        // symlink_metadata ensures links to directories are unlinked, not followed.
        fs::remove_file(path).await?;
    }
    Ok(Reply::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Arc<Scope>) {
        let directory = tempfile::tempdir().expect("temporary root");
        let scope = Scope::new(directory.path().to_path_buf());
        (directory, scope)
    }

    fn text<'a>(reply: &'a Reply, key: &str) -> &'a str {
        let Reply::Map(fields) = reply else {
            panic!("expected map");
        };
        let Some(Reply::Text(value)) = fields.get(key) else {
            panic!("expected text field {key}");
        };
        value
    }

    fn number(reply: &Reply, key: &str) -> i64 {
        let Reply::Map(fields) = reply else {
            panic!("expected map");
        };
        let Some(Reply::Number(value)) = fields.get(key) else {
            panic!("expected numeric field {key}");
        };
        *value
    }

    #[tokio::test]
    async fn paths_and_modes() {
        let (root, scope) = setup();
        let outside = tempfile::tempdir().expect("outside root");
        let absolute = outside.path().join("absolute.bin");
        fs::write(&absolute, [0, 255, 128])
            .await
            .expect("seed binary");
        let reader = open(scope.clone(), absolute, "read".into(), scope.cancel.clone())
            .await
            .expect("absolute read");
        assert!(matches!(reader, Reply::Handle(Resource::Reader(_))));
        assert!(
            open(
                scope.clone(),
                "missing".into(),
                "read".into(),
                scope.cancel.clone()
            )
            .await
            .is_err()
        );
        assert!(!root.path().join("missing").exists());

        for mode in ["write", "append"] {
            let writer = open(
                scope.clone(),
                mode.into(),
                mode.into(),
                scope.cancel.clone(),
            )
            .await
            .expect("create writer");
            assert!(matches!(writer, Reply::Handle(Resource::Writer(_))));
            assert!(root.path().join(mode).is_file());
        }
        fs::write(root.path().join("write"), b"old")
            .await
            .expect("seed write");
        drop(
            open(
                scope.clone(),
                "write".into(),
                "write".into(),
                scope.cancel.clone(),
            )
            .await
            .expect("truncate"),
        );
        assert!(
            fs::read(root.path().join("write"))
                .await
                .expect("read truncated")
                .is_empty()
        );
        fs::write(root.path().join("append"), [0, 255])
            .await
            .expect("seed append");
        drop(
            open(
                scope.clone(),
                "append".into(),
                "append".into(),
                scope.cancel.clone(),
            )
            .await
            .expect("append preserves bytes"),
        );
        assert_eq!(
            fs::read(root.path().join("append"))
                .await
                .expect("read binary"),
            [0, 255]
        );

        assert!(
            open(
                scope.clone(),
                "append".into(),
                "invalid".into(),
                scope.cancel.clone()
            )
            .await
            .is_err()
        );
        assert_eq!(
            fs::read(root.path().join("append"))
                .await
                .expect("unchanged"),
            [0, 255]
        );
    }

    #[tokio::test]
    async fn binary_stream_round_trip_and_append() {
        let (_root, scope) = setup();
        for (mode, bytes) in [("write", &b"\0\xff\x80"[..]), ("append", &b"tail\0"[..])] {
            let Reply::Handle(Resource::Writer(ref writer)) = open(
                scope.clone(),
                "binary".into(),
                mode.into(),
                scope.cancel.child_token(),
            )
            .await
            .expect("open writer") else {
                panic!("expected writer");
            };
            writer.write(bytes).await.expect("write binary");
            writer.close().await.expect("flush and close");
        }
        let Reply::Handle(Resource::Reader(ref reader)) = open(
            scope.clone(),
            "binary".into(),
            "read".into(),
            scope.cancel.child_token(),
        )
        .await
        .expect("open reader") else {
            panic!("expected reader");
        };
        assert_eq!(
            reader.read_all(32).await.expect("read binary"),
            b"\0\xff\x80tail\0"
        );
        assert_eq!(reader.read(1).await.expect("eof"), None);
    }

    #[tokio::test]
    async fn metadata_and_sorted_directory() {
        let (root, scope) = setup();
        fs::write(root.path().join("z.bin"), [0, 255, 128])
            .await
            .expect("seed binary");
        mkdir(scope.clone(), "a".into(), false)
            .await
            .expect("mkdir");
        let metadata = stat(scope.clone(), "z.bin".into())
            .await
            .expect("stat file");
        assert_eq!(text(&metadata, "type"), "file");
        assert_eq!(number(&metadata, "size"), 3);
        assert!(number(&metadata, "modifiedMs") > 0);
        assert_eq!(
            text(
                &stat(scope.clone(), "a".into())
                    .await
                    .expect("stat directory"),
                "type"
            ),
            "directory"
        );
        let Reply::List(entries) = read_dir(scope, ".".into()).await.expect("read directory")
        else {
            panic!("expected list");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(text(&entries[0], "name"), "a");
        assert_eq!(text(&entries[0], "type"), "directory");
        assert_eq!(text(&entries[1], "name"), "z.bin");
        assert_eq!(text(&entries[1], "type"), "file");
    }

    #[tokio::test]
    async fn mkdir_rename_and_nonrecursive_remove() {
        let (root, scope) = setup();
        assert!(mkdir(scope.clone(), "a/b".into(), false).await.is_err());
        assert!(!root.path().join("a").exists());
        mkdir(scope.clone(), "a/b".into(), true)
            .await
            .expect("recursive mkdir");
        assert!(remove(scope.clone(), "a".into()).await.is_err());
        assert!(root.path().join("a/b").is_dir());
        fs::write(root.path().join("a/b/binary"), [0, 255])
            .await
            .expect("seed binary");
        rename(scope.clone(), "a/b/binary".into(), "a/renamed".into())
            .await
            .expect("rename");
        assert!(!root.path().join("a/b/binary").exists());
        assert_eq!(
            fs::read(root.path().join("a/renamed"))
                .await
                .expect("renamed bytes"),
            [0, 255]
        );
        remove(scope.clone(), "a/renamed".into())
            .await
            .expect("unlink file");
        remove(scope.clone(), "a/b".into())
            .await
            .expect("empty directory");
        remove(scope.clone(), "a".into())
            .await
            .expect("empty parent");
        assert!(!root.path().join("a").exists());
        assert!(remove(scope, "missing".into()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_are_reported_and_unlinked_without_following() {
        let (root, scope) = setup();
        mkdir(scope.clone(), "target".into(), false)
            .await
            .expect("target directory");
        fs::write(root.path().join("target/keep"), b"keep")
            .await
            .expect("target data");
        std::os::unix::fs::symlink("target", root.path().join("link")).expect("directory link");
        std::os::unix::fs::symlink("absent", root.path().join("dangling")).expect("dangling link");
        for name in ["link", "dangling"] {
            assert_eq!(
                text(
                    &stat(scope.clone(), name.into()).await.expect("stat link"),
                    "type"
                ),
                "symlink"
            );
        }
        let Reply::List(entries) = read_dir(scope.clone(), ".".into())
            .await
            .expect("list links")
        else {
            panic!("expected list");
        };
        assert_eq!(text(&entries[0], "type"), "symlink");
        assert_eq!(text(&entries[1], "type"), "symlink");
        for name in ["link", "dangling"] {
            remove(scope.clone(), name.into())
                .await
                .expect("unlink link");
            assert!(fs::symlink_metadata(root.path().join(name)).await.is_err());
        }
        assert_eq!(
            fs::read(root.path().join("target/keep"))
                .await
                .expect("target retained"),
            b"keep"
        );
    }

    #[tokio::test]
    async fn directory_entry_limit() {
        let (root, scope) = setup();
        for index in 0..MAX_DIRECTORY_ENTRIES {
            std::fs::File::create(root.path().join(index.to_string())).expect("seed entry");
        }
        let Reply::List(entries) = read_dir(scope.clone(), ".".into())
            .await
            .expect("exact limit allowed")
        else {
            panic!("expected list");
        };
        assert_eq!(entries.len(), MAX_DIRECTORY_ENTRIES);
        std::fs::File::create(root.path().join("overflow")).expect("overflow entry");
        assert!(read_dir(scope, ".".into()).await.is_err());
    }
}
