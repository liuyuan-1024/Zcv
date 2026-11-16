//! LaTeX 编译命令及工作区反馈。

mod build;
mod toolchain;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context as _;
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, EntityId, Global, Subscription, Window,
};
use zcv_activity::{ActivityChanged, ActivityPhase, ActivitySource, BackgroundActivity};
use zcv_editor::Editor;
use zcv_project::FileSaveConflictPolicy;
use zcv_ui::{ToastAction, ToastKind};
use zcv_workspace::{ExternalPreviewProvider, ItemHandle, Workspace, register_external_preview};

#[derive(Default)]
struct ActiveBuilds(Vec<(PathBuf, EntityId)>);

struct LatexBuildStore(Entity<ActiveBuilds>);

impl Global for LatexBuildStore {}

pub fn init(cx: &mut App) {
    let builds = cx.new(|_| ActiveBuilds::default());
    cx.set_global(LatexBuildStore(builds));
    register_external_preview(LatexPreviewProvider, cx);
}

pub fn activity_source(workspace: EntityId, cx: &App) -> Option<Box<dyn ActivitySource>> {
    let builds = cx.try_global::<LatexBuildStore>()?.0.clone();
    Some(Box::new(LatexActivitySource { builds, workspace }))
}

struct LatexActivitySource {
    builds: Entity<ActiveBuilds>,
    workspace: EntityId,
}

impl ActivitySource for LatexActivitySource {
    fn observe(&self, changed: ActivityChanged, cx: &mut App) -> Subscription {
        cx.observe(&self.builds, move |_, cx| changed(cx))
    }

    fn collect(&self, activities: &mut Vec<BackgroundActivity>, cx: &App) {
        activities.extend(
            self.builds
                .read(cx)
                .0
                .iter()
                .filter(|(_, workspace)| *workspace == self.workspace)
                .map(|(path, _)| {
                    let name = path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy();
                    BackgroundActivity::new(format!("编译 LaTeX · {name}"), ActivityPhase::Running)
                }),
        );
    }
}

struct LatexPreviewProvider;

impl ExternalPreviewProvider for LatexPreviewProvider {
    fn supports(&self, item: &dyn ItemHandle, path: &Path, cx: &App) -> bool {
        build::is_latex_path(path)
            && item.act_as::<Editor>(cx).is_some_and(|editor| {
                editor.entity_id() == item.item_id()
                    && editor
                        .read(cx)
                        .multi_buffer()
                        .read(cx)
                        .singleton_source()
                        .is_some()
            })
    }

    fn label(&self) -> &'static str {
        "编译并预览 LaTeX"
    }

    fn open(&self, item: Box<dyn ItemHandle>, path: PathBuf, window: &mut Window, cx: &mut App) {
        if let Some(workspace) = window.root::<Workspace>().flatten() {
            workspace.update(cx, |workspace, cx| {
                compile_preview(workspace, item, path, window, cx);
            });
        }
    }
}

fn compile_preview(
    workspace: &mut Workspace,
    item: Box<dyn ItemHandle>,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let active = cx.global::<LatexBuildStore>().0.clone();
    let owner = cx.entity().entity_id();
    let started = active.update(cx, |builds, cx| {
        if builds.0.iter().any(|(source, _)| source == &path) {
            return false;
        }
        builds.0.push((path.clone(), owner));
        cx.notify();
        true
    });
    if !started {
        workspace.show_toast(
            ToastKind::Info,
            "当前 LaTeX 文档正在编译。",
            vec![],
            Some(Duration::from_secs(3)),
            cx,
        );
        return;
    }

    let save = item.save(
        workspace.project().clone(),
        FileSaveConflictPolicy::Reject,
        window,
        cx,
    );
    cx.spawn(async move |workspace, cx| {
        let result = match save.await.context("保存源文件失败") {
            Ok(()) => {
                let source = path.clone();
                smol::unblock(move || build::compile(&source)).await
            }
            Err(error) => Err(error),
        };
        active.update(cx, |builds, cx| {
            builds.0.retain(|(source, _)| source != &path);
            cx.notify();
        });
        if let Some(workspace) = workspace.upgrade() {
            workspace.update(cx, |workspace, cx| match result {
                Ok(path) => cx.open_with_system(&path),
                Err(error) => {
                    let actions = error
                        .downcast_ref::<toolchain::CompilerError>()
                        .map_or_else(Vec::new, |error| {
                            let output = error.output.clone();
                            vec![ToastAction::new("复制编译输出", move |_, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(output.clone()));
                            })]
                        });
                    workspace.show_toast(
                        ToastKind::Error,
                        format!("编译并预览 LaTeX 失败：{error:#}"),
                        actions,
                        None,
                        cx,
                    );
                }
            });
        }
    })
    .detach();
}
