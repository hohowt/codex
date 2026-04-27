use crate::memories::memory_extensions_root;
use crate::memories::memory_root;
use crate::memories::phase_one;
use crate::memories::storage::rollout_summary_file_stem_from_parts;
use codex_protocol::openai_models::ModelInfo;
use codex_state::Phase2InputSelection;
use codex_state::Stage1Output;
use codex_state::Stage1OutputRef;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;
use codex_utils_template::Template;
use std::path::Path;
use std::sync::LazyLock;
use tokio::fs;
use tracing::warn;

static CONSOLIDATION_PROMPT_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    parse_embedded_template(
        include_str!("../../templates/memories/consolidation.md"),
        "memories/consolidation.md",
    )
});
static STAGE_ONE_INPUT_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    parse_embedded_template(
        include_str!("../../templates/memories/stage_one_input.md"),
        "memories/stage_one_input.md",
    )
});
static MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    parse_embedded_template(
        include_str!("../../templates/memories/read_path.md"),
        "memories/read_path.md",
    )
});
static MEMORY_EXTENSIONS_FOLDER_STRUCTURE_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    parse_embedded_template(
        MEMORY_EXTENSIONS_FOLDER_STRUCTURE,
        "memories/extensions_folder_structure.md",
    )
});
static MEMORY_EXTENSIONS_PRIMARY_INPUTS_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    parse_embedded_template(
        MEMORY_EXTENSIONS_PRIMARY_INPUTS,
        "memories/extensions_primary_inputs.md",
    )
});

fn parse_embedded_template(source: &'static str, template_name: &str) -> Template {
    match Template::parse(source) {
        Ok(template) => template,
        Err(err) => panic!("内嵌模板 {template_name} 无效: {err}"),
    }
}

const MEMORY_EXTENSIONS_FOLDER_STRUCTURE: &str = r#"
记忆扩展（在 {{ memory_extensions_root }}/ 下）：

- <extension_name>/instructions.md
  - 用于解读额外记忆信号的源特定指导。如果存在扩展文件夹，你必须阅读其 instructions.md 来确定如何使用此记忆源。

如果用户有任何记忆扩展，你必须阅读每个扩展的指令来确定如何使用记忆源。如果没有扩展文件夹，仅使用标准记忆输入继续。
"#;

const MEMORY_EXTENSIONS_PRIMARY_INPUTS: &str = r#"
可选的源特定输入：
在 `{{ memory_extensions_root }}/` 下：

- `<extension_name>/instructions.md`
  - 如果扩展文件夹存在，首先读取每个 instructions.md，并在解读该扩展的记忆源时遵循它。
"#;

/// Builds the consolidation subagent prompt for a specific memory root.
pub(super) fn build_consolidation_prompt(
    memory_root: &Path,
    selection: &Phase2InputSelection,
) -> String {
    let memory_extensions_root = memory_extensions_root(memory_root);
    let memory_extensions_exist = memory_extensions_root.is_dir();
    let memory_root = memory_root.display().to_string();
    let memory_extensions_root = memory_extensions_root.display().to_string();
    let memory_extensions_folder_structure = if memory_extensions_exist {
        render_memory_extensions_block(
            &MEMORY_EXTENSIONS_FOLDER_STRUCTURE_TEMPLATE,
            &memory_extensions_root,
        )
    } else {
        String::new()
    };
    let memory_extensions_primary_inputs = if memory_extensions_exist {
        render_memory_extensions_block(
            &MEMORY_EXTENSIONS_PRIMARY_INPUTS_TEMPLATE,
            &memory_extensions_root,
        )
    } else {
        String::new()
    };
    let phase2_input_selection = render_phase2_input_selection(selection);
    CONSOLIDATION_PROMPT_TEMPLATE
        .render([
            ("memory_root", memory_root.as_str()),
            (
                "memory_extensions_folder_structure",
                memory_extensions_folder_structure.as_str(),
            ),
            (
                "memory_extensions_primary_inputs",
                memory_extensions_primary_inputs.as_str(),
            ),
            ("phase2_input_selection", phase2_input_selection.as_str()),
        ])
        .unwrap_or_else(|err| {
            warn!("渲染记忆合并提示模板失败: {err}");
            format!(
                "## 记忆 Phase 2（合并）\n在以下位置合并 Codex 记忆: {memory_root}\n\n{phase2_input_selection}"
            )
        })
}

fn render_memory_extensions_block(template: &Template, memory_extensions_root: &str) -> String {
    template
        .render([("memory_extensions_root", memory_extensions_root)])
        .unwrap_or_else(|err| {
            warn!("渲染记忆扩展提示块失败: {err}");
            String::new()
        })
}

fn render_phase2_input_selection(selection: &Phase2InputSelection) -> String {
    let retained = selection.retained_thread_ids.len();
    let added = selection.selected.len().saturating_sub(retained);
    let selected = if selection.selected.is_empty() {
        "- 无".to_string()
    } else {
        selection
            .selected
            .iter()
            .map(|item| {
                render_selected_input_line(
                    item,
                    selection.retained_thread_ids.contains(&item.thread_id),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let removed = if selection.removed.is_empty() {
        "- 无".to_string()
    } else {
        selection
            .removed
            .iter()
            .map(render_removed_input_line)
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "- 本次运行选定的输入: {}\n- 自上次成功 Phase 2 运行以来新增: {added}\n- 自上次成功 Phase 2 运行以来保留: {retained}\n- 自上次成功 Phase 2 运行以来移除: {}\n\n当前选定的 Phase 1 输入:\n{selected}\n\n自上次成功 Phase 2 选择中移除:\n{removed}\n",
        selection.selected.len(),
        selection.removed.len(),
    )
}

fn render_selected_input_line(item: &Stage1Output, retained: bool) -> String {
    let status = if retained { "保留" } else { "新增" };
    let rollout_summary_file = format!(
        "rollout_summaries/{}.md",
        rollout_summary_file_stem_from_parts(
            item.thread_id,
            item.source_updated_at,
            item.rollout_slug.as_deref(),
        )
    );
    format!(
        "- [{status}] thread_id={}, rollout_summary_file={rollout_summary_file}",
        item.thread_id
    )
}

fn render_removed_input_line(item: &Stage1OutputRef) -> String {
    let rollout_summary_file = format!(
        "rollout_summaries/{}.md",
        rollout_summary_file_stem_from_parts(
            item.thread_id,
            item.source_updated_at,
            item.rollout_slug.as_deref(),
        )
    );
    format!(
        "- thread_id={}, rollout_summary_file={rollout_summary_file}",
        item.thread_id
    )
}

/// Builds the stage-1 user message containing rollout metadata and content.
///
/// Large rollout payloads are truncated to 70% of the active model's effective
/// input window token budget while keeping both head and tail context.
pub(super) fn build_stage_one_input_message(
    model_info: &ModelInfo,
    rollout_path: &Path,
    rollout_cwd: &Path,
    rollout_contents: &str,
) -> anyhow::Result<String> {
    let rollout_token_limit = model_info
        .context_window
        .and_then(|limit| (limit > 0).then_some(limit))
        .map(|limit| limit.saturating_mul(model_info.effective_context_window_percent) / 100)
        .map(|limit| (limit.saturating_mul(phase_one::CONTEXT_WINDOW_PERCENT) / 100).max(1))
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(phase_one::DEFAULT_STAGE_ONE_ROLLOUT_TOKEN_LIMIT);
    let truncated_rollout_contents = truncate_text(
        rollout_contents,
        TruncationPolicy::Tokens(rollout_token_limit),
    );

    let rollout_path = rollout_path.display().to_string();
    let rollout_cwd = rollout_cwd.display().to_string();
    Ok(STAGE_ONE_INPUT_TEMPLATE.render([
        ("rollout_path", rollout_path.as_str()),
        ("rollout_cwd", rollout_cwd.as_str()),
        ("rollout_contents", truncated_rollout_contents.as_str()),
    ])?)
}

/// Build prompt used for read path. This prompt must be added to the developer instructions. In
/// case of large memory files, the `memory_summary.md` is truncated at
/// [phase_one::MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_SUMMARY_TOKEN_LIMIT].
pub(crate) async fn build_memory_tool_developer_instructions(codex_home: &Path) -> Option<String> {
    let base_path = memory_root(codex_home);
    let memory_summary_path = base_path.join("memory_summary.md");
    let memory_summary = fs::read_to_string(&memory_summary_path)
        .await
        .ok()?
        .trim()
        .to_string();
    let memory_summary = truncate_text(
        &memory_summary,
        TruncationPolicy::Tokens(phase_one::MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_SUMMARY_TOKEN_LIMIT),
    );
    if memory_summary.is_empty() {
        return None;
    }
    let base_path = base_path.display().to_string();
    MEMORY_TOOL_DEVELOPER_INSTRUCTIONS_TEMPLATE
        .render([
            ("base_path", base_path.as_str()),
            ("memory_summary", memory_summary.as_str()),
        ])
        .ok()
}

#[cfg(test)]
#[path = "prompts_tests.rs"]
mod tests;
