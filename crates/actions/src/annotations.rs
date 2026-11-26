//! Runner and Graftyard check-run annotations (`design/SHARED.md`, "Toolkit hostname heuristics and SHA pins";
//! `decisions/DECISIONS.md`, "`pin-map`"). The runner's `completejob` spellings are normalized here; the shard applies
//! one job's count and byte caps before recording them beside its `Finished`.

use graftyard_git::issues::{AnnotationLevel, CheckAnnotation};
use serde_json::Value;

pub const ANNOTATIONS_MAX: usize = 50;
pub const ANNOTATION_BYTES_MAX: usize = 8 * 1024;
pub const TITLE_MAX: usize = 255;
pub const MESSAGE_MAX: usize = 4096;

/// A JSON field in the runner's lower-camel or Pascal spelling.
fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).or_else(|| v.get(format!("{}{}", key[..1].to_uppercase(), &key[1..])))
}

fn truncated(s: &str, max: usize) -> Box<str> {
    s.chars().take(max).collect::<String>().into_boxed_str()
}

fn line(v: Option<&Value>) -> u32 {
    v.and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(0).max(1)
}

fn level(v: Option<&Value>) -> Option<AnnotationLevel> {
    match v? {
        Value::String(s) => match s.to_ascii_uppercase().as_str() {
            "NOTICE" => Some(AnnotationLevel::Notice),
            "WARNING" => Some(AnnotationLevel::Warning),
            "FAILURE" => Some(AnnotationLevel::Failure),
            _ => None,
        },
        Value::Number(n) => match n.as_u64() {
            Some(1) => Some(AnnotationLevel::Notice),
            Some(2) => Some(AnnotationLevel::Warning),
            Some(3) => Some(AnnotationLevel::Failure),
            _ => None,
        },
        _ => None,
    }
}

fn one(v: &Value) -> Option<CheckAnnotation> {
    let message = get(v, "message")?.as_str().filter(|s| !s.is_empty())?;
    let start_line = line(get(v, "startLine"));
    let end_line = line(get(v, "endLine")).max(start_line);
    let title = get(v, "title").and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| truncated(s, TITLE_MAX));
    Some(CheckAnnotation {
        path: get(v, "path").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or(".github").into(),
        start_line,
        end_line,
        level: level(get(v, "level"))?,
        message: truncated(message, MESSAGE_MAX),
        title,
    })
}

/// Every step's annotations in `stepResults` order, then the job-level annotations, with per-field limits applied.
pub fn from_complete(b: &Value) -> Vec<CheckAnnotation> {
    let mut out = Vec::new();
    for v in get(b, "stepResults").and_then(Value::as_array).into_iter().flatten() {
        out.extend(get(v, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));
    }
    out.extend(get(b, "annotations").and_then(Value::as_array).into_iter().flatten().filter_map(one));
    out
}

/// The `(owner/repository, ref)` pairs the step results name, without sub-action paths or duplicates.
pub fn step_actions(b: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for step in get(b, "stepResults").and_then(Value::as_array).into_iter().flatten() {
        let Some(action) = get(step, "action_name").and_then(Value::as_str) else { continue };
        let Some(reference) = get(step, "ref").and_then(Value::as_str).filter(|s| !s.is_empty()) else { continue };
        let mut parts = action.split('/');
        let (Some(owner), Some(repo), rest) = (parts.next(), parts.next(), parts.next()) else { continue };
        if action.starts_with("./")
            || action.starts_with(".\\")
            || owner.is_empty()
            || repo.is_empty()
            || rest.is_some_and(|path| path.is_empty())
        {
            continue;
        }
        let pair = (format!("{owner}/{repo}"), reference.to_owned());
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// The Graftyard annotation for an unapproved SHA pin to an upstream v4+ action.
pub fn pin_map_annotation(
    upstream: &str,
    upstream_sha: &str,
    fork: &str,
    fork_sha: &str,
    web: &str,
    org: &str,
) -> CheckAnnotation {
    let name = upstream.rsplit('/').next().unwrap_or(upstream);
    CheckAnnotation {
        path: ".github".into(),
        start_line: 1,
        end_line: 1,
        level: AnnotationLevel::Failure,
        message: format!(
            "`{upstream}@{upstream_sha}` is {name} v4+, which GHES does not support. Approve the pin map at {web}/organizations/{org}/settings/actions/pin-map, or pin `{fork}@{fork_sha}`."
        )
        .into(),
        title: Some("Pinned action needs the Graftyard pin map".into()),
    }
}

/// The Graftyard annotations first and the runner's after them, stopping at the first that does not fit either cap.
pub fn job_annotations(first: Vec<CheckAnnotation>, runner: Vec<CheckAnnotation>) -> Vec<CheckAnnotation> {
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for a in first.into_iter().chain(runner) {
        let more = a.message.len() + a.title.as_ref().map_or(0, |t| t.len());
        if out.len() == ANNOTATIONS_MAX || bytes.saturating_add(more) > ANNOTATION_BYTES_MAX {
            break;
        }
        bytes += more;
        out.push(a);
    }
    out
}

/// Whether a stored set is within both job caps. An empty set fits; an event must nevertheless contain annotations.
pub fn fits(a: &[CheckAnnotation]) -> bool {
    if a.len() > ANNOTATIONS_MAX {
        return false;
    }
    a.iter()
        .try_fold(0usize, |bytes, a| {
            let bytes = bytes.checked_add(a.message.len())?.checked_add(a.title.as_ref().map_or(0, |t| t.len()))?;
            (bytes <= ANNOTATION_BYTES_MAX).then_some(bytes)
        })
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn annotation(message: &str) -> CheckAnnotation {
        CheckAnnotation {
            path: ".github".into(),
            start_line: 1,
            end_line: 1,
            level: AnnotationLevel::Notice,
            message: message.into(),
            title: None,
        }
    }

    #[test]
    fn completejob_annotations_accept_both_spellings_and_keep_their_order() {
        let long_title = "t".repeat(300);
        let body = json!({
            "stepResults": [
                {"annotations": [
                    {"level": "warning", "message": "step-one-a", "startLine": 0},
                    {"Level": 3, "Message": "step-one-b", "Title": long_title, "StartLine": 9, "EndLine": 0},
                    {"level": "unknown", "message": "dropped level"},
                    {"level": "notice", "message": ""}
                ]},
                {"annotations": [{"level": "NOTICE", "message": "step-two", "path": "src/a.rs", "startLine": 4, "endLine": 2}]}
            ],
            "annotations": [{"level": "FAILURE", "message": "job-level", "path": ""}]
        });
        let got = from_complete(&body);
        assert_eq!(
            got.iter().map(|a| &*a.message).collect::<Vec<_>>(),
            ["step-one-a", "step-one-b", "step-two", "job-level"]
        );
        assert_eq!(
            (&*got[0].path, got[0].start_line, got[0].end_line, got[0].level),
            (".github", 1, 1, AnnotationLevel::Warning)
        );
        assert_eq!((got[1].start_line, got[1].end_line, got[1].level), (9, 9, AnnotationLevel::Failure));
        assert_eq!(got[1].title.as_deref().map(str::chars).map(Iterator::count), Some(TITLE_MAX));
        assert_eq!(
            (&*got[2].path, got[2].start_line, got[2].end_line, got[2].level),
            ("src/a.rs", 4, 4, AnnotationLevel::Notice)
        );
        assert_eq!((&*got[3].path, got[3].level), (".github", AnnotationLevel::Failure));

        let unicode = json!({"annotations": [{"level": 1, "message": "é".repeat(MESSAGE_MAX + 1), "title": ""}]});
        let got = from_complete(&unicode);
        assert_eq!(got[0].message.chars().count(), MESSAGE_MAX);
        assert_eq!(got[0].title, None);
    }

    #[test]
    fn a_job_stops_at_the_count_or_byte_cap() {
        let at_count = vec![annotation("x"); ANNOTATIONS_MAX];
        assert!(fits(&at_count));
        assert_eq!(job_annotations(Vec::new(), at_count.clone()).len(), ANNOTATIONS_MAX);
        let mut over_count = at_count;
        over_count.push(annotation("last"));
        assert_eq!(job_annotations(Vec::new(), over_count).len(), ANNOTATIONS_MAX);

        let exact_bytes = vec![annotation(&"a".repeat(4095)), annotation(&"b".repeat(4095)), annotation("cc")];
        assert!(fits(&exact_bytes));
        assert_eq!(job_annotations(Vec::new(), exact_bytes.clone()).len(), 3);
        let over_bytes = vec![annotation(&"a".repeat(4095)), annotation(&"b".repeat(4095)), annotation("ccc")];
        assert!(!fits(&over_bytes));
        let capped = job_annotations(Vec::new(), over_bytes);
        assert_eq!(capped, exact_bytes[..2]);
        assert!(fits(&capped));

        let first_large = vec![annotation(&"x".repeat(ANNOTATION_BYTES_MAX + 1)), annotation("small")];
        assert!(job_annotations(first_large.clone(), vec![annotation("later")]).is_empty(), "do not skip ahead");
        assert_eq!(
            job_annotations(vec![annotation("first")], vec![annotation("runner")]),
            [annotation("first"), annotation("runner")]
        );
    }

    #[test]
    fn step_actions_strip_subaction_paths_and_deduplicate() {
        let body = json!({"stepResults": [
            {"action_name": "actions/upload-artifact/sub", "ref": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"action_name": "actions/upload-artifact", "ref": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"action_name": "actions/download-artifact", "ref": "v4"},
            {"action_name": "actions/download-artifact", "ref": "v4"},
            {"action_name": "actions/upload-artifact", "ref": ""},
            {"action_name": "./probe", "ref": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"action_name": "one-part", "ref": "v1"}
        ]});
        assert_eq!(
            step_actions(&body),
            [
                ("actions/upload-artifact".into(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
                ("actions/download-artifact".into(), "v4".into())
            ]
        );
    }

    #[test]
    fn the_pin_map_message_names_the_exact_action_and_fork() {
        for action in ["upload-artifact", "download-artifact"] {
            let upstream = format!("actions/{action}");
            let upstream_sha = "a".repeat(40);
            let fork = format!("graftyard-actions/{action}");
            let fork_sha = "b".repeat(40);
            let a = pin_map_annotation(&upstream, &upstream_sha, &fork, &fork_sha, "https://g.example", "octo");
            assert_eq!(a.path.as_ref(), ".github");
            assert_eq!((a.start_line, a.end_line, a.level), (1, 1, AnnotationLevel::Failure));
            assert_eq!(a.title.as_deref(), Some("Pinned action needs the Graftyard pin map"));
            assert_eq!(
                a.message,
                format!("`{upstream}@{upstream_sha}` is {action} v4+, which GHES does not support. Approve the pin map at https://g.example/organizations/octo/settings/actions/pin-map, or pin `{fork}@{fork_sha}`.").into()
            );
        }
    }
}
