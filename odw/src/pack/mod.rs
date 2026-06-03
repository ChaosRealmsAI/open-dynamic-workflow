//! Embedded assets shipped inside the `odw` binary.
//!
//! odw is zero-install: it never writes files into a project. These consts are
//! the JS workflow runtime that `odw exec` runs, the HTML report generator +
//! its offline render libs (mermaid + marked) that `odw report` emits, and the
//! authoring contract text that `odw contract` prints.

pub(crate) const ODW_JS_RUNNER: &str = include_str!("templates/runtime/odw-js-runner.mjs");

pub(crate) const ODW_REPORT_MJS: &str = include_str!("templates/report/odw-report.mjs");
pub(crate) const REPORT_MERMAID_JS: &str = include_str!("templates/report/vendor/mermaid.min.js");
pub(crate) const REPORT_MARKED_JS: &str = include_str!("templates/report/vendor/marked.min.js");

pub(crate) fn contract_text() -> &'static str {
    include_str!("templates/contract.md")
}

/// TypeScript declarations for the workflow authoring API, surfaced via `odw spec`.
pub(crate) const WORKFLOW_API_DTS: &str = include_str!("templates/workflow-api.d.ts");
