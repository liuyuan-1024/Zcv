//! Editor 的安全局部重命名会话。
//!
//! Tree-sitter 绑定解析由 `zcv-language` 提供；
//! 本模块拥有重命名会话、事务入口和输入框定位，让语法查询、编辑状态与界面绘制保持各自的职责边界。

use zcv_multi_buffer::MultiBufferOffset;

use std::ops::Range;
use std::sync::Arc;

use gpui::{AppContext, Context, Entity, Focusable, Pixels, Point, point, px};
use zcv_actions::{CancelLocalRename, ConfirmLocalRename, RenameLocal};
use zcv_language::LocalBinding;
use zcv_text::BufferVersion;

use super::{Editor, EditorEvent, EditorMode, edit_metadata};
use crate::element::EditorInputLayout;
use crate::selection::{Selection, SelectionSet, replace_selections};

/// 行内局部重命名输入会话。
///
/// 输入框是独立的单行 Editor；
/// 源文档仍由外层 Editor 的 MultiBuffer 持有，提交时重新通过当前语法快照解析绑定，避免把输入框文本变成第二份文档状态。
pub(super) struct LocalRenameState {
    pub(super) offset: MultiBufferOffset,
    pub(super) version: BufferVersion,
    pub(super) name: String,
    pub(super) input: Entity<Editor>,
    pub(super) range: Range<usize>,
    pub(super) ranges: Arc<[Range<usize>]>,
    pub(super) position: Point<Pixels>,
    pub(super) width: Pixels,
    pub(super) line_height: Pixels,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum LocalRenameError {
    #[error("当前文档不是可安全重命名的单文件文档")]
    UnsupportedDocument,
    #[error("新名称不能为空或不是有效标识符")]
    InvalidName,
    #[error("当前位置没有可确定的局部绑定")]
    BindingNotFound,
    #[error("文档在重命名期间发生了变化，请重新尝试")]
    StaleDocument,
    #[error("局部重命名事务失败：{0}")]
    Edit(String),
}

impl Editor {
    /// 在当前光标位置安全重命名局部绑定。
    ///
    /// 只有语法层明确解析出的定义和引用会参与编辑；组合文档、过期快照和未解析名称均拒绝操作。
    pub(crate) fn rename_local_at(
        &mut self,
        offset: MultiBufferOffset,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalRenameError> {
        if self.is_read_only(cx) {
            return Err(LocalRenameError::UnsupportedDocument);
        }
        if !is_valid_identifier(new_name) {
            return Err(LocalRenameError::InvalidName);
        }

        let binding = self
            .local_bindings()
            .into_iter()
            .find(|binding| binding_contains(binding, offset.get()))
            .ok_or(LocalRenameError::BindingNotFound)?;

        let mut ranges = Vec::with_capacity(binding.references.len() + 1);
        ranges.push(binding.definition_range);
        ranges.extend(binding.references);
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        ranges.dedup();
        let targets = SelectionSet::new(
            ranges
                .into_iter()
                .map(|range| {
                    Selection::new(
                        MultiBufferOffset::new(range.start),
                        MultiBufferOffset::new(range.end),
                    )
                })
                .collect(),
        );
        let before_selections = self.resolved_selections();
        let metadata = edit_metadata("重命名局部绑定");
        let replacement = new_name.to_owned();
        self.change_with_after(before_selections, metadata.clone(), cx, move |buffer| {
            replace_selections(buffer, &targets, &replacement)
        })
        .map(|_| ())
        .map_err(|error| LocalRenameError::Edit(error.to_string()))
    }

    /// 由编辑器 action 触发局部重命名输入框。
    pub(crate) fn handle_rename_local(
        &mut self,
        _: &RenameLocal,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode != EditorMode::Full {
            cx.propagate();
            return;
        }
        if self.local_rename.is_some() {
            return;
        }

        let offset = self.resolved_selections().primary().head();
        let Some(binding) = self
            .local_bindings()
            .into_iter()
            .find(|binding| binding_contains(binding, offset.get()))
        else {
            cx.emit(EditorEvent::Error(
                "重命名局部绑定失败：当前位置没有可确定的局部绑定".into(),
            ));
            return;
        };

        let rename_range = binding.definition_range.clone();
        let mut rename_ranges = Vec::with_capacity(binding.references.len() + 1);
        rename_ranges.push(binding.definition_range.clone());
        rename_ranges.extend(binding.references.iter().cloned());
        rename_ranges.sort_unstable_by_key(|range| (range.start, range.end));
        rename_ranges.dedup();
        let line_height = self
            .last_line_height
            .unwrap_or_else(|| zcv_theme::typography::content_line(cx));
        let cursor_position = self
            .pixel_position_of_newest_cursor
            .unwrap_or_else(|| point(Pixels::ZERO, Pixels::ZERO));
        let (position, text_width) = self
            .input_layout
            .as_ref()
            .and_then(|layout| local_rename_geometry_for_layout(layout, &rename_range))
            .unwrap_or_else(|| {
                let text_width = px((binding.name.chars().count() as f32 * 8.).max(40.));
                (
                    point(
                        cursor_position.x - text_width,
                        cursor_position.y + line_height,
                    ),
                    text_width,
                )
            });
        let width = (text_width + px(8.)).max(px(56.));

        let name = binding.name.clone();
        let input = cx.new(|cx| {
            let mut input = Editor::single_line_with_content_typography(cx);
            input.set_text(&name, cx);
            input.set_selections(SelectionSet::new(vec![Selection::new(
                MultiBufferOffset::ZERO,
                MultiBufferOffset::new(name.len()),
            )]));
            input
        });
        let version = self.multi_buffer.read(cx).snapshot(cx).version();
        self.local_rename = Some(LocalRenameState {
            offset,
            version,
            name,
            input: input.clone(),
            range: rename_range,
            ranges: rename_ranges.into(),
            position,
            width,
            line_height,
        });
        window.focus(&input.focus_handle(cx), cx);
        cx.notify();
    }

    /// 提交行内局部重命名；输入框保留在原位，直到名称有效且绑定仍属于同一文本版本。
    pub(crate) fn handle_confirm_local_rename(
        &mut self,
        _: &ConfirmLocalRename,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.local_rename.as_ref() else {
            cx.propagate();
            return;
        };
        let new_name = state.input.read(cx).text(cx);
        let old_name = state.name.clone();
        let offset = state.offset;
        let version = state.version;

        if new_name.trim().is_empty() || new_name == old_name {
            self.finish_local_rename(window, cx);
            return;
        }
        if self.multi_buffer.read(cx).snapshot(cx).version() != version {
            self.finish_local_rename(window, cx);
            cx.emit(EditorEvent::Error(format!(
                "重命名局部绑定失败：{}",
                LocalRenameError::StaleDocument
            )));
            return;
        }

        match self.rename_local_at(offset, &new_name, cx) {
            Ok(()) => self.finish_local_rename(window, cx),
            Err(error) if matches!(error, LocalRenameError::InvalidName) => {
                cx.emit(EditorEvent::Error(format!("重命名局部绑定失败：{error}")));
            }
            Err(LocalRenameError::Edit(_)) => {
                self.finish_local_rename(window, cx);
            }
            Err(error) => {
                self.finish_local_rename(window, cx);
                cx.emit(EditorEvent::Error(format!("重命名局部绑定失败：{error}")));
            }
        }
    }

    /// 取消行内局部重命名并恢复主编辑器焦点。
    pub(crate) fn handle_cancel_local_rename(
        &mut self,
        _: &CancelLocalRename,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.local_rename.is_none() {
            cx.propagate();
            return;
        }
        self.finish_local_rename(window, cx);
    }

    fn finish_local_rename(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(state) = self.local_rename.take() else {
            return;
        };
        if state.input.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// 在新一帧布局完成后更新重命名输入框的容器内坐标。
    pub(crate) fn update_local_rename_geometry(&mut self, cx: &mut Context<Self>) {
        let geometry = self.local_rename.as_ref().and_then(|state| {
            self.input_layout
                .as_ref()
                .and_then(|layout| local_rename_geometry_for_layout(layout, &state.range))
        });
        let Some((position, width)) = geometry else {
            return;
        };
        let changed = self
            .local_rename
            .as_ref()
            .is_some_and(|state| state.position != position || state.width != width);
        if changed {
            if let Some(state) = self.local_rename.as_mut() {
                state.position = position;
                state.width = width;
            }
            cx.notify();
        }
    }

    pub(crate) fn local_rename_overlay(
        &self,
    ) -> Option<(Entity<Editor>, Point<Pixels>, Pixels, Pixels)> {
        let state = self.local_rename.as_ref()?;
        let (position, width) = self.local_rename_geometry(state);
        Some((state.input.clone(), position, width, state.line_height))
    }

    fn local_rename_geometry(&self, state: &LocalRenameState) -> (Point<Pixels>, Pixels) {
        self.input_layout
            .as_ref()
            .and_then(|layout| local_rename_geometry_for_layout(layout, &state.range))
            .map(|(position, text_width)| (position, (text_width + px(8.)).max(px(56.))))
            .unwrap_or((state.position, state.width))
    }

    pub(crate) fn local_rename_range(&self) -> Option<Range<usize>> {
        self.local_rename.as_ref().map(|state| state.range.clone())
    }

    pub(crate) fn local_rename_ranges(&self) -> Arc<[Range<usize>]> {
        self.local_rename
            .as_ref()
            .map_or_else(|| Arc::from([]), |state| state.ranges.clone())
    }
}

fn local_rename_geometry_for_layout(
    layout: &EditorInputLayout,
    range: &Range<usize>,
) -> Option<(Point<Pixels>, Pixels)> {
    let start = layout.caret_position_for_offset(MultiBufferOffset::new(range.start))?;
    let name_end = layout.caret_position_for_offset(MultiBufferOffset::new(range.end))?;
    let line_end = layout.line_end_position_for_offset(MultiBufferOffset::new(range.start))?;
    (start.y == name_end.y && start.y <= line_end.y).then_some((
        point(start.x, line_end.y + layout.line_height()),
        name_end.x - start.x,
    ))
}

fn binding_contains(binding: &LocalBinding, offset: usize) -> bool {
    std::iter::once(&binding.definition_range)
        .chain(binding.references.iter())
        .any(|range| range.start <= offset && offset <= range.end)
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_alphabetic())
        && chars.all(|character| character == '_' || character.is_alphanumeric())
}
