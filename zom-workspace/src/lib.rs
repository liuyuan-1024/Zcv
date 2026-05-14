use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zom_engine::{Buffer, BufferConfig, EngineResult};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BufferId(u64);

impl BufferId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub struct Workspace {
    next_buffer_id: u64,
    buffers: BTreeMap<BufferId, WorkspaceBuffer>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            next_buffer_id: 1,
            buffers: BTreeMap::new(),
        }
    }

    pub fn open_text(
        &mut self,
        path: Option<PathBuf>,
        text: impl Into<String>,
    ) -> EngineResult<BufferId> {
        let id = self.allocate_buffer_id();
        let buffer = WorkspaceBuffer {
            path,
            buffer: Buffer::from_text(text.into(), BufferConfig::default())?,
        };

        self.buffers.insert(id, buffer);
        Ok(id)
    }

    pub fn buffer(&self, id: BufferId) -> Option<&WorkspaceBuffer> {
        self.buffers.get(&id)
    }

    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut WorkspaceBuffer> {
        self.buffers.get_mut(&id)
    }

    pub fn buffers(&self) -> impl Iterator<Item = (BufferId, &WorkspaceBuffer)> {
        self.buffers.iter().map(|(id, buffer)| (*id, buffer))
    }

    fn allocate_buffer_id(&mut self) -> BufferId {
        let id = BufferId(self.next_buffer_id);
        self.next_buffer_id += 1;
        id
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct WorkspaceBuffer {
    path: Option<PathBuf>,
    buffer: Buffer,
}

impl WorkspaceBuffer {
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }
}
