//! AppImages packed from an AppDir: the AppImage runtime the AppDir carries at
//! [`RUNTIME`], followed by a zstd squashfs of the AppDir. Release packaging
//! and patch installs both pack with [`pack`], so a patched install has the
//! same files, modes and links as the released image (its compressed bytes
//! may differ, which nothing verifies: the files are what the patch pinned).
use super::super::patch::{self, Links, Node};
use super::*;
use backhand::{
    FilesystemCompressor, FilesystemWriter, NodeHeader,
    compression::{CompressionOptions, Compressor, Zstd},
};
use std::{collections::BTreeMap, io};

/// The AppImage runtime inside the AppDir. Never executed from there.
pub const RUNTIME: &str = ".captures-appimage-runtime";
/// Release packaging: the smallest download.
const RELEASE_LEVEL: u32 = 19;
/// Patch installs: fast on the user's machine; size only costs local disk.
pub const INSTALL_LEVEL: u32 = 3;

/// Release packaging: the AppImage of an AppDir on disk, execute bits from
/// file modes.
pub fn build(appdir: &Path) -> Result<Vec<u8>, Error> {
    let cancel = CancelToken::default();
    let executables = patch::executables(appdir, Links::Keep, &cancel)?;
    let entries = patch::tree(appdir, &executables, Links::Keep, &cancel)?;
    pack(appdir, &entries, RELEASE_LEVEL)
}

/// Pack `entries` from `appdir` into an AppImage. Every file is checked against
/// its listed size and SHA-256 before it is packed; nothing else under
/// `appdir` is read.
pub(super) fn pack(
    appdir: &Path,
    entries: &BTreeMap<String, Node>,
    level: u32,
) -> Result<Vec<u8>, Error> {
    let cancel = CancelToken::default();
    let mut runtime = None;
    let mut writer = FilesystemWriter::default();
    writer.set_time(0);
    writer.set_root_mode(0o755);
    writer.set_root_uid(0);
    writer.set_root_gid(0);
    // squashfs's largest block: the smallest image; mounts read whole blocks.
    writer.set_block_size(1 << 20);
    writer.set_compressor(
        FilesystemCompressor::new(
            Compressor::Zstd,
            Some(CompressionOptions::Zstd(Zstd {
                compression_level: level,
            })),
        )
        .map_err(|_| Error::Archive)?,
    );
    let header = |mode| NodeHeader::new(mode, 0, 0, 0);
    // Sorted paths put every folder before what it holds.
    for (path, node) in entries {
        check_cancel(&cancel)?;
        let pushed = match node {
            Node::Dir => writer.push_dir(path, header(0o755)),
            Node::Link { target } => writer.push_symlink(target, path, header(0o777)),
            Node::File {
                size,
                sha256,
                executable,
            } => {
                let file = path
                    .split('/')
                    .fold(appdir.to_owned(), |file, part| file.join(part));
                let metadata = fs::symlink_metadata(&file)?;
                if !metadata.is_file() || metadata.len() != *size {
                    return Err(Error::Package);
                }
                // Checked here and packed from the same path by the writer;
                // only this user's processes could swap it in between, and
                // they could replace the installed AppImage anyway.
                if patch::hash_file(&file, &cancel)? != *sha256 {
                    return Err(Error::Hash);
                }
                if path == RUNTIME {
                    runtime = Some(fs::read(&file)?);
                }
                let mode = if *executable { 0o755 } else { 0o644 };
                writer.push_file_from_path(file, path, header(mode))
            }
        };
        pushed.map_err(|_| Error::Archive)?;
    }
    let mut image = runtime.ok_or(Error::Package)?;
    let mut squashfs = io::Cursor::new(Vec::new());
    writer.write(&mut squashfs).map_err(|_| Error::Archive)?;
    image.extend_from_slice(squashfs.get_ref());
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backhand::{FilesystemReader, InnerNode};
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn packs_the_runtime_then_a_squashfs_with_the_listed_files_modes_and_links() {
        let root = tempfile::tempdir().unwrap();
        let appdir = root.path().join("Captures.AppDir");
        let runtime = b"\x7fELF fake AppImage runtime".to_vec();
        fs::create_dir_all(appdir.join("usr/bin")).unwrap();
        fs::create_dir_all(appdir.join("usr/lib")).unwrap();
        fs::write(appdir.join(RUNTIME), &runtime).unwrap();
        fs::write(appdir.join("usr/bin/captures"), b"program").unwrap();
        fs::set_permissions(
            appdir.join("usr/bin/captures"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(appdir.join("usr/lib/libgtk-4.so.1"), b"gtk").unwrap();
        symlink("libgtk-4.so.1", appdir.join("usr/lib/libgtk-4.so")).unwrap();
        symlink("usr/bin/captures", appdir.join("AppRun")).unwrap();

        let image = build(&appdir).unwrap();
        assert_eq!(&image[..runtime.len()], runtime);
        let reader = FilesystemReader::from_reader_with_offset(
            io::Cursor::new(image.clone()),
            runtime.len() as u64,
        )
        .unwrap();
        let mut found = BTreeMap::new();
        for node in reader.files() {
            let path = node.fullpath.to_string_lossy().into_owned();
            let value = match &node.inner {
                InnerNode::File(file) => {
                    let mut bytes = Vec::new();
                    reader.file(file).reader().read_to_end(&mut bytes).unwrap();
                    format!(
                        "file {:o} {}",
                        node.header.permissions,
                        String::from_utf8(bytes).unwrap()
                    )
                }
                InnerNode::Symlink(link) => format!("link {}", link.link.display()),
                InnerNode::Dir(_) => format!("dir {:o}", node.header.permissions),
                _ => "other".to_owned(),
            };
            found.insert(path, value);
        }
        let expected: BTreeMap<String, String> = [
            ("/", "dir 755"),
            (
                "/.captures-appimage-runtime",
                "file 644 \x7fELF fake AppImage runtime",
            ),
            ("/AppRun", "link usr/bin/captures"),
            ("/usr", "dir 755"),
            ("/usr/bin", "dir 755"),
            ("/usr/bin/captures", "file 755 program"),
            ("/usr/lib", "dir 755"),
            ("/usr/lib/libgtk-4.so", "link libgtk-4.so.1"),
            ("/usr/lib/libgtk-4.so.1", "file 644 gtk"),
        ]
        .into_iter()
        .map(|(path, value)| (path.to_owned(), value.to_owned()))
        .collect();
        assert_eq!(found, expected);
        // Packing again gives the same bytes: no timestamps or owners leak in.
        assert_eq!(build(&appdir).unwrap(), image);

        // A file that no longer matches what the patch pinned is never packed.
        let cancel = CancelToken::default();
        let executables = patch::executables(&appdir, Links::Keep, &cancel).unwrap();
        let entries = patch::tree(&appdir, &executables, Links::Keep, &cancel).unwrap();
        fs::write(appdir.join("usr/lib/libgtk-4.so.1"), b"GTK").unwrap();
        assert!(matches!(
            pack(&appdir, &entries, INSTALL_LEVEL),
            Err(Error::Hash)
        ));
        fs::remove_file(appdir.join(RUNTIME)).unwrap();
        let entries = patch::tree(&appdir, &executables, Links::Keep, &cancel).unwrap();
        assert!(matches!(
            pack(&appdir, &entries, INSTALL_LEVEL),
            Err(Error::Package)
        ));
    }
}
