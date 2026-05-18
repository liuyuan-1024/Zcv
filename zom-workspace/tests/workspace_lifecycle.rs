use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zom_engine::ByteOffset;
use zom_workspace::{BufferOrigin, Workspace, WorkspaceError};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "zom-workspace-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn open_text_should_track_active_buffer_and_close_fallback() {
    let mut workspace = Workspace::new();

    assert_eq!(workspace.active_buffer_id(), None);

    let first = workspace.open_text(None, "first").unwrap();
    let second = workspace.open_text(None, "second").unwrap();
    let third = workspace.open_text(None, "third").unwrap();

    assert_eq!(workspace.active_buffer_id(), Some(third));
    workspace.set_active_buffer(first).unwrap();
    assert_eq!(
        workspace.active_buffer().unwrap().buffer().text().as_ref(),
        "first"
    );

    workspace.close_buffer(second).unwrap();
    assert_eq!(workspace.active_buffer_id(), Some(first));

    workspace.close_buffer(first).unwrap();
    assert_eq!(workspace.active_buffer_id(), Some(third));

    workspace.close_buffer(third).unwrap();
    assert_eq!(workspace.active_buffer_id(), None);
    assert_eq!(workspace.buffers().count(), 0);
}

#[test]
fn open_file_and_save_file_should_preserve_path_and_clean_state() {
    let dir = TempDir::new("save-file");
    let path = dir.path().join("note.txt");
    fs::write(&path, "hello").unwrap();

    let mut workspace = Workspace::new();
    let id = workspace.open_file(path.clone()).unwrap();

    assert_eq!(workspace.active_buffer_id(), Some(id));
    assert_eq!(workspace.buffer_path(id).unwrap(), Some(path.as_path()));
    assert!(!workspace.is_buffer_dirty(id).unwrap());
    assert!(!workspace.is_buffer_read_only(id).unwrap());

    workspace
        .buffer_mut(id)
        .unwrap()
        .buffer_mut()
        .insert(ByteOffset::new(5), " 世界")
        .unwrap();
    assert!(workspace.is_buffer_dirty(id).unwrap());

    workspace.save_file(id).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "hello 世界");
    assert!(!workspace.is_buffer_dirty(id).unwrap());
}

#[test]
fn save_as_should_bind_scratch_buffer_to_new_path() {
    let dir = TempDir::new("save-as");
    let path = dir.path().join("scratch.txt");

    let mut workspace = Workspace::new();
    let id = workspace.open_text(None, "draft").unwrap();

    let err = workspace.save_file(id).unwrap_err();
    assert!(matches!(err, WorkspaceError::BufferHasNoPath(found) if found == id));
    assert_eq!(workspace.buffer_path(id).unwrap(), None);

    workspace
        .buffer_mut(id)
        .unwrap()
        .buffer_mut()
        .insert(ByteOffset::new(5), "\n")
        .unwrap();

    workspace.save_as(id, path.clone()).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "draft\n");
    assert_eq!(workspace.buffer_path(id).unwrap(), Some(path.as_path()));
    assert_eq!(
        workspace.buffer(id).unwrap().origin(),
        &BufferOrigin::File(path)
    );
    assert!(!workspace.is_buffer_dirty(id).unwrap());
}

#[test]
fn status_queries_should_report_missing_buffer() {
    let mut workspace = Workspace::new();
    let id = workspace.open_text(None, "alive").unwrap();
    workspace.close_buffer(id).unwrap();

    assert!(matches!(
        workspace.is_buffer_dirty(id),
        Err(WorkspaceError::BufferNotFound(found)) if found == id
    ));
    assert!(matches!(
        workspace.set_active_buffer(id),
        Err(WorkspaceError::BufferNotFound(found)) if found == id
    ));
    assert!(matches!(
        workspace.close_buffer(id),
        Err(WorkspaceError::BufferNotFound(found)) if found == id
    ));
}
