use codex_git_utils::merge_base_with_head;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::ReviewTarget;
use codex_utils_template::Template;
use std::path::Path;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedReviewRequest {
    pub target: ReviewTarget,
    pub prompt: String,
    pub user_facing_hint: String,
}

const UNCOMMITTED_PROMPT: &str =
    "审查当前的代码变更（已暂存、未暂存和未跟踪的文件）并提供优先级排列的发现。";

const BASE_BRANCH_PROMPT_BACKUP: &str = "审查相对于基准分支 '{{branch}}' 的代码变更。首先找到当前分支与 {{branch}} 的上游之间的合并差异，例如 (`git merge-base HEAD \"$(git rev-parse --abbrev-ref \"{{branch}}@{upstream}\")\"`)，然后对该 SHA 运行 `git diff` 以查看我们将合并到 {{branch}} 分支的变更。提供优先级排列的、可操作的发现。";
const BASE_BRANCH_PROMPT: &str = "审查相对于基准分支 '{{base_branch}}' 的代码变更。此比较的合并基准提交是 {{merge_base_sha}}。运行 `git diff {{merge_base_sha}}` 以检查相对于 {{base_branch}} 的变更。提供优先级排列的、可操作的发现。";
static BASE_BRANCH_PROMPT_BACKUP_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    Template::parse(BASE_BRANCH_PROMPT_BACKUP)
        .unwrap_or_else(|err| panic!("base branch backup review prompt must parse: {err}"))
});
static BASE_BRANCH_PROMPT_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    Template::parse(BASE_BRANCH_PROMPT)
        .unwrap_or_else(|err| panic!("base branch review prompt must parse: {err}"))
});

const COMMIT_PROMPT_WITH_TITLE: &str =
    "审查提交 {{sha}}（\"{{title}}\"）引入的代码变更。提供优先级排列的、可操作的发现。";
const COMMIT_PROMPT: &str = "审查提交 {{sha}} 引入的代码变更。提供优先级排列的、可操作的发现。";
static COMMIT_PROMPT_WITH_TITLE_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    Template::parse(COMMIT_PROMPT_WITH_TITLE)
        .unwrap_or_else(|err| panic!("commit review prompt with title must parse: {err}"))
});
static COMMIT_PROMPT_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    Template::parse(COMMIT_PROMPT)
        .unwrap_or_else(|err| panic!("commit review prompt must parse: {err}"))
});

pub fn resolve_review_request(
    request: ReviewRequest,
    cwd: &Path,
) -> anyhow::Result<ResolvedReviewRequest> {
    let target = request.target;
    let prompt = review_prompt(&target, cwd)?;
    let user_facing_hint = request
        .user_facing_hint
        .unwrap_or_else(|| user_facing_hint(&target));

    Ok(ResolvedReviewRequest {
        target,
        prompt,
        user_facing_hint,
    })
}

pub fn review_prompt(target: &ReviewTarget, cwd: &Path) -> anyhow::Result<String> {
    match target {
        ReviewTarget::UncommittedChanges => Ok(UNCOMMITTED_PROMPT.to_string()),
        ReviewTarget::BaseBranch { branch } => {
            if let Some(commit) = merge_base_with_head(cwd, branch)? {
                Ok(render_review_prompt(
                    &BASE_BRANCH_PROMPT_TEMPLATE,
                    [
                        ("base_branch", branch.as_str()),
                        ("merge_base_sha", commit.as_str()),
                    ],
                ))
            } else {
                Ok(render_review_prompt(
                    &BASE_BRANCH_PROMPT_BACKUP_TEMPLATE,
                    [("branch", branch.as_str())],
                ))
            }
        }
        ReviewTarget::Commit { sha, title } => {
            if let Some(title) = title {
                Ok(render_review_prompt(
                    &COMMIT_PROMPT_WITH_TITLE_TEMPLATE,
                    [("sha", sha.as_str()), ("title", title.as_str())],
                ))
            } else {
                Ok(render_review_prompt(
                    &COMMIT_PROMPT_TEMPLATE,
                    [("sha", sha.as_str())],
                ))
            }
        }
        ReviewTarget::Custom { instructions } => {
            let prompt = instructions.trim();
            if prompt.is_empty() {
                anyhow::bail!("审查提示不能为空");
            }
            Ok(prompt.to_string())
        }
    }
}

fn render_review_prompt<'a, const N: usize>(
    template: &Template,
    variables: [(&'a str, &'a str); N],
) -> String {
    template
        .render(variables)
        .unwrap_or_else(|err| panic!("review prompt template must render: {err}"))
}

pub fn user_facing_hint(target: &ReviewTarget) -> String {
    match target {
        ReviewTarget::UncommittedChanges => "当前变更".to_string(),
        ReviewTarget::BaseBranch { branch } => format!("相对于 '{branch}' 的变更"),
        ReviewTarget::Commit { sha, title } => {
            let short_sha: String = sha.chars().take(7).collect();
            if let Some(title) = title {
                format!("提交 {short_sha}: {title}")
            } else {
                format!("提交 {short_sha}")
            }
        }
        ReviewTarget::Custom { instructions } => instructions.trim().to_string(),
    }
}

impl From<ResolvedReviewRequest> for ReviewRequest {
    fn from(resolved: ResolvedReviewRequest) -> Self {
        ReviewRequest {
            target: resolved.target,
            user_facing_hint: Some(resolved.user_facing_hint),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn review_prompt_template_renders_base_branch_backup_variant() {
        assert_eq!(
            render_review_prompt(&BASE_BRANCH_PROMPT_BACKUP_TEMPLATE, [("branch", "main")]),
            "审查相对于基准分支 'main' 的代码变更。首先找到当前分支与 main 的上游之间的合并差异，例如 (`git merge-base HEAD \"$(git rev-parse --abbrev-ref \"main@{upstream}\")\"`)，然后对该 SHA 运行 `git diff` 以查看我们将合并到 main 分支的变更。提供优先级排列的、可操作的发现。"
        );
    }

    #[test]
    fn review_prompt_template_renders_base_branch_variant() {
        assert_eq!(
            render_review_prompt(
                &BASE_BRANCH_PROMPT_TEMPLATE,
                [("base_branch", "main"), ("merge_base_sha", "abc123")]
            ),
            "审查相对于基准分支 'main' 的代码变更。此比较的合并基准提交是 abc123。运行 `git diff abc123` 以检查相对于 main 的变更。提供优先级排列的、可操作的发现。"
        );
    }

    #[test]
    fn review_prompt_template_renders_commit_variant() {
        assert_eq!(
            review_prompt(
                &ReviewTarget::Commit {
                    sha: "deadbeef".to_string(),
                    title: None,
                },
                Path::new("."),
            )
            .expect("commit prompt should render"),
            "审查提交 deadbeef 引入的代码变更。提供优先级排列的、可操作的发现。"
        );
    }

    #[test]
    fn review_prompt_template_renders_commit_variant_with_title() {
        assert_eq!(
            review_prompt(
                &ReviewTarget::Commit {
                    sha: "deadbeef".to_string(),
                    title: Some("Fix bug".to_string()),
                },
                Path::new("."),
            )
            .expect("commit prompt should render"),
            "审查提交 deadbeef（\"Fix bug\"）引入的代码变更。提供优先级排列的、可操作的发现。"
        );
    }
}
